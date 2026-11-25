//! Plan windows and the Home dashboard's findings.

use crate::data::{day_label, Data};
use crate::fixture::Window;
use crate::*;

/// Projected used share at reset: a straight line through the first and
/// last readings of this window. None with fewer than 2 readings, because
/// one reading has no pace.
pub(crate) fn pace(w: &Window) -> Option<u32> {
    let points = points(w);
    let (&(t0, p0), &(t1, p1)) = (points.first()?, points.last()?);
    if points.len() < 2 || t1 <= t0 {
        return None;
    }
    let slope = (p1 - p0).max(0.0) / (t1 - t0);
    Some((p1 + slope * (f64::from(w.minutes) - t1).max(0.0)).round() as u32)
}

/// The readings to project from: unrounded on local data, the window's
/// whole-minute readings otherwise.
fn points(w: &Window) -> Vec<(f64, f64)> {
    if w.precise.is_empty() {
        w.readings
            .iter()
            .map(|&(t, p)| (f64::from(t), f64::from(p)))
            .collect()
    } else {
        w.precise.clone()
    }
}

/// When the window reaches 100% at its pace, in minutes from "today"
/// 00:00, if that comes before reset.
pub(crate) fn full_at(w: &Window) -> Option<i32> {
    let points = points(w);
    let (&(t0, p0), &(t1, p1)) = (points.first()?, points.last()?);
    if pace(w)? < 100 || p1 <= p0 {
        return None;
    }
    let minutes = t1 + (100.0 - p1.min(100.0)) * (t1 - t0) / (p1 - p0);
    Some(w.start? + minutes.round() as i32)
}

pub(crate) fn full_by(w: &Window, today: u32) -> Option<String> {
    full_at(w).map(|at| clock(today, at))
}

/// Minutes from `today` 00:00 as "13:22", or "Nov 23 09:00" on another day.
pub(crate) fn clock(today: u32, minutes: i32) -> String {
    let (day, rest) = (minutes.div_euclid(1440), minutes.rem_euclid(1440));
    let time = format!("{:02}:{:02}", rest / 60, rest % 60);
    if day == 0 {
        time
    } else {
        format!("{} {time}", day_label((today as i32 + day) as u32))
    }
}

/// A window that already filled is full until it resets: "full by" a time
/// that has passed would read as a forecast.
fn full_until(w: &Window, data: &Data) -> Option<String> {
    let end = ends(w)?;
    (used(w, data.now)? >= 100).then(|| clock(data.today, end))
}

/// When the window resets, in minutes from "today" 00:00.
fn ends(w: &Window) -> Option<i32> {
    Some(w.start? + w.minutes as i32)
}

/// A window whose reset time has passed: its last share is the previous
/// window's, not the current one's.
pub(crate) fn has_reset(w: &Window, now: i32) -> bool {
    ends(w).is_some_and(|end| end <= now)
}

/// The used share, or None with no reading in the current window.
pub(crate) fn used(w: &Window, now: i32) -> Option<u32> {
    if has_reset(w, now) {
        return None;
    }
    w.readings.last().map(|r| r.1)
}

/// The window that fills soonest, if any is on pace to fill. That is the
/// one that can stop work first, whatever its projected share: a weekly
/// window at 155% that fills in two days matters less than a 5-hour window
/// at 115% that fills this afternoon. Windows without a current reading
/// are left out.
///
/// A window whose last reading is 100% is full now, trend or not. A
/// forecast whose time has passed without a reading confirming it says
/// nothing about now, so it doesn't count.
pub(crate) fn soonest(windows: &[Window], now: i32) -> Option<(&Window, u32)> {
    windows
        .iter()
        .filter_map(|w| {
            let share = used(w, now)?;
            let at = if share >= 100 {
                now
            } else {
                full_at(w).filter(|&at| at > now)?
            };
            Some((w, at, share))
        })
        .min_by_key(|&(_, at, _)| at)
        .map(|(w, _, share)| (w, share))
}

pub(crate) fn filling(data: &Data) -> Option<(&Window, u32)> {
    soonest(&data.windows, data.now)
}

/// One window: its used share, pace at reset, reset time and source.
/// A window with no reading opens nothing. A window that has reset says
/// so on a second row.
fn window_rows(w: &Window, data: &Data) -> Vec<Row> {
    let reset = has_reset(w, data.now);
    let shown_pace = if reset { None } else { pace(w) };
    let at_reset = shown_pace.map_or("—".into(), |p| format!("~{p}%"));
    let share = used(w, data.now).map_or("—".into(), |u| format!("{u}%"));
    let resets = match ends(w) {
        Some(_) if reset => "reset".to_string(),
        Some(end) => clock(data.today, end),
        None => "—".into(),
    };
    let text = format!(
        "{:<15}{:>4}  {:>8}  {:<12} {}",
        format!("{} · {}", w.agent, w.name),
        share,
        at_reset,
        resets,
        w.source
    );
    // 5-hour rows open the day of their latest reading; weekly ones span
    // the period.
    let latest = w
        .start
        .zip(w.readings.last())
        .map(|(start, r)| ((start + r.0 as i32).div_euclid(1440) + data.today as i32) as u32);
    let row = match latest {
        _ if reset => line(text),
        Some(day) if w.minutes <= 300 && data.days.contains(&day) => {
            let action = if day == data.today {
                "See today's sessions"
            } else {
                "See that day's sessions"
            };
            link(text, View::Day(day), action)
        }
        Some(_) if w.minutes > 300 => link(text, View::Week, "See every day in this window"),
        _ => line(text),
    };
    let mut rows = vec![row];
    if reset {
        let since = if w.source == "relay" {
            "no relay reading since"
        } else {
            "no reading since"
        };
        rows.push(line(format!("  {} {} reset; {since}", w.agent, w.name)));
    }
    if let Some(note) = w.note {
        rows.push(line(format!("  {} {}: {note}", w.agent, w.name)));
    }
    rows
}

/// How recent weeks ended, and how to read the pace column.
fn history_rows(data: &Data) -> Vec<Row> {
    // The relay may have seen fewer than 4 weeks; it says so, never pads.
    let weekly = match &data.weekly_ends {
        Some(ends) => {
            let shown: Vec<String> = ends.iter().map(|e| format!("{e}%")).collect();
            let seen = match ends.len() {
                4 => String::new(),
                1 => " (1 week of relay data)".into(),
                n => format!(" ({n} weeks of relay data)"),
            };
            format!("Claude weekly ended at {}{seen}", shown.join(", "))
        }
        None if relay_missing(data) => "Claude weekly endings need the statusline relay".into(),
        None => "Claude weekly: no full week of relay data yet".into(),
    };
    let hits = match &data.limit_hits_by_agent {
        None => format!("5-hour limit stopped work {}×", data.limit_hits),
        Some(by) => {
            let parts: Vec<String> = by.iter().map(|(a, n)| format!("{a} {n}×")).collect();
            format!("Limits stopped work: {}", parts.join(", "))
        }
    };
    let mut rows = vec![line("── Last 4 weeks ──"), line(weekly), line(hits)];
    if relay_missing(data) {
        rows.push(line("Claude windows need the statusline"));
        rows.push(line("relay; Lens doesn't guess them."));
    }
    rows.extend([
        line("── How to read it ──"),
        line("At reset: straight line through this"),
        line("window's readings. — means one reading."),
        line("Provider meters are the source of truth."),
    ]);
    rows
}

/// Local data with no relay file: Claude readings can't exist.
fn relay_missing(data: &Data) -> bool {
    data.local()
        && data.weekly_ends.is_none()
        && data
            .windows
            .iter()
            .any(|w| w.note == Some("relay not set up"))
}

/// Plan headroom: each provider window's used share, its pace to reset,
/// and where the number came from. No plan is recommended: Lens can't see
/// what other plans allow.
pub(crate) fn headroom(data: &Data) -> Content {
    let answer = match filling(data) {
        Some((w, used)) => match full_until(w, data) {
            Some(until) => format!(
                "{} {} window is {used}% used. It's full until {until}.",
                w.agent, w.name
            ),
            None => format!(
                "{} {} window is {used}% used. At this pace it's full by {}.",
                w.agent,
                w.name,
                full_by(w, data.today).unwrap()
            ),
        },
        None => "No window is on pace to fill before it resets.".into(),
    };
    let mut rows: Vec<Row> = data
        .windows
        .iter()
        .flat_map(|w| window_rows(w, data))
        .collect();
    rows.extend(history_rows(data));
    (
        answer,
        format!(
            "Claude + Codex | {} {} | Plan share",
            day_label(data.today),
            clock(data.today, data.now)
        ),
        rows,
    )
}

/// Home's findings, most urgent first: a window on pace to fill (it can stop
/// work), an unusual day, a load-order gap, and partial input.
fn findings(all: &Data, count: bool) -> Vec<Row> {
    let (days, normal) = screen::daily(all, count);
    let mut rows = Vec::new();
    if let Some((w, used)) = filling(all) {
        rows.push(link(
            match full_until(w, all) {
                Some(until) => format!("{} {}: {used}%, full until {until}", w.agent, w.name),
                None => format!(
                    "{} {}: {used}%, full by {}",
                    w.agent,
                    w.name,
                    full_by(w, all.today).unwrap()
                ),
            },
            View::Headroom,
            "See every plan window and its pace",
        ));
    } else if all.local() {
        // The fixture always has a filling window; real weeks often don't,
        // and Plan headroom must still be reachable.
        rows.push(link(
            "Plan windows: none filling",
            View::Headroom,
            "See every plan window and its pace",
        ));
    }
    if let Some((day, value)) = days
        .iter()
        .filter(|d| !d.partial)
        .filter_map(|d| d.value.map(|v| (d.day, v)))
        .max_by_key(|&(_, v)| v)
        .filter(|&(_, v)| normal > 0 && v > normal * 3)
    {
        rows.push(link(
            format!("{}: {}× a normal day", day_label(day), value / normal),
            View::Day(day),
            "See the sessions behind that day",
        ));
    }
    // The fixture's one load-order gap. No log records load order.
    if !all.local() {
        rows.push(link(
            "review-kit: unknown load in Codex",
            View::Component(0, Scope::WEEK),
            "Inspect copies and use this week",
        ));
    }
    if let Some(id) = all.iter().position(|s| s.incomplete) {
        rows.push(link(
            format!(
                "{}: partial input{}",
                day_label(all[id].day),
                if all.local() { "" } else { ", unpriced" }
            ),
            View::Session(id),
            "Open the session",
        ));
    }
    rows
}

/// The dashboard: one sentence on what was used, then findings to open.
/// It describes the week; judgments live on the screens the findings open.
pub(crate) fn home(all: &Data, count: bool) -> Content {
    let requests: usize = all.iter().map(|s| s.requests.len()).sum();
    let cents = if any_priced(all.iter()) {
        usd(total(all.iter(), false))
    } else {
        "unpriced".into()
    };
    let mut rows = findings(all, count);
    rows.push(link(
        "Every day and its sessions",
        View::Week,
        "Open daily activity",
    ));
    let answer = if all.is_empty() {
        "No sessions in the last 7 days".into()
    } else {
        format!(
            "{} sessions · {requests} requests · {} tokens · {} {}",
            all.session_count(),
            screen::compact(screen::total_tokens(all)),
            cents,
            all.dollars()
        )
    };
    (
        answer,
        format!(
            "All workspaces | All agents | {} | {}",
            all.period(),
            all.unit(count)
        ),
        rows,
    )
}
