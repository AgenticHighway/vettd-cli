//! Tests for `apply.rs` (see the `gate.rs`/`gate_tests.rs` convention). The `attachment` branch has
//! its own tests in `attachments_tests.rs`.
//!
//! Each test drives the same path the reader will: build a line, project it, apply it. Nothing here
//! constructs a `Projected` by hand, so a change to the projection that breaks the application
//! shows up as a failure rather than as two files that agree only with themselves.

use super::*;
use crate::observe::claude_code::project::project;
use crate::observe::types::{SessionRef, UNKNOWN};
use serde_json::{json, Value};

/// Apply `lines` to fresh facts and return both the facts and the scratch state.
pub(super) fn read(lines: &[Value]) -> (SessionFacts, ReadState) {
    read_as(lines, None)
}

/// Apply `lines` as a child session whose sidecar declared `expected_agent`.
fn read_as(lines: &[Value], expected_agent: Option<&str>) -> (SessionFacts, ReadState) {
    let mut facts = SessionFacts::new(SessionRef::default());
    let mut state = ReadState::new();
    for line in lines {
        let text = serde_json::to_vec(line).expect("the fixture line serialises");
        if let Some(projected) = project(line, text.len() as u64, expected_agent) {
            apply(&mut facts, &mut state, projected);
        } else {
            facts.lines_unknown_type += 1;
        }
        facts.lines_seen += 1;
    }
    (facts, state)
}

/// Fixture timestamps one second apart, so a line fits on one line.
const T: [&str; 8] = [
    "2026-08-15T10:00:00Z",
    "2026-08-15T10:00:01Z",
    "2026-08-15T10:00:02Z",
    "2026-08-15T10:00:03Z",
    "2026-08-15T10:00:04Z",
    "2026-08-15T10:00:05Z",
    "2026-08-15T10:00:06Z",
    "2026-08-15T10:00:07Z",
];

/// A `user` line carrying `content` at `timestamp`.
fn user(timestamp: &str, content: Value) -> Value {
    json!({"type": "user", "timestamp": timestamp, "message": {"content": content}})
}

/// A meta `user` line carrying the string `text`.
fn meta(timestamp: &str, text: &str) -> Value {
    json!({"type": "user", "timestamp": timestamp, "isMeta": true, "message": {"content": text}})
}

/// An `assistant` line whose message holds `blocks`.
fn assistant(timestamp: &str, id: &str, blocks: Value) -> Value {
    json!({"type": "assistant", "timestamp": timestamp,
           "message": {"id": id, "model": "mdl", "content": blocks}})
}

/// A `user` line at `timestamp` holding one `tool_result` for `tool_use_id`.
fn result(timestamp: &str, tool_use_id: &str, block: Value) -> Value {
    let mut block = block;
    block["type"] = json!("tool_result");
    block["tool_use_id"] = json!(tool_use_id);
    json!({"type": "user", "timestamp": timestamp, "message": {"content": [block]}})
}

/// The permission-mode tally is a field of the facts and never a forbids bucket.
///
/// The prototype keeps it in `forbids["_permission_modes"]`, from where it reaches the envelope's
/// dynamic-forbid sidecar. A permission mode is a closed enum value on the wire, so forbidding it
/// there would make every well-formed record fail its own gate check. Nothing may write an
/// `_`-prefixed bucket, and this is the bucket that tempted the prototype into it.
#[test]
fn mode_counts_is_a_field_and_never_a_forbids_bucket() {
    let (facts, _) = read(&[
        json!({"type": "user", "timestamp": T[0], "permissionMode": "plan"}),
        json!({"type": "user", "timestamp": T[1], "permissionMode": "acceptEdits"}),
        json!({"type": "user", "timestamp": T[2], "permissionMode": "acceptEdits"}),
    ]);
    assert_eq!(facts.mode_counts["plan"], 1);
    assert_eq!(facts.mode_counts["acceptEdits"], 2);
    assert_eq!(facts.permission_mode, "acceptEdits");
    assert!(facts.forbids.keys().all(|bucket| !bucket.starts_with('_')));
    for names in facts.forbids.values() {
        assert!(!names.contains("plan") && !names.contains("acceptEdits"));
    }
}

/// A tie between permission modes is broken by the alphabetically smallest name.
///
/// `claude_code.py:381`'s docstring says ties keep the earlier mode, but
/// `max(sorted(counts.items()), key=count)` keeps the first element of a key-sorted sequence. The
/// goldens were generated from the code, not the docstring, so the port reproduces the code — and
/// this test is what stops a well-meaning "fix" toward the docstring.
#[test]
fn a_permission_mode_tie_keeps_the_alphabetically_smallest_not_the_earliest() {
    let (facts, _) = read(&[
        json!({"type": "user", "timestamp": T[0], "permissionMode": "plan"}),
        json!({"type": "user", "timestamp": T[1], "permissionMode": "acceptEdits"}),
    ]);
    assert_eq!(facts.mode_counts["plan"], facts.mode_counts["acceptEdits"]);
    assert_eq!(facts.permission_mode, "acceptEdits");
    let (facts, _) = read(&[
        json!({"type": "user", "timestamp": T[0], "permissionMode": "zeta"}),
        json!({"type": "user", "timestamp": T[1], "permissionMode": "alpha"}),
        json!({"type": "user", "timestamp": T[2], "permissionMode": "zeta"}),
    ]);
    assert_eq!(facts.permission_mode, "zeta");
}

/// The first line that names a version, entrypoint or effort wins, and an absent one stays
/// `unknown`. These describe the run as a whole and are reported as closed strata; letting a later
/// line overwrite them would make the stratum depend on where a read happened to resume.
#[test]
fn the_first_declared_environment_value_wins() {
    let (facts, _) = read(&[
        json!({"type": "user", "timestamp": T[0], "version": "1.0", "entrypoint": "cli"}),
        json!({"type": "user", "timestamp": T[1], "version": "2.0", "entrypoint": "sdk"}),
    ]);
    assert_eq!(facts.harness_version, "1.0");
    assert_eq!(facts.entrypoint, "cli");
    assert_eq!(facts.effort, UNKNOWN);
    assert_eq!(facts.permission_mode, UNKNOWN);
}

/// A line whose timestamp does not parse is stamped with the last timestamp seen, or zero before
/// there is one. Every duration in the model is a difference of harness stamps; substituting the
/// collector's clock would produce latencies that mix two clocks and cannot be compared.
#[test]
fn an_unparsable_timestamp_falls_back_to_the_last_one_seen() {
    let (early, _) = read(&[user("not-a-timestamp", json!("hi"))]);
    assert_eq!((early.first_ts_ms, early.last_ts_ms), (None, None));
    let (facts, _) = read(&[
        assistant(
            T[5],
            "m1",
            json!([{"type": "tool_use", "id": "t1", "name": "Bash"}]),
        ),
        result("", "t1", json!({})),
    ]);
    assert_eq!(facts.first_ts_ms, Some(1_786_788_005_000));
    assert_eq!(facts.last_ts_ms, Some(1_786_788_005_000));
    assert_eq!(facts.tool_calls[0].result_ts_ms, Some(1_786_788_005_000));
    assert_eq!(facts.tool_calls[0].latency_ms(), Some(0));
}

/// One API response split over several lines is counted once, and the first line for its id is the
/// one kept. `claude_code.py:395-403` looks like it keeps the largest `output_tokens` instead, but
/// that branch sits inside `mid not in seen_message_ids` and can never run; the per-file rule is
/// first-wins, and choosing the fullest usage is `extract`'s tree-wide job.
#[test]
fn the_first_line_for_a_message_id_wins_within_one_file() {
    let (facts, _) = read(&[
        json!({"type": "assistant", "timestamp": T[0], "message": {
            "id": "m1", "model": "first", "usage": {"input_tokens": 1, "output_tokens": 4}, "content": []}}),
        json!({"type": "assistant", "timestamp": T[1], "message": {
            "id": "m1", "model": "second", "usage": {"input_tokens": 10, "output_tokens": 99}, "content": []}}),
    ]);
    assert_eq!(facts.usages.len(), 1);
    let usage = &facts.usages["m1"];
    assert_eq!((usage.input_tokens, usage.output_tokens), (1, 4));
    assert_eq!(usage.model, "first");
    assert_eq!(
        facts.models,
        [("first".to_string(), 1)].into_iter().collect()
    );
}

/// A tool_use pairs with the tool_result that names it, and a result naming no open call is dropped
/// in silence. There is no field on any type for an outcome with no call to attach it to; inventing
/// one would mean inventing a call that the session never made.
#[test]
fn a_result_with_no_open_call_is_dropped() {
    let (facts, state) = read(&[
        assistant(
            T[0],
            "m1",
            json!([{"type": "tool_use", "id": "t1", "name": "Bash"}]),
        ),
        json!({"type": "user", "timestamp": T[2], "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1"},
            {"type": "tool_result", "tool_use_id": "ghost", "is_error": true},
            {"type": "tool_result", "tool_use_id": "t1"},
        ]}}),
    ]);
    assert_eq!(facts.tool_calls.len(), 1);
    assert_eq!(facts.tool_calls[0].latency_ms(), Some(2000));
    assert_eq!(facts.tool_calls[0].failure_class, None);
    assert!(state.open.is_empty());
}

/// A failed call is a denial when the operator interrupted it or the result carried a denial
/// phrase, and a tool error otherwise. Only the tool error is rate-bearing, so this split is what
/// keeps an operator's "no" out of an asset's reliability number.
#[test]
fn a_denial_and_a_tool_error_are_different_failure_classes() {
    let calls = json!([
        {"type": "tool_use", "id": "denied", "name": "Edit"},
        {"type": "tool_use", "id": "interrupted", "name": "Bash"},
        {"type": "tool_use", "id": "broken", "name": "Bash"},
    ]);
    let (facts, _) = read(&[
        assistant(T[0], "m1", calls),
        result(
            T[1],
            "denied",
            json!({"is_error": true, "content": "permission was denied"}),
        ),
        json!({"type": "user", "timestamp": T[2],
               "toolUseResult": {"interrupted": true},
               "message": {"content": [{"type": "tool_result", "tool_use_id": "interrupted",
                                        "is_error": true, "content": "stopped"}]}}),
        result(T[3], "broken", json!({"is_error": true, "content": "boom"})),
    ]);
    let classes: Vec<Option<&str>> = facts
        .tool_calls
        .iter()
        .map(|call| call.failure_class.as_deref())
        .collect();
    assert_eq!(
        classes,
        vec![
            Some(FAILURE_USER_DENIED),
            Some(FAILURE_USER_DENIED),
            Some(FAILURE_TOOL_ERROR)
        ]
    );
    assert_eq!(
        (FAILURE_USER_DENIED, FAILURE_TOOL_ERROR),
        ("user_denied", "tool_error")
    );
    assert!(facts.tool_calls[1].interrupted);
}

/// A turn is counted only for a line with prose that is not meta, not entirely tool results, and
/// not injected by the harness. `user_turns` is reported as a measure of human involvement, so
/// every machine-authored line that reaches the same log must be excluded from it.
#[test]
fn user_turns_count_only_a_persons_prose() {
    let (facts, _) = read(&[
        user(T[0], json!("a real prompt")),
        user(T[1], json!([{"type": "text", "text": "another prompt"}])),
        user(T[2], json!([{"type": "tool_result", "tool_use_id": "x"}])),
        user(T[3], json!("<system-reminder>be brief</system-reminder>")),
        user(T[4], json!([])),
        user(T[5], json!(null)),
        meta(T[6], "a meta line with prose"),
        user(
            T[7],
            json!([{"type": "tool_result", "tool_use_id": "x"}, {"type": "text", "text": "a word"}]),
        ),
    ]);
    assert_eq!(facts.user_turns, 3);
}

/// An MCP call is forbidden under both its full tool name and its server; a `Skill` under its skill
/// name; an `Agent` under its subagent type only when that type is not a harness built-in. A
/// built-in name such as `Plan` or `claude` is a substring of legitimate closed-enum values on the
/// wire, so forbidding it would make every record fail the gate for no reason.
#[test]
fn a_builtin_agent_type_is_never_forbidden() {
    let (facts, _) = read(&[assistant(
        T[0],
        "m1",
        json!([
            {"type": "tool_use", "id": "t1", "name": "mcp__srv__tool"},
            {"type": "tool_use", "id": "t2", "name": "Skill", "input": {"skill": "alpha"}},
            {"type": "tool_use", "id": "t3", "name": "Agent", "input": {"subagent_type": "Explore"}},
            {"type": "tool_use", "id": "t4", "name": "Agent", "input": {"subagent_type": "reviewer"}},
            {"type": "tool_use", "id": "t5", "name": "Bash"},
        ]),
    )]);
    let names = &facts.forbids["loaded_set_names"];
    assert!(names.contains("mcp__srv__tool") && names.contains("srv"));
    assert!(names.contains("alpha") && names.contains("reviewer"));
    assert!(!names.contains("Explore") && !names.contains("Bash") && !names.contains("Agent"));
    assert_eq!(facts.tool_calls[0].server.as_deref(), Some("srv"));
    assert_eq!(facts.tool_calls[2].agent_type.as_deref(), Some("Explore"));
    assert_eq!(facts.forbids["tool_use_ids"].len(), 5);
}

/// An `Agent` result that names an agent id links the spawn to its child session, and an async
/// acknowledgement marks the call async even when `toolUseResult` does not. The link is what lets a
/// child transcript's calls be attributed to the agent that spawned it.
#[test]
fn an_agent_spawn_links_its_child_and_reads_as_async() {
    let (facts, _) = read(&[
        assistant(
            T[0],
            "m1",
            json!([{"type": "tool_use", "id": "t1", "name": "Agent", "input": {"subagent_type": "rev"}}]),
        ),
        json!({"type": "user", "timestamp": T[1],
               "toolUseResult": {"agentId": "fx1", "status": "async_launched"},
               "message": {"content": [{"type": "tool_result", "tool_use_id": "t1",
                                        "content": "Async agent launched successfully."}]}}),
    ]);
    let call = &facts.tool_calls[0];
    assert_eq!(call.child_key.as_deref(), Some("fx1"));
    assert!(call.is_async);
    assert!(facts.forbids["agent_ids"].contains("fx1"));
}

/// An assistant line whose `attributionAgent` matches the sidecar's declared type corroborates the
/// child, and a mismatch does not. Corroboration is what separates an observed parent/child link
/// from one inferred purely from the directory layout.
#[test]
fn corroboration_needs_the_sidecars_agent_type() {
    let line = json!({"type": "assistant", "timestamp": T[0],
                      "attributionAgent": "fx-reviewer", "message": {"id": "m1", "content": []}});
    let (_, matched) = read_as(std::slice::from_ref(&line), Some("fx-reviewer"));
    let (_, mismatched) = read_as(std::slice::from_ref(&line), Some("other"));
    let (_, unknown) = read_as(&[line], None);
    assert!(matched.corroborated);
    assert!(!mismatched.corroborated && !unknown.corroborated);
}

/// An `attributionMcpServer` counts once per assistant line and is forbidden by name. It is the
/// harness's own statement about which server answered, which is the strongest attribution signal
/// available, so it is counted rather than inferred.
#[test]
fn mcp_attribution_is_counted_per_line() {
    let (facts, _) = read(&[
        json!({"type": "assistant", "timestamp": T[0], "attributionMcpServer": "srv"}),
        json!({"type": "assistant", "timestamp": T[1], "attributionMcpServer": "srv"}),
        json!({"type": "assistant", "timestamp": T[2], "attributionMcpServer": ""}),
    ]);
    assert_eq!(
        facts.mcp_attribution_counts,
        [("srv".to_string(), 2)].into_iter().collect()
    );
    assert!(facts.forbids["loaded_set_names"].contains("srv"));
}

/// A `summary` line is a compaction and nothing else. Compactions are reported as a count because a
/// compacted session is one whose context was rewritten, which changes what any later measurement
/// means — and because the summary text is exactly the kind of prose that must not be kept.
#[test]
fn a_summary_line_counts_as_a_compaction_and_keeps_no_text() {
    let (facts, _) = read(&[
        json!({"type": "summary", "timestamp": T[0], "summary": "SECRETVALUE"}),
        json!({"type": "summary", "timestamp": T[1], "summary": "SECRETVALUE"}),
    ]);
    assert_eq!(facts.compactions, 2);
    assert!(!format!("{facts:?}").contains("SECRETVALUE"));
}

/// The reference applies two *different* tests to `message.id`, and collapsing them into one loses
/// information. `claude_code.py:393`'s `if mid:` keeps an empty id out of `models`, `usages` and
/// the `message_ids` forbid bucket, but `_open_call` (`:418`) copies the same id onto
/// `ToolCall.message_id` with no test at all — so an empty id must reach the call while the
/// bookkeeping block is skipped. Projecting it away as `None` would look identical here and differ
/// on the wire the moment anything downstream keys on the id.
#[test]
fn an_empty_message_id_reaches_the_tool_call_but_not_the_bookkeeping() {
    let (facts, _) = read_as(
        &[assistant(
            "2026-01-01T00:00:00.000Z",
            "",
            serde_json::json!([{
                "type": "tool_use",
                "id": "t1",
                "name": "Bash",
                "input": {"command": "ls"}
            }]),
        )],
        None,
    );

    assert_eq!(facts.tool_calls.len(), 1);
    assert_eq!(
        facts.tool_calls[0].message_id.as_deref(),
        Some(""),
        "the empty id is copied onto the call verbatim"
    );
    assert!(facts.models.is_empty(), "`if mid:` skips the models block");
    assert!(facts.usages.is_empty(), "`if mid:` skips the usages block");
    assert!(
        !facts.forbids.contains_key("message_ids"),
        "`if mid:` skips the forbid bucket, so no empty needle is planted"
    );
}
