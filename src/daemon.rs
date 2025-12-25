use crate::app::NotesApp;
use crate::paths::DataPaths;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn ensure_daemon_running(paths: &DataPaths) -> Result<()> {
    if env::var("NOTES_DISABLE_DAEMON").is_ok() {
        return Ok(());
    }

    install_autostart(paths)?;

    if daemon_running(paths)? {
        return Ok(());
    }

    let exe = env::current_exe().context("Failed to resolve current executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Ok(notes_home) = env::var("NOTES_HOME") {
        cmd.env("NOTES_HOME", notes_home);
    }

    cmd.spawn().context("Failed to start notes daemon")?;
    Ok(())
}

fn daemon_running(paths: &DataPaths) -> Result<bool> {
    if !paths.daemon_pid.exists() {
        return Ok(false);
    }

    let pid_str = fs::read_to_string(&paths.daemon_pid).unwrap_or_default();
    let pid: i32 = match pid_str.trim().parse() {
        Ok(pid) => pid,
        Err(_) => return Ok(false),
    };

    if pid <= 0 {
        return Ok(false);
    }

    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid, 0) };
        if result == 0 {
            return Ok(true);
        }

        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            let _ = fs::remove_file(&paths.daemon_pid);
            return Ok(false);
        }
    }

    #[cfg(not(unix))]
    {
        return Ok(true);
    }

    Ok(false)
}

pub fn run_daemon(paths: &DataPaths) -> Result<()> {
    paths.ensure_dirs()?;
    write_pid(&paths)?;
    log_line(&paths, "daemon started")?;

    let (tx, rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .context("Failed to initialize file watcher")?;
    watcher
        .watch(&paths.files, RecursiveMode::NonRecursive)
        .with_context(|| format!("Failed to watch {}", paths.files.display()))?;

    let cooldown = Duration::from_secs(30);
    let mut pending = false;
    let mut last_event = Instant::now();

    loop {
        match rx.recv_timeout(cooldown) {
            Ok(Ok(event)) => {
                if event.paths.iter().any(|path| {
                    path.extension()
                        .map(|ext| ext.eq_ignore_ascii_case("md"))
                        .unwrap_or(false)
                }) {
                    pending = true;
                    last_event = Instant::now();
                }
            }
            Ok(Err(err)) => {
                let _ = log_line(&paths, &format!("watch error: {err}"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if pending && last_event.elapsed() >= cooldown {
                    if let Err(err) = sync_snapshots(&paths) {
                        let _ = log_line(&paths, &format!("sync error: {err}"));
                    }
                    pending = false;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn sync_snapshots(paths: &DataPaths) -> Result<()> {
    let mut app = NotesApp::load()?;
    let updated = app.snapshot_all_changes()?;
    if !updated.is_empty() {
        log_line(
            paths,
            &format!("updated {} note(s): {}", updated.len(), updated.join(", ")),
        )?;
    }
    app.save()?;
    Ok(())
}

fn write_pid(paths: &DataPaths) -> Result<()> {
    let pid = std::process::id();
    fs::write(&paths.daemon_pid, pid.to_string())
        .with_context(|| format!("Failed to write {}", paths.daemon_pid.display()))
}

fn log_line(paths: &DataPaths, message: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.daemon_log)
        .with_context(|| format!("Failed to open {}", paths.daemon_log.display()))?;
    writeln!(file, "[{}] {}", Utc::now().to_rfc3339(), message)?;
    Ok(())
}

fn install_autostart(paths: &DataPaths) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        install_launchd(paths)?;
    }

    #[cfg(target_os = "linux")]
    {
        install_systemd(paths)?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn install_launchd(paths: &DataPaths) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Unable to determine home directory"))?;
    let agents_dir = home.join("Library").join("LaunchAgents");
    fs::create_dir_all(&agents_dir)?;

    let plist_path = agents_dir.join("com.notes.daemon.plist");
    if plist_path.exists() {
        return Ok(());
    }

    let exe = env::current_exe().context("Failed to resolve current executable")?;
    let mut env_block = String::new();
    if let Ok(notes_home) = env::var("NOTES_HOME") {
        env_block = format!(
            "    <key>EnvironmentVariables</key>\n    <dict>\n      <key>NOTES_HOME</key>\n      <string>{}</string>\n    </dict>\n",
            notes_home
        );
    }

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.notes.daemon</string>
    <key>ProgramArguments</key>
    <array>
      <string>{}</string>
      <string>daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{}</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
{}
  </dict>
</plist>
"#,
        exe.display(),
        paths.daemon_log.display(),
        paths.daemon_log.display(),
        env_block
    );

    fs::write(&plist_path, plist)?;

    let uid = unsafe { libc::getuid() };
    let status = std::process::Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{}", uid),
            plist_path.to_str().unwrap_or(""),
        ])
        .status();

    if let Ok(status) = status {
        if !status.success() {
            let _ = log_line(
                paths,
                "launchctl bootstrap failed; you may need to load the LaunchAgent manually",
            );
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn install_systemd(paths: &DataPaths) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Unable to determine home directory"))?;
    let user_dir = home.join(".config").join("systemd").join("user");
    fs::create_dir_all(&user_dir)?;

    let service_path = user_dir.join("notes-daemon.service");
    if service_path.exists() {
        return Ok(());
    }

    let exe = env::current_exe().context("Failed to resolve current executable")?;
    let mut env_line = String::new();
    if let Ok(notes_home) = env::var("NOTES_HOME") {
        env_line = format!("Environment=NOTES_HOME={}\n", notes_home);
    }

    let service = format!(
        r#"[Unit]
Description=Notes daemon

[Service]
ExecStart={} daemon
Restart=on-failure
{}

[Install]
WantedBy=default.target
"#,
        exe.display(),
        env_line
    );

    fs::write(&service_path, service)?;

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let status = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "notes-daemon.service"])
        .status();

    if let Ok(status) = status {
        if !status.success() {
            let _ = log_line(
                paths,
                "systemctl enable failed; you may need to enable the service manually",
            );
        }
    }

    Ok(())
}
