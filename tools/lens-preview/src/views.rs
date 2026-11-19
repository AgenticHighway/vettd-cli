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
        View::Find => find(&page.filter),
        View::Day(day) => day_sessions(day, all, count),
        View::Session(id) => session(id, &all[id], bar_width),
        View::Tokens(id) => tokens(&all[id]),
    }
}

fn help() -> Content {
    (
        "Keyboard shortcuts".into(),
        "Esc returns to your previous selection and scope".into(),
        [
            "↑ / ↓   Move selection; preview updates",
            "PgUp/Dn Scroll by a screen",
            "Enter   Inspect the selected object",
            "Esc     Back to previous scope",
            "p       Widen a one-day scope to the week",
            "i       Component inventory",
            "Tab     Inventory: cycle type",
            "w a e   Inventory: workspace, agent, evidence",
            "x       Inventory: clear filters",
            "/       Find a component",
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
            link(
                format!(
                    "Nov {day}  {label:>8}  {}{}",
                    bar(value, peak, bar_width),
                    if partial { "  partial" } else { "" }
                ),
                View::Day(day),
                "See contributing sessions",
            )
        })
        .collect();
    (
        "Activity over time".into(),
        format!(
            "{PERIOD} | All workspaces | {} per day",
            if count { "Requests" } else { "Priced USD" }
        ),
        rows,
    )
}

fn find(filter: &str) -> Content {
    let rows = COMPONENTS
        .iter()
        .enumerate()
        .map(|(i, c)| (i, format!("{} {}", c.kind, c.name)))
        .filter(|(_, name)| {
            name.to_ascii_lowercase()
                .contains(&filter.to_ascii_lowercase())
        })
        .map(|(i, name)| {
            link(
                name,
                View::Component(i, Scope::WEEK),
                "Inspect copies and use this week",
            )
        })
        .collect();
    (
        "Find a component".into(),
        format!("{PERIOD} | Search: {filter}_"),
        rows,
    )
}

fn day_sessions(day: u32, all: &[Session], count: bool) -> Content {
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
            .map(|(id, s)| session_row(id, s, count))
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

fn tokens(s: &Session) -> Content {
    let mut rows = incomplete_rows(s);
    for r in &s.requests {
        rows.push(line(format!("{} / {}", r.worker, r.model)));
        rows.extend(
            PARTS
                .iter()
                .zip(r.tokens)
                .map(|(part, tokens)| line(format!("{part:<14} {tokens:>8} tokens"))),
        );
    }
    (
        format!("{} / token and cache detail", s.name),
        session_scope(s),
        rows,
    )
}

/// Cost by part (only when something is priced) and by request.
fn cost_rows(s: &Session, bar_width: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let parts: [u32; 4] = std::array::from_fn(|i| s.requests.iter().map(|r| r.cents()[i]).sum());
    if priced(s) > 0 {
        rows.extend(PARTS.iter().zip(parts).map(|(part, cost)| {
            line(format!(
                "{part:<13} {:>7}  {}",
                usd(cost),
                bar(cost, s.cents(), bar_width)
            ))
        }));
    }
    rows.extend(s.requests.iter().map(|r| {
        let cost = if r.milli_usd_per_ktok.is_some() {
            usd(r.cents().iter().sum())
        } else {
            "unpriced".into()
        };
        line(format!("{} / {}   {cost}", r.worker, r.model))
    }));
    rows
}

fn session(id: usize, s: &Session, bar_width: usize) -> Content {
    let mut rows = incomplete_rows(s);
    rows.extend(cost_rows(s, bar_width));
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
