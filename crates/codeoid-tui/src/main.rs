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
use codeoid_client::resolve_token;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
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

    /// A ready-to-use ZeroID access token (JWT). Takes precedence over `--api-key`.
    #[arg(long, env = "CODEOID_TOKEN")]
    token: Option<String>,

    /// ZeroID API key (prefix `zid_sk_`). If set, the client will exchange it
    /// for an access token at `--zeroid-url` before connecting.
    #[arg(long, env = "CODEOID_API_KEY")]
    api_key: Option<String>,

    /// ZeroID base URL, used when exchanging an API key for a token.
    #[arg(long, env = "ZEROID_URL", default_value = "http://localhost:8899")]
    zeroid_url: String,

    /// Path to write a file log (tracing). Stderr is reserved for the TUI.
    #[arg(long, env = "CODEOID_LOG_FILE")]
    log_file: Option<std::path::PathBuf>,

    /// Disable mouse capture. With capture on (default) the wheel scrolls
    /// the transcript regardless of focus; with capture off, your terminal
    /// handles wheel + click-drag selection natively (Shift+drag also
    /// works for selection while capture is enabled).
    #[arg(long, env = "CODEOID_NO_MOUSE")]
    no_mouse: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.log_file.as_deref())?;

    let token = resolve_token(cli.token.as_deref(), cli.api_key.as_deref(), &cli.zeroid_url)
        .await
        .context("failed to resolve auth token")?;

    let mouse = !cli.no_mouse;
    let mut terminal = setup_terminal(mouse)?;
    let res = App::new(cli.url, token).run(&mut terminal).await;
    restore_terminal(&mut terminal, mouse)?;
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

fn setup_terminal(mouse: bool) -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Bracketed paste tells the terminal to wrap pasted text in escape
    // markers so we receive it as a single Event::Paste(String) instead
    // of a stream of keypresses — without this, embedded newlines in a
    // paste each fire SubmitPrompt.
    //
    // Mouse capture (when enabled) lets us route wheel events to the
    // transcript regardless of which pane is focused. Tradeoff: the
    // terminal stops handling click-drag selection natively. Most modern
    // terminals fall back to "Shift+drag = native selection" while
    // capture is on, which is the standard convention used by Helix,
    // Zellij, and Alacritty-with-mouse.
    if mouse {
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture)?;
    } else {
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    }
    let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    Ok(terminal)
}

fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mouse: bool,
) -> Result<()> {
    disable_raw_mode()?;
    if mouse {
        execute!(
            terminal.backend_mut(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        )?;
    } else {
        execute!(terminal.backend_mut(), DisableBracketedPaste, LeaveAlternateScreen)?;
    }
    terminal.show_cursor()?;
    Ok(())
}
