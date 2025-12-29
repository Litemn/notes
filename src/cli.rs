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

#[derive(Clone, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}
