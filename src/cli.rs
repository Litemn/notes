use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "notes", about = "Local notes with version control")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
    /// Launch the desktop UI
    Ui,
    /// Run the background daemon that syncs versions
    Daemon,
    /// Generate shell completions
    Completions { shell: CompletionShell },
    #[command(hide = true)]
    /// List note ids for shell completion
    Ids,
}

#[derive(Subcommand)]
pub enum BulletAction {
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
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}
