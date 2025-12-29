mod app;
mod bullet;
mod cli;
mod completions;
mod daemon;
mod paths;
mod ui;
mod utils;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use completions::print_completions;
use daemon::{ensure_daemon_running, run_daemon};
use bullet::{handle_bullet_command, run_interactive};
use ui::run_ui;
use utils::launch_subl_if_installed;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut app = app::NotesApp::load()?;

    if !matches!(
        cli.command,
        Commands::Daemon
            | Commands::Completions { .. }
            | Commands::Ids
            | Commands::Bullet { .. }
            | Commands::BulletInteractive
    ) {
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
            run_interactive()?;
        }
        Commands::Ui => {
            run_ui()?;
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
