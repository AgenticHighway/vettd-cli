//! Tests for `skills.rs` — the in-band skill bodies and their synthetic calls.
//!
//! They drive `apply` through the projection and reuse `apply_tests`'s `read` helper, so a change to
//! the projection that stops delivering a meta line fails here too.

use super::super::tests::read;
use super::*;
use crate::observe::canonical::hex_sha256;
use serde_json::json;

/// A meta `user` line carrying the string `text`, `second` seconds into the fixture session.
fn meta(second: u32, text: &str) -> serde_json::Value {
    json!({"type": "user", "timestamp": format!("2026-08-15T10:00:{second:02}Z"),
           "isMeta": true, "message": {"content": text}})
}

/// Milliseconds of `2026-08-15T10:00:00Z`, the fixture session's first second.
const BASE_MS: i64 = 1_786_788_000_000;

/// A `<skill-format>true</skill-format>` command records nothing on its own: it parks a pending
/// skill whose body arrives on the next meta line, and a meta line that names its own command never
/// consumes it. Getting this wrong either loses the skill body's hash or attributes the wrong body
/// to the skill, and the body hash is the whole of the evidence that a skill ran.
#[test]
fn the_pending_skill_takes_its_body_from_the_next_meta_line() {
    let body = "# alpha body\nStep one.";
    let (facts, state) = read(&[
        meta(
            0,
            "<command-name>alpha</command-name>\n<skill-format>true</skill-format>",
        ),
        meta(1, "<command-name>beta</command-name>\nbeta body"),
        meta(2, body),
    ]);
    let recorded: Vec<(&str, String, i64)> = facts
        .in_band_assets
        .iter()
        .map(|asset| {
            (
                asset.name.as_str(),
                asset.content_sha256.clone(),
                asset.ts_ms,
            )
        })
        .collect();
    assert_eq!(
        recorded,
        vec![
            ("beta", hex_sha256(b"\nbeta body"), BASE_MS + 1_000),
            ("alpha", hex_sha256(body.as_bytes()), BASE_MS + 2_000),
        ]
    );
    assert!(state.pending_skill.is_none());
    assert!(facts
        .in_band_assets
        .iter()
        .all(|asset| asset.kind == InBandKind::SkillBody));
}

/// A pending skill that never receives a body records nothing at all. A skill whose body was never
/// seen has no content hash, and an asset without one cannot be identified — recording it would
/// invent an observation.
#[test]
fn a_pending_skill_with_no_body_records_nothing() {
    let (facts, state) = read(&[
        meta(
            0,
            "<command-name>alpha</command-name>\n<skill-format>true</skill-format>",
        ),
        json!({"type": "user", "timestamp": "2026-08-15T10:00:01Z",
               "message": {"content": "an ordinary turn"}}),
    ]);
    assert!(facts.in_band_assets.is_empty());
    assert!(facts.tool_calls.is_empty());
    assert_eq!(
        state.pending_skill,
        Some(PendingSkill {
            name: "alpha".to_string(),
            ts_ms: BASE_MS,
        })
    );
}

/// An in-band skill records a synthetic call that is paired with itself, async, and not forbidden as
/// a tool-use id. `is_async` is how the shared model spells "no latency to measure": the harness
/// injects the body without a round trip, so a measured duration of zero would be a fiction. The
/// synthetic id is not an identifier the harness ever wrote, so forbidding it would add a string the
/// gate must then look for in vain.
#[test]
fn an_in_band_skill_records_a_self_paired_async_call() {
    let (facts, _) = read(&[
        meta(0, "<command-name>alpha</command-name>\nbody"),
        meta(1, "<command-name>alpha</command-name>\nagain"),
    ]);
    let ids: Vec<&str> = facts
        .tool_calls
        .iter()
        .map(|call| call.tool_use_id.as_str())
        .collect();
    assert_eq!(ids, vec!["synthetic-skill-1", "synthetic-skill-2"]);
    let call = &facts.tool_calls[0];
    assert_eq!(call.name, "Skill");
    assert_eq!(call.skill.as_deref(), Some("alpha"));
    assert!(call.is_async && call.paired());
    assert_eq!(call.is_error, Some(false));
    assert_eq!(call.latency_ms(), Some(0));
    assert_eq!(
        call.input_fingerprint,
        sha256_json(&json!({"skill": "alpha"}))
    );
    assert!(!facts.forbids.contains_key("tool_use_ids"));
    assert!(facts.forbids["loaded_set_names"].contains("alpha"));
}
