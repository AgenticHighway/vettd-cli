//! Plan windows and the Home dashboard's findings.

use crate::fixture::{self, Session, Window, PERIOD};
use crate::*;

/// Projected used share at reset: a straight line through the first and
/// last readings of this window. None with fewer than 2 readings, because
/// one reading has no pace.
pub(crate) fn pace(w: &Window) -> Option<u32> {
    let (&(t0, p0), &(t1, p1)) = (w.readings.first()?, w.readings.last()?);
    if w.readings.len() < 2 || t1 <= t0 {
        return None;
    }
    let slope = f64::from(p1.saturating_sub(p0)) / f64::from(t1 - t0);
    Some((f64::from(p1) + slope * f64::from(w.minutes - t1)).round() as u32)
}

/// When the window reaches 100% at its pace, in minutes from Nov 18 00:00,
/// if that comes before reset.
pub(crate) fn full_at(w: &Window) -> Option<i32> {
    let (&(t0, p0), &(t1, p1)) = (w.readings.first()?, w.readings.last()?);
    if pace(w)? < 100 || p1 <= p0 {
        return None;
    }
    let minutes =
        f64::from(t1) + f64::from(100 - p1.min(100)) * f64::from(t1 - t0) / f64::from(p1 - p0);
    Some(w.start + minutes.round() as i32)
}

pub(crate) fn full_by(w: &Window) -> Option<String> {
    full_at(w).map(clock)
}

/// Minutes from Nov 18 00:00 as "13:22", or "Nov 23 09:00" on another day.
pub(crate) fn clock(minutes: i32) -> String {
    let (day, rest) = (minutes.div_euclid(1440), minutes.rem_euclid(1440));
    let time = format!("{:02}:{:02}", rest / 60, rest % 60);
    if day == 0 {
        time
    } else {
        format!("Nov {} {time}", 18 + day)
    }
}

pub(crate) fn used(w: &Window) -> u32 {
    w.readings.last().map_or(0, |r| r.1)
}

/// The window that fills soonest, if any is on pace to fill. That is the
/// one that can stop work first, whatever its projected share: a weekly
/// window at 155% that fills in two days matters less than a 5-hour window
/// at 115% that fills this afternoon.
pub(crate) fn soonest(windows: &[Window]) -> Option<(&Window, u32)> {
    windows
        .iter()
        .filter_map(|w| full_at(w).map(|at| (w, at)))
        .min_by_key(|&(_, at)| at)
        .map(|(w, _)| (w, used(w)))
}

pub(crate) fn filling() -> Option<(&'static Window, u32)> {
    soonest(&fixture::WINDOWS)
}

/// One window: its used share, pace at reset, reset time and source.
fn window_row(w: &Window) -> Row {
    let at_reset = pace(w).map_or("—".into(), |p| format!("~{p}%"));
    // 5-hour windows are today's work; weekly ones span the period.
    let (view, action) = if w.minutes <= 300 {
        (View::Day(18), "See today's sessions")
    } else {
        (View::Week, "See every day in this window")
    };
    link(
        format!(
            "{:<15}{:>4}  {:>8}  {:<12} {}",
            format!("{} · {}", w.agent, w.name),
            format!("{}%", used(w)),
            at_reset,
            clock(w.start + w.minutes as i32),
            w.source
        ),
        view,
        action,
    )
}

/// How recent weeks ended, and how to read the pace column.
fn history_rows() -> Vec<Row> {
    let ends: Vec<String> = fixture::CLAUDE_WEEKLY_ENDS
        .iter()
        .map(|e| format!("{e}%"))
        .collect();
    vec![
        line("── Last 4 weeks ──"),
        line(format!("Claude weekly ended at {}", ends.join(", "))),
        line(format!(
            "5-hour limit stopped work {}×",
            fixture::LIMIT_HITS
        )),
        line("── How to read it ──"),
        line("At reset: straight line through this"),
        line("window's readings. — means one reading."),
        line("Provider meters are the source of truth."),
    ]
}

/// Plan headroom: each provider window's used share, its pace to reset,
/// and where the number came from. No plan is recommended: Lens can't see
/// what other plans allow.
pub(crate) fn headroom() -> Content {
    let answer = match filling() {
        Some((w, used)) => format!(
            "{} {} window is {used}% used. At this pace it's full by {}.",
            w.agent,
            w.name,
            full_by(w).unwrap()
        ),
        None => "No window is on pace to fill before it resets.".into(),
    };
    let mut rows: Vec<Row> = fixture::WINDOWS.iter().map(window_row).collect();
    rows.extend(history_rows());
    (
        answer,
        format!(
            "Claude + Codex | Nov 18 {} | Plan share",
            clock(fixture::NOW)
        ),
        rows,
    )
}

/// Home's findings, most urgent first: a window on pace to fill (it can stop
/// work), an unusual day, a load-order gap, and partial input.
fn findings(all: &[Session], count: bool) -> Vec<Row> {
    let (days, normal) = screen::daily(all, count);
    let mut rows = Vec::new();
    if let Some((w, used)) = filling() {
        rows.push(link(
            format!(
                "{} {}: {used}%, full by {}",
                w.agent,
                w.name,
                full_by(w).unwrap()
            ),
            View::Headroom,
            "See every plan window and its pace",
        ));
    }
    if let Some(((day, value), normal)) = days
        .iter()
        .filter(|d| !d.partial)
        .filter_map(|d| d.value.map(|v| (d.day, v)))
        .max_by_key(|&(_, v)| v)
        .zip(normal.filter(|&n| n > 0))
        .filter(|&((_, v), n)| v > n * 3)
    {
        rows.push(link(
            format!("Nov {day}: {}× a normal day", value / normal),
            View::Day(day),
            "See the sessions behind that day",
        ));
    }
    rows.push(link(
        "review-kit: unknown load in Codex",
        View::Component(0, Scope::WEEK),
        "Inspect copies and use this week",
    ));
    if let Some(id) = all.iter().position(|s| s.incomplete) {
        rows.push(link(
            format!("Nov {}: partial input, unpriced", all[id].day),
            View::Session(id),
            "Open the session",
        ));
    }
    rows
}

/// The dashboard: one sentence on what was used, then findings to open.
/// It describes the week; judgments live on the screens the findings open.
pub(crate) fn home(all: &[Session], count: bool) -> Content {
    let requests: usize = all.iter().map(|s| s.requests.len()).sum();
    let cents: u32 = all.iter().map(Session::cents).sum();
    let mut rows = findings(all, count);
    rows.push(link(
        "Every day and its sessions",
        View::Week,
        "Open daily activity",
    ));
    (
        format!(
            "{} sessions · {requests} requests · {} tokens · {} priced",
            all.len(),
            screen::compact(screen::total_tokens(all)),
            usd(cents)
        ),
        format!(
            "All workspaces | All agents | {PERIOD} | {}",
            if count { "Requests" } else { "Priced USD" }
        ),
        rows,
    )
}
