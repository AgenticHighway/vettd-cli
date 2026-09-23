//! Contract parity — what the CLI emits must be inside the declared surface.
//!
//! The manifest checks (scripts/check-scanner-field-gate.sh) prove the
//! *declared* surface fields are actually mapped into the contract. This
//! module proves the dual: every object key the CLI *actually serializes* is
//! declared in `scanner-data-contract.json` at the location it is emitted.
//!
//! The test serializes the maximally-populated payload (the same fixture the
//! disclosure tests use — see `max_payload` in disclosure.rs) and walks the
//! serialized [`serde_json::Value`] against the real contract schema. A new
//! serialized field with no contract property fails the test — no grep-based
//! check can satisfy this, because the walk runs on the emitted bytes, not on
//! source text.

use super::disclosure::max_payload;

/// The repo-root `scanner-data-contract.json`, embedded at compile time so the
/// test always walks the real schema (no runtime file lookup, no stale copy).
fn contract_schema() -> serde_json::Value {
    let json = include_str!("../../../../scanner-data-contract.json");
    let schema: serde_json::Value =
        serde_json::from_str(json).expect("scanner-data-contract.json must be valid JSON");
    schema
}

/// Recursively assert that every object key in the serialized `value` is
/// declared by the JSON-schema node `schema` at the location `path`.
///
/// - Object values require a `properties` map naming every emitted key.
/// - Array values recurse through `items`.
/// - Free-form nodes (`rawReport`, signal `payload`) declare no properties —
///   their opaque contents are skipped, mirroring the disclosure walker.
/// - `oneOf` switches (the contract's `detectedSource`) accept a value whose
///   keys are declared by at least one branch.
fn assert_keys_declared(value: &serde_json::Value, schema: &serde_json::Value, path: &str) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(choices) = schema.get("oneOf").and_then(|v| v.as_array()) {
                let covered = choices.iter().any(|choice| {
                    choice
                        .get("properties")
                        .and_then(|p| p.as_object())
                        .is_some_and(|props| map.keys().all(|k| props.get(k).is_some()))
                });
                if !covered {
                    panic!(
                        "contract parity: object keys at '{path}' are not declared by any \
                         oneOf branch of scanner-data-contract.json"
                    );
                }
                return;
            }
            let props = schema.get("properties").and_then(|p| p.as_object());
            if props.is_none() {
                // Free-form blob (e.g. rawReport / signal payload): opaque
                // contents are transmitted as-is; nothing to check inside.
                return;
            }
            for (key, child) in map {
                let child_schema = props.unwrap().get(key);
                if child_schema.is_none() {
                    panic!(
                        "contract parity: serialized key '{key}' at '{path}' is NOT declared \
                         in scanner-data-contract.json — the emitted payload escaped the \
                         declared surface. Declare it in the contract schema or stop \
                         emitting it."
                    );
                }
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                assert_keys_declared(child, child_schema.unwrap(), &child_path);
            }
        }
        serde_json::Value::Array(items) => {
            let item_schema = schema.get("items");
            if item_schema.is_some() {
                let arr_path = format!("{path}[]");
                for item in items {
                    assert_keys_declared(item, item_schema.unwrap(), &arr_path);
                }
            }
        }
        _ => {}
    }
}

/// Every object key emitted by the maximally-populated payload must be
/// declared in the contract schema at the location it is emitted. This is the
/// compiled parity guarantee: a serialized field with no contract property
/// (or a contractPath pointing at the wrong parent) fails the test.
#[test]
fn every_serialized_key_is_declared_in_contract_schema() {
    let payload = max_payload();
    let value = serde_json::to_value(&payload).unwrap();
    assert_keys_declared(&value, &contract_schema(), "");
}

/// The walker must reject a payload key the schema does not declare — proves
/// the parity check is a real structural walk, not a template that silently
/// ignores unknown keys.
#[test]
fn undeclared_serialized_key_is_rejected() {
    let mut value = serde_json::to_value(max_payload()).unwrap();
    // Inject an unknown key into skills[0]; the walker must reject it.
    value["skills"][0]["mysteryField"] = serde_json::json!("x");
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_keys_declared(&value, &contract_schema(), "");
    }));
    assert!(
        res.is_err(),
        "an undeclared serialized key must fail the contract parity walk"
    );
}

/// Removing a declared property from the contract schema must fail the walk
/// on the emitted key — this is the exact regression the parity test guards:
/// a serialized key with no contract property at its location.
#[test]
fn removing_a_contract_property_fails_parity() {
    let mut schema = contract_schema();
    schema["properties"]["skills"]["items"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("hasAssets");
    let value = serde_json::to_value(max_payload()).unwrap();
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_keys_declared(&value, &schema, "");
    }));
    assert!(
        res.is_err(),
        "removing a declared contract property must fail the parity walk"
    );
}
