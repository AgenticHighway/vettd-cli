//! Builds each view's answer, scope and rows.

use crate::fixture::{Session, COMPONENTS, PARTS, PERIOD};
use crate::*;

/// A view's answer line, scope chips and rows.
pub(crate) type Content = (String, String, Vec<Row>);

pub(crate) fn content(
    page: &Page,
    all: &[Session],
    count: bool,
    width: u16,
    rt: &Runtime,
) -> Content {
    let bar_width = usize::from(width.saturating_sub(30).min(34));
    match page.view {
        View::Inventory
        | View::Component(..)
        | View::Placement(_)
        | View::Observation(..)
        | View::Signal(..)
        | View::Change(..)
        | View::Apply(..) => components::rows(page, all, rt),
        View::Home => home(all, count),
        View::Headroom => headroom(),
        View::Help => help(),
        View::Week => week(all, count, bar_width),
        View::Find => find(&page.filter, all, count, width),
        View::Day(day) => day_sessions(day, all, count, width),
        View::Session(id) => session(id, &all[id], bar_width, width),
        View::Tokens(id) => tokens(&all[id], width),
    }
}

fn help() -> Content {
    (
        "Keyboard shortcuts".into(),
        "Esc returns to your previous selection and scope".into(),
        [
            "↑ / ↓   Move selection; preview updates",
            "PgUp/Dn Scroll by a screen",
            "Home/End First or last row (g / G too)",
            "← / →   Home: choose a day's bar",
            "1–7     Home, Week: open that day",
            "Enter   Inspect the selected object",
            "Esc     Back to previous scope",
            "p       Widen a one-day scope to the week",
            "i       Component inventory",
            "Tab     Inventory: cycle type",
            "w a e   Inventory: workspace, agent, evidence",
            "x       Inventory: clear filters",
            "/       Find a day, session or component",
            "c       Toggle priced USD and requests",
            "q       Quit (Ctrl+C also works in search)",
        ]
        .into_iter()
        .map(line)
        .collect(),
    )
}

fn week(all: &[Session], count: bool, bar_width: usize) -> Content {
    let days: Vec<_> = (12..=18)
        .map(|day| {
            let sessions: Vec<_> = all.iter().filter(|s| s.day == day).collect();
            let value = sessions.iter().map(|s| measure(s, count)).sum();
            let partial = sessions.iter().any(|s| s.incomplete);
            (day, value, sessions.len(), partial)
        })
        .collect();
    let peak = days.iter().map(|day| day.1).max().unwrap_or(1);
    let with_input = days.iter().filter(|d| d.2 > 0).count();
    let total: u32 = days.iter().map(|d| d.1).sum();
    // An answer, not a label: how many days had input and what they add to.
    let answer = format!(
        "{with_input} of {} days had input · {}{}",
        days.len(),
        units(total, count),
        if count { "" } else { " priced" }
    );
    let rows = days
        .into_iter()
        .map(|(day, value, n, partial)| {
            let label = if n == 0 {
                "No input".into()
            } else if count {
                value.to_string()
            } else {
                usd(value)
            };
            Cells::default()
                .add(format!("Nov {day}  "), None)
                .add(format!("{label:>8}"), (n > 0).then_some(Emphasis::Figure))
                .add(
                    format!(
                        "  {}{}",
                        bar(value, peak, bar_width),
                        if partial { "  partial" } else { "" }
                    ),
                    None,
                )
                .link(View::Day(day), "See contributing sessions")
        })
        .collect();
    (
        answer,
        format!(
            "{PERIOD} | All workspaces | {} per day",
            if count { "Requests" } else { "Priced USD" }
        ),
        rows,
    )
}

/// Find: with a search, matching days, then sessions, then components,
/// each group under a divider with its count. With no search, components
/// only, as before sessions and days could be found.
fn find(filter: &str, all: &[Session], count: bool, width: u16) -> Content {
    let needle = filter.to_ascii_lowercase();
    let matches = |text: &str| text.to_ascii_lowercase().contains(&needle);
    let components: Vec<Row> = COMPONENTS
        .iter()
        .enumerate()
        .map(|(i, c)| (i, format!("{} {}", c.kind, c.name)))
        .filter(|(_, name)| matches(name))
        .map(|(i, name)| {
            link(
                name,
                View::Component(i, Scope::WEEK),
                "Inspect copies and use this week",
            )
        })
        .collect();
    let title = "Find a day, session or component".to_string();
    let scope = format!("{PERIOD} | Search: {filter}_");
    if filter.is_empty() {
        return (title, scope, components);
    }
    let days: Vec<Row> = (12..=18)
        .filter(|day| matches(&format!("Nov {day}")))
        .map(|day| {
            let sessions: Vec<_> = all.iter().filter(|s| s.day == day).collect();
            let label = if sessions.is_empty() {
                "No input".into()
            } else {
                units(sessions.iter().map(|s| measure(s, count)).sum(), count)
            };
            link(
                format!("Nov {day}  {label:>8}  {} sessions", sessions.len()),
                View::Day(day),
                "See contributing sessions",
            )
        })
        .collect();
    let mut found: Vec<_> = all
        .iter()
        .enumerate()
        .filter(|(_, s)| matches(s.name) || matches(s.workspace))
        .collect();
    found.sort_by_key(|(_, s)| (s.day, std::cmp::Reverse(measure(s, count))));
    // The Day row layout after a "Nov dd " column, sized to what matched.
    let room = usize::from(width).saturating_sub(3 + 34 + 7);
    let ws = found
        .iter()
        .map(|(_, s)| s.workspace.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(4, room.max(4));
    let sessions: Vec<Row> = found
        .into_iter()
        .map(|(id, s)| {
            let row = session_row(id, s, count, ws);
            let prefix = format!("Nov {} ", s.day);
            let shift = prefix.chars().count();
            Row {
                text: format!("{prefix}{}", row.text),
                spans: row
                    .spans
                    .into_iter()
                    .map(|(at, e)| (at.start + shift..at.end + shift, e))
                    .collect(),
                ..row
            }
        })
        .collect();
    let mut rows = Vec::new();
    for (name, group) in [
        ("Days", days),
        ("Sessions", sessions),
        ("Components", components),
    ] {
        if !group.is_empty() {
            rows.push(line(format!("── {name} ({})", group.len())));
            rows.extend(group);
        }
    }
    (title, scope, rows)
}

/// The workspace column for one day: its longest name, clamped so the
/// fixed columns still fit `width` (the list width). Shorter days keep the
/// narrow column, so the default corpus renders as before.
pub(crate) fn day_ws_width(day: u32, all: &[Session], width: u16) -> usize {
    // Row marker (3) plus name, agent and total with their separators (34).
    let room = usize::from(width).saturating_sub(3 + 34);
    let longest = all
        .iter()
        .filter(|s| s.day == day)
        .map(|s| s.workspace.chars().count())
        .max()
        .unwrap_or(0);
    longest.clamp(4, room.max(4))
}

/// The Day column header, aligned with the rows `day_sessions` builds.
pub(crate) fn day_header(day: u32, all: &[Session], width: u16) -> String {
    let ws = day_ws_width(day, all, width);
    format!(
        "{:<16} {:<ws$} {:<7} {:>8}",
        "SESSION", "WS", "AGENT", "TOTAL"
    )
}

fn day_sessions(day: u32, all: &[Session], count: bool, width: u16) -> Content {
    let ws = day_ws_width(day, all, width);
    let mut sessions: Vec<_> = all
        .iter()
        .enumerate()
        .filter(|(_, s)| s.day == day)
        .collect();
    sessions.sort_by_key(|(_, s)| std::cmp::Reverse(measure(s, count)));
    let total: u32 = sessions.iter().map(|(_, s)| measure(s, count)).sum();
    let prior: Vec<_> = all.iter().filter(|s| s.day + 1 == day).collect();
    let comparison = if sessions.is_empty()
        || prior.is_empty()
        || prior.iter().any(|s| !comparable(s, count))
        || sessions.iter().any(|(_, s)| !comparable(s, count))
    {
        "No comparable previous day".into()
    } else {
        let previous = prior.iter().map(|s| measure(s, count)).sum::<u32>();
        format!(
            "Prior day {} | {}{} change",
            units(previous, count),
            if total >= previous { "+" } else { "-" },
            units(total.abs_diff(previous), count)
        )
    };
    (
        format!("Nov {day} sessions"),
        format!(
            "Nov {day} UTC | All workspaces | {} | {comparison}",
            units(total, count)
        ),
        sessions
            .into_iter()
            .map(|(id, s)| session_row(id, s, count, ws))
            .collect(),
    )
}

fn session_scope(s: &Session) -> String {
    format!(
        "Nov {} UTC | Workspace {} | {} | {}",
        s.day,
        s.workspace,
        s.agent,
        cost_label(s)
    )
}

fn incomplete_rows(s: &Session) -> Vec<Row> {
    if s.incomplete {
        vec![line("Input incomplete")]
    } else {
        Vec::new()
    }
}

/// "worker / model", with the model shortened so the row, plus `reserved`
/// columns after it, fits `room`. The model keeps at least one character.
fn worker_model(worker: &str, model: &str, reserved: usize, room: usize) -> String {
    let model_room = room
        .saturating_sub(worker.chars().count() + 3 + reserved)
        .max(1);
    format!("{worker} / {}", shorten(model, model_room))
}

fn tokens(s: &Session, width: u16) -> Content {
    let room = usize::from(width).saturating_sub(3);
    let mut rows = incomplete_rows(s);
    for r in &s.requests {
        rows.push(line(worker_model(r.worker, r.model, 0, room)));
        rows.extend(PARTS.iter().zip(r.tokens).map(|(part, tokens)| {
            Cells::default()
                .add(format!("{part:<14} "), None)
                .add(format!("{tokens:>8}"), Some(Emphasis::Figure))
                .add(" tokens", None)
                .line()
        }));
    }
    (
        format!("{} / token and cache detail", s.name),
        session_scope(s),
        rows,
    )
}

/// Cost by part (only when something is priced) and by request.
fn cost_rows(s: &Session, bar_width: usize, width: u16) -> Vec<Row> {
    let room = usize::from(width).saturating_sub(3);
    let mut rows = Vec::new();
    let parts: [u32; 4] = std::array::from_fn(|i| s.requests.iter().map(|r| r.cents()[i]).sum());
    if priced(s) > 0 {
        rows.extend(PARTS.iter().zip(parts).map(|(part, cost)| {
            Cells::default()
                .add(format!("{part:<13} "), None)
                .add(format!("{:>7}", usd(cost)), Some(Emphasis::Figure))
                .add(format!("  {}", bar(cost, s.cents(), bar_width)), None)
                .line()
        }));
    }
    rows.extend(s.requests.iter().map(|r| {
        let priced = r.milli_usd_per_ktok.is_some();
        let cost = if priced {
            usd(r.cents().iter().sum())
        } else {
            "unpriced".into()
        };
        Cells::default()
            .add(
                format!(
                    "{}   ",
                    worker_model(r.worker, r.model, 3 + cost.chars().count(), room)
                ),
                None,
            )
            .add(&cost, priced.then_some(Emphasis::Figure))
            .line()
    }));
    rows
}

fn session(id: usize, s: &Session, bar_width: usize, width: u16) -> Content {
    let mut rows = incomplete_rows(s);
    rows.extend(cost_rows(s, bar_width, width));
    rows.push(link(
        "Token and cache detail",
        View::Tokens(id),
        "See token counts by model and worker",
    ));
    rows.push(line("── Components observed ──"));
    rows.extend(s.observed.iter().map(|u| {
        let c = &COMPONENTS[u.component];
        link(
            format!(
                "{:<5} {:<13} {:<4} x{}",
                c.kind,
                c.name,
                u.version.unwrap_or("?"),
                u.count
            ),
            View::Component(u.component, Scope::of(s)),
            "Inspect component in this session's scope",
        )
    }));
    (
        format!("{} / cost components", s.name),
        session_scope(s),
        rows,
    )
}
