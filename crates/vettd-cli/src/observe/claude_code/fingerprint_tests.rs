//! Tests for `fingerprint.rs`.
//!
//! The expected digests are the prototype's own output, taken from
//! `spikes/828-passive-observer/prototype` with
//! `python3 -c "import sys; sys.path.insert(0,'.'); from sources.claude_code import _sha256_json as h; print(h(VALUE))"`,
//! so a drift in either implementation shows up here.

use super::*;
use serde_json::json;

/// The fingerprint preimage is CPython's `json.dumps(sort_keys, ensure_ascii, tight separators)`.
/// The four expected digests come from the prototype itself; they are what makes "the same input
/// twice" mean the same thing in the port as in the reference the goldens were generated from.
#[test]
fn sha256_json_matches_the_prototype_digests() {
    assert_eq!(
        sha256_json(&Value::Null),
        "74234e98afe7498fb5daf1f36ac2d78acc339464f950703b8c019892f982b90b"
    );
    assert_eq!(
        sha256_json(&json!({})),
        "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    );
    assert_eq!(
        sha256_json(&json!({"skill": "skill-alpha"})),
        "42c1d09815d38b082e242870f50a8d2b2ef6922f291dd6239e0dff3ce3a156aa"
    );
    assert_eq!(
        sha256_json(&json!({"b": 1.5, "a": "é😀", "z": [1, {"k": null}], "t": true})),
        "ca10fbdeafa6e135716f8e1440116b0703d0ef0689e2075193baa3d1b21484f0"
    );
}

/// The preimage is ASCII, key-sorted and tightly separated — the three properties that make two
/// structurally equal inputs hash equal regardless of how the harness spelled them, and that keep
/// the digest reproducible on a machine with a different locale or hash seed.
#[test]
fn the_fingerprint_preimage_is_ascii_and_key_sorted() {
    let mut rendered = String::new();
    write_fingerprint(
        &json!({"z": 1, "a": "é😀\u{1}", "m": [true, null], "\"q\\": "\n\t"}),
        &mut rendered,
    );
    assert_eq!(
        rendered,
        r#"{"\"q\\":"\n\t","a":"\u00e9\ud83d\ude00\u0001","m":[true,null],"z":1}"#
    );
    assert!(rendered.is_ascii());
}

/// A float reaches the preimage rather than erroring, unlike the egress canonicaliser. Tool inputs
/// are arbitrary harness JSON and do carry floats; refusing one here would mean no fingerprint at
/// all for the call, and `canonical_json`'s float refusal must not be relaxed to accommodate that.
#[test]
fn a_float_input_is_fingerprinted_where_canonical_json_would_refuse() {
    let value = json!({"temperature": 0.7});
    let mut rendered = String::new();
    write_fingerprint(&value, &mut rendered);
    assert_eq!(rendered, r#"{"temperature":0.7}"#);
    assert_eq!(sha256_json(&value).len(), 64);
    assert!(crate::observe::canonical::canonical_json(&value).is_err());
}
