use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use regex::Regex;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{mpsc, OnceLock};
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
    /// Bullet journal - quick capture and task management
    #[command(alias = "b")]
    Bullet {
        #[command(subcommand)]
        action: Option<BulletAction>,

        /// Quick add text (defaults to task)
        #[arg(trailing_var_arg = true, num_args = 0..)]
        text: Vec<String>,

        /// Create a task entry
        #[arg(short = 't', long, conflicts_with_all = ["event", "note"])]
        task: bool,

        /// Create an event entry
        #[arg(short = 'e', long, conflicts_with_all = ["task", "note"])]
        event: bool,

        /// Create a note entry
        #[arg(short = 'n', long, conflicts_with_all = ["task", "event"])]
        note: bool,

        /// Target date (YYYY-MM-DD, "today", "yesterday", "tomorrow")
        #[arg(short = 'd', long)]
        date: Option<String>,

        /// Add to weekly log instead of daily
        #[arg(short = 'w', long, conflicts_with = "monthly")]
        weekly: bool,

        /// Add to monthly log instead of daily
        #[arg(short = 'm', long, conflicts_with = "weekly")]
        monthly: bool,
    },
    /// Interactive bullet journal mode
    #[command(alias = "bi")]
    BulletInteractive,
}

#[derive(Subcommand)]
enum BulletAction {
    /// List journal entries
    #[command(alias = "ls")]
    List {
        /// Specific date to show (default: today)
        #[arg(short = 'd', long)]
        date: Option<String>,

        /// Show current week's entries
        #[arg(short = 'w', long, conflicts_with_all = ["month", "date"])]
        week: bool,

        /// Show current month's entries
        #[arg(short = 'm', long, conflicts_with_all = ["week", "date"])]
        month: bool,
    },

    /// Show incomplete/pending tasks
    #[command(alias = "p")]
    Pending {
        /// Include tasks from past N days (default: 7)
        #[arg(short = 'd', long, default_value = "7")]
        days: u32,
    },

    /// Mark a task as complete
    #[command(alias = "x")]
    Complete {
        /// Entry ID or partial match
        entry: String,
    },

    /// Migrate incomplete tasks to today
    #[command(alias = "mg")]
    Migrate {
        /// Migrate all incomplete tasks from yesterday
        #[arg(short = 'a', long)]
        all: bool,

        /// Source date to migrate from (default: yesterday)
        #[arg(short = 'f', long)]
        from: Option<String>,
    },

    /// Open the journal file in editor
    #[command(alias = "o")]
    Open {
        /// Date to open (default: today)
        #[arg(short = 'd', long)]
        date: Option<String>,

        /// Open weekly file
        #[arg(short = 'w', long)]
        weekly: bool,

        /// Open monthly file
        #[arg(short = 'm', long)]
        monthly: bool,
    },

    /// Search journal entries
    Search {
        /// Search query
        query: String,
    },

    /// Interactive mode
    #[command(alias = "i")]
    Interactive,

    /// List entry IDs for shell completion
    #[command(hide = true)]
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

// ============================================================================
// Bullet Journal Types
// ============================================================================

/// Number of days to search back when completing tasks
const TASK_COMPLETION_SEARCH_DAYS: i64 = 30;

/// Number of days to search back when searching journal entries
const JOURNAL_SEARCH_DAYS: i64 = 90;

/// Number of days to look back for listing entry IDs (shell completion)
const ENTRY_IDS_SEARCH_DAYS: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum BulletType {
    Task,
    Event,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum TaskState {
    Incomplete,
    Complete,
    Migrated,
    Scheduled,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct BulletEntry {
    id: String,
    bullet_type: BulletType,
    task_state: Option<TaskState>,
    content: String,
    created_at: DateTime<Utc>,
    date: NaiveDate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum JournalPeriod {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct JournalEntryRef {
    date_key: String,
    bullet_type: BulletType,
    task_state: Option<TaskState>,
    content_preview: String,
}

#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct JournalIndex {
    entries: HashMap<String, JournalEntryRef>,
}

struct BulletJournal {
    paths: DataPaths,
    index: JournalIndex,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut app = NotesApp::load()?;

    if !matches!(
        cli.command,
        Commands::Daemon
            | Commands::Completions { .. }
            | Commands::Ids
            | Commands::Bullet { .. }
            | Commands::BulletInteractive
    ) {
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
        Commands::Bullet {
            action,
            text,
            task,
            event,
            note,
            date,
            weekly,
            monthly,
        } => {
            handle_bullet_command(action, text, task, event, note, date, weekly, monthly)?;
        }
        Commands::BulletInteractive => {
            let paths = DataPaths::new()?;
            let mut journal = BulletJournal::load(paths)?;
            journal.run_interactive()?;
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

    // Journal path methods
    fn journal_root(&self) -> PathBuf {
        self.root.join("journal")
    }

    fn journal_index(&self) -> PathBuf {
        self.journal_root().join("index.json")
    }

    fn journal_daily_dir(&self) -> PathBuf {
        self.journal_root().join("daily")
    }

    fn journal_weekly_dir(&self) -> PathBuf {
        self.journal_root().join("weekly")
    }

    fn journal_monthly_dir(&self) -> PathBuf {
        self.journal_root().join("monthly")
    }

    fn daily_file(&self, date: NaiveDate) -> PathBuf {
        self.journal_daily_dir()
            .join(format!("{}.md", date.format("%Y-%m-%d")))
    }

    fn weekly_file(&self, year: i32, week: u32) -> PathBuf {
        self.journal_weekly_dir()
            .join(format!("{}-W{:02}.md", year, week))
    }

    fn monthly_file(&self, year: i32, month: u32) -> PathBuf {
        self.journal_monthly_dir()
            .join(format!("{}-{:02}.md", year, month))
    }

    fn ensure_journal_dirs(&self) -> Result<()> {
        fs::create_dir_all(self.journal_daily_dir())?;
        fs::create_dir_all(self.journal_weekly_dir())?;
        fs::create_dir_all(self.journal_monthly_dir())?;
        Ok(())
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
    paths.ensure_journal_dirs()?;
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

    // Watch journal directories
    let _ = watcher.watch(&paths.journal_daily_dir(), RecursiveMode::NonRecursive);
    let _ = watcher.watch(&paths.journal_weekly_dir(), RecursiveMode::NonRecursive);
    let _ = watcher.watch(&paths.journal_monthly_dir(), RecursiveMode::NonRecursive);

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

// ============================================================================
// Bullet Journal Implementation
// ============================================================================

impl BulletJournal {
    fn load(paths: DataPaths) -> Result<Self> {
        paths.ensure_journal_dirs()?;

        let index = if paths.journal_index().exists() {
            let content = fs::read_to_string(paths.journal_index())
                .with_context(|| format!("Failed to read {}", paths.journal_index().display()))?;
            serde_json::from_str::<JournalIndex>(&content).unwrap_or_default()
        } else {
            JournalIndex::default()
        };

        Ok(Self { paths, index })
    }

    fn save(&self) -> Result<()> {
        let serialized = serde_json::to_string_pretty(&self.index)?;
        fs::write(self.paths.journal_index(), serialized)
            .with_context(|| format!("Failed to write {}", self.paths.journal_index().display()))
    }

    fn add_entry(
        &mut self,
        content: &str,
        bullet_type: BulletType,
        date: NaiveDate,
        period: JournalPeriod,
    ) -> Result<String> {
        let id = generate_entry_id();
        let now = Utc::now();

        let task_state = match bullet_type {
            BulletType::Task => Some(TaskState::Incomplete),
            _ => None,
        };

        // Calculate preview before entry is moved
        let preview = if content.len() > 50 {
            format!("{}...", &content[..47])
        } else {
            content.to_string()
        };

        let entry = BulletEntry {
            id: id.clone(),
            bullet_type,
            task_state,
            content: content.to_string(),
            created_at: now,
            date,
        };

        // Get file path and date key based on period
        let (file_path, date_key) = match period {
            JournalPeriod::Daily => {
                let path = self.paths.daily_file(date);
                let key = date.format("%Y-%m-%d").to_string();
                (path, key)
            }
            JournalPeriod::Weekly => {
                let week = date.iso_week().week();
                let year = date.iso_week().year();
                let path = self.paths.weekly_file(year, week);
                let key = format!("{}-W{:02}", year, week);
                (path, key)
            }
            JournalPeriod::Monthly => {
                let path = self.paths.monthly_file(date.year(), date.month());
                let key = format!("{}-{:02}", date.year(), date.month());
                (path, key)
            }
        };

        // Read existing content or create new file
        let mut entries = if file_path.exists() {
            let file_content = fs::read_to_string(&file_path)?;
            parse_journal_file(&file_content, date)?
        } else {
            Vec::new()
        };

        entries.push(entry);

        // Write file
        let file_content = format_journal_file(&entries, period, &date_key);
        fs::write(&file_path, &file_content)?;

        self.index.entries.insert(
            id.clone(),
            JournalEntryRef {
                date_key,
                bullet_type,
                task_state,
                content_preview: preview,
            },
        );

        Ok(id)
    }

    fn list_daily(&self, date: NaiveDate) -> Result<Vec<BulletEntry>> {
        let file_path = self.paths.daily_file(date);
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&file_path)?;
        parse_journal_file(&content, date)
    }

    fn list_weekly(&self, year: i32, week: u32) -> Result<Vec<BulletEntry>> {
        let file_path = self.paths.weekly_file(year, week);
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&file_path)?;
        // Use first day of week as date
        let date = NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon)
            .unwrap_or_else(|| Utc::now().date_naive());
        parse_journal_file(&content, date)
    }

    fn list_monthly(&self, year: i32, month: u32) -> Result<Vec<BulletEntry>> {
        let file_path = self.paths.monthly_file(year, month);
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&file_path)?;
        let date = NaiveDate::from_ymd_opt(year, month, 1)
            .unwrap_or_else(|| Utc::now().date_naive());
        parse_journal_file(&content, date)
    }

    fn list_pending(&self, days_back: u32) -> Result<Vec<BulletEntry>> {
        let today = Utc::now().date_naive();
        let mut pending = Vec::new();

        for i in 0..days_back {
            let date = today - chrono::Duration::days(i as i64);
            let entries = self.list_daily(date)?;
            for entry in entries {
                if entry.bullet_type == BulletType::Task
                    && entry.task_state == Some(TaskState::Incomplete)
                {
                    pending.push(entry);
                }
            }
        }

        Ok(pending)
    }

    fn complete_task(&mut self, partial_id: &str) -> Result<()> {
        let today = Utc::now().date_naive();

        // Search in recent daily files
        for i in 0..TASK_COMPLETION_SEARCH_DAYS {
            let date = today - chrono::Duration::days(i);
            let file_path = self.paths.daily_file(date);
            if !file_path.exists() {
                continue;
            }

            let content = fs::read_to_string(&file_path)?;
            let mut entries = parse_journal_file(&content, date)?;
            let mut found = false;

            for entry in &mut entries {
                if entry.id.starts_with(partial_id) && entry.bullet_type == BulletType::Task {
                    entry.task_state = Some(TaskState::Complete);
                    found = true;

                    // Update index
                    if let Some(ref_entry) = self.index.entries.get_mut(&entry.id) {
                        ref_entry.task_state = Some(TaskState::Complete);
                    }
                    break;
                }
            }

            if found {
                let date_key = date.format("%Y-%m-%d").to_string();
                let content = format_journal_file(&entries, JournalPeriod::Daily, &date_key);
                fs::write(&file_path, content)?;
                return Ok(());
            }
        }

        bail!("Entry not found: {}", partial_id)
    }

    fn migrate_tasks(&mut self, from_date: NaiveDate, all: bool) -> Result<Vec<String>> {
        let today = Utc::now().date_naive();
        let from_file = self.paths.daily_file(from_date);

        if !from_file.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&from_file)?;
        let mut from_entries = parse_journal_file(&content, from_date)?;
        let mut migrated_ids = Vec::new();

        // Find incomplete tasks
        let mut tasks_to_migrate: Vec<usize> = Vec::new();
        for (i, entry) in from_entries.iter().enumerate() {
            if entry.bullet_type == BulletType::Task
                && entry.task_state == Some(TaskState::Incomplete)
                && (all || tasks_to_migrate.is_empty())
            {
                tasks_to_migrate.push(i);
            }
        }

        if tasks_to_migrate.is_empty() {
            return Ok(Vec::new());
        }

        // Create new entries in today's file
        let to_file = self.paths.daily_file(today);
        let mut to_entries = if to_file.exists() {
            let content = fs::read_to_string(&to_file)?;
            parse_journal_file(&content, today)?
        } else {
            Vec::new()
        };

        for &idx in &tasks_to_migrate {
            let old_entry = &from_entries[idx];
            let new_id = generate_entry_id();

            let new_entry = BulletEntry {
                id: new_id.clone(),
                bullet_type: BulletType::Task,
                task_state: Some(TaskState::Incomplete),
                content: old_entry.content.clone(),
                created_at: Utc::now(),
                date: today,
            };

            to_entries.push(new_entry);
            migrated_ids.push(new_id);
        }

        // Mark original tasks as migrated
        for &idx in &tasks_to_migrate {
            from_entries[idx].task_state = Some(TaskState::Migrated);
        }

        // Write both files
        let from_key = from_date.format("%Y-%m-%d").to_string();
        let to_key = today.format("%Y-%m-%d").to_string();

        fs::write(
            &from_file,
            format_journal_file(&from_entries, JournalPeriod::Daily, &from_key),
        )?;
        fs::write(
            &to_file,
            format_journal_file(&to_entries, JournalPeriod::Daily, &to_key),
        )?;

        Ok(migrated_ids)
    }

    fn search(&self, query: &str) -> Result<Vec<BulletEntry>> {
        let needle = query.to_lowercase();
        let today = Utc::now().date_naive();
        let mut results = Vec::new();

        // Search in daily files
        for i in 0..JOURNAL_SEARCH_DAYS {
            let date = today - chrono::Duration::days(i);
            let file_path = self.paths.daily_file(date);
            if !file_path.exists() {
                continue;
            }

            let content = fs::read_to_string(&file_path)?;
            let entries = parse_journal_file(&content, date)?;

            for entry in entries {
                if entry.content.to_lowercase().contains(&needle) {
                    results.push(entry);
                }
            }
        }

        Ok(results)
    }

    fn open_file(&self, date: NaiveDate, period: JournalPeriod) -> Result<PathBuf> {
        let file_path = match period {
            JournalPeriod::Daily => self.paths.daily_file(date),
            JournalPeriod::Weekly => {
                let week = date.iso_week().week();
                let year = date.iso_week().year();
                self.paths.weekly_file(year, week)
            }
            JournalPeriod::Monthly => self.paths.monthly_file(date.year(), date.month()),
        };

        // Create file if it doesn't exist
        if !file_path.exists() {
            let date_key = match period {
                JournalPeriod::Daily => date.format("%Y-%m-%d").to_string(),
                JournalPeriod::Weekly => {
                    let week = date.iso_week().week();
                    let year = date.iso_week().year();
                    format!("{}-W{:02}", year, week)
                }
                JournalPeriod::Monthly => format!("{}-{:02}", date.year(), date.month()),
            };
            let content = format_journal_file(&[], period, &date_key);
            fs::write(&file_path, content)?;
        }

        Ok(file_path)
    }

    fn run_interactive(&mut self) -> Result<()> {
        use std::io::{BufRead, Write};

        println!("Bullet Journal Interactive Mode");
        println!("Commands: t <text> = task, e <text> = event, n <text> = note");
        println!("          l = list today, p = pending, x <id> = complete, q = quit");
        println!();

        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();

        loop {
            print!("> ");
            stdout.flush()?;

            let mut input = String::new();
            if stdin.lock().read_line(&mut input)? == 0 {
                break;
            }

            let input = input.trim();
            if input.is_empty() {
                continue;
            }

            let today = Utc::now().date_naive();

            match input.chars().next() {
                Some('t') => {
                    let content = input[1..].trim();
                    if !content.is_empty() {
                        let id = self.add_entry(
                            content,
                            BulletType::Task,
                            today,
                            JournalPeriod::Daily,
                        )?;
                        println!("Added task: {} [{}]", content, &id[..4.min(id.len())]);
                    }
                }
                Some('e') => {
                    let content = input[1..].trim();
                    if !content.is_empty() {
                        let id = self.add_entry(
                            content,
                            BulletType::Event,
                            today,
                            JournalPeriod::Daily,
                        )?;
                        println!("Added event: {} [{}]", content, &id[..4.min(id.len())]);
                    }
                }
                Some('n') => {
                    let content = input[1..].trim();
                    if !content.is_empty() {
                        let id = self.add_entry(
                            content,
                            BulletType::Note,
                            today,
                            JournalPeriod::Daily,
                        )?;
                        println!("Added note: {} [{}]", content, &id[..4.min(id.len())]);
                    }
                }
                Some('l') => {
                    let entries = self.list_daily(today)?;
                    print_entries(&entries);
                }
                Some('p') => {
                    let entries = self.list_pending(7)?;
                    print_entries(&entries);
                }
                Some('x') => {
                    let id = input[1..].trim();
                    if !id.is_empty() {
                        match self.complete_task(id) {
                            Ok(()) => println!("Marked complete: {}", id),
                            Err(e) => println!("Error: {}", e),
                        }
                    }
                }
                Some('q') | Some('Q') => break,
                _ => {
                    // Default to task
                    let id =
                        self.add_entry(input, BulletType::Task, today, JournalPeriod::Daily)?;
                    println!("Added task: {} [{}]", input, &id[..4.min(id.len())]);
                }
            }
        }

        self.save()?;
        Ok(())
    }

    fn list_ids(&self) -> Result<()> {
        let today = Utc::now().date_naive();

        // Collect IDs from recent daily files
        for i in 0..ENTRY_IDS_SEARCH_DAYS {
            let date = today - chrono::Duration::days(i);
            let entries = self.list_daily(date)?;
            for entry in entries {
                // Truncate content if too long
                let content = if entry.content.len() > 40 {
                    format!("{}...", &entry.content[..37])
                } else {
                    entry.content.clone()
                };
                println!("{}\t({})", &entry.id[..8.min(entry.id.len())], content);
            }
        }

        Ok(())
    }
}

fn generate_entry_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let random = RandomState::new().build_hasher().finish();
    format!(
        "{:08x}{:04x}",
        (timestamp & 0xFFFFFFFF) as u32,
        (random & 0xFFFF) as u16
    )
}

fn parse_journal_file(content: &str, date: NaiveDate) -> Result<Vec<BulletEntry>> {
    static ENTRY_RE: OnceLock<Regex> = OnceLock::new();
    let entry_re = ENTRY_RE.get_or_init(|| {
        Regex::new(r"^- \[(.)\] (.+?) \{id:([a-f0-9]{8,12})\}$").unwrap()
    });
    let mut entries = Vec::new();

    for line in content.lines() {
        if let Some(caps) = entry_re.captures(line) {
            let marker = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let text = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let id = caps.get(3).map(|m| m.as_str()).unwrap_or("");

            let (bullet_type, task_state) = match marker {
                " " => (BulletType::Task, Some(TaskState::Incomplete)),
                "x" => (BulletType::Task, Some(TaskState::Complete)),
                ">" => (BulletType::Task, Some(TaskState::Migrated)),
                "<" => (BulletType::Task, Some(TaskState::Scheduled)),
                "o" => (BulletType::Event, None),
                "-" => (BulletType::Note, None),
                _ => continue,
            };

            entries.push(BulletEntry {
                id: id.to_string(),
                bullet_type,
                task_state,
                content: text.to_string(),
                created_at: Utc::now(), // We don't store this in file
                date,
            });
        }
    }

    Ok(entries)
}

fn format_journal_file(entries: &[BulletEntry], period: JournalPeriod, date_key: &str) -> String {
    let mut output = String::new();

    // Header
    let header = match period {
        JournalPeriod::Daily => format!("# Daily Log - {}\n\n", date_key),
        JournalPeriod::Weekly => format!("# Weekly Log - {}\n\n", date_key),
        JournalPeriod::Monthly => format!("# Monthly Log - {}\n\n", date_key),
    };
    output.push_str(&header);

    // Group by type
    let tasks: Vec<_> = entries
        .iter()
        .filter(|e| e.bullet_type == BulletType::Task)
        .collect();
    let events: Vec<_> = entries
        .iter()
        .filter(|e| e.bullet_type == BulletType::Event)
        .collect();
    let notes: Vec<_> = entries
        .iter()
        .filter(|e| e.bullet_type == BulletType::Note)
        .collect();

    if !tasks.is_empty() {
        output.push_str("## Tasks\n");
        for entry in tasks {
            output.push_str(&format_entry(entry));
        }
        output.push('\n');
    }

    if !events.is_empty() {
        output.push_str("## Events\n");
        for entry in events {
            output.push_str(&format_entry(entry));
        }
        output.push('\n');
    }

    if !notes.is_empty() {
        output.push_str("## Notes\n");
        for entry in notes {
            output.push_str(&format_entry(entry));
        }
        output.push('\n');
    }

    output
}

fn format_entry(entry: &BulletEntry) -> String {
    let marker = match (entry.bullet_type, entry.task_state) {
        (BulletType::Task, Some(TaskState::Incomplete)) => " ",
        (BulletType::Task, Some(TaskState::Complete)) => "x",
        (BulletType::Task, Some(TaskState::Migrated)) => ">",
        (BulletType::Task, Some(TaskState::Scheduled)) => "<",
        (BulletType::Event, _) => "o",
        (BulletType::Note, _) => "-",
        _ => " ",
    };
    format!("- [{}] {} {{id:{}}}\n", marker, entry.content, entry.id)
}

fn print_entries(entries: &[BulletEntry]) {
    if entries.is_empty() {
        println!("No entries.");
        return;
    }

    for entry in entries {
        let marker = match (entry.bullet_type, entry.task_state) {
            (BulletType::Task, Some(TaskState::Incomplete)) => "[ ]",
            (BulletType::Task, Some(TaskState::Complete)) => "[x]",
            (BulletType::Task, Some(TaskState::Migrated)) => "[>]",
            (BulletType::Task, Some(TaskState::Scheduled)) => "[<]",
            (BulletType::Event, _) => "[o]",
            (BulletType::Note, _) => "[-]",
            _ => "[ ]",
        };
        println!(
            "{} {} ({}) [{}]",
            marker,
            entry.content,
            entry.date.format("%Y-%m-%d"),
            &entry.id[..4.min(entry.id.len())]
        );
    }
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    let today = Utc::now().date_naive();
    match s.to_lowercase().as_str() {
        "today" => Ok(today),
        "yesterday" => Ok(today - chrono::Duration::days(1)),
        "tomorrow" => Ok(today + chrono::Duration::days(1)),
        _ => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .with_context(|| format!("Invalid date format: {}. Use YYYY-MM-DD", s)),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_bullet_command(
    action: Option<BulletAction>,
    text: Vec<String>,
    task: bool,
    event: bool,
    note: bool,
    date: Option<String>,
    weekly: bool,
    monthly: bool,
) -> Result<()> {
    let paths = DataPaths::new()?;
    let mut journal = BulletJournal::load(paths)?;

    // Handle subcommands
    if let Some(action) = action {
        match action {
            BulletAction::List {
                date: list_date,
                week,
                month,
            } => {
                let today = Utc::now().date_naive();
                let entries = if week {
                    let week_num = today.iso_week().week();
                    let year = today.iso_week().year();
                    journal.list_weekly(year, week_num)?
                } else if month {
                    journal.list_monthly(today.year(), today.month())?
                } else {
                    let target_date = list_date.map(|d| parse_date(&d)).transpose()?.unwrap_or(today);
                    journal.list_daily(target_date)?
                };
                print_entries(&entries);
            }
            BulletAction::Pending { days } => {
                let entries = journal.list_pending(days)?;
                print_entries(&entries);
            }
            BulletAction::Complete { entry } => {
                journal.complete_task(&entry)?;
                println!("Marked complete: {}", entry);
            }
            BulletAction::Migrate { all, from } => {
                let today = Utc::now().date_naive();
                let from_date = from
                    .map(|d| parse_date(&d))
                    .transpose()?
                    .unwrap_or(today - chrono::Duration::days(1));
                let migrated = journal.migrate_tasks(from_date, all)?;
                if migrated.is_empty() {
                    println!("No tasks to migrate.");
                } else {
                    println!("Migrated {} task(s) to today.", migrated.len());
                }
            }
            BulletAction::Open {
                date: open_date,
                weekly: open_weekly,
                monthly: open_monthly,
            } => {
                let today = Utc::now().date_naive();
                let target_date = open_date.map(|d| parse_date(&d)).transpose()?.unwrap_or(today);
                let period = if open_weekly {
                    JournalPeriod::Weekly
                } else if open_monthly {
                    JournalPeriod::Monthly
                } else {
                    JournalPeriod::Daily
                };
                let path = journal.open_file(target_date, period)?;
                launch_subl_if_installed(&path);
                println!("{}", path.display());
            }
            BulletAction::Search { query } => {
                let entries = journal.search(&query)?;
                print_entries(&entries);
            }
            BulletAction::Interactive => {
                journal.run_interactive()?;
                return Ok(());
            }
            BulletAction::Ids => {
                journal.list_ids()?;
                return Ok(());
            }
        }
        journal.save()?;
        return Ok(());
    }

    // Quick add mode
    if !text.is_empty() {
        let content = text.join(" ");
        let today = Utc::now().date_naive();
        let target_date = date.map(|d| parse_date(&d)).transpose()?.unwrap_or(today);

        let bullet_type = if event {
            BulletType::Event
        } else if note {
            BulletType::Note
        } else if task {
            BulletType::Task
        } else {
            // Default to task when no flag is specified
            BulletType::Task
        };

        let period = if weekly {
            JournalPeriod::Weekly
        } else if monthly {
            JournalPeriod::Monthly
        } else {
            JournalPeriod::Daily
        };

        let id = journal.add_entry(&content, bullet_type, target_date, period)?;
        journal.save()?;

        let type_name = match bullet_type {
            BulletType::Task => "task",
            BulletType::Event => "event",
            BulletType::Note => "note",
        };
        println!("Added {}: {} [{}]", type_name, content, &id[..4.min(id.len())]);
        return Ok(());
    }

    // No text and no subcommand - show today's entries
    let today = Utc::now().date_naive();
    let entries = journal.list_daily(today)?;
    print_entries(&entries);

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

_notes_bullet_ids() {
  # Output format: "id<TAB>(description)" - we extract just the id for completion
  NOTES_DISABLE_DAEMON=1 notes bullet ids 2>/dev/null | cut -f1 | tr '\n' ' '
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
    bullet|b)
      if [[ $COMP_CWORD -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "list pending complete migrate open search interactive" -- "$cur") )
      elif [[ $COMP_CWORD -eq 3 ]]; then
        local subcmd="${COMP_WORDS[2]}"
        if [[ "$subcmd" == "complete" || "$subcmd" == "x" ]]; then
          local ids
          ids=$(_notes_bullet_ids)
          COMPREPLY=( $(compgen -W "$ids" -- "$cur") )
        fi
      fi
      return 0
      ;;
  esac
}

complete -F _notes_complete notes
"#;

const ZSH_COMPLETION: &str = r#"#compdef notes

_notes_ids() {
  local -a ids
  ids=(${(f)"$(NOTES_DISABLE_DAEMON=1 notes ids 2>/dev/null)"})
  _describe 'note id' ids
}

_notes_bullet_ids() {
  local -a ids
  # Format: "id (description)" -> zsh format "id:description"
  ids=(${(f)"$(NOTES_DISABLE_DAEMON=1 notes bullet ids 2>/dev/null | sed 's/\t/:/g' | sed 's/(\(.*\))/\1/')"})
  _describe 'entry id' ids
}

_notes_bullet() {
  local -a subcmds
  subcmds=(
    'list:List journal entries'
    'pending:Show incomplete/pending tasks'
    'complete:Mark a task as complete'
    'migrate:Migrate incomplete tasks to today'
    'open:Open the journal file in editor'
    'search:Search journal entries'
    'interactive:Interactive mode'
  )

  # CURRENT counts all words: notes=1, bullet=2, subcommand=3, arg=4
  if (( CURRENT == 3 )); then
    _describe 'bullet subcommand' subcmds
  elif (( CURRENT == 4 )); then
    case "$words[3]" in
      complete|x)
        _notes_bullet_ids
        ;;
    esac
  fi
}

_notes() {
  local -a commands
  commands=(
    'new:Create a new note'
    'open:Open an existing note'
    'list:List all notes'
    'versions:List all versions for a note'
    'rollback:Roll back to a specific version'
    'delete:Delete a note'
    'search:Search notes by text'
    'daemon:Run the background daemon'
    'completions:Generate shell completions'
    'bullet:Bullet journal'
    'b:Bullet journal (alias)'
    'bi:Bullet journal interactive mode'
  )

  if (( CURRENT == 2 )); then
    _describe 'command' commands
  elif (( CURRENT >= 3 )); then
    case "$words[2]" in
      open|versions|delete|rollback)
        _notes_ids
        ;;
      bullet|b)
        _notes_bullet
        ;;
    esac
  fi
}

_notes "$@"
"#;

const FISH_COMPLETION: &str = r#"function __notes_ids
    NOTES_DISABLE_DAEMON=1 notes ids 2>/dev/null
end

function __notes_bullet_ids
    # Output format: "id<TAB>(description)" - Fish handles this natively for descriptions
    NOTES_DISABLE_DAEMON=1 notes bullet ids 2>/dev/null
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

function __notes_bullet_subcommand
    set -l cmd (commandline -opc)
    if test (count $cmd) -eq 2
        if test "$cmd[2]" = "bullet" -o "$cmd[2]" = "b"
            return 0
        end
    end
    return 1
end

function __notes_bullet_needs_id
    set -l cmd (commandline -opc)
    if test (count $cmd) -eq 3
        if test "$cmd[2]" = "bullet" -o "$cmd[2]" = "b"
            if test "$cmd[3]" = "complete" -o "$cmd[3]" = "x"
                return 0
            end
        end
    end
    return 1
end

complete -c notes -n '__notes_needs_id' -a '(__notes_ids)'
complete -c notes -n '__notes_bullet_subcommand' -a 'list pending complete migrate open search interactive'
complete -c notes -n '__notes_bullet_needs_id' -a '(__notes_bullet_ids)'
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
