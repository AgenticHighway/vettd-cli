//! [`SessionFacts`] (with its child tree) -> [`RunFacts`].
//!
//! Port of `spikes/828-passive-observer/prototype/extract.py`, function for function.
//!
//! Harness-neutral derivation of per-run facts. Everything here is arithmetic over the typed facts
//! a source produced; no session content exists at this layer to leak. The one field that could be
//! mistaken for content — [`RunFacts::tool_class_shares`] — is local only: it is the input to
//! [`taskcat::categorize`] and only the resulting closed category egresses.
//!
//! Scope of "the run" (the contract leaves this implicit, so `extract.py:6-18` states it and so
//! does this):
//!
//! - Counts, token totals, invocations, in-band assets and forbids merge over the WHOLE tree (main
//!   transcript plus every linked child transcript). Usages are deduplicated by provider message id
//!   across the tree so a response logged in two places counts once.
//! - [`run_outcome`], `turns`, `loaded_events` and `truncated` describe the MAIN transcript only: a
//!   child's "user" lines are the parent's task prompt, not a person's turns, and the loaded set the
//!   segments are cut from is the parent's.
//! - A parent `Agent` call is linked to a direct child by `child_key` (`toolUseResult.agentId`) or
//!   by the child's `child_meta["toolUseId"]` (D4). Its [`InvocationObs`] carries the child's exact
//!   token total and the child's outcome; a linked child's own tokens are ALSO in the run totals.
//! - "Total tokens" of a child = input + output + cache_creation + cache_read. `cached_input`,
//!   `thinking` and `reasoning` are subsets of input/output for the providers that report them and
//!   are excluded so nothing is counted twice.
//!
//! The token and tool-call arithmetic lives in the sibling `extract_tally.rs` and is re-exported
//! here, so callers still say `observe::extract::sum_tokens`: the split is the 400-line file budget
//! (CONTRIBUTING.md "File size limits") and carries no meaning of its own.
//!
//! **Named divergence from the Python.** Python integers are unbounded; these sums are `i64`/`u64`
//! and saturate. Every input is a token count, a byte length or a line count from one machine's
//! logs, so the saturation point is unreachable in practice; saturating rather than wrapping is
//! chosen so that an absurd transcript degrades a number instead of panicking a read.

#[path = "extract_tally.rs"]
mod tally;

use std::collections::{BTreeMap, BTreeSet};

use tally::{count, is_rate_bearing, is_user_denial, latency, sum_over, tokens_basis};
// Re-exported so this module's surface is `observe::extract::*`, exactly as `extract.py` is one
// module: the split into two files is the 400-line file budget and nothing else. `unused_imports` is
// allowed because the non-test build of this phase has no caller for the tallies `attribute` and
// `envelope` will consume in Phase 4; the tests below use every one of them today.
#[allow(unused_imports)]
pub(crate) use tally::{
    child_tokens_total, dedupe_usages, repeated_tool_calls, sum_tokens, sum_tokens_by_model,
    tool_class, tool_class_shares, total_tokens, EDIT_TOOLS, READ_TOOLS, SHELL_TOOLS,
};

use super::taskcat;
use super::types::{
    utc_day, InvocationObs, RunFacts, SessionFacts, ToolCall, ASSET_AGENT, ASSET_MCP_SERVER,
    ASSET_SKILL, FAILURE_CLASSES,
};

/// The main transcript was still being written when it was read (`extract.py:38`).
pub(crate) const OUTCOME_TRUNCATED: &str = "truncated";
/// The session compacted its context and did not then end its turn.
pub(crate) const OUTCOME_COMPACTED: &str = "compacted";
/// A tool call was cut off, or never resolved at all.
pub(crate) const OUTCOME_INTERRUPTED: &str = "interrupted";
/// The last assistant message stopped with `end_turn`.
pub(crate) const OUTCOME_COMPLETED: &str = "completed";
/// None of the above — the transcript does not say how the run ended.
pub(crate) const OUTCOME_UNKNOWN: &str = "unknown";

/// `sources/base.py:22`. Indexed out of [`FAILURE_CLASSES`] so the two cannot drift apart.
pub(super) const FAILURE_USER_DENIED: &str = FAILURE_CLASSES[2];
/// `sources/base.py:23`.
const FAILURE_INTERRUPTED: &str = FAILURE_CLASSES[3];
/// `sources/base.py:24`.
const FAILURE_UNKNOWN: &str = FAILURE_CLASSES[4];

/// The published tool-mix classes (`extract.py:43`, D2). Every one is present in
/// [`tool_class_shares`] even at zero, so the shape taskcat sees is stable.
pub(crate) const TOOL_CLASSES: [&str; 5] = ["edit", "read", "shell", "mcp", "other"];

/// A `(name, input_fingerprint)` group of this size or larger is a repeat (`extract.py:47`).
pub(crate) const REPEAT_THRESHOLD: u64 = 3;

/// Harness permission-mode spellings mapped to the gate's enum (`extract.py:49-56`).
const PERMISSION_MODES: [(&str, &str); 6] = [
    ("acceptEdits", "accept_edits"),
    ("bypassPermissions", "bypass"),
    ("dontAsk", "dont_ask"),
    ("plan", "plan"),
    ("default", "default"),
    ("auto", "auto"),
];

/// The gate's `enums.permission_mode` (`extract.py:255`). A source that pre-maps (the Codex
/// approval policy) passes through unchanged.
pub(crate) const PERMISSION_ENUM: [&str; 7] = [
    "default",
    "plan",
    "accept_edits",
    "bypass",
    "auto",
    "dont_ask",
    "unknown",
];

/// The gate's `enums.effort` (`extract.py:57`), minus `unknown` which is the fallback.
const EFFORTS: [&str; 5] = ["minimal", "low", "medium", "high", "xhigh"];

/// Derive [`RunFacts`] for the run rooted at `facts`.
///
/// `now_ms` is only the fallback for a session that carries no harness timestamp at all
/// (`extract.py:71-73`).
pub(crate) fn extract(facts: &SessionFacts, now_ms: i64) -> RunFacts {
    let tree = walk(facts);
    let calls: Vec<&ToolCall> = tree.iter().flat_map(|f| f.tool_calls.iter()).collect();
    let usages = dedupe_usages(&tree);
    let (first, last) = span(&tree, now_ms);
    RunFacts {
        session_key: facts.ref_.session_key.clone(),
        harness: facts.ref_.harness.clone(),
        harness_version: facts.harness_version.clone(),
        entrypoint_class: entrypoint_class(&facts.entrypoint).to_string(),
        effort: effort_class(&facts.effort).to_string(),
        permission_mode: permission_mode(&facts.permission_mode).to_string(),
        model: taskcat::allowlist_model(Some(dominant_model(&tree))).to_string(),
        observed_day: utc_day(first),
        first_ts_ms: first,
        last_ts_ms: last,
        run_outcome: run_outcome(facts).to_string(),
        turns: facts.user_turns,
        tool_calls: calls.len() as u64,
        tool_failures: count(&calls, is_rate_bearing),
        user_denials: count(&calls, is_user_denial),
        subagent_runs: tree.len() as u64 - 1,
        compactions: sum_over(&tree, |f| f.compactions),
        unpaired_tool_uses: count(&calls, |c| !c.paired()),
        repeated_tool_calls: repeated_tool_calls(&calls),
        tokens: sum_tokens(&usages),
        tokens_basis: tokens_basis(&usages).to_string(),
        tokens_by_model: sum_tokens_by_model(&usages),
        mcp_corroborations: merge_mcp_corroborations(&tree),
        tool_class_shares: tool_class_shares(&calls),
        invocations: invocations(facts),
        loaded_events: facts.loaded_events.clone(),
        in_band_assets: tree.iter().flat_map(|f| f.in_band_assets.clone()).collect(),
        lines_seen: sum_over(&tree, |f| f.lines_seen),
        lines_unknown_type: sum_over(&tree, |f| f.lines_unknown_type),
        bytes_read: sum_over(&tree, |f| f.bytes_read),
        parse_errors: sum_over(&tree, |f| f.parse_errors),
        truncated: facts.truncated,
        forbids: merge_forbids(&tree),
    }
}

// -- tree helpers --------------------------------------------------------------------------------

/// Depth-first, parent before children, in the order the source linked them (`extract.py:120-124`).
///
/// The order is part of the contract: [`dedupe_usages`] keeps the first entry on a tie, so
/// "parent first" is what decides which of two equal usages survives.
pub(crate) fn walk(facts: &SessionFacts) -> Vec<&SessionFacts> {
    let mut out = vec![facts];
    for child in &facts.children {
        out.extend(walk(child));
    }
    out
}

/// Harness-native MCP attribution markers, summed over the tree (`extract.py:140-145`). The keys
/// are server names and are **local only**.
pub(crate) fn merge_mcp_corroborations(tree: &[&SessionFacts]) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for facts in tree {
        for (server, n) in &facts.mcp_attribution_counts {
            let slot = out.entry(server.clone()).or_insert(0);
            *slot = slot.saturating_add(*n);
        }
    }
    out
}

/// Union of every bucket of local-only names in the tree (`extract.py:148-153`), so the gate
/// checker sees a child's ids as well as the parent's.
pub(crate) fn merge_forbids(tree: &[&SessionFacts]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for facts in tree {
        for (bucket, values) in &facts.forbids {
            out.entry(bucket.clone())
                .or_default()
                .extend(values.iter().cloned());
        }
    }
    out
}

/// `(first, last)` over the tree, falling back to `now_ms` (`extract.py:156-161`).
///
/// `last` is clamped up to `first` so a tree whose only stamps are out of order can never report a
/// negative duration.
fn span(tree: &[&SessionFacts], now_ms: i64) -> (i64, i64) {
    let first = tree.iter().filter_map(|f| f.first_ts_ms).min();
    let last = tree.iter().filter_map(|f| f.last_ts_ms).max();
    let first = first.unwrap_or(now_ms);
    (first, last.unwrap_or(first).max(first))
}

/// Most frequent model by response count in the MAIN transcript (`extract.py:164-178`).
///
/// The whole tree is used only when the main transcript carried no model. Sub-agents may run on a
/// different model, which is why the envelope also carries `tokens_by_model`. Ties break on the
/// smaller name so the result is deterministic. `"unknown"` when no response carried a model, which
/// [`taskcat::allowlist_model`] then reports as `"other"`.
pub(crate) fn dominant_model<'a>(tree: &[&'a SessionFacts]) -> &'a str {
    let main_has_models = tree.first().is_some_and(|f| !f.models.is_empty());
    let sources = if main_has_models { &tree[..1] } else { tree };
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for facts in sources {
        for (model, n) in &facts.models {
            let slot = counts.entry(model.as_str()).or_insert(0);
            *slot = slot.saturating_add(*n);
        }
    }
    // BTreeMap iteration is by name, so `max_by_key` on the count alone would keep the LAST maximum;
    // the Python sorts by `(-count, name)` and takes the first, i.e. the smallest name among ties.
    counts
        .into_iter()
        .fold(None::<(&str, u64)>, |best, (model, n)| match best {
            Some((_, top)) if top >= n => best,
            _ => Some((model, n)),
        })
        .map(|(model, _)| model)
        .unwrap_or("unknown")
}

// -- run shape -----------------------------------------------------------------------------------

/// Entrypoint string -> closed enum, by substring, case-insensitively (`extract.py:252-262`).
///
/// Precedence is remote > ide > sdk > cli > unknown, so `"sdk-cli"` is `sdk` and `"remote-cli"` is
/// `remote`. The Python accepts `Optional[str]` and folds `None` into `""`; both are `unknown`, so
/// this takes `&str` and the empty string stands in for the missing value.
pub(crate) fn entrypoint_class(raw: &str) -> &'static str {
    let raw = raw.to_lowercase();
    if raw.contains("remote") {
        "remote"
    } else if raw.contains("vscode") || raw.contains("jetbrains") || raw.contains("ide") {
        "ide"
    } else if raw.contains("sdk") {
        "sdk"
    } else if raw.contains("cli") {
        "cli"
    } else {
        "unknown"
    }
}

/// Permission mode -> closed enum (`extract.py:268-273`).
///
/// Claude Code's camelCase values are mapped; a value already in [`PERMISSION_ENUM`] (a source that
/// pre-maps, like the Codex approval policy) passes through unchanged. Matching is exact:
/// `acceptedits` is `unknown`, not `accept_edits`.
pub(crate) fn permission_mode(raw: &str) -> &'static str {
    if let Some(known) = PERMISSION_ENUM.iter().find(|mode| **mode == raw) {
        return known;
    }
    PERMISSION_MODES
        .iter()
        .find(|(harness, _)| *harness == raw)
        .map(|(_, mapped)| *mapped)
        .unwrap_or("unknown")
}

/// Effort -> closed enum (`extract.py:276-279`).
///
/// Anything the gate does not list — including harness values it has never heard of — is `unknown`.
/// The contract is silent on effort; this is what keeps the payload gate-clean regardless.
pub(crate) fn effort_class(raw: &str) -> &'static str {
    EFFORTS
        .iter()
        .find(|effort| **effort == raw)
        .copied()
        .unwrap_or("unknown")
}

/// Decision table, first match wins (`extract.py:286-297`):
/// truncated > compacted > interrupted > completed > unknown.
///
/// Only the transcript `facts` itself is consulted — children have their own outcome, which is what
/// [`child_failure_class`] reads.
pub(crate) fn run_outcome(facts: &SessionFacts) -> &'static str {
    if facts.truncated {
        return OUTCOME_TRUNCATED;
    }
    if facts.compactions > 0 && facts.last_stop_reason.as_deref() != Some("end_turn") {
        return OUTCOME_COMPACTED;
    }
    if interrupted_at_end(&facts.tool_calls) {
        return OUTCOME_INTERRUPTED;
    }
    if facts.last_stop_reason.as_deref() == Some("end_turn") {
        return OUTCOME_COMPLETED;
    }
    OUTCOME_UNKNOWN
}

/// Any call without a result, or the last call in transcript order marked interrupted
/// (`extract.py:300-304`).
///
/// An interrupt that is not the last call does not make a finished run interrupted: the run went on
/// afterwards.
fn interrupted_at_end(calls: &[ToolCall]) -> bool {
    calls.iter().any(|c| !c.paired()) || calls.last().is_some_and(|c| c.interrupted)
}

// -- invocations ---------------------------------------------------------------------------------

/// Explicit asset invocations over the tree: parent's calls first, then each child's
/// (`extract.py:310-323`).
///
/// The precedence within one call is skill > server > agent_type, and a call that names none of the
/// three is not an invocation of anything. Names are **local only**; `attribute` hashes them.
///
/// The three tests are on a *non-empty* name because the Python's `if call.skill:` is a truthiness
/// test, not a presence test. The difference is not cosmetic: with a presence test, a call carrying
/// `skill = Some("")` alongside a real `server` is attributed to the wrong asset *type*, and the
/// empty name then seeds a phantom `AssetKey` whose `name_hash` reaches `assets[]` — neither
/// `seed_invocations` here nor `attribute.py` guards it, because upstream never emits one.
pub(crate) fn invocations(facts: &SessionFacts) -> Vec<InvocationObs> {
    let mut out = Vec::new();
    for call in &facts.tool_calls {
        if let Some(skill) = call.skill.as_deref().filter(|name| !name.is_empty()) {
            out.push(simple_invocation(ASSET_SKILL, skill, call));
        } else if let Some(server) = call.server.as_deref().filter(|name| !name.is_empty()) {
            out.push(simple_invocation(ASSET_MCP_SERVER, server, call));
        } else if call
            .agent_type
            .as_deref()
            .is_some_and(|name| !name.is_empty())
        {
            out.push(agent_invocation(call, linked_child(facts, call)));
        }
    }
    for child in &facts.children {
        out.extend(invocations(child));
    }
    out
}

fn simple_invocation(asset_type: &str, name: &str, call: &ToolCall) -> InvocationObs {
    InvocationObs {
        asset_type: asset_type.to_string(),
        name: name.to_string(),
        ts_ms: call.ts_ms,
        latency_ms: latency(call),
        failure_class: call.failure_class.clone(),
        is_async: call.is_async,
        ..Default::default()
    }
}

/// The direct child this `Agent` call spawned, by `child_key` or by the child's own `toolUseId`
/// (`extract.py:326-332`).
///
/// Both keys are tried per child, in child order, so the first child that matches either wins.
fn linked_child<'a>(facts: &'a SessionFacts, call: &ToolCall) -> Option<&'a SessionFacts> {
    facts.children.iter().find(|child| {
        let by_key = call
            .child_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .is_some_and(|key| child.ref_.session_key == key);
        let by_meta = child.ref_.child_meta.get("toolUseId") == Some(&call.tool_use_id);
        by_key || by_meta
    })
}

/// The agent invocation for a spawn (`extract.py:335-348`).
///
/// The parent's result is only a spawn ack (D4): outcome and tokens come from the child when one is
/// linked. The parent's own failure class (a denied or failed spawn) still takes precedence, because
/// then there is no child run to speak of.
fn agent_invocation(call: &ToolCall, child: Option<&SessionFacts>) -> InvocationObs {
    let mut obs = simple_invocation(ASSET_AGENT, call.agent_type.as_deref().unwrap_or(""), call);
    if let Some(child) = child {
        // `failure or _child_failure_class(child)` is a truthiness `or`, so an empty parent class
        // falls through to the child's. Keeping `Some("")` would put a value outside FAILURE_CLASSES
        // on the wire, which the gate rejects.
        obs.failure_class = obs
            .failure_class
            .filter(|class| !class.is_empty())
            .or_else(|| child_failure_class(child).map(str::to_string));
        obs.corroborated = child
            .ref_
            .child_meta
            .get("corroborated")
            .is_some_and(|value| value.to_lowercase() == "true");
        obs.child_tokens_total = child_tokens_total(child);
    }
    obs
}

/// Child outcome -> failure class (`extract.py:351-358`).
///
/// Never a rate-bearing class: a child's own tool errors are counted on the tools it called, not on
/// the agent as a whole, so a sub-agent cannot inflate its own type's non-success rate twice.
fn child_failure_class(child: &SessionFacts) -> Option<&'static str> {
    match run_outcome(child) {
        OUTCOME_INTERRUPTED => Some(FAILURE_INTERRUPTED),
        OUTCOME_TRUNCATED | OUTCOME_UNKNOWN => Some(FAILURE_UNKNOWN),
        _ => None,
    }
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
