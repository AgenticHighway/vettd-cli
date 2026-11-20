mod app;
mod components;
#[cfg(test)]
mod e2e;
mod fixture;
mod model;
mod plan;
mod screen;
#[cfg(test)]
mod tests;
mod views;

pub(crate) use app::*;
pub(crate) use model::*;
pub(crate) use plan::*;
pub(crate) use views::*;

use fixture::{Session, AGENTS, COMPONENTS, PARTS, PERIOD};
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        cursor,
        event::{self, Event},
        execute,
        style::ResetColor,
        terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
    },
};
use std::io::{self, BufWriter};

struct Terminal;

impl Terminal {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stderr(), EnterAlternateScreen, cursor::Hide)?;
        Ok(guard)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), ResetColor, cursor::Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn run(all: &[Session]) -> io::Result<()> {
    let _guard = Terminal::enter()?;
    // The TUI draws on stderr so stdout stays free for machine output.
    let mut term = ratatui::Terminal::new(CrosstermBackend::new(BufWriter::new(io::stderr())))?;
    let mut app = App::new();
    let mono = screen::no_color();
    loop {
        let size = term.size()?;
        let size = (size.width, size.height);
        let shown = app.frame(all, size);
        term.draw(|f| app.draw(f, all, &shown, mono))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !app.press(all, size, key) {
            break;
        }
    }
    Ok(())
}

const CORPUS_ERROR: &str = "error: --corpus must be default or stress";

/// `--json` and `--corpus default|stress` may appear anywhere; other
/// arguments are ignored. Returns (json, corpus name).
fn parse_args(args: &[String]) -> Result<(bool, &'static str), &'static str> {
    let mut json = false;
    let mut corpus = "default";
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--corpus" => {
                corpus = match it.next().map(String::as_str) {
                    Some("default") => "default",
                    Some("stress") => "stress",
                    _ => return Err(CORPUS_ERROR),
                }
            }
            _ => {}
        }
    }
    Ok((json, corpus))
}

/// The whole synthetic corpus, so every number a view shows can be rebuilt
/// from the export: sessions, inventory, plan windows and the Home split.
/// The stress corpus reuses the default synthetic plan windows and token
/// sources, since no session records them.
fn export(corpus: &str, sessions: &[Session]) -> serde_json::Value {
    let workspaces: std::collections::BTreeSet<&str> =
        sessions.iter().map(|s| s.workspace).collect();
    let token_sources: Vec<_> = fixture::SOURCE_SHARES
        .iter()
        .map(|&(source, basis_points)| {
            serde_json::json!({"source": source, "basis_points": basis_points})
        })
        .collect();
    serde_json::json!({
        "synthetic": true,
        "corpus": corpus,
        "period": PERIOD,
        "snapshot": fixture::SNAPSHOT,
        "workspaces": workspaces,
        "agents": AGENTS,
        "components": COMPONENTS,
        "placements": fixture::PLACEMENTS,
        "scans": fixture::SCANS,
        "windows": fixture::WINDOWS,
        "now_minutes_from_nov_18": fixture::NOW,
        "claude_weekly_ends_percent": fixture::CLAUDE_WEEKLY_ENDS,
        "limit_hits": fixture::LIMIT_HITS,
        "token_sources": token_sources,
        "sessions": sessions,
    })
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (json, corpus) = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let sessions = if corpus == "stress" {
        fixture::stress_sessions()
    } else {
        fixture::sessions()
    };
    if json {
        println!("{}", export(corpus, &sessions));
        return Ok(());
    }
    run(&sessions)
}
