//! Codeoid TUI — a Ratatui cockpit for the Codeoid daemon.
//!
//! Boots a terminal UI, connects to the daemon over WebSocket, and wires
//! keyboard + network events into a single event-loop reducer.

#![deny(missing_debug_implementations)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::too_many_lines)]

mod app;
mod commands;
mod event;
mod keymap;
mod render;
mod state;
mod ui;

use std::io;

use anyhow::{Context, Result};
use clap::Parser;
use codeoid_client::connect;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tracing_subscriber::EnvFilter;

use crate::app::App;

#[derive(Debug, Parser)]
#[command(name = "codeoid-tui", version, about = "Terminal cockpit for Codeoid")]
struct Cli {
    /// Daemon WebSocket URL.
    #[arg(long, env = "CODEOID_URL", default_value = "ws://127.0.0.1:7400")]
    url: String,

    /// ZeroID JWT for auth.
    #[arg(long, env = "CODEOID_TOKEN")]
    token: String,

    /// Path to write a file log (tracing). Stderr is reserved for the TUI.
    #[arg(long, env = "CODEOID_LOG_FILE")]
    log_file: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.log_file.as_deref())?;

    let connected = connect(&cli.url, &cli.token)
        .await
        .context("failed to connect to daemon")?;

    let mut terminal = setup_terminal()?;
    let res = App::new(connected).run(&mut terminal).await;
    restore_terminal(&mut terminal)?;
    res
}

fn init_tracing(log_file: Option<&std::path::Path>) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    match log_file {
        Some(path) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("open log file {}", path.display()))?;
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(file)
                .with_ansi(false)
                .init();
        }
        None => {
            // No log file + TUI active == drop logs on the floor. Otherwise
            // stderr would paint over the UI.
            let sink = std::io::sink;
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(sink)
                .with_ansi(false)
                .init();
        }
    }
    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
