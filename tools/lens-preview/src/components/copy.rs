//! One exact copy, one recorded observation, a signal's matches, and a
//! staged change.

use super::*;

fn competes(a: &Placement, b: &Placement) -> bool {
    a.component == b.component
        && a.agent == b.agent
        && resolvable(a)
        && resolvable(b)
        && (a.workspace.is_none() || b.workspace.is_none() || a.workspace == b.workspace)
}

/// The copy's identity: where it lives, its version, artifact and scan.
fn identity_rows(p: &Placement) -> Vec<Row> {
    let mut rows = vec![
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
    ];
    rows.extend(p.native.iter().map(|n| line(*n)));
    rows
}

/// Which copy loads in each workspace this copy applies to.
fn resolution_rows(pid: usize, p: &Placement, rt: &Runtime) -> Vec<Row> {
    if !resolvable(p) {
        return vec![line(format!(
            "State     {}",
            state(pid, None, &rt.disabled)
        ))];
    }
    let agent = p.agent.unwrap_or_default();
    let mut rows: Vec<Row> = contexts(p, None)
        .into_iter()
        .map(|w| {
            let text = match resolve(p.component, w, agent, &rt.disabled) {
                Resolution::Copy(q) if q == pid => "this copy".into(),
                other => describe(other),
            };
            line(format!("Loads in {w}: {text}"))
        })
        .collect();
    if agent == "Claude" {
        rows.push(line("Rule      fixture: workspace > user"));
    }
    rows
}

fn action_row(pid: usize, p: &Placement, rt: &Runtime) -> Row {
    if !p.can_disable {
        return line("Actions   read-only in this adapter");
    }
    let disabled = rt.disabled.contains(&pid);
    link(
        if disabled {
            "Stage re-enable"
        } else {
            "Stage disable"
        },
        View::Change(pid, !disabled),
        "Preview this change before applying",
    )
}

/// Competing copies, the containing plugin, and contained items.
fn relation_rows(pid: usize, p: &Placement, rt: &Runtime) -> Vec<Row> {
    let mut rows = Vec::new();
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
    rows
}

/// Recorded uses of this exact artifact, matched by artifact only.
fn seen_rows(p: &Placement, all: &[Session]) -> Vec<Row> {
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
        return vec![line("Observed  none with this artifact")];
    }
    seen.into_iter()
        .map(|(s, u, count)| {
            link(
                format!("Observed  Nov {} {} x{count}", all[s].day, all[s].name),
                View::Observation(s, u),
                "Inspect observation of this artifact",
            )
        })
        .collect()
}

pub(super) fn placement(pid: usize, all: &[Session], rt: &Runtime) -> (String, String, Vec<Row>) {
    let p = &PLACEMENTS[pid];
    let comp = &COMPONENTS[p.component];
    let mut rows = Vec::new();
    if let Some((target, message)) = &rt.notice {
        if *target == pid {
            rows.push(line(message.clone()));
        }
    }
    rows.extend(identity_rows(p));
    rows.extend(resolution_rows(pid, p, rt));
    rows.push(action_row(pid, p, rt));
    rows.extend(relation_rows(pid, p, rt));
    rows.extend(seen_rows(p, all));
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

/// Where an observed artifact is installed today, if anywhere.
fn installed_row(artifact: Option<&'static str>) -> Row {
    let installed = artifact.and_then(|a| PLACEMENTS.iter().position(|p| p.artifact == Some(a)));
    match (installed, artifact) {
        (Some(q), _) => link(
            format!("Installed {} copy, same artifact", label(&PLACEMENTS[q])),
            View::Placement(q),
            "Inspect the installed copy with this artifact",
        ),
        (None, Some(_)) => line("Installed none with this artifact"),
        (None, None) => line("Installed unknown: artifact not recorded"),
    }
}

pub(super) fn observation(sid: usize, u: usize, all: &[Session]) -> (String, String, Vec<Row>) {
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
        line(if estimate_missing(s) {
            "Context   Unpriced (partial input)".to_string()
        } else if s.incomplete {
            format!(
                "Context   ≥{} est., component (partial input)",
                usd(usage.context_cents)
            )
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
    rows.extend([
        installed_row(usage.artifact),
        line("History   recorded; not a change target"),
        link(
            format!("{} on Nov {} in {}", comp.name, s.day, s.workspace),
            View::Component(usage.component, scope),
            "Inspect component in this scope",
        ),
    ]);
    (
        format!("{} {} / observation", comp.kind, comp.name),
        format!("{} | Recorded history", scope.label()),
        rows,
    )
}

pub(super) fn signal_rows(
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

pub(super) fn change(pid: usize, disable: bool, rt: &Runtime) -> (String, String, Vec<Row>) {
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
