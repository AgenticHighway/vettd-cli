//! Application of projected `attachment` lines — the loaded set and the in-band rules files.
//!
//! Split out of `apply.rs` for the file-length budget; port of `_apply_attachment`, `_add_event`
//! and `_forbid_all` (`claude_code.py:493-535`).

use super::ReadState;
use crate::observe::claude_code::project::{mcp_server, ProjectedAttachment};
use crate::observe::types::{
    InBandAsset, InBandKind, LoadedSetEvent, LoadedSetKind, SessionFacts, BUILTIN_AGENT_TYPES,
};

/// Apply an `attachment` line (`_apply_attachment`, `claude_code.py:493-521`).
///
/// An attachment subtype the reader does not interpret is counted, exactly like a line type it does
/// not consume; a line with no attachment object at all counts the same way.
pub(crate) fn apply_attachment(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    attachment: Option<ProjectedAttachment>,
    ts_ms: i64,
) {
    let Some(attachment) = attachment else {
        facts.lines_unknown_type += 1;
        return;
    };
    match attachment {
        ProjectedAttachment::Unconsumed => facts.lines_unknown_type += 1,
        ProjectedAttachment::SkillListing {
            names,
            listing_bytes,
        } => {
            forbid_all(facts, &names);
            let event = LoadedSetEvent {
                ts_ms,
                kind: LoadedSetKind::Initial,
                skills: names,
                listing_bytes,
                ..Default::default()
            };
            add_event(facts, state, event);
        }
        ProjectedAttachment::AgentListingDelta { types, is_initial } => {
            apply_agent_listing(facts, state, types, is_initial, ts_ms);
        }
        ProjectedAttachment::McpInstructionsDelta { names } => {
            // The tool names are unchanged; a pending server just resolved.
            forbid_all(facts, &names);
            let event = LoadedSetEvent {
                ts_ms,
                kind: LoadedSetKind::Delta,
                ..Default::default()
            };
            add_event(facts, state, event);
        }
        other => apply_tools_or_memory(facts, state, other, ts_ms),
    }
}

/// The `agent_listing_delta` branch of [`apply_attachment`] (`claude_code.py:511-513`).
///
/// A harness built-in agent type stays in the event's own list but is never forbidden: as a
/// substring it would collide with legitimate closed-enum values on the wire.
fn apply_agent_listing(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    types: Vec<String>,
    is_initial: bool,
    ts_ms: i64,
) {
    let named: Vec<String> = types
        .iter()
        .filter(|agent| !BUILTIN_AGENT_TYPES.contains(&agent.as_str()))
        .cloned()
        .collect();
    forbid_all(facts, &named);
    let event = LoadedSetEvent {
        ts_ms,
        kind: initial_or_delta(is_initial),
        agent_types: types,
        ..Default::default()
    };
    add_event(facts, state, event);
}

/// The `deferred_tools_delta` and `nested_memory` branches of [`apply_attachment`], split out to
/// keep both functions inside the function-length budget.
fn apply_tools_or_memory(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    attachment: ProjectedAttachment,
    ts_ms: i64,
) {
    match attachment {
        ProjectedAttachment::DeferredToolsDelta {
            added,
            pending,
            failed,
            removed,
            readded,
            schema_bytes,
        } => {
            // The first deferred-tools listing of a read is the session's initial set; every later
            // one is a change to it.
            let kind = initial_or_delta(!state.seen_deferred);
            state.seen_deferred = true;
            forbid_mcp_names(facts, added.iter().chain(&removed).chain(&readded));
            forbid_all(facts, &pending);
            forbid_all(facts, &failed);
            add_event(
                facts,
                state,
                LoadedSetEvent {
                    ts_ms,
                    kind,
                    tool_names: added,
                    pending_mcp: pending,
                    failed_mcp: failed,
                    removed,
                    readded,
                    tool_schema_bytes: schema_bytes,
                    ..Default::default()
                },
            );
        }
        ProjectedAttachment::NestedMemory {
            basename,
            sha256,
            byte_len,
        } => apply_nested_memory(facts, state, basename, sha256, byte_len, ts_ms),
        _ => unreachable!("apply_attachment handles every other subtype"),
    }
}

/// Forbid every MCP tool name in `names` along with its server, and skip every non-MCP name
/// (`claude_code.py:505-507`).
///
/// A built-in tool name such as `Bash` or `Read` is not an asset and is deliberately left out: as a
/// substring it would collide with legitimate values on the wire.
fn forbid_mcp_names<'a>(facts: &mut SessionFacts, names: impl Iterator<Item = &'a String>) {
    for name in names {
        if let Some(server) = mcp_server(name) {
            facts.note_forbid("loaded_set_names", Some(name));
            facts.note_forbid("loaded_set_names", Some(server));
        }
    }
}

/// The `nested_memory` branch of [`apply_attachment`] (`claude_code.py:515-521`).
///
/// A rules file can be listed before or after the session's initial loaded-set event, so the
/// basename is both remembered for an initial event still to come and appended to one already seen.
fn apply_nested_memory(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    basename: String,
    sha256: String,
    byte_len: i64,
    ts_ms: i64,
) {
    facts.in_band_assets.push(InBandAsset {
        kind: InBandKind::RulesFile,
        name: basename.clone(),
        content_sha256: sha256,
        byte_len,
        ts_ms,
    });
    facts.note_forbid("loaded_set_names", Some(&basename));
    if let Some(index) = state.initial_event {
        facts.loaded_events[index]
            .rules_files
            .push(basename.clone());
    }
    state.rules_files.push(basename);
}

/// Append a loaded-set event, and let the first `initial` one absorb the rules files seen so far
/// (`_add_event`, `claude_code.py:524-528`).
fn add_event(facts: &mut SessionFacts, state: &mut ReadState, mut event: LoadedSetEvent) {
    if event.kind == LoadedSetKind::Initial && state.initial_event.is_none() {
        event.rules_files.extend(state.rules_files.iter().cloned());
        state.initial_event = Some(facts.loaded_events.len());
    }
    facts.loaded_events.push(event);
}

/// `Initial` when `is_initial`, `Delta` otherwise.
fn initial_or_delta(is_initial: bool) -> LoadedSetKind {
    if is_initial {
        LoadedSetKind::Initial
    } else {
        LoadedSetKind::Delta
    }
}

/// Forbid every name in `names` under `loaded_set_names` (`_forbid_all`, `claude_code.py:533-535`).
fn forbid_all(facts: &mut SessionFacts, names: &[String]) {
    for name in names {
        facts.note_forbid("loaded_set_names", Some(name));
    }
}

#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
