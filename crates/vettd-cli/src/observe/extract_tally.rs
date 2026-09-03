//! Token and tool-call arithmetic for [`super`] — the pure tallies `extract` sums over a run.
//!
//! Split out of `extract.rs` for the 400-line file budget (CONTRIBUTING.md "File size limits");
//! every item here is `extract.py`'s and is re-exported under `observe::extract::*`, so the module
//! boundary is a file boundary and nothing else.
//!
//! Nothing here reads a name or a piece of content: the inputs are counts, ids and fingerprints the
//! source already projected. [`tool_class_shares`] is the one output that stays local — it is
//! `taskcat::categorize`'s input, and only the category egresses.

use std::collections::BTreeMap;

use super::super::taskcat;
use super::super::types::{SessionFacts, TokenTotals, ToolCall, Usage, RATE_BEARING_FAILURES};
use super::{walk, FAILURE_USER_DENIED, REPEAT_THRESHOLD, TOOL_CLASSES};

/// Tools that change files (`extract.py:44`).
pub(crate) const EDIT_TOOLS: [&str; 5] =
    ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"];
/// Tools that only look (`extract.py:45`).
pub(crate) const READ_TOOLS: [&str; 6] = ["Read", "Glob", "Grep", "LS", "WebFetch", "WebSearch"];
/// Tools that run a command (`extract.py:46`).
pub(crate) const SHELL_TOOLS: [&str; 3] = ["Bash", "shell", "exec"];

/// `"harness_usage"` when any response reported usage, else `"none"` (`extract.py:104`).
pub(super) fn tokens_basis(usages: &BTreeMap<&str, &Usage>) -> &'static str {
    if usages.is_empty() {
        "none"
    } else {
        "harness_usage"
    }
}

pub(super) fn count(calls: &[&ToolCall], pred: fn(&ToolCall) -> bool) -> u64 {
    calls.iter().filter(|c| pred(c)).count() as u64
}

pub(super) fn sum_over(tree: &[&SessionFacts], pick: fn(&SessionFacts) -> u64) -> u64 {
    tree.iter().fold(0u64, |acc, f| acc.saturating_add(pick(f)))
}

/// A failure that moves an asset's observed non-success rate (`sources/base.py:29`).
pub(super) fn is_rate_bearing(call: &ToolCall) -> bool {
    call.failure_class
        .as_deref()
        .is_some_and(|class| RATE_BEARING_FAILURES.contains(&class))
}

pub(super) fn is_user_denial(call: &ToolCall) -> bool {
    call.failure_class.as_deref() == Some(FAILURE_USER_DENIED)
}

/// One [`Usage`] per provider message id across the tree (`extract.py:127-137`).
///
/// A streamed response is written as several lines whose usage grows as output streams, so the
/// entry with the largest `output_tokens` wins; on a tie the first occurrence (parent first, by
/// [`walk`] order) is kept. This is the tree-wide rule and is deliberately *not* the per-file rule:
/// within one file the reader already kept the first line for a message id.
pub(crate) fn dedupe_usages<'a>(tree: &[&'a SessionFacts]) -> BTreeMap<&'a str, &'a Usage> {
    let mut seen: BTreeMap<&str, &Usage> = BTreeMap::new();
    for facts in tree {
        for (mid, usage) in &facts.usages {
            let better = seen
                .get(mid.as_str())
                .is_none_or(|current| usage.output_tokens > current.output_tokens);
            if better {
                seen.insert(mid.as_str(), usage);
            }
        }
    }
    seen
}

// -- tokens --------------------------------------------------------------------------------------

/// Envelope-shaped token totals (`extract.py:184-193`).
///
/// Nullable buckets stay `None` unless at least one usage reported a value for them, so a provider
/// without that bucket is "absent", not zero — a distinction the cloud needs, because averaging a
/// cache-read rate over providers that have no cache would be meaningless.
pub(crate) fn sum_tokens(usages: &BTreeMap<&str, &Usage>) -> TokenTotals {
    let mut out = TokenTotals::zeroed_non_null();
    for usage in usages.values() {
        add(&mut out.input, Some(usage.input_tokens));
        add(&mut out.output, Some(usage.output_tokens));
        add(&mut out.cache_creation, usage.cache_creation);
        add(&mut out.cache_read, usage.cache_read);
        add(&mut out.cached_input, usage.cached_input);
        add(&mut out.thinking, usage.thinking);
        add(&mut out.reasoning, usage.reasoning);
    }
    out
}

/// `out[key] = (out[key] or 0) + value` for a reported value; a `None` value leaves the bucket
/// exactly as it was, absent or not.
fn add(slot: &mut Option<i64>, value: Option<i64>) {
    if let Some(value) = value {
        *slot = Some(slot.unwrap_or(0).saturating_add(value));
    }
}

/// Envelope-shaped token totals per allowlisted model id (`extract.py:196-201`), because sub-agents
/// may run on another model than the run's dominant one.
pub(crate) fn sum_tokens_by_model(
    usages: &BTreeMap<&str, &Usage>,
) -> BTreeMap<String, TokenTotals> {
    let mut by_model: BTreeMap<&str, BTreeMap<&str, &Usage>> = BTreeMap::new();
    for (mid, usage) in usages {
        by_model
            .entry(taskcat::allowlist_model(Some(&usage.model)))
            .or_default()
            .insert(mid, usage);
    }
    by_model
        .into_iter()
        .map(|(model, group)| (model.to_string(), sum_tokens(&group)))
        .collect()
}

/// input + output + cache_creation + cache_read (`extract.py:204-206`).
///
/// `cached_input`, `thinking` and `reasoning` are subsets of input/output for the providers that
/// report them and are excluded so nothing is counted twice.
pub(crate) fn total_tokens(tokens: &TokenTotals) -> i64 {
    [
        tokens.input,
        tokens.output,
        tokens.cache_creation,
        tokens.cache_read,
    ]
    .iter()
    .fold(0i64, |acc, bucket| acc.saturating_add(bucket.unwrap_or(0)))
}

/// Exact total of a child run from its own transcript tree, or `None` when the child carries no
/// usage record at all (`extract.py:209-215`). No evidence is not zero.
pub(crate) fn child_tokens_total(child: &SessionFacts) -> Option<i64> {
    let tree = walk(child);
    let usages = dedupe_usages(&tree);
    if usages.is_empty() {
        return None;
    }
    Some(total_tokens(&sum_tokens(&usages)))
}

// -- tool calls ----------------------------------------------------------------------------------

/// Number of calls belonging to a `(name, input_fingerprint)` group of size >= [`REPEAT_THRESHOLD`]
/// (`extract.py:221-224`).
///
/// The **members** are counted, not the groups: three identical calls contribute 3 and four
/// contribute 4, which is what makes the number comparable to `tool_calls`.
pub(crate) fn repeated_tool_calls(calls: &[&ToolCall]) -> u64 {
    let mut groups: BTreeMap<(&str, &str), u64> = BTreeMap::new();
    for call in calls {
        *groups
            .entry((call.name.as_str(), call.input_fingerprint.as_str()))
            .or_insert(0) += 1;
    }
    groups.values().filter(|n| **n >= REPEAT_THRESHOLD).sum()
}

/// Published tool-mix classification (`extract.py:227-234`, D2).
///
/// MCP is decided FIRST: a Codex MCP tool is named `<server>__<tool>` without the `mcp__` prefix but
/// has `server` set, and an `mcp__` tool whose suffix happens to read like an edit is still MCP.
/// Matching is case-sensitive: `read` is not the `Read` tool.
pub(crate) fn tool_class(name: &str, server: Option<&str>) -> &'static str {
    if server.is_some_and(|s| !s.is_empty()) || name.starts_with("mcp__") {
        return "mcp";
    }
    if EDIT_TOOLS.contains(&name) {
        "edit"
    } else if READ_TOOLS.contains(&name) {
        "read"
    } else if SHELL_TOOLS.contains(&name) {
        "shell"
    } else {
        "other"
    }
}

/// count/total per class, every class present (`extract.py:237-242`).
///
/// All zeros when there are no calls, which [`taskcat::categorize`] maps to `unspecified`. The
/// result is **local only**: only the category it produces egresses.
pub(crate) fn tool_class_shares(calls: &[&ToolCall]) -> BTreeMap<String, f64> {
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for call in calls {
        *counts
            .entry(tool_class(&call.name, call.server.as_deref()))
            .or_insert(0) += 1;
    }
    let total = calls.len() as f64;
    TOOL_CLASSES
        .iter()
        .map(|class| {
            let share = match total {
                0.0 => 0.0,
                total => counts.get(class).copied().unwrap_or(0) as f64 / total,
            };
            ((*class).to_string(), share)
        })
        .collect()
}

/// Harness-clock duration, or `None` for an async spawn or an unpaired call (`extract.py:245-246`).
///
/// Neither has a duration the harness clock can vouch for: an async spawn's "result" is only the
/// ack, and an unpaired call has no result at all.
pub(super) fn latency(call: &ToolCall) -> Option<i64> {
    if call.is_async || !call.paired() {
        None
    } else {
        call.latency_ms()
    }
}
