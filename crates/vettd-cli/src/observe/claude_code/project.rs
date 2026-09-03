//! Projection of one Claude Code transcript line — the only place raw content is touched.
//!
//! Port of the "projection" section of `spikes/828-passive-observer/prototype/sources/claude_code.py`
//! (lines 168-343). [`project`] takes a parsed line and returns a [`Projected`] whose fields are
//! hashes, byte lengths, booleans, counts and closed-vocabulary names. The caller drops the
//! [`serde_json::Value`] immediately afterwards, so message text, thinking, tool inputs, tool
//! results, `toolUseResult` bodies and attachment bodies exist only for the duration of this call.
//!
//! Nothing here returns a `Value`: once a line is projected, the raw tree is unreachable. Names and
//! harness ids that do survive are local-only and reach the gate as dynamic forbids, never the wire.
//!
//! **File layout.** CONTRIBUTING.md caps a file at 400 lines and this projection does not fit in
//! one, so it is three private submodules declared here rather than in `claude_code/mod.rs`: the
//! data types (`projected.rs`), the content-bearing projections (`content.rs`) and the tool-input
//! fingerprint (`fingerprint.rs`). Everything callers need is re-exported below, so the seam stays
//! exactly `claude_code::project::{project, Projected, …}` as `docs/vettd-observe-port-plan.md`
//! names it.
//!
//! Two deliberate shifts of *where* a check happens, both observationally identical to the Python:
//!
//! * The `isinstance(value, str) and value` guards that the Python spreads over `_note_env`,
//!   `_apply_assistant` and `_pair_result` are applied here instead ([`nonempty_str`]). For those
//!   fields every consumer treats `""`, a non-string and a missing key the same way, so folding
//!   the guard in loses nothing and keeps `apply.rs` free of type tests. `message.id` is the one
//!   exception and uses [`str_value`]: `claude_code.py:221` does *not* apply the emptiness filter
//!   there, and `_open_call` (`:418`) copies the id onto `ToolCall.message_id` without a
//!   truthiness test, so an empty id must survive projection as `Some("")`. The truthiness test
//!   that guards the models/usages/forbids block stays in `apply.rs`, where the Python has it.
//! * Python truthiness (`bool(raw.get("isMeta"))`, `bool(tur.get("interrupted"))`, …) is applied
//!   here rather than at application time ([`truthy`]), which is what lets `is_meta`, `interrupted`,
//!   `is_async` and `is_error` be plain `bool`s instead of carrying a raw value past the privacy
//!   boundary.

#[path = "content.rs"]
mod content;
#[path = "fingerprint.rs"]
mod fingerprint;
#[path = "projected.rs"]
mod projected;

use serde_json::{Map, Value};

// The seam every caller outside this module uses. `ProjectedMessage` is deliberately absent: it is
// only ever reached through `Projected::message`, and re-exporting a name nothing outside names is
// an `unused_imports` warning, which this workspace builds with `-D warnings`.
pub(super) use fingerprint::sha256_json;
pub(super) use projected::{
    Digest, LineKind, Projected, ProjectedAttachment, ProjectedBlock, ProjectedCommand,
    ProjectedToolUseResult, ProjectedUsage,
};

/// Top-level keys copied out of a line (`claude_code.py:57-60`, `TOP_KEYS`).
///
/// This is the allowlist for the line's *scalar* fields: one named [`Projected`] field each, and
/// nothing else at that level survives. (`cwd`, `gitBranch`, `slug`, `mcpMeta`, `message`,
/// `toolUseResult`, `attachment` and the two `attribution*` keys have their own projections.)
/// `top_keys_are_exactly_what_the_projection_reads` is what keeps this list and [`project`] in step.
const TOP_KEYS: [&str; 13] = [
    "type",
    "uuid",
    "parentUuid",
    "timestamp",
    "sessionId",
    "isSidechain",
    "agentId",
    "version",
    "entrypoint",
    "permissionMode",
    "effort",
    "sourceToolAssistantUUID",
    "isMeta",
];

/// Project one parsed line, or `None` when its `type` is outside `CONSUMED_TYPES`.
///
/// `expected_agent` is the `agentType` from a child session's sidecar; a line whose
/// `attributionAgent` matches it corroborates the spawn. `line_len` is the byte length of the raw
/// line. The caller must drop `raw` as soon as this returns — that drop is the privacy boundary.
pub(super) fn project(
    raw: &Value,
    line_len: u64,
    expected_agent: Option<&str>,
) -> Option<Projected> {
    let object = raw.as_object()?;
    let kind = LineKind::from_type(object.get("type"))?;
    let message = object.get("message").and_then(Value::as_object);
    let is_meta = truthy(object.get("isMeta"));
    Some(Projected {
        kind,
        uuid: nonempty_str(object.get("uuid")),
        parent_uuid: nonempty_str(object.get("parentUuid")),
        timestamp: nonempty_str(object.get("timestamp")),
        session_id: nonempty_str(object.get("sessionId")),
        is_sidechain: truthy(object.get("isSidechain")),
        agent_id: nonempty_str(object.get("agentId")),
        version: nonempty_str(object.get("version")),
        entrypoint: nonempty_str(object.get("entrypoint")),
        permission_mode: nonempty_str(object.get("permissionMode")),
        effort: nonempty_str(object.get("effort")),
        source_tool_assistant_uuid: nonempty_str(object.get("sourceToolAssistantUUID")),
        is_meta,
        line_len,
        names: harvest_names(object),
        message: message.map(|message| content::project_message(message, is_meta)),
        tool_use_result: content::project_tool_use_result(object.get("toolUseResult")),
        attachment: object
            .get("attachment")
            .and_then(Value::as_object)
            .map(content::project_attachment),
        attribution_matches: expected_agent.is_some_and(|agent| {
            object.get("attributionAgent").and_then(Value::as_str) == Some(agent)
        }),
        mcp_attribution: nonempty_str(object.get("attributionMcpServer")),
    })
}

/// Local-only names the line carries, tagged with the forbids bucket each belongs to
/// (`_harvest_names`, `claude_code.py:191-200`).
fn harvest_names(object: &Map<String, Value>) -> Vec<(&'static str, String)> {
    const SOURCES: [(&str, &str); 5] = [
        ("slug", "slugs"),
        ("cwd", "cwd_and_branches"),
        ("gitBranch", "cwd_and_branches"),
        ("sessionId", "harness_session_ids"),
        ("agentId", "agent_ids"),
    ];
    let mut out = Vec::new();
    for (key, bucket) in SOURCES {
        if let Some(value) = nonempty_str(object.get(key)) {
            out.push((bucket, value));
        }
    }
    for name in names_in(object.get("mcpMeta"), 0) {
        out.push(("loaded_set_names", name));
    }
    out
}

/// String values under any `name` key of an `mcpMeta` blob — server identity, nothing else
/// (`_names_in`, `claude_code.py:203-213`).
///
/// A `name` whose value is an object is not descended into, exactly as in the Python's `elif`.
/// Object keys are visited in sorted order here and in insertion order there; the result feeds a
/// `BTreeSet` of forbids, so the order is not observable.
fn names_in(node: Option<&Value>, depth: u32) -> Vec<String> {
    let Some(object) = node.and_then(Value::as_object).filter(|_| depth <= 4) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (key, value) in object {
        if key == "name" {
            if let Some(name) = nonempty_str(Some(value)) {
                found.push(name);
            }
        } else if value.is_object() {
            found.extend(names_in(Some(value), depth + 1));
        }
    }
    found
}

/// The server segment of an `mcp__<server>__<tool>` name, or `None` for any other name
/// (`_mcp_server`, `claude_code.py:538-542`).
pub(super) fn mcp_server(name: &str) -> Option<&str> {
    if !name.starts_with("mcp__") {
        return None;
    }
    let parts: Vec<&str> = name.split("__").collect();
    parts
        .get(1)
        .copied()
        .filter(|server| parts.len() >= 3 && !server.is_empty())
}

/// A non-empty string value, or `None` for anything else (`_str_or_none`, `claude_code.py:620-621`,
/// and the `isinstance(value, str) and value` guards it stands in for).
/// A string value kept verbatim, empty string included.
///
/// Only `message.id` needs this: see the module docs for why it is not [`nonempty_str`].
pub(super) fn str_value(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

fn nonempty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The string values of a list, dropping every non-string element (`_str_list`,
/// `claude_code.py:624-625`).
fn str_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// An integer value, or `None` for a float, a bool or anything else (`_int_or_none`,
/// `claude_code.py:616-617`; Python's `isinstance(value, int) and not isinstance(value, bool)`).
///
/// An integer beyond `i64` is `None` here and an integer in Python, whose ints are unbounded. Only
/// token counts reach this, and the gate bounds those far below `i64::MAX`.
fn int_or_none(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64)
}

/// Python truthiness of a JSON value, as `bool(...)` would compute it: absent, `null`, `false`,
/// zero, and every empty string, list and object are false; everything else is true.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_none_or(|value| value != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
    }
}

/// `str(value)` of a truthy JSON scalar, mirroring `str(tur["agentId"])` behind an `if`. A falsy
/// value is `None`, and a container has no meaningful id form so it is `None` too.
fn stringify_if_truthy(value: Option<&Value>) -> Option<String> {
    if !truthy(value) {
        return None;
    }
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(_) => Some("True".to_string()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
