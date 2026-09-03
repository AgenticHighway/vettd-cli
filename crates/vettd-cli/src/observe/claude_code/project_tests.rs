//! Tests for `project.rs` (see the `gate.rs`/`gate_tests.rs` convention). The content-bearing
//! projections have their own tests in `content_tests.rs`, and the fingerprint in
//! `fingerprint_tests.rs`.

use super::*;
use serde_json::json;

/// Project `raw` as a line the reader consumes.
fn projected(raw: Value) -> Projected {
    project(&raw, 0, None).expect("the fixture line has a consumed type")
}

/// Every key in `TOP_KEYS` reaches a named field of `Projected`, and a key outside the allowlist
/// reaches nothing. The invariant is the privacy one: the projection is an allowlist of harness
/// keys, so a field the harness adds tomorrow cannot appear in the facts without a code change.
#[test]
fn top_keys_are_exactly_what_the_projection_reads() {
    let raw = json!({
        "type": "assistant", "uuid": "u", "parentUuid": "p", "timestamp": "t", "sessionId": "s",
        "isSidechain": true, "agentId": "a", "version": "v", "entrypoint": "e",
        "permissionMode": "m", "effort": "f", "sourceToolAssistantUUID": "x", "isMeta": true,
        "unknownFutureKey": "must not survive", "cost_usd": 1.25,
    });
    let projected = projected(raw);
    assert_eq!(TOP_KEYS.len(), 13);
    assert_eq!(projected.kind, LineKind::Assistant);
    let carried = [
        projected.uuid.as_deref(),
        projected.parent_uuid.as_deref(),
        projected.timestamp.as_deref(),
        projected.session_id.as_deref(),
        projected.agent_id.as_deref(),
        projected.version.as_deref(),
        projected.entrypoint.as_deref(),
        projected.permission_mode.as_deref(),
        projected.effort.as_deref(),
        projected.source_tool_assistant_uuid.as_deref(),
    ];
    let expected = [
        Some("u"),
        Some("p"),
        Some("t"),
        Some("s"),
        Some("a"),
        Some("v"),
        Some("e"),
        Some("m"),
        Some("f"),
        Some("x"),
    ];
    assert_eq!(carried, expected);
    assert!(projected.is_sidechain && projected.is_meta);
    let rendered = format!("{projected:?}");
    assert!(!rendered.contains("must not survive") && !rendered.contains("1.25"));
}

/// A line type outside `CONSUMED_TYPES` is not projected at all. The caller counts it instead, so
/// an unfamiliar line shape is never interpreted — the guarantee that makes new harness line types
/// safe rather than merely unsupported.
#[test]
fn an_unconsumed_line_type_is_not_projected() {
    for kind in ["queue-operation", "system", "", "1"] {
        assert!(project(&json!({"type": kind, "message": {"content": "x"}}), 0, None).is_none());
    }
    assert!(project(&json!({"message": {"content": "x"}}), 0, None).is_none());
    assert!(project(&json!("not an object"), 0, None).is_none());
    for (kind, expected) in [
        ("user", LineKind::User),
        ("assistant", LineKind::Assistant),
        ("attachment", LineKind::Attachment),
        ("summary", LineKind::Summary),
    ] {
        assert_eq!(projected(json!({"type": kind})).kind, expected);
    }
}

/// Names are harvested into the buckets the gate checks, `mcpMeta` server names included, and the
/// recursion stops descending past depth 4. Every one of these is a local string that must never
/// appear on the wire; the buckets are how the gate proves it did not.
#[test]
fn harvest_names_tags_each_local_name_with_its_bucket() {
    let raw = json!({
        "type": "user", "slug": "sl", "cwd": "/w", "gitBranch": "br", "sessionId": "sid",
        "agentId": "aid", "mcpMeta": {"_meta": {"info": {"name": "srv"}, "name": "top",
                                     "a": {"b": {"c": {"d": {"name": "too-deep"}}}}}},
    });
    assert_eq!(
        projected(raw).names,
        vec![
            ("slugs", "sl".to_string()),
            ("cwd_and_branches", "/w".to_string()),
            ("cwd_and_branches", "br".to_string()),
            ("harness_session_ids", "sid".to_string()),
            ("agent_ids", "aid".to_string()),
            ("loaded_set_names", "srv".to_string()),
            ("loaded_set_names", "top".to_string()),
        ]
    );
    assert!(projected(json!({"type": "user", "slug": "", "cwd": 7}))
        .names
        .is_empty());
}

/// `mcp__<server>__<tool>` yields its server segment and nothing else does. Every MCP asset is
/// keyed on that segment, so a name shape that resolves wrongly would attribute one server's calls
/// to another.
#[test]
fn mcp_server_takes_the_middle_segment_only() {
    assert_eq!(mcp_server("mcp__srv__tool"), Some("srv"));
    assert_eq!(mcp_server("mcp__srv__group__tool"), Some("srv"));
    assert_eq!(mcp_server("mcp__srv"), None);
    assert_eq!(mcp_server("mcp____tool"), None);
    assert_eq!(mcp_server("mcp__"), None);
    assert_eq!(mcp_server("Bash"), None);
    assert_eq!(mcp_server("xmcp__srv__tool"), None);
}

/// `attributionAgent` corroborates a child only against the type its sidecar declared, and
/// `attributionMcpServer` is kept only when it names something. Corroboration is what upgrades a
/// guessed parent/child link to an observed one, so a vacuous match must not count.
#[test]
fn attribution_matches_only_against_the_expected_agent() {
    let raw = json!({"type": "assistant", "attributionAgent": "fx", "attributionMcpServer": "srv"});
    assert!(
        project(&raw, 0, Some("fx"))
            .expect("consumed")
            .attribution_matches
    );
    assert!(
        !project(&raw, 0, Some("other"))
            .expect("consumed")
            .attribution_matches
    );
    assert!(
        !project(&raw, 0, None)
            .expect("consumed")
            .attribution_matches
    );
    let bare = json!({"type": "assistant", "attributionMcpServer": ""});
    assert!(
        !project(&bare, 0, Some("fx"))
            .expect("consumed")
            .attribution_matches
    );
    assert_eq!(
        project(&raw, 0, None)
            .expect("consumed")
            .mcp_attribution
            .as_deref(),
        Some("srv")
    );
    assert_eq!(
        project(&bare, 0, None).expect("consumed").mcp_attribution,
        None
    );
}

/// Python truthiness decides every boolean the projection carries, and a non-string scalar never
/// becomes a name. Both rules exist so the reader's answers do not change when the harness spells a
/// flag `1`, `"yes"` or `true` — and so that a coerced value can never be mistaken for a name.
#[test]
fn truthiness_and_string_guards_match_the_pythons() {
    assert!(truthy(Some(&json!(1))) && truthy(Some(&json!("x"))) && truthy(Some(&json!([0]))));
    assert!(!truthy(None) && !truthy(Some(&json!(0))) && !truthy(Some(&json!(""))));
    assert!(!truthy(Some(&json!([]))) && !truthy(Some(&json!({}))) && !truthy(Some(&json!(null))));
    assert_eq!(nonempty_str(Some(&json!("v"))), Some("v".to_string()));
    assert_eq!(nonempty_str(Some(&json!(""))), None);
    assert_eq!(nonempty_str(Some(&json!(7))), None);
    assert_eq!(str_list(Some(&json!(["a", 1, null, "b"]))), vec!["a", "b"]);
    assert!(str_list(Some(&json!("not a list"))).is_empty());
    assert_eq!(int_or_none(Some(&json!(5))), Some(5));
    assert_eq!(int_or_none(Some(&json!(5.0))), None);
    assert_eq!(int_or_none(Some(&json!(true))), None);
    assert_eq!(
        stringify_if_truthy(Some(&json!(77))),
        Some("77".to_string())
    );
    assert_eq!(stringify_if_truthy(Some(&json!(0))), None);
}
