mod app;
mod components;
mod data;
#[cfg(test)]
mod e2e;
mod fixture;
mod local;
mod model;
mod plan;
mod pricing;
mod relay;
mod screen;
#[cfg(test)]
mod tests;
mod views;

pub(crate) use app::*;
pub(crate) use model::*;
pub(crate) use plan::*;
pub(crate) use views::*;

use data::{Data, Source};
use fixture::{Session, AGENTS, COMPONENTS, PARTS};
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
use std::path::PathBuf;

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

fn run(all: &Data) -> io::Result<()> {
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

/// The whole synthetic corpus, so every number a view shows can be rebuilt
/// from the export: sessions, inventory, plan windows and the Home split.
/// The stress corpus reuses the default synthetic plan windows and token
/// sources, since no session records them.
fn export(data: &Data, corpus: &str) -> serde_json::Value {
    let workspaces: std::collections::BTreeSet<&str> = data.iter().map(|s| s.workspace).collect();
    let token_sources: Vec<_> = data
        .token_sources
        .iter()
        .map(|s| serde_json::json!({"source": s.name, "basis_points": s.basis_points}))
        .collect();
    serde_json::json!({
        "source": data.source.name(),
        "synthetic": true,
        "corpus": corpus,
        "period": data.period(),
        "snapshot": fixture::SNAPSHOT,
        "workspaces": workspaces,
        "agents": AGENTS,
        "components": COMPONENTS,
        "placements": fixture::PLACEMENTS,
        "scans": fixture::SCANS,
        "windows": data.windows,
        "now_minutes_from_nov_18": data.now,
        "claude_weekly_ends_percent": data.weekly_ends,
        "limit_hits": data.limit_hits,
        "token_sources": token_sources,
        "sessions": data.sessions,
    })
}

pub(crate) const CORPUS_ERROR: &str = "--corpus takes default or stress";
pub(crate) const LOCAL_CORPUS_ERROR: &str = "--corpus needs --source fixture";

/// `--json`, `--source <name>`, `--corpus <name>` and the relay options,
/// in any order. Other arguments are ignored.
pub(crate) struct Args {
    pub(crate) json: bool,
    pub(crate) source: Source,
    /// The synthetic corpus, when `--corpus` was given.
    pub(crate) corpus: Option<&'static str>,
    pub(crate) relay: bool,
    pub(crate) relay_file: Option<PathBuf>,
    pub(crate) then: Option<String>,
    /// A bad option. The relay ignores it: it must never fail.
    pub(crate) error: Option<String>,
}

pub(crate) fn parse_args(args: impl IntoIterator<Item = String>) -> Args {
    let mut parsed = Args {
        json: false,
        source: Source::Fixture,
        corpus: None,
        relay: false,
        relay_file: None,
        then: None,
        error: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--json" => {
                parsed.json = true;
                continue;
            }
            "--relay" => {
                parsed.relay = true;
                continue;
            }
            "--relay-file" => {
                parsed.relay_file = args.next().map(PathBuf::from);
                continue;
            }
            "--then" => {
                parsed.then = args.next();
                continue;
            }
            "--corpus" => {
                match args.next().as_deref() {
                    Some("default") => parsed.corpus = Some("default"),
                    Some("stress") => parsed.corpus = Some("stress"),
                    _ => parsed.error = Some(CORPUS_ERROR.into()),
                }
                continue;
            }
            "--source" => args.next(),
            _ => match arg.strip_prefix("--source=") {
                Some(value) => Some(value.to_string()),
                None => continue,
            },
        };
        match value.as_deref().and_then(Source::parse) {
            Some(source) => parsed.source = source,
            None => parsed.error = Some("--source takes fixture or local".into()),
        }
    }
    // A corpus is synthetic; local data is not, so the pair is a mistake.
    if parsed.source == Source::Local && parsed.corpus.is_some() && parsed.error.is_none() {
        parsed.error = Some(LOCAL_CORPUS_ERROR.into());
    }
    parsed
}

fn main() -> io::Result<()> {
    let args = parse_args(std::env::args().skip(1));
    if args.relay {
        relay::run(args.relay_file, args.then.as_deref());
        return Ok(());
    }
    if let Some(message) = args.error {
        eprintln!("vettd-lens-preview: {message}");
        std::process::exit(2);
    }
    let data = match args.source {
        Source::Fixture if args.corpus == Some("stress") => Data::stress(),
        Source::Fixture => Data::fixture(),
        Source::Local => local::load(args.relay_file),
    };
    if args.json {
        let json = match data.source {
            Source::Fixture => export(&data, args.corpus.unwrap_or("default")),
            Source::Local => local::export(&data),
        };
        println!("{json}");
        return Ok(());
    }
    run(&data)
}
