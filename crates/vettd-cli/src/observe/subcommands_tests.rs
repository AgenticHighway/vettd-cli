//! Tests for [`super`]. The duplicate-key scan gets the most attention because it is the one rule
//! here that exists to catch a deliberately crafted payload rather than a mistake.

use super::*;

/// Invariant: a duplicated key anywhere in the document is caught. `serde_json` keeps the LAST
/// value for a repeated key, so a payload could carry a leak in the first copy and a clean value in
/// the second and still validate cleanly — the gate would inspect only what survived parsing. That
/// is the whole reason this is a read failure rather than a violation.
/// Cannot prove the scan matches a full JSON parser on every pathological input; it is a scanner,
/// and its job is to refuse rather than to interpret.
#[test]
fn a_duplicated_key_is_detected_anywhere_in_the_document() {
    assert_eq!(
        first_duplicate_key(r#"{"a": 1, "a": 2}"#).as_deref(),
        Some("a")
    );
    assert_eq!(
        first_duplicate_key(r#"{"outer": {"leak": "secret", "leak": "clean"}}"#).as_deref(),
        Some("leak")
    );
    assert_eq!(
        first_duplicate_key(r#"{"records": [{"run_id": "a", "run_id": "b"}]}"#).as_deref(),
        Some("run_id")
    );
}

/// Invariant: the same key name in DIFFERENT objects is not a duplicate. Every record in an
/// envelope has a `run_id`; treating that as a duplicate would reject every real payload.
#[test]
fn the_same_key_in_sibling_objects_is_not_a_duplicate() {
    assert_eq!(
        first_duplicate_key(r#"{"records": [{"run_id": "a"}, {"run_id": "b"}]}"#),
        None
    );
    assert_eq!(
        first_duplicate_key(r#"{"a": {"n": 1}, "b": {"n": 2}}"#),
        None
    );
    // A key repeated at different depths of the same chain is also fine.
    assert_eq!(
        first_duplicate_key(r#"{"signals": {"signals": {"n": 1}}}"#),
        None
    );
}

/// Invariant: a string VALUE that looks like a key is not treated as one. A key is a string
/// followed by a colon; without that test, an envelope whose asset name happened to repeat would
/// be rejected as malformed.
#[test]
fn string_values_are_not_mistaken_for_keys() {
    assert_eq!(first_duplicate_key(r#"{"a": "x", "b": "x"}"#), None);
    assert_eq!(first_duplicate_key(r#"{"a": ["k", "k"]}"#), None);
    // A colon inside a value does not make it a key either.
    assert_eq!(
        first_duplicate_key(r#"{"a": "skill:name", "b": "skill:name"}"#),
        None
    );
}

/// Invariant: an escaped quote inside a key or value does not desynchronise the scan. Getting this
/// wrong would make the scanner read a value as a key and reject valid payloads — or, worse, skip
/// past a real duplicate.
#[test]
fn escapes_do_not_desynchronise_the_scan() {
    assert_eq!(first_duplicate_key(r#"{"a\"b": 1, "c": 2}"#), None);
    assert!(
        first_duplicate_key(r#"{"a\"b": 1, "a\"b": 2}"#).is_some(),
        "a duplicate is still caught when the key contains an escape"
    );
    assert_eq!(
        first_duplicate_key(r#"{"path": "C:\\x", "other": "C:\\x"}"#),
        None
    );
}

/// Invariant: duplicate identity is defined by the decoded JSON string, not by its encoded bytes.
/// Otherwise an attacker can put a leak under `"records"`, overwrite it under `"\u0072ecords"`,
/// and rely on serde_json retaining only the second value before the telemetry gate runs.
#[test]
fn equivalent_json_key_escapes_are_duplicates() {
    assert_eq!(
        first_duplicate_key(r#"{"records": {"leak": true}, "\u0072ecords": []}"#).as_deref(),
        Some("records")
    );
    assert_eq!(
        first_duplicate_key(r#"{"a\"b": 1, "a\u0022b": 2}"#).as_deref(),
        Some("a\"b")
    );
}

/// Invariant: a read failure must never turn an existing user-authored config into an empty file.
/// Invalid UTF-8 is a portable way to make `read_to_string` fail while preserving a real file.
#[test]
fn enable_preserves_an_unreadable_existing_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".vettd.toml");
    let original = b"[access]\nmode = \"licensed\"\n\xff";
    std::fs::write(&path, original).expect("write invalid UTF-8 config");

    assert_eq!(enable_at(&path), EXIT_RUNTIME);
    assert_eq!(
        std::fs::read(&path).expect("read preserved config"),
        original,
        "a read error must leave the user-authored file byte-for-byte unchanged"
    );
}

/// Invariant: a clean envelope scans clean. Without this the other assertions could all pass with a
/// scanner that reported a duplicate for everything.
#[test]
fn a_clean_document_reports_nothing() {
    assert_eq!(first_duplicate_key("{}"), None);
    assert_eq!(first_duplicate_key("[]"), None);
    assert_eq!(
        first_duplicate_key(r#"{"envelope_version": "0.1.0", "records": []}"#),
        None
    );
    let golden = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/observe/golden/envelope.json"),
    )
    .expect("golden committed");
    assert_eq!(
        first_duplicate_key(&golden),
        None,
        "the committed golden must scan clean"
    );
}
