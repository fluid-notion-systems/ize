//! Izev - TUI interface for Ize version control filesystem

use anyhow::Result;
use clap::{Parser, Subcommand};

mod app;
mod event;
mod tui;
mod ui;

use app::App;
use tui::Tui;

/// Izev - TUI interface for Ize
#[derive(Parser, Debug)]
#[command(name = "izev")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the Ize repository
    #[arg(short, long, default_value = ".")]
    path: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Launch the interactive TUI
    Tui,
    /// Show repository status
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Status) => {
            println!("Repository status for: {}", cli.path);
            // TODO: Implement status display
            Ok(())
        }
        Some(Commands::Tui) | None => {
            // Default to TUI mode
            run_tui(&cli.path)
        }
    }
}

fn run_tui(path: &str) -> Result<()> {
    let mut tui = Tui::new()?;
    let mut app = App::new(path.to_string());

    tui.enter()?;

    while app.is_running() {
        tui.draw(&mut app)?;

        if let Some(event) = event::poll_event()? {
            app.handle_event(event)?;
        }
    }

    tui.exit()?;
    Ok(())
}
