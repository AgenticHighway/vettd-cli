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
use fixture::{Session, AGENTS, COMPONENTS, PARTS, WORKSPACES};
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

/// The whole corpus, so every number a view shows can be rebuilt from the
/// export: sessions, inventory, plan windows and the Home split.
fn export(data: &Data) -> serde_json::Value {
    let token_sources: Vec<_> = data
        .token_sources
        .iter()
        .map(|s| serde_json::json!({"source": s.name, "basis_points": s.basis_points}))
        .collect();
    serde_json::json!({
        "source": data.source.name(),
        "synthetic": true,
        "period": data.period(),
        "snapshot": fixture::SNAPSHOT,
        "workspaces": WORKSPACES,
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

/// `--json`, `--source <name>` and the relay options, in any order.
struct Args {
    json: bool,
    source: Source,
    relay: bool,
    relay_file: Option<PathBuf>,
    then: Option<String>,
    /// A bad option. The relay ignores it: it must never fail.
    error: Option<String>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Args {
    let mut parsed = Args {
        json: false,
        source: Source::Fixture,
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
        Source::Fixture => Data::fixture(),
        Source::Local => local::load(args.relay_file),
    };
    if args.json {
        let json = match data.source {
            Source::Fixture => export(&data),
            Source::Local => local::export(&data),
        };
        println!("{json}");
        return Ok(());
    }
    run(&data)
}
