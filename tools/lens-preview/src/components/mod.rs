use super::{line, link, usd, Page, Row, Runtime, Scope, View};
use crate::fixture::{
    Placement, Session, AGENTS, COMPONENTS, KINDS, PLACEMENTS, SCANS, SNAPSHOT, WORKSPACES,
};
use std::collections::BTreeSet;

mod copy;
mod inventory;

use copy::{change, observation, placement, signal_rows};
use inventory::{component, inventory};

pub const EVIDENCE: [&str; 4] = ["Scan match", "No scan", "Unverified", "Unknown"];

/// Scan evidence is looked up by artifact identity only.
pub fn scan(artifact: Option<&str>) -> Option<&'static str> {
    SCANS
        .iter()
        .find(|(id, _)| Some(*id) == artifact)
        .map(|(_, evidence)| *evidence)
}

fn scan_text(artifact: Option<&str>) -> &'static str {
    match (artifact, scan(artifact)) {
        (_, Some(found)) => found,
        (Some(_), None) => "None for this artifact",
        (None, None) => "Unavailable: no artifact",
    }
}

fn applies(p: &Placement, scope: &Scope) -> bool {
    let ws = p.workspace.is_none() || scope.ws.is_none() || p.workspace == scope.ws;
    let agent = p.agent.is_none() || scope.agent.is_none() || p.agent == scope.agent;
    ws && agent
}

fn copies_of(component: usize, scope: &Scope) -> Vec<usize> {
    (0..PLACEMENTS.len())
        .filter(|&i| PLACEMENTS[i].component == component && applies(&PLACEMENTS[i], scope))
        .collect()
}

fn uses_in(all: &[Session], component: usize, scope: &Scope) -> Vec<(usize, usize)> {
    all.iter()
        .enumerate()
        .filter(|(_, s)| scope.has(s))
        .flat_map(|(sid, s)| {
            s.observed
                .iter()
                .enumerate()
                .filter(move |(_, u)| u.component == component)
                .map(move |(u, _)| (sid, u))
        })
        .collect()
}

fn label(p: &Placement) -> String {
    let scope = p.scope.to_lowercase();
    p.workspace
        .map_or(scope.clone(), |w| format!("{w} {scope}"))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resolution {
    Copy(usize),
    NoCopy,
    Unknown(&'static str),
}

fn resolvable(p: &Placement) -> bool {
    COMPONENTS[p.component].kind == "skill" && matches!(p.scope, "Workspace" | "User")
}

/// Demo precedence, valid only inside this fixture: for Claude skills an
/// enabled workspace copy wins over a user copy. Other hosts stay unknown.
pub fn resolve(component: usize, ws: &str, agent: &str, disabled: &BTreeSet<usize>) -> Resolution {
    let candidates: Vec<usize> = (0..PLACEMENTS.len())
        .filter(|&i| {
            let p = &PLACEMENTS[i];
            p.component == component
                && resolvable(p)
                && p.agent == Some(agent)
                && (p.workspace.is_none() || p.workspace == Some(ws))
        })
        .collect();
    if candidates.is_empty() {
        return Resolution::NoCopy;
    }
    if agent != "Claude" {
        return Resolution::Unknown("adapter cannot resolve Codex skills");
    }
    candidates
        .into_iter()
        .filter(|i| !disabled.contains(i))
        .min_by_key(|&i| PLACEMENTS[i].scope != "Workspace")
        .map_or(Resolution::NoCopy, Resolution::Copy)
}

fn describe(res: Resolution) -> String {
    match res {
        Resolution::Copy(q) => {
            let p = &PLACEMENTS[q];
            format!(
                "{} {} ({})",
                label(p),
                p.version.unwrap_or("?"),
                p.artifact.unwrap_or("no artifact")
            )
        }
        Resolution::NoCopy => "No enabled copy".into(),
        Resolution::Unknown(why) => format!("Unknown: {why}"),
    }
}

fn contexts(p: &Placement, ws: Option<&'static str>) -> Vec<&'static str> {
    match (p.workspace, ws) {
        (Some(w), _) | (None, Some(w)) => vec![w],
        (None, None) => WORKSPACES.to_vec(),
    }
}

pub fn state(pid: usize, ws: Option<&'static str>, disabled: &BTreeSet<usize>) -> &'static str {
    let p = &PLACEMENTS[pid];
    if disabled.contains(&pid) {
        return "Disabled";
    }
    if !resolvable(p) {
        return match (p.scope, COMPONENTS[p.component].kind) {
            ("Built-in", _) => "Built-in",
            ("Plugin", _) => "Via plugin",
            (_, "MCP") => "Unverified",
            (_, "plugin") => "Installed",
            (_, "unknown") => "Unknown",
            _ => "Configured",
        };
    }
    let agent = p.agent.unwrap_or_default();
    let mut labels: Vec<_> = contexts(p, ws)
        .into_iter()
        .map(|w| match resolve(p.component, w, agent, disabled) {
            Resolution::Copy(q) if q == pid => "Effective",
            Resolution::Copy(_) => "Fallback",
            Resolution::Unknown(_) => "Unknown",
            Resolution::NoCopy => "Not loaded",
        })
        .collect();
    labels.dedup();
    if labels.len() == 1 {
        labels[0]
    } else {
        "Per workspace"
    }
}

pub fn evidence(p: &Placement) -> &'static str {
    match (p.scope, COMPONENTS[p.component].kind) {
        (_, "MCP") => "Unverified",
        ("Built-in", _) => "Built-in",
        (_, "unknown") => "Unknown",
        _ if scan(p.artifact).is_some() => "Scan match",
        _ if p.artifact.is_some() => "No scan",
        _ => "Unknown",
    }
}

/// Applies a staged change to process-local state only.
pub fn apply(pid: usize, disable: bool, rt: &mut Runtime) {
    if disable {
        rt.disabled.insert(pid);
    } else {
        rt.disabled.remove(&pid);
    }
    let p = &PLACEMENTS[pid];
    let ws = p.workspace.unwrap_or_default();
    let now = resolve(p.component, ws, p.agent.unwrap_or_default(), &rt.disabled);
    rt.notice = Some((
        pid,
        format!(
            "Applied in memory: {}. {ws} loads {}",
            if disable { "disabled" } else { "re-enabled" },
            describe(now)
        ),
    ));
}

fn pick(values: &[&'static str], index: usize) -> Option<&'static str> {
    index.checked_sub(1).map(|i| values[i])
}

pub fn rows(page: &Page, all: &[Session], rt: &Runtime) -> (String, String, Vec<Row>) {
    match page.view {
        View::Inventory => inventory(page, all, rt),
        View::Component(c, scope) => component(c, scope, all, rt),
        View::Placement(pid) => placement(pid, all, rt),
        View::Observation(sid, u) => observation(sid, u, all),
        View::Signal(c, signal, scope) => signal_rows(c, signal, scope, all),
        View::Change(pid, disable) | View::Apply(pid, disable) => change(pid, disable, rt),
        _ => unreachable!("component view"),
    }
}
