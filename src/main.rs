use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "notes", about = "Local notes with version control")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new note (optional title)
    New { title: Option<String> },
    /// Open an existing note by title or id
    Open { title: String },
    /// List all notes and their latest versions
    List,
    /// List all versions for a note
    Versions { title: String },
    /// Roll back to a specific (or previous) version
    Rollback {
        title: String,
        #[arg(short, long)]
        version: Option<u32>,
    },
    /// Delete a note by unique title
    Delete { title: String },
    /// Search notes by text in the latest version
    Search { query: String },
    /// Run the background daemon that syncs versions
    Daemon,
    /// Generate shell completions
    Completions { shell: CompletionShell },
    #[command(hide = true)]
    /// List note ids for shell completion
    Ids,
}

#[derive(Clone, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Index {
    notes: HashMap<String, NoteMeta>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NoteMeta {
    title: String,
    slug: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    current_version: u32,
    versions: Vec<VersionMeta>,
    working_hash: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct VersionMeta {
    version: u32,
    path: String,
    hash: String,
    created_at: DateTime<Utc>,
}

struct DataPaths {
    root: PathBuf,
    versions: PathBuf,
    files: PathBuf,
    index: PathBuf,
    daemon_pid: PathBuf,
    daemon_log: PathBuf,
}

struct NotesApp {
    paths: DataPaths,
    index: Index,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut app = NotesApp::load()?;

    if !matches!(cli.command, Commands::Daemon | Commands::Completions { .. } | Commands::Ids) {
        ensure_daemon_running(&app.paths)?;
    }

    match cli.command {
        Commands::New { title } => {
            let path = app.create_note(title)?;
            app.save()?;
            launch_subl_if_installed(&path);
            println!("{}", path.display());
        }
        Commands::Open { title } => {
            let path = app.open_note(&title)?;
            app.save()?;
            launch_subl_if_installed(&path);
            println!("{}", path.display());
        }
        Commands::List => {
            let _ = app.snapshot_all_changes()?;
            app.list_notes()?;
            app.save()?;
        }
        Commands::Versions { title } => {
            let _ = app.snapshot_all_changes()?;
            app.list_versions(&title)?;
            app.save()?;
        }
        Commands::Rollback { title, version } => {
            let path = app.rollback(&title, version)?;
            app.save()?;
            println!("{}", path.display());
        }
        Commands::Delete { title } => {
            let deleted = app.delete_note_by_title(&title)?;
            app.save()?;
            println!("Deleted note: {}", deleted);
        }
        Commands::Search { query } => {
            let _ = app.snapshot_all_changes()?;
            app.search(&query)?;
            app.save()?;
        }
        Commands::Daemon => {
            run_daemon(app.paths)?;
        }
        Commands::Completions { shell } => {
            print_completions(shell);
        }
        Commands::Ids => {
            app.list_ids()?;
        }
    }

    Ok(())
}

impl NotesApp {
    fn load() -> Result<Self> {
        let paths = DataPaths::new()?;
        paths.ensure_dirs()?;

        let index = if paths.index.exists() {
            let content = fs::read_to_string(&paths.index)
                .with_context(|| format!("Failed to read {}", paths.index.display()))?;
            serde_json::from_str::<Index>(&content)
                .with_context(|| format!("Failed to parse {}", paths.index.display()))?
        } else {
            Index::default()
        };

        Ok(Self { paths, index })
    }

    fn save(&self) -> Result<()> {
        let serialized = serde_json::to_string_pretty(&self.index)?;
        fs::write(&self.paths.index, serialized)
            .with_context(|| format!("Failed to write {}", self.paths.index.display()))
    }

    fn create_note(&mut self, title: Option<String>) -> Result<PathBuf> {
        let now = Utc::now();
        let title = title.unwrap_or_else(|| format!("note-{}", now.format("%Y%m%d-%H%M%S")));
        let mut slug = slugify(&title);

        let mut counter = 1;
        while self.index.notes.contains_key(&slug) {
            counter += 1;
            slug = format!("{}-{}", slugify(&title), counter);
        }

        let note_dir = self.paths.versions.join(&slug);
        fs::create_dir_all(&note_dir)
            .with_context(|| format!("Failed to create {}", note_dir.display()))?;

        let version_number = 1;
        let version_path_rel = format!("versions/{}/{:07}.md", slug, version_number);
        let version_path = self.paths.root.join(&version_path_rel);
        fs::create_dir_all(
            version_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid version path"))?,
        )?;
        fs::write(&version_path, b"")?;

        let working_path = self.paths.working_file(&slug);
        fs::create_dir_all(
            working_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid working path"))?,
        )?;
        fs::write(&working_path, b"")?;

        let hash = hash_bytes(b"");
        let version = VersionMeta {
            version: version_number,
            path: version_path_rel,
            hash: hash.clone(),
            created_at: now,
        };

        let meta = NoteMeta {
            title: title.clone(),
            slug: slug.clone(),
            created_at: now,
            updated_at: now,
            current_version: version_number,
            versions: vec![version],
            working_hash: Some(hash),
        };

        self.index.notes.insert(slug.clone(), meta);
        Ok(working_path)
    }

    fn open_note(&mut self, identifier: &str) -> Result<PathBuf> {
        let slug = self
            .resolve_slug(identifier)
            .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;

        self.snapshot_if_changed(&slug)?;

        Ok(self.paths.working_file(&slug))
    }

    fn list_notes(&self) -> Result<()> {
        if self.index.notes.is_empty() {
            println!("No notes yet. Run `notes new` to create one.");
            return Ok(());
        }

        let mut notes: Vec<&NoteMeta> = self.index.notes.values().collect();
        notes.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));

        for note in notes {
            let path = self.paths.working_file(&note.slug);
            println!(
                "- {} (id: {}) versions: {} current: {} path: {}",
                note.title,
                note.slug,
                note.versions.len(),
                note.current_version,
                path.display()
            );
        }

        Ok(())
    }

    fn list_versions(&mut self, identifier: &str) -> Result<()> {
        let slug = self
            .resolve_slug(identifier)
            .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;
        let note = self
            .index
            .notes
            .get(&slug)
            .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;

        println!("Versions for {}:", note.title);
        for version in &note.versions {
            println!(
                "  v{} @ {} ({})",
                version.version,
                version.created_at.to_rfc3339(),
                version.path
            );
        }

        Ok(())
    }

    fn list_ids(&self) -> Result<()> {
        let mut ids: Vec<&String> = self.index.notes.keys().collect();
        ids.sort();
        for id in ids {
            println!("{}", id);
        }
        Ok(())
    }

    fn rollback(&mut self, identifier: &str, target_version: Option<u32>) -> Result<PathBuf> {
        let slug = self
            .resolve_slug(identifier)
            .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;

        self.snapshot_if_changed(&slug)?;

        let (target, current_version) = {
            let note = self
                .index
                .notes
                .get(&slug)
                .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;

            let desired = match target_version {
                Some(v) => v,
                None => note.current_version.saturating_sub(1),
            };

            if desired == 0 {
                bail!("No previous version to roll back to");
            }

            let target = note
                .versions
                .iter()
                .find(|v| v.version == desired)
                .ok_or_else(|| anyhow!("Version {} not found", desired))?
                .clone();

            (target, note.current_version)
        };

        let content = fs::read(self.paths.root.join(&target.path))
            .with_context(|| format!("Failed to read {}", target.path))?;

        let hash = hash_bytes(&content);
        let new_version_number = current_version + 1;
        let new_version_rel = format!("versions/{}/{:07}.md", slug, new_version_number);
        let new_version_path = self.paths.root.join(&new_version_rel);
        fs::create_dir_all(
            new_version_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid version path"))?,
        )?;
        fs::write(&new_version_path, &content)?;

        let now = Utc::now();
        let new_meta = VersionMeta {
            version: new_version_number,
            path: new_version_rel,
            hash: hash.clone(),
            created_at: now,
        };

        if let Some(note) = self.index.notes.get_mut(&slug) {
            note.versions.push(new_meta);
            note.current_version = new_version_number;
            note.updated_at = now;
            note.working_hash = Some(hash);
        }

        let working_path = self.paths.working_file(&slug);
        fs::write(&working_path, &content)?;

        Ok(working_path)
    }

    fn delete_note_by_title(&mut self, title: &str) -> Result<String> {
        let slug = self.resolve_unique_title_slug(title)?;

        let note = self
            .index
            .notes
            .remove(&slug)
            .ok_or_else(|| anyhow!("Note not found: {}", title))?;

        let working_path = self.paths.working_file(&slug);
        if working_path.exists() {
            fs::remove_file(&working_path)
                .with_context(|| format!("Failed to remove {}", working_path.display()))?;
        }

        let versions_dir = self.paths.versions.join(&slug);
        if versions_dir.exists() {
            fs::remove_dir_all(&versions_dir)
                .with_context(|| format!("Failed to remove {}", versions_dir.display()))?;
        }

        Ok(note.slug)
    }

    fn search(&mut self, query: &str) -> Result<()> {
        let needle = query.to_lowercase();
        let mut matches_found = false;

        let mut notes: Vec<&NoteMeta> = self.index.notes.values().collect();
        notes.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));

        for note in notes {
            let content = fs::read_to_string(self.paths.current_version_path(note))
                .unwrap_or_else(|_| String::new());
            if content.to_lowercase().contains(&needle) {
                matches_found = true;
                println!("- {} (id: {})", note.title, note.slug);
            }
        }

        if !matches_found {
            println!("No matches found.");
        }

        Ok(())
    }

    fn snapshot_all_changes(&mut self) -> Result<Vec<String>> {
        let slugs: Vec<String> = self.index.notes.keys().cloned().collect();
        let mut updated = Vec::new();
        for slug in slugs {
            if self.snapshot_if_changed(&slug)? {
                updated.push(slug);
            }
        }
        Ok(updated)
    }

    fn snapshot_if_changed(&mut self, slug: &str) -> Result<bool> {
        self.ensure_working_copy_exists(slug)?;
        let note = self
            .index
            .notes
            .get_mut(slug)
            .ok_or_else(|| anyhow!("Note not found: {}", slug))?;
        let working_path = self.paths.working_file(slug);
        let content = fs::read(&working_path)
            .with_context(|| format!("Failed to read {}", working_path.display()))?;
        let hash = hash_bytes(&content);

        if let Some(last) = note.versions.last() {
            if last.hash == hash {
                note.working_hash = Some(hash);
                return Ok(false);
            }
        }

        let new_version_number = note.current_version + 1;
        let version_rel = format!("versions/{}/{:07}.md", slug, new_version_number);
        let version_path = self.paths.root.join(&version_rel);
        fs::create_dir_all(
            version_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid version path"))?,
        )?;
        fs::write(&version_path, &content)?;

        let now = Utc::now();
        let meta = VersionMeta {
            version: new_version_number,
            path: version_rel,
            hash: hash.clone(),
            created_at: now,
        };

        note.versions.push(meta);
        note.current_version = new_version_number;
        note.updated_at = now;
        note.working_hash = Some(hash);

        Ok(true)
    }

    fn ensure_working_copy_exists(&self, slug: &str) -> Result<()> {
        let working_path = self.paths.working_file(slug);
        if working_path.exists() {
            return Ok(());
        }

        let note = self
            .index
            .notes
            .get(slug)
            .ok_or_else(|| anyhow!("Note not found: {}", slug))?;
        let source = self.paths.current_version_path(note);
        let content =
            fs::read(&source).with_context(|| format!("Failed to read {}", source.display()))?;
        fs::create_dir_all(
            working_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid working path"))?,
        )?;
        fs::write(&working_path, content)?;

        Ok(())
    }

    fn resolve_slug(&self, identifier: &str) -> Option<String> {
        if self.index.notes.contains_key(identifier) {
            return Some(identifier.to_string());
        }

        let id_lower = identifier.to_lowercase();
        self.index
            .notes
            .values()
            .find(|note| note.title.to_lowercase() == id_lower || note.slug == id_lower)
            .map(|note| note.slug.clone())
    }

    fn resolve_unique_title_slug(&self, title: &str) -> Result<String> {
        let matches: Vec<&NoteMeta> = self
            .index
            .notes
            .values()
            .filter(|note| note.slug.to_lowercase() == title)
            .collect();

        match matches.as_slice() {
            [] => bail!("Note not found: {}", title),
            [note] => Ok(note.slug.clone()),
            _ => bail!("Multiple notes match title: {}", title),
        }
    }
}

impl DataPaths {
    fn new() -> Result<Self> {
        let root = match env::var("NOTES_HOME") {
            Ok(path) => PathBuf::from(path),
            Err(_) => dirs::home_dir()
                .map(|p| p.join(".notes"))
                .ok_or_else(|| anyhow!("Unable to determine home directory"))?,
        };

        Ok(Self {
            index: root.join("index.json"),
            versions: root.join("versions"),
            files: root.join("files"),
            daemon_pid: root.join("daemon.pid"),
            daemon_log: root.join("daemon.log"),
            root,
        })
    }

    fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.versions)?;
        fs::create_dir_all(&self.files)?;
        Ok(())
    }

    fn working_file(&self, slug: &str) -> PathBuf {
        self.files.join(format!("{slug}.md"))
    }

    fn current_version_path(&self, note: &NoteMeta) -> PathBuf {
        if let Some(version) = note
            .versions
            .iter()
            .find(|v| v.version == note.current_version)
        {
            return self.root.join(&version.path);
        }

        self.root
            .join(&note.versions.last().expect("note has versions").path)
    }
}

fn ensure_daemon_running(paths: &DataPaths) -> Result<()> {
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

fn run_daemon(paths: DataPaths) -> Result<()> {
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

fn print_completions(shell: CompletionShell) {
    let script = match shell {
        CompletionShell::Bash => BASH_COMPLETION,
        CompletionShell::Zsh => ZSH_COMPLETION,
        CompletionShell::Fish => FISH_COMPLETION,
    };
    print!("{script}");
}

const BASH_COMPLETION: &str = r#"_notes_ids() {
  NOTES_DISABLE_DAEMON=1 notes ids 2>/dev/null | tr '\n' ' '
}

_notes_complete() {
  local cur cmd
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  cmd="${COMP_WORDS[1]}"

  case "$cmd" in
    open|versions|delete|rollback)
      local has_title=0
      local i=2
      while [[ $i -lt $COMP_CWORD ]]; do
        local word="${COMP_WORDS[i]}"
        if [[ "$word" == "--version" || "$word" == "-v" ]]; then
          ((i+=2))
          continue
        fi
        if [[ "$word" != -* ]]; then
          has_title=1
          break
        fi
        ((i+=1))
      done
      if [[ $has_title -eq 0 && "$cur" != -* ]]; then
        local ids
        ids=$(_notes_ids)
        COMPREPLY=( $(compgen -W "$ids" -- "$cur") )
      fi
      return 0
      ;;
  esac
}

complete -F _notes_complete notes
"#;

const ZSH_COMPLETION: &str = r#"#compdef notes
autoload -U +X bashcompinit && bashcompinit

_notes_ids() {
  NOTES_DISABLE_DAEMON=1 notes ids 2>/dev/null | tr '\n' ' '
}

_notes_complete() {
  local cur cmd
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  cmd="${COMP_WORDS[1]}"

  case "$cmd" in
    open|versions|delete|rollback)
      local has_title=0
      local i=2
      while [[ $i -lt $COMP_CWORD ]]; do
        local word="${COMP_WORDS[i]}"
        if [[ "$word" == "--version" || "$word" == "-v" ]]; then
          ((i+=2))
          continue
        fi
        if [[ "$word" != -* ]]; then
          has_title=1
          break
        fi
        ((i+=1))
      done
      if [[ $has_title -eq 0 && "$cur" != -* ]]; then
        local ids
        ids=$(_notes_ids)
        COMPREPLY=( $(compgen -W "$ids" -- "$cur") )
      fi
      return 0
      ;;
  esac
}

complete -F _notes_complete notes
"#;

const FISH_COMPLETION: &str = r#"function __notes_ids
    NOTES_DISABLE_DAEMON=1 notes ids 2>/dev/null
end

function __notes_needs_id
    set -l cmd (commandline -opc)
    if test (count $cmd) -lt 2
        return 1
    end
    set -l sub $cmd[2]
    switch $sub
        case open versions delete rollback
            set -l i 3
            while test $i -le (count $cmd)
                set -l word $cmd[$i]
                if test "$word" = "--version" -o "$word" = "-v"
                    set i (math $i + 2)
                    continue
                end
                if not string match -qr '^-.*' -- $word
                    return 1
                end
                set i (math $i + 1)
            end
            return 0
    end
    return 1
end

complete -c notes -n '__notes_needs_id' -a '(__notes_ids)'
"#;

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() || c == '-' || c == '_' {
            if !slug.ends_with('-') {
                slug.push('-');
            }
        }
    }

    if slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        "note".to_string()
    } else {
        slug
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn launch_subl_if_installed(path: &PathBuf) {
    if !is_subl_available() {
        return;
    }

    let _ = std::process::Command::new("subl")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn is_subl_available() -> bool {
    std::process::Command::new("subl")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
