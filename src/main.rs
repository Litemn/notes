mod app;
mod cli;
mod completions;
mod daemon;
mod paths;
mod utils;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use completions::print_completions;
use daemon::{ensure_daemon_running, run_daemon};
use utils::launch_subl_if_installed;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut app = app::NotesApp::load()?;

    if !matches!(cli.command, Commands::Daemon | Commands::Completions { .. } | Commands::Ids) {
        ensure_daemon_running(app.paths())?;
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
            run_daemon(app.paths())?;
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
