use super::{line, link, usd, Page, Row, Runtime, Scope, View};
use crate::fixture::{
    Placement, Session, AGENTS, COMPONENTS, KINDS, PLACEMENTS, SCANS, SNAPSHOT, WORKSPACES,
};
use std::collections::{BTreeMap, BTreeSet};

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
        View::Inventory => inventory(page, rt),
        View::Component(c, scope) => component(c, scope, all, rt),
        View::Placement(pid) => placement(pid, all, rt),
        View::Observation(sid, u) => observation(sid, u, all),
        View::Signal(c, signal, scope) => signal_rows(c, signal, scope, all),
        View::Change(pid, disable) | View::Apply(pid, disable) => change(pid, disable, rt),
        _ => unreachable!("component view"),
    }
}

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

fn inventory(page: &Page, rt: &Runtime) -> (String, String, Vec<Row>) {
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
        .filter_map(|(c, comp)| {
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
        })
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

fn component(c: usize, scope: Scope, all: &[Session], rt: &Runtime) -> (String, String, Vec<Row>) {
    let comp = &COMPONENTS[c];
    let uses = uses_in(all, c, &scope);
    let mut rows = Vec::new();
    if uses.is_empty() {
        rows.push(line("No use observed in this scope"));
    } else {
        let usage = |&(s, u): &(usize, usize)| &all[s].observed[u];
        let calls: u32 = uses.iter().map(|k| usage(k).count).sum();
        let cents: u32 = uses
            .iter()
            .filter(|(s, _)| !all[*s].incomplete)
            .map(|k| usage(k).context_cents)
            .sum();
        let identified: u32 = uses
            .iter()
            .map(usage)
            .filter(|u| u.artifact.is_some())
            .map(|u| u.count)
            .sum();
        rows.push(line(format!("{calls} calls | {} est. context", usd(cents))));
        rows.push(line(format!(
            "{identified}/{calls} calls with artifact identity"
        )));
        let unpriced = uses.iter().filter(|(s, _)| all[*s].incomplete).count();
        if unpriced > 0 {
            rows.push(line(format!("{unpriced} observation(s) in unpriced input")));
        }
    }
    rows.push(line("── Copies in snapshot ──"));
    for p in copies_of(c, &scope) {
        let pl = &PLACEMENTS[p];
        rows.push(link(
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
        ));
    }
    let mut signals = BTreeMap::new();
    if !uses.is_empty() {
        rows.push(line(format!("── Observed {} ──", scope.period())));
    }
    for &(s, u) in &uses {
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
    for (signal, n) in signals {
        rows.push(link(
            format!("{signal:<20} {n:>3}"),
            View::Signal(c, signal, scope),
            "Show matching observations",
        ));
    }
    (
        format!("{} {}", comp.kind, comp.name),
        format!("{} | Calls, est. context USD", scope.label()),
        rows,
    )
}

fn competes(a: &Placement, b: &Placement) -> bool {
    a.component == b.component
        && a.agent == b.agent
        && resolvable(a)
        && resolvable(b)
        && (a.workspace.is_none() || b.workspace.is_none() || a.workspace == b.workspace)
}

fn placement(pid: usize, all: &[Session], rt: &Runtime) -> (String, String, Vec<Row>) {
    let p = &PLACEMENTS[pid];
    let comp = &COMPONENTS[p.component];
    let mut rows = Vec::new();
    if let Some((target, message)) = &rt.notice {
        if *target == pid {
            rows.push(line(message.clone()));
        }
    }
    rows.extend([
        line(format!("Locator   {}", p.locator)),
        line(format!(
            "Host      {} | {} scope",
            p.agent.unwrap_or("Unknown"),
            p.scope
        )),
        line(format!("Version   {}", p.version.unwrap_or("Not declared"))),
        line(format!(
            "Artifact  {}",
            p.artifact.unwrap_or("None recorded")
        )),
        line(format!("Format    {}", p.format)),
        line(format!("Scan      {}", scan_text(p.artifact))),
    ]);
    rows.extend(p.native.iter().map(|n| line(*n)));
    if resolvable(p) {
        let agent = p.agent.unwrap_or_default();
        for w in contexts(p, None) {
            let res = resolve(p.component, w, agent, &rt.disabled);
            let text = match res {
                Resolution::Copy(q) if q == pid => "this copy".into(),
                other => describe(other),
            };
            rows.push(line(format!("Loads in {w}: {text}")));
        }
        if agent == "Claude" {
            rows.push(line("Rule      fixture: workspace > user"));
        }
    } else {
        rows.push(line(format!(
            "State     {}",
            state(pid, None, &rt.disabled)
        )));
    }
    if p.can_disable {
        let disabled = rt.disabled.contains(&pid);
        rows.push(link(
            if disabled {
                "Stage re-enable"
            } else {
                "Stage disable"
            },
            View::Change(pid, !disabled),
            "Preview this change before applying",
        ));
    } else {
        rows.push(line("Actions   read-only in this adapter"));
    }
    for (q, other) in PLACEMENTS.iter().enumerate() {
        if q != pid && competes(p, other) {
            rows.push(link(
                format!(
                    "Competing {} {} ({})",
                    label(other),
                    other.version.unwrap_or("?"),
                    state(q, p.workspace, &rt.disabled)
                ),
                View::Placement(q),
                "Inspect competing copy",
            ));
        }
    }
    if let Some(parent) = p.parent {
        rows.push(link(
            format!(
                "Part of   plugin {}",
                COMPONENTS[PLACEMENTS[parent].component].name
            ),
            View::Placement(parent),
            "Inspect containing plugin",
        ));
    }
    for (q, child) in PLACEMENTS.iter().enumerate() {
        if child.parent == Some(pid) {
            let c = &COMPONENTS[child.component];
            rows.push(link(
                format!("Contains  {} {}", c.kind, c.name),
                View::Placement(q),
                "Inspect contained item",
            ));
        }
    }
    let seen: Vec<_> = all
        .iter()
        .enumerate()
        .flat_map(|(s, session)| {
            session
                .observed
                .iter()
                .enumerate()
                .filter(|(_, u)| p.artifact.is_some() && u.artifact == p.artifact)
                .map(move |(u, usage)| (s, u, usage.count))
        })
        .collect();
    if p.artifact.is_some() && seen.is_empty() {
        rows.push(line("Observed  none with this artifact"));
    }
    for (s, u, count) in seen {
        rows.push(link(
            format!("Observed  Nov {} {} x{count}", all[s].day, all[s].name),
            View::Observation(s, u),
            "Inspect observation of this artifact",
        ));
    }
    (
        format!("{} {} / {} copy", comp.kind, comp.name, label(p)),
        format!(
            "{} | {} | {SNAPSHOT}",
            p.workspace
                .map_or("All workspaces".into(), |w| format!("Workspace {w}")),
            p.agent.unwrap_or("Unknown host")
        ),
        rows,
    )
}

fn observation(sid: usize, u: usize, all: &[Session]) -> (String, String, Vec<Row>) {
    let s = &all[sid];
    let usage = &s.observed[u];
    let comp = &COMPONENTS[usage.component];
    let scope = Scope::of(s);
    let mut rows = vec![
        link(
            format!("Session   {} | {} | {}", s.name, s.workspace, s.agent),
            View::Session(sid),
            "Open session cost and tokens",
        ),
        line(format!(
            "Version   {}",
            usage.version.unwrap_or("Not recorded")
        )),
        line(format!(
            "Artifact  {}",
            usage.artifact.unwrap_or("Not recorded")
        )),
        line(format!("Scan      {}", scan_text(usage.artifact))),
        line(if s.incomplete {
            "Context   Unpriced (partial input)".to_string()
        } else {
            format!("Context   {} est., component", usd(usage.context_cents))
        }),
    ];
    rows.extend(usage.signals.iter().map(|&(signal, n)| {
        link(
            format!("Signal    {signal} x{n}"),
            View::Signal(usage.component, signal, scope),
            "Show matching observations",
        )
    }));
    let installed = usage
        .artifact
        .and_then(|a| PLACEMENTS.iter().position(|p| p.artifact == Some(a)));
    rows.push(match (installed, usage.artifact) {
        (Some(q), _) => link(
            format!("Installed {} copy, same artifact", label(&PLACEMENTS[q])),
            View::Placement(q),
            "Inspect the installed copy with this artifact",
        ),
        (None, Some(_)) => line("Installed none with this artifact"),
        (None, None) => line("Installed unknown: artifact not recorded"),
    });
    rows.push(line("History   recorded; not a change target"));
    rows.push(link(
        format!("{} on Nov {} in {}", comp.name, s.day, s.workspace),
        View::Component(usage.component, scope),
        "Inspect component in this scope",
    ));
    (
        format!("{} {} / observation", comp.kind, comp.name),
        format!("{} | Recorded history", scope.label()),
        rows,
    )
}

fn signal_rows(
    c: usize,
    signal: &'static str,
    scope: Scope,
    all: &[Session],
) -> (String, String, Vec<Row>) {
    let mut rows = vec![line("Cost is attributed at component scope.")];
    for (s, u) in uses_in(all, c, &scope) {
        let (session, usage) = (&all[s], &all[s].observed[u]);
        if let Some((_, n)) = usage.signals.iter().find(|(name, _)| *name == signal) {
            rows.push(link(
                format!(
                    "Nov {} {:<3} {:<14} {:<4} x{n}",
                    session.day,
                    session.workspace,
                    session.name,
                    usage.version.unwrap_or("?")
                ),
                View::Observation(s, u),
                "Inspect matching observation",
            ));
        }
    }
    if rows.len() == 1 {
        rows.push(line("No matching observations in this scope"));
    }
    (
        format!("{} / {signal}", COMPONENTS[c].name),
        scope.label(),
        rows,
    )
}

fn change(pid: usize, disable: bool, rt: &Runtime) -> (String, String, Vec<Row>) {
    let p = &PLACEMENTS[pid];
    let ws = p.workspace.unwrap_or_default();
    let agent = p.agent.unwrap_or_default();
    let now = resolve(p.component, ws, agent, &rt.disabled);
    let mut next = rt.disabled.clone();
    if disable {
        next.insert(pid);
    } else {
        next.remove(&pid);
    }
    let after = resolve(p.component, ws, agent, &next);
    let verb = if disable { "disable" } else { "re-enable" };
    let rows = vec![
        line(format!(
            "Target    {} {} ({})",
            COMPONENTS[p.component].name,
            p.version.unwrap_or("?"),
            p.artifact.unwrap_or("no artifact")
        )),
        line(format!("Locator   {}", p.locator)),
        line(format!("Context   {agent} in workspace {ws}")),
        line(format!("Loads now {}", describe(now))),
        line(format!("Predicted {}", describe(after))),
        line(match after {
            Resolution::Unknown(_) => "Basis     none; outcome unknown",
            _ => "Basis     fixture rule: workspace > user",
        }),
        line("Unchanged other workspaces and history"),
        link(
            format!("Apply {verb} in memory"),
            View::Apply(pid, disable),
            "Apply for this process only",
        ),
        line("Esc cancels; nothing changes until Apply"),
    ];
    (
        format!("Preview: {verb} {} copy", label(p)),
        format!("Workspace {ws} | {agent} | Simulation | {SNAPSHOT}"),
        rows,
    )
}
