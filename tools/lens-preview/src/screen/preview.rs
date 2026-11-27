//! The preview pane: what Enter would open, explained before opening it.

use super::*;

pub(super) fn preview(
    view: View,
    all: &Data,
    count: bool,
    width: u16,
    rt: &Runtime,
) -> (String, Vec<String>) {
    let (title, scope, rows) = content(&Page::new(view), all, count, width, rt);
    match view {
        View::Day(day) => day_preview(day, all, count),
        View::Session(id) => {
            let session = &all[id];
            let mut detail = vec![
                format!(
                    "{} | {} | {}",
                    session.agent,
                    session.workspace,
                    day_label(session.day)
                ),
                cost_label(session, all.local()),
                String::new(),
            ];
            detail.extend(
                rows.into_iter()
                    .filter(|row| row.open.is_none() && !row.text.starts_with("──"))
                    .map(|row| row.text),
            );
            (all[id].name.into(), detail)
        }
        View::Tokens(_) => (
            crumb(view, all),
            rows.into_iter()
                .map(|row| row.text.replace(" tokens", " tok"))
                .collect(),
        ),
        View::Component(..) | View::Placement(_) | View::Change(..) => {
            let mut detail = vec![scope];
            detail.extend(rows.into_iter().map(|row| row.text));
            (title, detail)
        }
        _ => (
            crumb(view, all),
            rows.into_iter().map(|row| row.text).collect(),
        ),
    }
}

/// A day's total, its change from the day before by cost part, and the
/// session that contributed most.
fn day_preview(day: u32, all: &[Session], count: bool) -> (String, Vec<String>) {
    let current: Vec<_> = all.iter().filter(|s| s.day == day).collect();
    let prior: Vec<_> = all.iter().filter(|s| s.day + 1 == day).collect();
    let total: u32 = total(current.iter().copied(), count);
    let mut preview = vec![
        headline(&current, total, count),
        format!("{} sessions", current.len()),
    ];
    if current.is_empty() {
        preview[0] = "No session input".into();
    } else {
        preview.extend(comparison(day, &current, &prior, total, count));
    }
    preview.extend(lead(&current, count));
    (format!("{} | UTC", day_label(day)), preview)
}

fn headline(current: &[&Session], total: u32, count: bool) -> String {
    if count {
        format!("{total} requests")
    } else if current.iter().all(|s| priced(s) == 0) {
        "No priced requests".into()
    } else {
        format!("{} priced USD", usd(total))
    }
}

/// The change from the prior day, only when both days are comparable.
fn comparison(
    day: u32,
    current: &[&Session],
    prior: &[&Session],
    total: u32,
    count: bool,
) -> Vec<String> {
    if prior.is_empty() || current.iter().chain(prior).any(|s| !comparable(s, count)) {
        let mut lines = vec!["No comparable prior day".to_string()];
        if current.iter().any(|s| s.incomplete) {
            lines.push("Partial / unpriced input".into());
        }
        return lines;
    }
    if count {
        let previous: u32 = prior.iter().map(|s| measure(s, true)).sum();
        return vec![format!(
            "{:+} req vs {}",
            i64::from(total) - i64::from(previous),
            day_label(day - 1)
        )];
    }
    let sum = |sessions: &[&Session], i: usize| -> u32 {
        sessions
            .iter()
            .flat_map(|s| &s.requests)
            .map(|r| r.cents()[i])
            .sum()
    };
    let previous: u32 = super::total(prior.iter().copied(), false);
    let delta =
        |a: u32, b: u32| format!("{}{}", if a >= b { "+" } else { "−" }, usd(a.abs_diff(b)));
    let mut lines = vec![
        String::new(),
        format!("{} vs {}", delta(total, previous), day_label(day - 1)),
    ];
    lines.extend(
        PARTS
            .iter()
            .enumerate()
            .map(|(i, part)| format!("{part:<13} {:>8}", delta(sum(current, i), sum(prior, i)))),
    );
    lines
}

fn lead(current: &[&Session], count: bool) -> Vec<String> {
    let Some(lead) = current
        .iter()
        .filter(|s| count || priced(s) != 0)
        .max_by_key(|s| measure(s, count))
    else {
        return Vec::new();
    };
    vec![
        String::new(),
        if count {
            "Most requests"
        } else {
            "Largest priced session"
        }
        .into(),
        format!("{}  {}", lead.name, units(measure(lead, count), count)),
    ]
}
