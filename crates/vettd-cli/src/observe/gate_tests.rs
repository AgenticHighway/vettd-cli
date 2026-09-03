//! Tests for [`super`], re-expressing every invariant of
//! `spikes/828-passive-observer/prototype/tests/test_gate.py` (27 tests) plus the three the Rust
//! port owes on its own: the one gate regex the `regex` crate cannot compile, the hand-written
//! replacement's semantics, and gate/schema leaf-path parity.
//!
//! Every value below is invented; hashes are sha256 of fixture labels. None of these tests can
//! prove the gate JSON lists the right fields — they prove the checker enforces whatever it says.
//!
//! The Python's `test_cli_exit_codes` drives `check_field_gate.py` as a subprocess. There is no
//! subprocess here: `gate.rs` is a library and `observe check` is wired in a later phase, so that
//! test is re-expressed at the `check()` level as
//! `pass_then_violations_are_reported_like_the_cli`, keeping the three assertions it makes about
//! what a caller sees (clean payload, unknown key on the parent path, `--dynamic` hit).

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{epoch_in_string, python_whitespace, Dynamic, Pattern, GATE, GATE_JSON};

/// The sibling artifact the gate must stay in step with; see `schema_and_gate_leaf_paths_agree`.
const SCHEMA_JSON: &str = include_str!("../../../../telemetry-envelope.schema.json");

const NULL_UUID: &str = "00000000-0000-4000-8000-000000000000";
const DAY: &str = "2026-01-01";

fn hex64(label: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("fixture:{label}").as_bytes());
    format!("{:x}", hasher.finalize())
}

/// One record, one asset, one bom entry: the smallest payload that satisfies every gate rule.
/// Ported value-for-value from `test_gate.py:26-66`.
fn minimal_valid_payload() -> Value {
    let asset_id = hex64("asset-a");
    let bom_version = hex64("bom-a");
    json!({
        "envelope_version": "0.2.0",
        "extractor_version": "proto-0.1.0",
        "gate_version": 1,
        "emitted_day": DAY,
        "resource": {
            "device_id": NULL_UUID,
            "device_id_source": "placeholder",
            "harness": "claude_code",
            "harness_version": "1.0.0",
            "collector": "prototype",
            "collector_version": "0.1.0",
        },
        "records": [{
            "run_id": hex64("run-a"),
            "observed_day": DAY,
            "model": "claude-sonnet-5",
            "entrypoint_class": "cli",
            "effort": "medium",
            "permission_mode": "default",
            "task_category": "mixed",
            "bom_version": bom_version,
            "loaded_set_basis": "harness_log",
            "run_outcome": "completed",
            "counts": {
                "turns": 2, "tool_calls": 5, "tool_failures": 1, "user_denials": 0,
                "subagent_runs": 0, "compactions": 0, "unpaired_tool_uses": 0,
                "loaded_set_changes": 0, "repeated_tool_calls": 0,
            },
            "tokens": {
                "input": 100, "cache_creation": null, "cache_read": 40, "cached_input": null,
                "output": 30, "thinking": null, "reasoning": null, "basis": "harness_usage",
            },
            "tokens_by_model": [{
                "model": "claude-sonnet-5", "input": 100, "cache_creation": null,
                "cache_read": 40, "cached_input": null, "output": 30, "thinking": null,
                "reasoning": null,
            }],
            "assets": [{
                "asset_id": asset_id,
                "asset_type": "skill",
                "key_basis": "name_hash",
                "tier": "inferred",
                "binding": "not_applicable",
                "direct_evidence_available": true,
                "signals": {
                    "invocations": {"n": 3},
                    "failures": {
                        "tool_error": 1, "timeout": 0, "user_denied": 0, "interrupted": 0,
                        "unknown": 0,
                    },
                    "harness_corroborations": null,
                    "latency_ms": {"n": 3, "sum": 900, "min": 200, "max": 400, "sumsq": "290000"},
                    "tokens_attributed": null,
                    "context_cost_est": {"tokens": 120, "method": "listing_bytes_div4"},
                },
            }],
        }],
        "bom": [{"bom_version": bom_version, "asset_ids": [asset_id]}],
        "coverage": {
            "sessions_seen": 1, "sessions_emitted": 1, "sessions_skipped_unparseable": 0,
            "lines_seen": 20, "lines_unknown_type": 0, "bytes_read": 4096,
            "truncated_sessions": 0, "window_days": 30, "cursor_state": "fresh",
            "run_id_basis": "test_secret",
        },
    })
}

/// Build a [`Dynamic`] the way the Python tests pass a dict of sets.
fn dynamic(sets: &[(&str, &[&str])]) -> Dynamic {
    let mut raw: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, values) in sets {
        let entries = values.iter().map(|value| (*value).to_string()).collect();
        raw.insert((*name).to_string(), entries);
    }
    Dynamic::normalize(&raw)
}

#[track_caller]
fn assert_violation(violations: &[String], path: &str, rule: &str) {
    let prefix = format!("{path}: {rule}: ");
    assert!(
        violations.iter().any(|line| line.starts_with(&prefix)),
        "expected {prefix:?} in {violations:?}"
    );
}

// ---- positive -----------------------------------------------------------------------------

/// The helper payload satisfies every static gate rule with no dynamic sets, so later tests that
/// mutate one field prove that field's rule and nothing else. Cannot prove the gate lists
/// everything a real emitter writes.
#[test]
fn minimal_payload_passes_with_empty_dynamic_sets() {
    assert_eq!(
        GATE.check(&minimal_valid_payload(), &Dynamic::empty()),
        Vec::<String>::new()
    );
    assert_eq!(
        GATE.check(&minimal_valid_payload(), &dynamic(&[])),
        Vec::<String>::new()
    );
}

/// Populated dynamic sets only fail on an actual substring hit, so handing the checker the whole
/// local vocabulary does not make every payload unsendable. Cannot prove a real loaded-set name
/// never collides with a permitted enum value (that is the emitter's exposure).
#[test]
fn minimal_payload_passes_with_populated_non_matching_sets() {
    let sets = dynamic(&[
        (
            "loaded_set_names",
            &["quantum-widget-skill", "orbital-lint"],
        ),
        (
            "cwd_and_branches",
            &["/opt/invented/workspace", "feature/invented-branch"],
        ),
        (
            "harness_session_ids",
            &["11111111-2222-4333-8444-555555555555"],
        ),
        ("current_username", &["nobody-invented"]),
    ]);
    assert_eq!(
        GATE.check(&minimal_valid_payload(), &sets),
        Vec::<String>::new()
    );
}

/// Empty and 1-2 char set entries are ignored ("cl" would otherwise hit "claude_code") while a
/// 3-char entry is still enforced on a free string field. Cannot prove 3 is the right floor.
#[test]
fn dynamic_entries_shorter_than_three_chars_are_skipped() {
    let short = dynamic(&[("slugs", &["", "cl", "ai"])]);
    assert_eq!(
        GATE.check(&minimal_valid_payload(), &short),
        Vec::<String>::new()
    );

    let mut payload = minimal_valid_payload();
    payload["extractor_version"] = json!("proto-cli-1");
    let violations = GATE.check(&payload, &dynamic(&[("slugs", &["cli"])]));
    assert_violation(&violations, "extractor_version", "dynamic:slugs");
}

/// A nullable object leaf may be null or a full object, and a scalar there is a type error — so
/// "absent" and "present" are the only two shapes on the wire. Cannot prove the object's children
/// are semantically right, only present and typed.
#[test]
fn nullable_object_accepts_null_and_populated_but_not_scalar() {
    let mut payload = minimal_valid_payload();
    let stats = json!({"n": 1, "sum": 50, "min": 50, "max": 50, "sumsq": "2500"});
    payload["records"][0]["assets"][0]["signals"]["tokens_attributed"] = stats;
    assert_eq!(
        GATE.check(&payload, &Dynamic::empty()),
        Vec::<String>::new()
    );

    payload["records"][0]["assets"][0]["signals"]["tokens_attributed"] = json!(5);
    let violations = GATE.check(&payload, &Dynamic::empty());
    let path = "records[0].assets[0].signals.tokens_attributed";
    assert_violation(&violations, path, "type_mismatch");
}

// ---- structure ----------------------------------------------------------------------------

/// A key the gate does not list is a violation reported on its parent path, and the key itself is
/// never echoed because it could be content. Cannot prove keys inside a string value are seen;
/// that is what the forbidden-value patterns are for.
#[test]
fn unknown_leaf_key_fails() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["duration_ms"] = json!(1234);
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0]", "unknown_key");
    assert!(
        !violations.iter().any(|line| line.contains("duration_ms")),
        "the unknown key was echoed: {violations:?}"
    );
}

/// An unlisted object with no leaves is still rejected, so a walker that only sees leaves cannot
/// be bypassed by an empty container. Cannot prove non-empty unknown objects report children.
#[test]
fn unknown_intermediate_object_fails_even_when_empty() {
    let mut payload = minimal_valid_payload();
    payload["resource"]["extra"] = json!({});
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "resource", "unknown_key");
}

/// A required field absent from a written object is reported on the parent path, so a payload
/// cannot silently drop a field the cloud will index on. Cannot prove that every field the schema
/// marks required is also required in the gate.
#[test]
fn missing_required_key_fails() {
    let mut payload = minimal_valid_payload();
    let coverage = payload["coverage"]
        .as_object_mut()
        .expect("coverage is an object");
    let _ = coverage.remove("window_days");
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "coverage", "missing_required");
}

/// Null is only accepted where the gate says nullable, so "unknown" cannot be smuggled into a
/// field the cloud reads as a number. Cannot prove the nullable fields are the right ones.
#[test]
fn null_in_non_nullable_field_fails() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["tokens"]["input"] = Value::Null;
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].tokens.input", "null_not_allowed");
}

/// A bool does not pass as an integer count even though the Python it is ported from treats
/// `True` as 1. Cannot prove other integer-like types (floats) are rejected; the type check does.
#[test]
fn boolean_is_not_an_integer() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["counts"]["turns"] = json!(true);
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].counts.turns", "type_mismatch");
}

// ---- value-level negatives, one per case ----------------------------------------------------

/// An absolute POSIX path inside a string leaf is caught by the value-level pattern, which is the
/// rule a key-path walker cannot express. Cannot prove relative paths are caught.
#[test]
fn absolute_path_in_string_fails() {
    let violations = check_extractor_version("/opt/invented/workspace/tool");
    assert_violation(&violations, "extractor_version", "pattern:abs_posix_path");
}

/// A URL scheme inside a string leaf is caught, so an endpoint cannot ride out in a version
/// string. Cannot prove scheme-less hostnames are caught by this rule (`dotted_host` covers those).
#[test]
fn url_in_string_fails() {
    let violations = check_extractor_version("https://invented.example/path");
    assert_violation(&violations, "extractor_version", "pattern:url_scheme");
}

/// A loaded-set name handed over at run time is caught as a case-insensitive substring, and the
/// message does not echo the name — reporting a leak must not itself leak. Cannot prove names the
/// emitter failed to collect are caught.
#[test]
fn loaded_set_name_via_dynamic_set_fails_case_insensitively() {
    let mut payload = minimal_valid_payload();
    payload["extractor_version"] = json!("Quantum-Widget-Skill");
    let sets = dynamic(&[("loaded_set_names", &["quantum-widget-skill"])]);
    let violations = GATE.check(&payload, &sets);
    assert_violation(&violations, "extractor_version", "dynamic:loaded_set_names");
    assert!(
        !violations
            .iter()
            .any(|line| line.to_lowercase().contains("widget")),
        "the local-only name was echoed: {violations:?}"
    );
}

/// A day path only accepts `YYYY-MM-DD`, so finer time resolution cannot egress there and
/// correlate a record with a wall-clock event. Cannot prove the day is a UTC day, not a local one.
#[test]
fn second_resolution_timestamp_in_observed_day_fails() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["observed_day"] = json!("2026-01-01T12:34:56Z");
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].observed_day", "format_mismatch");
}

/// A day that passes the format regex but is not a real calendar date is still rejected, so the
/// day column the cloud retains on is always parseable. This is the `NaiveDate` half of the rule
/// the format regex cannot express; it cannot prove the day was computed in UTC.
#[test]
fn non_calendar_day_fails_even_with_the_right_shape() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["observed_day"] = json!("2026-02-30");
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].observed_day", "format_mismatch");
}

/// A hash path rejects a uuid (a harness session id) by exact hex64 format, so a raw harness
/// identifier cannot take a pseudonym's place. Cannot prove a hex64 value is a real HMAC.
#[test]
fn uuid_in_run_id_fails_exact_hex64() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["run_id"] = json!(NULL_UUID);
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].run_id", "format_mismatch");
}

/// The `uuid_any` pattern catches a uuid in a string whose own format would allow it
/// (`extractor_version` admits hex and dashes), so format alone is never the whole rule. Cannot
/// prove `device_id` is the only allowed uuid path beyond what the gate lists.
#[test]
fn uuid_outside_device_id_in_free_string_fails() {
    let violations = check_extractor_version("11111111-2222-4333-8444-555555555555");
    assert_violation(&violations, "extractor_version", "pattern:uuid_any");
}

/// Enums are closed, so a new harness value cannot appear on the wire before the gate admits it.
/// Cannot prove enum membership is semantically right.
#[test]
fn bad_enum_fails() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["effort"] = json!("extreme");
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].effort", "not_in_enum");
}

/// `asset_id` must be exactly 64 lowercase hex chars, so a readable name cannot sit in the column
/// the cloud joins on. Cannot prove the value is content-derived.
#[test]
fn non_hex_asset_id_fails() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["assets"][0]["asset_id"] = json!("g".repeat(64));
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(
        &violations,
        "records[0].assets[0].asset_id",
        "format_mismatch",
    );
}

/// A custom provider model id is rejected rather than passed through, so `taskcat` must map it to
/// "other" and a private model name never egresses. Cannot prove the allowlist covers every
/// legitimate provider prefix.
#[test]
fn off_allowlist_model_fails() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["model"] = json!(format!("acme{}", "-finetune-v3"));
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].model", "not_in_enum");
}

/// A token-shaped value is caught by the defence-in-depth pattern, so a credential that reached a
/// string leaf by some other bug still cannot leave. Cannot prove every provider's token prefix is
/// in the list.
#[test]
fn bearer_like_value_fails() {
    let violations = check_extractor_version(&format!("gh{}{}", "p_", "x1y2z3w4v5u6t7s8"));
    assert_violation(&violations, "extractor_version", "pattern:bearer_like");
}

/// A unix-seconds value in a count is flagged by the epoch rule as well as by bounds, so a
/// timestamp cannot hide in a numeric field. Cannot prove values outside the two epoch ranges are
/// timestamps of some other scale.
#[test]
fn epoch_number_in_count_field_fails() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["counts"]["turns"] = json!(1_700_000_000);
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].counts.turns", "epoch_in_number");
    assert_violation(&violations, "records[0].counts.turns", "out_of_bounds");
}

/// The epoch rule is independent of bounds: `bytes_read` admits 1.7e9, and the value is rejected
/// anyway. Cannot prove a genuine 1.7 GB read never happens — the rule is a deliberate trade.
#[test]
fn epoch_rule_fires_where_bounds_do_not() {
    let mut payload = minimal_valid_payload();
    payload["coverage"]["bytes_read"] = json!(1_700_000_000);
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "coverage.bytes_read", "epoch_in_number");
    assert!(
        !violations.iter().any(|line| line.contains("out_of_bounds")),
        "bytes bounds should admit 1.7e9: {violations:?}"
    );
}

/// Sums of squares are exact-format decimal strings, so timestamp-pattern checks intended for free
/// text do not reject legitimate magnitudes.
#[test]
fn sum_of_squares_decimal_is_not_treated_as_free_text() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["assets"][0]["signals"]["latency_ms"]["sumsq"] = json!("1700000000");
    assert_eq!(
        GATE.check(&payload, &Dynamic::empty()),
        Vec::<String>::new()
    );
}

/// Any whitespace in a string leaf is rejected, because no permitted value contains one and free
/// text always does. Cannot prove free text without whitespace is caught by this rule alone.
#[test]
fn whitespace_in_string_fails() {
    let violations = check_extractor_version("proto 0.1.0");
    assert_violation(&violations, "extractor_version", "pattern:whitespace");
}

/// An MCP tool/server name in harness form is caught, so a local server's name cannot ride out in
/// a string leaf. Cannot prove server names outside the `mcp__` form are caught by this pattern.
#[test]
fn mcp_tool_name_in_string_fails() {
    let violations = check_extractor_version(&format!("mcp__{}__list", "invented-server"));
    assert_violation(&violations, "extractor_version", "pattern:mcp_tool_name");
}

/// A clean payload yields no lines, an unknown key is reported on its parent path, and a dynamic
/// set the caller supplies is honoured — the three things `check_field_gate.py`'s CLI test proves
/// about what a caller sees. Cannot prove the exit codes of `observe check`, which is wired later.
#[test]
fn pass_then_violations_are_reported_like_the_cli() {
    let good = minimal_valid_payload();
    assert_eq!(GATE.check(&good, &Dynamic::empty()), Vec::<String>::new());

    let mut bad = minimal_valid_payload();
    bad["records"][0]["extra"] = json!("x");
    let violations = GATE.check(&bad, &Dynamic::empty());
    assert!(
        violations
            .iter()
            .any(|line| line.starts_with("records[0]: unknown_key")),
        "expected an unknown_key line: {violations:?}"
    );

    let sets = dynamic(&[("loaded_set_names", &["proto-0.1.0"])]);
    let violations = GATE.check(&good, &sets);
    assert_violation(&violations, "extractor_version", "dynamic:loaded_set_names");
}

// ---- enum fields are closed ------------------------------------------------------------------

/// Enum-typed fields are exempt from the dynamic substring rule because their value space is
/// closed: the value is one of the gate's own literals, so it cannot carry a local-only name (a
/// skill called "run" would otherwise fail every record whose outcome is "truncated"). Cannot
/// prove a non-enum field with the same substring is caught — the next test's job.
#[test]
fn short_name_inside_enum_literal_passes() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["run_outcome"] = json!("truncated");
    let sets = dynamic(&[("loaded_set_names", &["run", "cat"])]);
    let violations = GATE.check(&payload, &sets);
    let hits: Vec<&String> = violations
        .iter()
        .filter(|l| l.contains("run_outcome"))
        .collect();
    assert!(hits.is_empty(), "enum field must be exempt: {hits:?}");
}

/// The same short name is still caught on a free string field, so the enum exemption narrows the
/// rule rather than disabling it. Cannot prove every free string field is reachable by a name.
#[test]
fn same_name_still_caught_on_a_free_string_field() {
    let mut payload = minimal_valid_payload();
    payload["extractor_version"] = json!("proto-run-1");
    let sets = dynamic(&[("loaded_set_names", &["run"])]);
    let violations = GATE.check(&payload, &sets);
    assert_violation(&violations, "extractor_version", "dynamic:loaded_set_names");
}

// ---- Rust-port-specific invariants ------------------------------------------------------------

/// Exactly one gate pattern needs the hand-written replacement. If a future gate edit adds another
/// lookaround the port would silently stop enforcing it, so this fails the moment a second regex
/// stops compiling — and equally if `epoch_in_string` ever becomes compilable and the `Fn` arm is
/// dead weight. Cannot prove the compiled regexes mean the same thing as Python's `re`.
#[test]
fn gate_has_no_uncompilable_regex_except_epoch() {
    let doc: Value = serde_json::from_str(GATE_JSON).expect("gate JSON parses");
    let patterns = doc["forbiddenValuePatterns"]
        .as_array()
        .expect("forbiddenValuePatterns is a list");
    for entry in patterns {
        let id = entry["id"].as_str().expect("every pattern has an id");
        let source = entry["regex"].as_str().expect("every pattern has a regex");
        let compiled = Regex::new(source).is_ok();
        if id == "epoch_in_string" {
            assert!(
                !compiled,
                "epoch_in_string compiled; drop the hand-written fn"
            );
        } else {
            assert!(compiled, "gate pattern {id} no longer compiles");
        }
    }
    // `whitespace` compiles, but the `regex` crate's `\s` is `White_Space` while Python's is
    // `str.isspace()`; it is hand-written for semantics, not for compilability.
    let hand_written: Vec<&str> = GATE
        .patterns
        .iter()
        .filter(|(_, pattern)| matches!(pattern, Pattern::Fn(_)))
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        hand_written,
        vec!["whitespace", "epoch_in_string"],
        "only these two patterns may bypass the gate's own regex source"
    );
}

/// `python_whitespace` reproduces what Python's `re` means by the gate's `\s`: `str.isspace()`,
/// which is Unicode `White_Space` *plus* U+001C..U+001F. Compiling the gate source verbatim would
/// let those four separators through the one pattern whose job is to catch any free text, so a
/// string field added later with a permissive format would leak them. The exhaustive sweep is the
/// point: it is the same comparison the reviewer ran against CPython, frozen as a test.
#[test]
fn python_whitespace_fn_matches_python_isspace() {
    // Unicode White_Space, the regex crate's `\s`. Verified against CPython 3.11 with
    // `python3 -c "print([hex(c) for c in range(0x110000) if chr(c).isspace()])"`.
    const PYTHON_ONLY: [char; 4] = ['\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}'];
    let crate_whitespace = Regex::new(r"\s").expect("the crate's own class compiles");
    for c in PYTHON_ONLY {
        assert!(
            python_whitespace(&c.to_string()),
            "U+{:04X} is whitespace to Python and must be to us",
            c as u32
        );
        assert!(
            !crate_whitespace.is_match(&c.to_string()),
            "U+{:04X} would have slipped through the crate's own \\s",
            c as u32
        );
    }
    for c in [
        ' ', '\t', '\n', '\r', '\u{b}', '\u{c}', '\u{85}', '\u{a0}', '\u{2028}', '\u{3000}',
    ] {
        assert!(python_whitespace(&c.to_string()), "U+{:04X}", c as u32);
    }
    for c in ['a', '0', '_', '-', '\u{0}', '\u{7f}', '\u{200b}', 'é'] {
        assert!(!python_whitespace(&c.to_string()), "U+{:04X}", c as u32);
    }
    assert!(
        python_whitespace("a\u{1c}b"),
        "embedded separator still matches"
    );
    assert!(
        !python_whitespace(""),
        "the empty string holds no whitespace"
    );
}

/// The hand-written `epoch_in_string` reproduces the lookaround regex it replaces: a maximal run
/// of digits of length 10 or 13 starting `1` with a second digit `5`-`9`. The boundary cases are
/// the point — a digit embedded in a longer run is not a timestamp, and treating it as one would
/// make the rule fire on hashes and version strings. Cannot prove every 10-digit run really is a
/// timestamp; the gate accepts that over-match deliberately.
#[test]
fn epoch_in_string_fn_matches_lookaround_semantics() {
    let positives = [
        "1700000000",
        "1500000000",
        "1999999999",
        "1700000000000",
        "1500000000000",
        "1999999999999",
        "abc1700000000def",
        "v1700000000",
        "1700000000-x",
        "x1700000000000y",
        "1500000000x1700000000",
    ];
    let negatives = [
        "",
        "170000000",
        "17000000000",
        "170000000000",
        "17000000000000",
        "1700000000000000",
        "01700000000",
        "11700000000",
        "91700000000",
        "2700000000",
        "1400000000",
        "1499999999",
        "1a00000000",
        "proto-0.1.0",
        "claude-sonnet-5",
        "2026-01-01",
    ];
    for case in positives {
        assert!(
            epoch_in_string(case),
            "{case:?} should match epoch_in_string"
        );
    }
    for case in negatives {
        assert!(
            !epoch_in_string(case),
            "{case:?} should not match epoch_in_string"
        );
    }
}

/// The gate and the envelope schema describe the same payload, so a field added to one without
/// the other is caught here rather than by the cloud rejecting real traffic. The only permitted
/// difference is the three nullable-object/array container paths the gate lists once as
/// `type: object` and the schema expresses structurally. Cannot prove either document is right
/// about a field's reason or category — only that they cover the same leaves.
#[test]
fn schema_and_gate_leaf_paths_agree() {
    let schema: Value = serde_json::from_str(SCHEMA_JSON).expect("schema JSON parses");
    let defs = schema["$defs"].as_object().expect("schema has $defs");
    let mut schema_paths = BTreeSet::new();
    schema_leaf_paths(&schema, defs, "", &mut schema_paths);

    let gate_paths: BTreeSet<String> = GATE.fields.keys().cloned().collect();
    let gate_only: Vec<&str> = gate_paths
        .difference(&schema_paths)
        .map(String::as_str)
        .collect();
    let schema_only: Vec<&str> = schema_paths
        .difference(&gate_paths)
        .map(String::as_str)
        .collect();
    assert!(
        schema_only.is_empty(),
        "schema leaves missing from the gate: {schema_only:?}"
    );
    assert_eq!(
        gate_only,
        vec![
            "records[].assets[].signals.context_cost_est",
            "records[].assets[].signals.tokens_attributed",
            "records[].tokens_by_model[]",
        ],
        "gate/schema leaf-path difference changed"
    );
}

// ---- helpers ---------------------------------------------------------------------------------

/// `extractor_version` is the payload's one free-form string leaf, so it is where the Python tests
/// plant every value-level negative.
fn check_extractor_version(value: &str) -> Vec<String> {
    let mut payload = minimal_valid_payload();
    payload["extractor_version"] = json!(value);
    GATE.check(&payload, &Dynamic::empty())
}

/// Follow `$ref` chains into `#/$defs/`.
fn resolve<'a>(node: &'a Value, defs: &'a Map<String, Value>) -> &'a Value {
    let mut current = node;
    while let Some(reference) = current.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .expect("schema $ref targets #/$defs/");
        current = defs.get(name).expect("schema $ref resolves");
    }
    current
}

/// Derive the schema's leaf paths in the gate's `pathSyntax`: dot-joined keys, array elements as
/// `[]`. A `oneOf` is a nullable wrapper, so the null branch is dropped and the other is walked in
/// place — which is why the schema has no leaf for a nullable object, and the gate has one.
fn schema_leaf_paths<'a>(
    node: &'a Value,
    defs: &'a Map<String, Value>,
    prefix: &str,
    out: &mut BTreeSet<String>,
) {
    let node = resolve(node, defs);
    if let Some(branches) = node.get("oneOf").and_then(Value::as_array) {
        for branch in branches {
            let branch = resolve(branch, defs);
            if branch.get("type").and_then(Value::as_str) == Some("null") {
                continue;
            }
            schema_leaf_paths(branch, defs, prefix, out);
        }
        return;
    }
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        for (key, child) in properties {
            let child_prefix = super::join(prefix, key);
            schema_leaf_paths(child, defs, &child_prefix, out);
        }
        return;
    }
    if let Some(items) = node.get("items") {
        schema_leaf_paths(items, defs, &format!("{prefix}[]"), out);
        return;
    }
    out.insert(prefix.to_string());
}

/// Decimal strings preserve sums of squares above JavaScript's safe integer range. The format is
/// canonical and bounded, so alternate spellings and values above 1e21 are rejected.
#[test]
fn sumsq_decimal_is_exact_canonical_and_bounded() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["assets"][0]["signals"]["latency_ms"]["sumsq"] =
        json!("50000000000000000000");
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert!(
        violations.is_empty(),
        "an in-bounds sumsq above u64::MAX must pass exactly: {violations:?}"
    );

    for bad in [json!("2000000000000000000000"), json!("01"), json!(50)] {
        payload["records"][0]["assets"][0]["signals"]["latency_ms"]["sumsq"] = bad;
        assert!(
            !GATE.check(&payload, &Dynamic::empty()).is_empty(),
            "a noncanonical, out-of-range, or numeric sumsq must fail"
        );
    }
}

/// A float-shaped integer inside `u64` range stays a `type_mismatch`, matching Python, where
/// `isinstance(2.0, int)` is false. The oversized-integer allowance above must not become a general
/// "any whole number is an integer" rule, or the gate would stop distinguishing counts from floats.
#[test]
fn small_float_is_still_not_an_integer() {
    let mut payload = minimal_valid_payload();
    payload["records"][0]["counts"]["turns"] = serde_json::json!(2.0_f64);
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "records[0].counts.turns", "type_mismatch");
}

/// Python's `datetime.date.fromisoformat` has `MINYEAR == 1`, so "0000-01-01" is not a calendar
/// date; chrono's year range includes 0 and would have accepted it. A day path that the cloud
/// stores as a retention key must mean the same thing on both sides of the wire.
#[test]
fn year_zero_is_not_a_calendar_day() {
    for bad in ["0000-01-01", "0000-02-29", "0000-12-31"] {
        let mut payload = minimal_valid_payload();
        payload["emitted_day"] = serde_json::json!(bad);
        let violations = GATE.check(&payload, &Dynamic::empty());
        assert_violation(&violations, "emitted_day", "format_mismatch");
    }
    // Year 1 is the first year Python accepts, so it must pass here too.
    let mut payload = minimal_valid_payload();
    payload["emitted_day"] = serde_json::json!("0001-01-01");
    assert!(GATE.check(&payload, &Dynamic::empty()).is_empty());
}

/// The whitespace rule must fire at the `check` level, not merely in the helper: a free-text leak
/// carrying a field separator is exactly what this pattern exists to stop. Confirmed against the
/// reference, which reports both `format_mismatch` and `pattern:whitespace` for the same value.
#[test]
fn ascii_separator_in_a_free_string_trips_the_whitespace_pattern() {
    let mut payload = minimal_valid_payload();
    payload["extractor_version"] = Value::String(format!("a{}b", '\u{1c}'));
    let violations = GATE.check(&payload, &Dynamic::empty());
    assert_violation(&violations, "extractor_version", "pattern:whitespace");
}
