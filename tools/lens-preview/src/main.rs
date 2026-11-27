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
use std::sync::mpsc;
use std::time::Duration;

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
    let mut started = false;
    let mut reading: Option<mpsc::Receiver<Past>> = None;
    loop {
        let size = term.size()?;
        let size = (size.width, size.height);
        let shown = app.frame(all, size);
        term.draw(|f| app.draw(f, all, &shown, mono))?;
        // History reads every log file, so it starts only once the first
        // screen is up and runs beside the TUI. `q` never waits for it.
        if all.local() && !started {
            started = true;
            reading = Some(read_history());
        }
        if let Some(rx) = &reading {
            if !app.take_history(rx) {
                reading = None;
                continue;
            }
            // Poll while reading, so progress and the list show without a key.
            if !event::poll(Duration::from_millis(200))? {
                continue;
            }
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !app.press(all, size, key) {
            break;
        }
    }
    Ok(())
}

/// Starts History's read on its own thread. It sends progress as it goes,
/// then the sessions.
fn read_history() -> mpsc::Receiver<Past> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let progress = tx.clone();
        let data = local::history(Some(Box::new(move |read, found| {
            let _ = progress.send(Past::Reading(read, found));
        })));
        let _ = tx.send(Past::Ready(data));
    });
    rx
}

/// The whole synthetic corpus, so every number a view shows can be rebuilt
/// from the export: sessions, inventory, plan windows and the Home split.
/// The stress corpus reuses the default synthetic plan windows and token
/// sources, since no session records them.
fn export(data: &Data, corpus: &str) -> serde_json::Value {
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
        "workspaces": fixture::workspaces(data),
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
pub(crate) const HISTORY_ERROR: &str = "--history needs --json";

/// `--json`, `--source <name>`, `--corpus <name>` and the relay options,
/// in any order. Other arguments are ignored.
pub(crate) struct Args {
    pub(crate) json: bool,
    /// `--json --history`: add every session History lists.
    pub(crate) history: bool,
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
        history: false,
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
            "--history" => {
                parsed.history = true;
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
    // History is only exported; the TUI reads it on its own.
    if parsed.history && !parsed.json && parsed.error.is_none() {
        parsed.error = Some(HISTORY_ERROR.into());
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
        let mut json = match data.source {
            Source::Fixture => export(&data, args.corpus.unwrap_or("default")),
            Source::Local => local::export(&data),
        };
        // Opt-in, so plain `--json` stays byte-identical and never waits
        // for the full read.
        if args.history {
            let history = match data.source {
                Source::Fixture => serde_json::json!(data.corpus_history().sessions),
                Source::Local => local::history_export(&local::history(None)),
            };
            json.as_object_mut()
                .unwrap()
                .insert("history".into(), history);
        }
        println!("{json}");
        return Ok(());
    }
    run(&data)
}
