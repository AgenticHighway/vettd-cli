//! Application of projected lines to the growing [`SessionFacts`].
//!
//! Port of the "application of projected lines" section of
//! `spikes/828-passive-observer/prototype/sources/claude_code.py` (lines 345-529). Everything here
//! works on a [`Projected`], never on a raw line: by the time [`apply`] is called the transcript
//! text is already a set of hashes, lengths, booleans and closed-vocabulary names.
//!
//! [`ReadState`] is per-read scratch — the calls still waiting for a result, and the bookkeeping the
//! contract keys on "first" events. It is discarded when the read returns; nothing in it belongs to
//! the facts.
//!
//! The `attachment` and in-band-skill halves live in the private `attachments.rs` and `skills.rs`
//! submodules declared here (rather than in `claude_code/mod.rs`) so every file stays inside
//! CONTRIBUTING.md's 400-line budget; the seam callers use is still exactly
//! `claude_code::apply::{ReadState, apply}`.
//!
//! Three defects of the prototype are fixed here rather than reproduced (see
//! `docs/vettd-observe-port-plan.md`, "Three confirmed prototype defects"):
//!
//! 1. `claude_code.py:376-382` keeps the permission-mode tally in
//!    `facts.forbids["_permission_modes"]`, where scratch state leaks into the gate's dynamic
//!    forbids sidecar. [`SessionFacts::mode_counts`] is a real field and no `_`-prefixed bucket is
//!    ever written.
//! 2. `claude_code.py:395-403`'s "keep the fullest usage" branch is unreachable — its enclosing
//!    guard is `mid not in state.seen_message_ids`, so the entry it compares against is always
//!    absent. Per file the **first** line for a `message.id` wins; choosing the largest
//!    `output_tokens` is the tree-wide rule that belongs to `extract`'s `dedupe_usages`.
//! 3. The permission-mode tie-break is documented as "keep the earlier mode" and implemented as
//!    `max(sorted(counts.items()), key=count)`, which keeps the **alphabetically smallest**.
//!    [`most_frequent_mode`] keeps the code's behaviour, deliberately, and says so.

#[path = "attachments.rs"]
mod attachments;
#[path = "skills.rs"]
mod skills;

use std::collections::{BTreeMap, BTreeSet};

use skills::PendingSkill;

use super::project::{
    mcp_server, LineKind, Projected, ProjectedBlock, ProjectedToolUseResult, ProjectedUsage,
};
use crate::observe::types::{
    parse_ts_ms, SessionFacts, ToolCall, Usage, BUILTIN_AGENT_TYPES, FAILURE_CLASSES, UNKNOWN,
};

/// `tool_error` from [`FAILURE_CLASSES`] — the asset itself failed. Rate-bearing.
const FAILURE_TOOL_ERROR: &str = FAILURE_CLASSES[0];

/// `user_denied` from [`FAILURE_CLASSES`] — the operator refused or interrupted the call. Never
/// rate-bearing: it is a fact about the person, not about the asset.
const FAILURE_USER_DENIED: &str = FAILURE_CLASSES[2];

/// Per-read scratch (`_ReadState`, `claude_code.py:63-74`), discarded when the read returns.
#[derive(Debug, Default)]
pub(super) struct ReadState {
    /// `tool_use_id` -> index of the still-unpaired call in `SessionFacts::tool_calls`. The Python
    /// holds the `ToolCall` object itself; an index is the same aliasing without the alias.
    pub open: BTreeMap<String, usize>,
    pub seen_message_ids: BTreeSet<String>,
    pub seen_deferred: bool,
    /// Index of the first `initial` event in `SessionFacts::loaded_events`, which later
    /// `nested_memory` lines append their basenames to.
    pub initial_event: Option<usize>,
    pub rules_files: Vec<String>,
    pub synthetic: u64,
    /// Set when an assistant line's `attributionAgent` matched the child sidecar's `agentType`; the
    /// reader copies it into the child ref's `child_meta`.
    pub corroborated: bool,
    pub pending_skill: Option<PendingSkill>,
}

impl ReadState {
    /// Fresh scratch for one read.
    pub(super) fn new() -> Self {
        ReadState::default()
    }
}

/// Fold one projected line into `facts` (`_apply`, `claude_code.py:348-365`).
///
/// A line whose `timestamp` did not parse is stamped with the last timestamp seen, or `0` when the
/// read has not seen one yet: durations must stay differences of harness stamps, so the collector's
/// own clock is never substituted. Only a stamp that actually parsed moves
/// `first_ts_ms`/`last_ts_ms`.
pub(super) fn apply(facts: &mut SessionFacts, state: &mut ReadState, projected: Projected) {
    for (bucket, value) in &projected.names {
        facts.note_forbid(bucket, Some(value));
    }
    let ts_ms = match projected.timestamp.as_deref().and_then(parse_ts_ms) {
        Some(parsed) => {
            facts.first_ts_ms = Some(facts.first_ts_ms.map_or(parsed, |first| first.min(parsed)));
            facts.last_ts_ms = Some(facts.last_ts_ms.map_or(parsed, |last| last.max(parsed)));
            parsed
        }
        None => facts.last_ts_ms.unwrap_or(0),
    };
    note_env(facts, &projected);
    match projected.kind {
        LineKind::Summary => facts.compactions += 1,
        LineKind::Attachment => {
            attachments::apply_attachment(facts, state, projected.attachment, ts_ms);
        }
        LineKind::Assistant => apply_assistant(facts, state, projected, ts_ms),
        LineKind::User => apply_user(facts, state, projected, ts_ms),
    }
}

/// Record the environment strings a line declares (`_note_env`, `claude_code.py:368-382`).
///
/// The first line that names a version, entrypoint or effort wins; the permission mode is
/// re-derived on every line that declares one, because a session can enter and leave plan mode.
fn note_env(facts: &mut SessionFacts, projected: &Projected) {
    set_if_unknown(&mut facts.harness_version, projected.version.as_deref());
    set_if_unknown(&mut facts.entrypoint, projected.entrypoint.as_deref());
    set_if_unknown(&mut facts.effort, projected.effort.as_deref());
    let Some(mode) = projected.permission_mode.as_deref() else {
        return;
    };
    *facts.mode_counts.entry(mode.to_string()).or_insert(0) += 1;
    facts.permission_mode = most_frequent_mode(&facts.mode_counts);
}

/// Overwrite `field` with `value` only while it is still [`UNKNOWN`].
fn set_if_unknown(field: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        if field == UNKNOWN {
            *field = value.to_string();
        }
    }
}

/// The most frequently declared permission mode, ties broken by the **alphabetically smallest**
/// name (`claude_code.py:381`).
///
/// `max(sorted(counts.items()), key=lambda kv: kv[1])` returns the first maximal element of a
/// key-sorted sequence, so the Python's docstring ("ties keep the earlier one") describes something
/// the code does not do. Iterating a `BTreeMap` — which is key-sorted — and replacing only on a
/// strictly greater count reproduces the code, which is what the goldens were generated from.
fn most_frequent_mode(counts: &BTreeMap<String, u64>) -> String {
    let mut best: Option<(&String, u64)> = None;
    for (mode, count) in counts {
        if best.is_none_or(|(_, best_count)| *count > best_count) {
            best = Some((mode, *count));
        }
    }
    best.map_or_else(|| UNKNOWN.to_string(), |(mode, _)| mode.clone())
}

/// Apply an `assistant` line (`_apply_assistant`, `claude_code.py:385-408`).
fn apply_assistant(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    projected: Projected,
    ts_ms: i64,
) {
    if let Some(server) = &projected.mcp_attribution {
        *facts
            .mcp_attribution_counts
            .entry(server.clone())
            .or_insert(0) += 1;
        facts.note_forbid("loaded_set_names", Some(server));
    }
    let attribution_matches = projected.attribution_matches;
    let message = projected.message.unwrap_or_default();
    if let Some(reason) = message.stop_reason {
        facts.last_stop_reason = Some(reason);
    }
    let message_id = message.id;
    // `claude_code.py:393` guards this block with `if mid:`, which an empty id fails. The id still
    // reaches `ToolCall.message_id` below, because `_open_call` (`:418`) copies it unconditionally.
    if let Some(id) = message_id.as_deref().filter(|id| !id.is_empty()) {
        facts.note_forbid("message_ids", Some(id));
        // One API response is split over several lines; the first line for an id is the one kept.
        if state.seen_message_ids.insert(id.to_string()) {
            let model = message.model.unwrap_or_else(|| UNKNOWN.to_string());
            *facts.models.entry(model.clone()).or_insert(0) += 1;
            if let Some(usage) = message.usage {
                facts.usages.insert(
                    id.to_string(),
                    to_usage(id.to_string(), model, ts_ms, &usage),
                );
            }
        }
    }
    for block in message.blocks {
        open_call(facts, state, block, message_id.as_deref(), ts_ms);
    }
    if attribution_matches {
        state.corroborated = true;
    }
}

/// Build a [`Usage`] from a projected `usage` object (`_usage`, `claude_code.py:411-414`).
///
/// An unreported `input_tokens`/`output_tokens` counts as zero (they are always reported in
/// practice); the cache and thinking fields stay `Option`, because "not reported" and "reported as
/// zero" are different facts. `cached_input` and `reasoning` are Codex-only and stay `None`.
fn to_usage(message_id: String, model: String, ts_ms: i64, usage: &ProjectedUsage) -> Usage {
    Usage {
        message_id,
        model,
        ts_ms,
        input_tokens: usage.input_tokens.unwrap_or(0),
        output_tokens: usage.output_tokens.unwrap_or(0),
        cache_creation: usage.cache_creation_input_tokens,
        cache_read: usage.cache_read_input_tokens,
        cached_input: None,
        thinking: usage.thinking_tokens,
        reasoning: None,
    }
}

/// Open a tool call and remember it until its result arrives (`_open_call`,
/// `claude_code.py:417-433`).
///
/// The Python's caller-side guard (`b["type"] == "tool_use" and b["id"]`) lives here as the two
/// early returns: a block that is not a `tool_use`, or one with no id, opens nothing.
fn open_call(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    block: ProjectedBlock,
    message_id: Option<&str>,
    ts_ms: i64,
) {
    let ProjectedBlock::ToolUse {
        id,
        name,
        input_fingerprint,
        skill,
        agent_type,
    } = block
    else {
        return;
    };
    let Some(id) = id else {
        return;
    };
    let mut call = ToolCall {
        tool_use_id: id,
        name: name.unwrap_or_else(|| UNKNOWN.to_string()),
        ts_ms,
        message_id: message_id.map(str::to_string),
        input_fingerprint,
        ..Default::default()
    };
    note_call_identity(facts, &mut call, skill, agent_type);
    facts.note_forbid("tool_use_ids", Some(&call.tool_use_id));
    state
        .open
        .insert(call.tool_use_id.clone(), facts.tool_calls.len());
    facts.tool_calls.push(call);
}

/// Attach the local-only name a call carries and forbid it (`claude_code.py:420-430`).
///
/// A built-in agent type is deliberately *not* forbidden: as a substring it would collide with
/// legitimate closed-enum values on the wire (`agent`, `plan`, `code_edit`).
fn note_call_identity(
    facts: &mut SessionFacts,
    call: &mut ToolCall,
    skill: Option<String>,
    agent_type: Option<String>,
) {
    if call.name.starts_with("mcp__") {
        call.server = mcp_server(&call.name).map(str::to_string);
        facts.note_forbid("loaded_set_names", Some(&call.name));
        facts.note_forbid("loaded_set_names", call.server.as_deref());
    } else if call.name == "Skill" {
        call.skill = skill;
        facts.note_forbid("loaded_set_names", call.skill.as_deref());
    } else if call.name == "Agent" {
        call.agent_type = agent_type;
        let named = call
            .agent_type
            .as_deref()
            .filter(|agent| !BUILTIN_AGENT_TYPES.contains(agent));
        facts.note_forbid("loaded_set_names", named);
    }
}

/// Apply a `user` line (`_apply_user`, `claude_code.py:436-459`).
///
/// A turn is counted only for a line that carries prose, is not a meta line, is not made up
/// entirely of tool results, and was not injected by the harness — those four conditions are what
/// separates a person's turn from the machinery around it.
fn apply_user(facts: &mut SessionFacts, state: &mut ReadState, projected: Projected, ts_ms: i64) {
    let tool_use_result = projected.tool_use_result;
    let message = projected.message.unwrap_or_default();
    let mut results = 0usize;
    let mut has_text = message.content_is_str;
    for block in &message.blocks {
        match block {
            ProjectedBlock::ToolResult { .. } => {
                results += 1;
                pair_result(facts, state, block, &tool_use_result, ts_ms);
            }
            ProjectedBlock::Text => has_text = true,
            _ => {}
        }
    }
    facts.note_forbid("agent_ids", tool_use_result.agent_id.as_deref());
    let result_only = !message.blocks.is_empty() && results == message.blocks.len();
    if has_text && !projected.is_meta && !result_only && !message.injected {
        facts.user_turns += 1;
    }
    if projected.is_meta {
        skills::apply_meta(facts, state, message.command, message.meta_text, ts_ms);
    }
}

/// Pair a `tool_result` with the call it answers (`_pair_result`, `claude_code.py:462-475`).
///
/// A result whose id matches no open call is dropped in silence: there is no field on any type for
/// an outcome with nothing to attach it to, and inventing one would mean inventing a call.
fn pair_result(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    block: &ProjectedBlock,
    tool_use_result: &ProjectedToolUseResult,
    ts_ms: i64,
) {
    let ProjectedBlock::ToolResult {
        tool_use_id,
        is_error,
        denial,
        async_ack,
    } = block
    else {
        return;
    };
    let Some(index) = tool_use_id.as_ref().and_then(|id| state.open.remove(id)) else {
        return;
    };
    let call = &mut facts.tool_calls[index];
    call.result_ts_ms = Some(ts_ms);
    call.is_error = Some(*is_error);
    call.interrupted = tool_use_result.interrupted;
    call.is_async = tool_use_result.is_async || *async_ack;
    if call.name == "Agent" {
        call.child_key.clone_from(&tool_use_result.agent_id);
    }
    if *is_error {
        let class = if call.interrupted || *denial {
            FAILURE_USER_DENIED
        } else {
            FAILURE_TOOL_ERROR
        };
        call.failure_class = Some(class.to_string());
    }
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
