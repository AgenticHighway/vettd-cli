//! Builds each view's answer, scope and rows.

use crate::data::{day_label, Data, BUCKETS, NOT_COUNTED};
use crate::fixture::{Request, Session, COMPONENTS, PARTS};
use crate::*;

/// A view's answer line, scope chips and rows.
pub(crate) type Content = (String, String, Vec<Row>);

pub(crate) fn content(page: &Page, all: &Data, count: bool, width: u16, rt: &Runtime) -> Content {
    let bar_width = usize::from(width.saturating_sub(30).min(34));
    match page.view {
        View::Inventory
        | View::Component(..)
        | View::Placement(_)
        | View::Observation(..)
        | View::Signal(..)
        | View::Change(..)
        | View::Apply(..) => {
            // Their copies and observations are fixture data, so on local
            // data they read the fixture's sessions, not real ones.
            if all.local() {
                let fixture = crate::fixture::sessions();
                synthetic(components::rows(page, &fixture, rt), all)
            } else {
                components::rows(page, all, rt)
            }
        }
        View::Home => home(all, count),
        View::Headroom => headroom(all),
        View::Help => help(),
        View::Week => week(all, count, bar_width),
        View::Find => synthetic(find(&page.filter), all),
        View::Day(day) => day_sessions(day, all, count, width),
        View::Session(id) => session(id, &all[id], bar_width, width, all.local()),
        View::Tokens(id) => tokens(&all[id], width, all.local()),
    }
}

/// On local data the inventory and everything it opens stays fixture
/// data: no log records configured copies. The scope chips say so.
fn synthetic((title, scope, rows): Content, all: &Data) -> Content {
    if all.local() {
        (title, format!("synthetic | {scope}"), rows)
    } else {
        (title, scope, rows)
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

fn week(all: &Data, count: bool, bar_width: usize) -> Content {
    let days: Vec<_> = all
        .days
        .iter()
        .map(|&day| {
            let sessions: Vec<_> = all.iter().filter(|s| s.day == day).collect();
            let value = total(sessions.iter().copied(), count);
            // A day with unpriced requests is a floor in dollars.
            let partial = sessions.iter().any(|s| !comparable(s, count));
            let unpriced = !count && !sessions.is_empty() && !any_priced(sessions.iter().copied());
            (day, value, sessions.len(), partial, unpriced)
        })
        .collect();
    let peak = days.iter().map(|day| day.1).max().unwrap_or(1);
    let with_input = days.iter().filter(|d| d.2 > 0).count();
    let unknown = !count && with_input > 0 && days.iter().all(|d| d.2 == 0 || d.4);
    let sum: u32 = days.iter().map(|d| d.1).sum();
    // An answer, not a label: how many days had input and what they add to.
    let answer = format!(
        "{with_input} of {} days had input · {}{}",
        days.len(),
        if unknown {
            "unpriced".into()
        } else {
            units(sum, count)
        },
        if count {
            String::new()
        } else {
            format!(" {}", all.dollars())
        }
    );
    let rows = days
        .into_iter()
        .map(|(day, value, n, partial, unpriced)| {
            let label = if n == 0 {
                "No input".into()
            } else if unpriced {
                "unpriced".into()
            } else if count {
                value.to_string()
            } else {
                usd(value)
            };
            link(
                format!(
                    "{:<6}  {label:>8}  {}{}",
                    day_label(day),
                    bar(value, peak, bar_width),
                    if partial { "  partial" } else { "" }
                ),
                View::Day(day),
                "See contributing sessions",
            )
        })
        .collect();
    (
        answer,
        format!(
            "{} | All workspaces | {} per day",
            all.period(),
            all.unit(count)
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
        format!("{} | Search: {filter}_", Scope::WEEK.period()),
        rows,
    )
}

/// The workspace column for one day: its longest name, clamped so the
/// fixed columns still fit `width` (the list width). Shorter days keep the
/// narrow column, so the default corpus renders as before.
pub(crate) fn day_ws_width(day: u32, all: &[Session], width: u16) -> usize {
    // Row marker (3) plus name, agent and total with their separators (34),
    // plus the " ≥" a partial session ends with (2), so it is never cut.
    let room = usize::from(width).saturating_sub(3 + 34 + 2);
    let longest = all
        .iter()
        .filter(|s| s.day == day)
        .map(|s| cells(s.workspace))
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

fn day_sessions(day: u32, all: &Data, count: bool, width: u16) -> Content {
    let ws = day_ws_width(day, all, width);
    let mut sessions: Vec<_> = all
        .iter()
        .enumerate()
        .filter(|(_, s)| s.day == day)
        .collect();
    sessions.sort_by_key(|(_, s)| std::cmp::Reverse(measure(s, count)));
    let total: u32 = total(sessions.iter().map(|(_, s)| *s), count);
    let prior: Vec<_> = all.iter().filter(|s| s.day + 1 == day).collect();
    let comparison = if sessions.is_empty()
        || prior.is_empty()
        || prior.iter().any(|s| !comparable(s, count))
        || sessions.iter().any(|(_, s)| !comparable(s, count))
    {
        "No comparable previous day".into()
    } else {
        let previous = crate::total(prior.iter().copied(), count);
        format!(
            "Prior day {} | {}{} change",
            units(previous, count),
            if total >= previous { "+" } else { "-" },
            units(total.abs_diff(previous), count)
        )
    };
    let label = day_label(day);
    (
        // Partial rows end in "≥" (at least); the title says what it means.
        if sessions.iter().any(|(_, s)| s.incomplete) {
            format!("{label} sessions  ·  ≥ partial")
        } else {
            format!("{label} sessions")
        },
        format!(
            "{label} UTC | All workspaces | {}{} | {comparison}",
            units(total, count),
            if count || !all.local() {
                ""
            } else {
                " API-equivalent"
            }
        ),
        sessions
            .into_iter()
            .map(|(id, s)| session_row(id, s, count, ws))
            .collect(),
    )
}

fn session_scope(s: &Session, local: bool) -> String {
    format!(
        "{} UTC | Workspace {} | {} | {}",
        day_label(s.day),
        s.workspace,
        s.agent,
        cost_label(s, local)
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
    let model_room = room.saturating_sub(cells(worker) + 3 + reserved).max(1);
    format!("{worker} / {}", shorten(model, model_room))
}

/// Requests grouped by worker and model, in the order they first appear.
/// A local session can hold thousands of requests; the fixture's groups
/// each hold one, so its rows are unchanged.
fn groups(s: &Session) -> Vec<(&'static str, &'static str, Vec<&Request>)> {
    let mut groups: Vec<(&'static str, &'static str, Vec<&Request>)> = Vec::new();
    for r in &s.requests {
        match groups
            .iter_mut()
            .find(|g| g.0 == r.worker && g.1 == r.model)
        {
            Some(g) => g.2.push(r),
            None => groups.push((r.worker, r.model, vec![r])),
        }
    }
    groups
}

fn tokens(s: &Session, width: u16, local: bool) -> Content {
    let room = usize::from(width).saturating_sub(3);
    let mut rows = incomplete_rows(s);
    for (worker, model, requests) in groups(s) {
        let many = if requests.len() == 1 {
            String::new()
        } else {
            format!(" ({} requests)", requests.len())
        };
        let reserved = cells(&many);
        rows.push(line(format!(
            "{}{many}",
            worker_model(worker, model, reserved, room)
        )));
        rows.extend(PARTS.iter().enumerate().map(|(i, part)| {
            let tokens: u64 = requests.iter().map(|r| u64::from(r.tokens[i])).sum();
            line(format!("{part:<14} {tokens:>8} tokens"))
        }));
        if local {
            rows.push(line(match crate::pricing::lookup(model) {
                Some(rate) => format!("Rates checked {} at {}", rate.checked, rate.source),
                None => "No published rate: unpriced".into(),
            }));
        }
    }
    if local {
        rows.extend(source_rows(s));
    }
    (
        format!("{} / token and cache detail", s.name),
        session_scope(s, local),
        rows,
    )
}

/// Where this session's context came from, estimated, with the method
/// and what the estimate leaves out.
fn source_rows(s: &Session) -> Vec<Row> {
    let mut rows = vec![line("── Where tokens came from (estimated) ──")];
    let total: u64 = s.sources.iter().sum();
    if total == 0 {
        rows.push(line("No context growth to split"));
    }
    for (name, &tokens) in BUCKETS.iter().zip(&s.sources) {
        if tokens > 0 {
            rows.push(line(format!(
                "{name:<20} {:>8} tokens  {:>3}% estimated",
                tokens,
                tokens * 100 / total.max(1)
            )));
        }
    }
    rows.extend([
        line("Method: each request's context growth"),
        line("split across the blocks added since the"),
        line("previous request, by their characters."),
        line(format!("Not counted (unverified): {NOT_COUNTED}")),
    ]);
    rows
}

/// Cost by part (only when something is priced) and by worker and model.
fn cost_rows(s: &Session, bar_width: usize, width: u16) -> Vec<Row> {
    let room = usize::from(width).saturating_sub(3);
    let mut rows = Vec::new();
    let parts: [u32; 4] = std::array::from_fn(|i| {
        s.requests
            .iter()
            .map(|r| r.cents_exact()[i])
            .sum::<f64>()
            .round() as u32
    });
    if priced(s) > 0 {
        rows.extend(PARTS.iter().zip(parts).map(|(part, cost)| {
            line(format!(
                "{part:<13} {:>7}  {}",
                usd(cost),
                bar(cost, s.cents(), bar_width)
            ))
        }));
    }
    rows.extend(groups(s).into_iter().map(|(worker, model, requests)| {
        let unpriced = requests.iter().filter(|r| !r.priced()).count();
        let cents: f64 = requests.iter().flat_map(|r| r.cents_exact()).sum();
        let cost = match unpriced {
            0 => usd(cents.round() as u32),
            n if n == requests.len() => "unpriced".into(),
            n => format!("{} + {n} unpriced", usd(cents.round() as u32)),
        };
        line(format!(
            "{}   {cost}",
            worker_model(worker, model, 3 + cells(&cost), room)
        ))
    }));
    rows
}

/// Skills and MCP servers a local session used, from its logs. They name
/// what ran, not a configured copy, so they open nothing.
fn used_rows(s: &Session) -> Vec<Row> {
    let mut rows = vec![line("── Skills and MCP used ──")];
    if s.used.is_empty() {
        rows.push(line("None recorded"));
    }
    rows.extend(s.used.iter().map(|u| {
        let copy = if u.kind == "skill" {
            format!(" copy {}", u.copy)
        } else {
            String::new()
        };
        line(format!(
            "{:<5} {:<16} x{}{copy}{}",
            u.kind,
            u.name.chars().take(16).collect::<String>(),
            u.count,
            if u.estimated { " (estimated)" } else { "" }
        ))
    }));
    rows
}

fn session(id: usize, s: &Session, bar_width: usize, width: u16, local: bool) -> Content {
    let mut rows = incomplete_rows(s);
    rows.extend(cost_rows(s, bar_width, width));
    rows.push(link(
        "Token and cache detail",
        View::Tokens(id),
        "See token counts by model and worker",
    ));
    if local {
        rows.extend(used_rows(s));
    } else {
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
    }
    (
        format!("{} / cost components", s.name),
        session_scope(s, local),
        rows,
    )
}
