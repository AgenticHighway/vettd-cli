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

use fixture::{Session, AGENTS, COMPONENTS, PARTS, PERIOD, WORKSPACES};
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

fn main() -> io::Result<()> {
    let sessions = fixture::sessions();
    if std::env::args().nth(1).as_deref() == Some("--json") {
        println!(
            "{}",
            serde_json::json!({
                "synthetic": true,
                "period": PERIOD,
                "snapshot": fixture::SNAPSHOT,
                "workspaces": WORKSPACES,
                "agents": AGENTS,
                "components": COMPONENTS,
                "placements": fixture::PLACEMENTS,
                "scans": fixture::SCANS,
                "windows": fixture::WINDOWS,
                "sessions": sessions,
            })
        );
        return Ok(());
    }
    run(&sessions)
}
