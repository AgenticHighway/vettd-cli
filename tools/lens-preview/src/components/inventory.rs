//! The component inventory and one component's copies and recorded use.

use super::*;
use crate::fixture::Component;
use std::collections::BTreeMap;

/// Room left in an inventory row beside the preview pane at 80 columns.
const SUMMARY_WIDTH: usize = 18;

/// Keeps the row's state readable: unknown first, then the full list if it
/// fits, otherwise the first state and how many more the copies carry.
fn state_summary(states: &BTreeSet<&'static str>) -> String {
    let mut ordered: Vec<_> = states.iter().copied().collect();
    ordered.sort_by_key(|s| *s != "Unknown");
    let joined = ordered.join("/");
    if joined.chars().count() <= SUMMARY_WIDTH {
        joined
    } else {
        format!("{} +{} more", ordered[0], ordered.len() - 1)
    }
}

/// One inventory row, or None when no copy passes the evidence filter.
fn component_row(
    c: usize,
    comp: &Component,
    scope: Scope,
    evid: Option<&str>,
    rt: &Runtime,
) -> Option<Row> {
    let copies: Vec<usize> = copies_of(c, &scope)
        .into_iter()
        .filter(|&p| evid.is_none_or(|e| evidence(&PLACEMENTS[p]) == e))
        .collect();
    if copies.is_empty() {
        return None;
    }
    let states: BTreeSet<_> = copies
        .iter()
        .map(|&p| state(p, scope.ws, &rt.disabled))
        .collect();
    let summary = state_summary(&states);
    Some(link(
        format!(
            "{:<7} {:<15} {:>2} {summary}",
            comp.kind,
            comp.name,
            copies.len()
        ),
        View::Component(c, scope),
        "Inspect component, copies and use",
    ))
}

pub(super) fn inventory(page: &Page, rt: &Runtime) -> (String, String, Vec<Row>) {
    let [kind, ws, agent, evid] = page.filters;
    let scope = Scope {
        ws: pick(&WORKSPACES, ws),
        agent: pick(&AGENTS, agent),
        ..Scope::WEEK
    };
    let kind = pick(&KINDS, kind);
    let evid = pick(&EVIDENCE, evid);
    let rows: Vec<Row> = COMPONENTS
        .iter()
        .enumerate()
        .filter(|(_, c)| kind.is_none_or(|k| c.kind == k))
        .filter_map(|(c, comp)| component_row(c, comp, scope, evid, rt))
        .collect();
    let scope_text = format!(
        "{} of {} | {} | {} | {} | {} | {SNAPSHOT}",
        rows.len(),
        COMPONENTS.len(),
        scope
            .ws
            .map_or("All workspaces".into(), |w| format!("Workspace {w}")),
        scope.agent.unwrap_or("All agents"),
        kind.unwrap_or("All types"),
        evid.unwrap_or("Any evidence"),
    );
    ("Component inventory".into(), scope_text, rows)
}

/// Calls, estimated context cost and identity coverage for the uses.
fn usage_summary(uses: &[(usize, usize)], all: &[Session]) -> Vec<Row> {
    if uses.is_empty() {
        return vec![line("No use observed in this scope")];
    }
    let usage = |&(s, u): &(usize, usize)| &all[s].observed[u];
    let calls: u32 = uses.iter().map(|k| usage(k).count).sum();
    let cents: u32 = uses
        .iter()
        .filter(|(s, _)| !estimate_missing(&all[*s]))
        .map(|k| usage(k).context_cents)
        .sum();
    let identified: u32 = uses
        .iter()
        .map(usage)
        .filter(|u| u.artifact.is_some())
        .map(|u| u.count)
        .sum();
    let mut rows = vec![
        line(format!("{calls} calls | {} est. context", usd(cents))),
        line(format!("{identified}/{calls} calls with artifact identity")),
    ];
    let unpriced = uses
        .iter()
        .filter(|(s, _)| estimate_missing(&all[*s]))
        .count();
    if unpriced > 0 {
        rows.push(line(format!("{unpriced} observation(s) in unpriced input")));
    }
    let partial = uses
        .iter()
        .filter(|(s, _)| all[*s].incomplete && !estimate_missing(&all[*s]))
        .count();
    if partial > 0 {
        rows.push(line(format!(
            "{partial} observation(s) in partial input: their estimates are floors"
        )));
    }
    rows
}

fn copy_rows(c: usize, scope: &Scope, rt: &Runtime) -> Vec<Row> {
    let mut rows = vec![line("── Copies in snapshot ──")];
    rows.extend(copies_of(c, scope).into_iter().map(|p| {
        let pl = &PLACEMENTS[p];
        link(
            format!(
                "{:<4} {:<6} {:<9} {:<5} {}",
                pl.workspace.unwrap_or("all"),
                pl.agent.unwrap_or("?"),
                pl.scope,
                pl.version.unwrap_or("-"),
                state(p, scope.ws, &rt.disabled)
            ),
            View::Placement(p),
            "Inspect this exact copy",
        )
    }));
    rows
}

/// Each recorded use, then the signals they carried, summed.
fn observed_rows(c: usize, scope: Scope, uses: &[(usize, usize)], all: &[Session]) -> Vec<Row> {
    let mut rows = Vec::new();
    if !uses.is_empty() {
        rows.push(line(format!("── Observed {} ──", scope.period())));
    }
    let mut signals = BTreeMap::new();
    for &(s, u) in uses {
        let (session, usage) = (&all[s], &all[s].observed[u]);
        for &(signal, n) in usage.signals {
            *signals.entry(signal).or_insert(0) += n;
        }
        rows.push(link(
            format!(
                "Nov {} {:<3} {:<14} {:<4} {:>2} calls",
                session.day,
                session.workspace,
                session.name,
                usage.version.unwrap_or("?"),
                usage.count
            ),
            View::Observation(s, u),
            "Inspect this observation",
        ));
    }
    if !signals.is_empty() {
        rows.push(line("── Signals ──"));
    }
    rows.extend(signals.into_iter().map(|(signal, n)| {
        link(
            format!("{signal:<20} {n:>3}"),
            View::Signal(c, signal, scope),
            "Show matching observations",
        )
    }));
    rows
}

pub(super) fn component(
    c: usize,
    scope: Scope,
    all: &[Session],
    rt: &Runtime,
) -> (String, String, Vec<Row>) {
    let comp = &COMPONENTS[c];
    let uses = uses_in(all, c, &scope);
    let mut rows = usage_summary(&uses, all);
    rows.extend(copy_rows(c, &scope, rt));
    rows.extend(observed_rows(c, scope, &uses, all));
    (
        format!("{} {}", comp.kind, comp.name),
        format!("{} | Calls, est. context USD", scope.label()),
        rows,
    )
}
