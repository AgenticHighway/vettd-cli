//! Tests for `attachments.rs` — the `attachment` branch of the application.
//!
//! They drive `apply` through the projection, like `apply_tests.rs`, and reuse its `read` helper so
//! both halves of the application are exercised by the same driver.

use super::super::tests::read;
use super::*;
use serde_json::json;

/// The first `initial` loaded-set event absorbs the rules files seen before it, and later ones
/// append to that same event. A nested-memory line can precede or follow the listing, and the
/// segment model reads the initial event as "what was loaded at session start" — so a rules file
/// must land there whichever side of the listing the harness wrote it.
#[test]
fn the_first_initial_event_absorbs_rules_files_from_either_side() {
    let memory = |name: &str, ts: &str| {
        json!({"type": "attachment", "timestamp": ts, "attachment": {
            "type": "nested_memory", "path": format!("/w/{name}"), "content": "be brief"}})
    };
    let (facts, _) = read(&[
        memory("EARLY.md", "2026-08-15T10:00:00Z"),
        json!({"type": "attachment", "timestamp": "2026-08-15T10:00:01Z", "attachment": {
            "type": "skill_listing", "names": ["alpha"], "content": "- alpha: does things\n"}}),
        memory("LATE.md", "2026-08-15T10:00:02Z"),
        json!({"type": "attachment", "timestamp": "2026-08-15T10:00:03Z", "attachment": {
            "type": "agent_listing_delta", "addedTypes": ["rev"], "isInitial": true}}),
    ]);
    let initial = &facts.loaded_events[0];
    assert_eq!(initial.kind, LoadedSetKind::Initial);
    assert_eq!(
        initial.rules_files,
        vec!["EARLY.md".to_string(), "LATE.md".to_string()]
    );
    assert_eq!(initial.skills, vec!["alpha".to_string()]);
    // The second initial event is not the one rules files attach to.
    assert!(facts.loaded_events[1].rules_files.is_empty());
    assert_eq!(
        facts
            .in_band_assets
            .iter()
            .map(|asset| asset.kind)
            .collect::<Vec<_>>(),
        vec![InBandKind::RulesFile, InBandKind::RulesFile]
    );
    assert!(facts.forbids["loaded_set_names"].contains("EARLY.md"));
}

/// The first deferred-tools listing of a read is the session's initial set and every later one is a
/// delta. Segments start at an initial event and only a delta may fold into the segment before it,
/// so mislabelling the second listing would split one session into two loaded sets.
#[test]
fn the_first_deferred_tools_listing_is_initial_and_the_rest_are_deltas() {
    let delta = |ts: &str, name: &str| {
        json!({"type": "attachment", "timestamp": ts, "attachment": {
            "type": "deferred_tools_delta", "addedNames": [name],
            "addedLines": [format!("{name}: description")],
            "pendingMcpServers": [], "failedMcpServers": []}})
    };
    let (facts, _) = read(&[
        delta("2026-08-15T10:00:00Z", "mcp__srv__a"),
        delta("2026-08-15T10:00:01Z", "mcp__srv__b"),
        json!({"type": "attachment", "timestamp": "2026-08-15T10:00:02Z", "attachment": {
            "type": "mcp_instructions_delta", "addedNames": ["srv"]}}),
    ]);
    let kinds: Vec<LoadedSetKind> = facts.loaded_events.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds,
        vec![
            LoadedSetKind::Initial,
            LoadedSetKind::Delta,
            LoadedSetKind::Delta
        ]
    );
    assert_eq!(facts.loaded_events[0].tool_schema_bytes["srv"], 24);
    assert!(facts.loaded_events[2].tool_names.is_empty());
    assert!(facts.forbids["loaded_set_names"].contains("mcp__srv__a"));
    assert!(facts.forbids["loaded_set_names"].contains("srv"));
}

/// Only MCP tool names are forbidden from a deferred-tools listing — a built-in tool name is not.
/// `Bash`, `Read` and their kind are substrings of legitimate values on the wire, so forbidding them
/// would make every record fail the gate; removed and re-added names are forbidden alongside added
/// ones because a server that left the set was still loaded at some point in the run.
#[test]
fn only_mcp_tool_names_are_forbidden_from_a_listing() {
    let (facts, _) = read(&[
        json!({"type": "attachment", "timestamp": "2026-08-15T10:00:00Z",
        "attachment": {"type": "deferred_tools_delta",
                       "addedNames": ["Bash", "mcp__added__x"],
                       "removedNames": ["mcp__gone__y"], "readdedNames": ["mcp__back__z"],
                       "pendingMcpServers": ["pend"], "failedMcpServers": ["fail"]}}),
    ]);
    let names = &facts.forbids["loaded_set_names"];
    for expected in [
        "mcp__added__x",
        "added",
        "mcp__gone__y",
        "gone",
        "mcp__back__z",
        "back",
        "pend",
        "fail",
    ] {
        assert!(names.contains(expected), "expected {expected:?} forbidden");
    }
    assert!(!names.contains("Bash"));
    let event = &facts.loaded_events[0];
    assert_eq!(event.removed, vec!["mcp__gone__y".to_string()]);
    assert_eq!(event.readded, vec!["mcp__back__z".to_string()]);
    assert_eq!(event.failed_mcp, vec!["fail".to_string()]);
}

/// An attachment the reader does not interpret is counted as an unknown line, exactly like a line
/// type it does not consume — and so is a line with no attachment object. Counting is the whole
/// response: interpreting an unfamiliar attachment is how content leaks, and silently ignoring it
/// would hide the coverage gap from the report.
#[test]
fn an_uninterpreted_attachment_is_counted_not_interpreted() {
    let (facts, _) = read(&[
        json!({"type": "attachment", "timestamp": "2026-08-15T10:00:00Z",
               "attachment": {"type": "diagnostics", "content": "SECRETVALUE"}}),
        json!({"type": "attachment", "timestamp": "2026-08-15T10:00:01Z"}),
        json!({"type": "queue-operation", "timestamp": "2026-08-15T10:00:02Z", "content": "SECRETVALUE"}),
    ]);
    assert_eq!(facts.lines_unknown_type, 3);
    assert!(facts.loaded_events.is_empty());
    assert!(!format!("{facts:?}").contains("SECRET"));
}

/// A harness built-in agent type never reaches the forbids from an agent listing, though it stays in
/// the event's own list. The listing is what the harness loaded; the forbids are what the gate hunts
/// for on the wire, and a built-in name there would collide with a legitimate enum value.
#[test]
fn an_agent_listing_forbids_only_the_non_builtin_types() {
    let (facts, _) = read(&[
        json!({"type": "attachment", "timestamp": "2026-08-15T10:00:00Z",
        "attachment": {"type": "agent_listing_delta",
                       "addedTypes": ["Explore", "Plan", "custom-reviewer"], "isInitial": false}}),
    ]);
    let event = &facts.loaded_events[0];
    assert_eq!(event.kind, LoadedSetKind::Delta);
    assert_eq!(
        event.agent_types,
        vec![
            "Explore".to_string(),
            "Plan".to_string(),
            "custom-reviewer".to_string()
        ]
    );
    let names = &facts.forbids["loaded_set_names"];
    assert!(names.contains("custom-reviewer"));
    assert!(!names.contains("Explore") && !names.contains("Plan"));
}
