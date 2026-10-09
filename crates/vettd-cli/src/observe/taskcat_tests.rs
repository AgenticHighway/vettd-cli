//! Tests for [`super`], ported from
//! `spikes/828-passive-observer/prototype/tests/test_taskcat.py`.
//!
//! Every share and model id below is invented. None of these can prove the published boundaries are
//! the *right* boundaries; they prove the code implements the boundaries the contract publishes
//! under `RULES_VERSION`, inclusively, in the stated precedence.

use super::*;
use serde_json::Value;

const GATE_JSON: &str = include_str!("../../../../telemetry-field-gate.json");

fn shares(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs
        .iter()
        .map(|(class, share)| ((*class).to_string(), *share))
        .collect()
}

/// The boundary constants are the ones published as `taskcat-1`. If a boundary moves this fails,
/// which is the point: a changed rule set must ship under a new `RULES_VERSION`, because
/// `extractor_version` carries it and the cloud uses it to decide whether two observations are
/// comparable. Cannot prove that callers actually embed `RULES_VERSION` in `extractor_version`.
#[test]
fn rules_version_pins_the_boundaries() {
    assert_eq!(RULES_VERSION, "taskcat-1");
    assert_eq!(
        (
            MCP_HEAVY_MIN,
            CODE_EDIT_MIN,
            SHELL_OPS_MIN,
            CODE_EXPLORE_MIN
        ),
        (0.5, 0.25, 0.5, 0.5)
    );
}

/// No tool calls — an empty share map or an all-zero one — is `unspecified`, never `mixed`, so a
/// run with nothing to classify stays distinguishable from a genuinely mixed one. Collapsing the
/// two would let empty runs dilute a category's statistics. Cannot prove how `extract` builds the
/// shares for an empty run.
#[test]
fn total_zero_is_unspecified() {
    assert_eq!(categorize(&shares(&[])), CATEGORY_UNSPECIFIED);
    assert_eq!(
        categorize(&shares(&[
            ("edit", 0.0),
            ("read", 0.0),
            ("shell", 0.0),
            ("mcp", 0.0),
            ("other", 0.0),
        ])),
        CATEGORY_UNSPECIFIED
    );
}

/// The mcp boundary is inclusive: exactly 0.5 is `mcp_heavy`. An exclusive comparison would put a
/// run with half its calls in MCP into a different category than the contract promises.
#[test]
fn mcp_boundary_is_inclusive_at_half() {
    assert_eq!(
        categorize(&shares(&[("mcp", 0.5), ("other", 0.5)])),
        CATEGORY_MCP_HEAVY
    );
    assert_ne!(
        categorize(&shares(&[("mcp", 0.49), ("other", 0.51)])),
        CATEGORY_MCP_HEAVY
    );
}

/// The edit boundary is inclusive at a quarter, and just below it — with no other rule met — the
/// run is `mixed`. Cannot prove 0.25 is the right threshold for real tool mixes.
#[test]
fn edit_boundary_is_inclusive_at_quarter() {
    assert_eq!(
        categorize(&shares(&[("edit", 0.25), ("other", 0.75)])),
        CATEGORY_CODE_EDIT
    );
    assert_eq!(
        categorize(&shares(&[("edit", 0.24), ("other", 0.76)])),
        CATEGORY_MIXED
    );
}

/// Shell and read are inclusive at half and fall to `mixed` just below when nothing else applies.
/// Cannot prove anything about classes the rule set does not name.
#[test]
fn shell_and_read_boundaries_inclusive_at_half() {
    assert_eq!(
        categorize(&shares(&[("shell", 0.5), ("other", 0.5)])),
        CATEGORY_SHELL_OPS
    );
    assert_eq!(
        categorize(&shares(&[("shell", 0.49), ("other", 0.51)])),
        CATEGORY_MIXED
    );
    assert_eq!(
        categorize(&shares(&[("read", 0.5), ("other", 0.5)])),
        CATEGORY_CODE_EXPLORE
    );
    assert_eq!(
        categorize(&shares(&[("read", 0.49), ("other", 0.51)])),
        CATEGORY_MIXED
    );
}

/// When several boundaries are met the earlier rule wins (mcp, then edit, then shell, then read),
/// so the category is a deterministic function of the shares rather than of map iteration order.
/// Cannot prove that this precedence matches what a person would call the task.
#[test]
fn precedence_is_mcp_then_edit_then_shell_then_read() {
    assert_eq!(
        categorize(&shares(&[("mcp", 0.5), ("edit", 0.5)])),
        CATEGORY_MCP_HEAVY
    );
    assert_eq!(
        categorize(&shares(&[("edit", 0.25), ("shell", 0.75)])),
        CATEGORY_CODE_EDIT
    );
    assert_eq!(
        categorize(&shares(&[("edit", 0.25), ("read", 0.75)])),
        CATEGORY_CODE_EDIT
    );
    assert_eq!(
        categorize(&shares(&[("shell", 0.5), ("read", 0.5)])),
        CATEGORY_SHELL_OPS
    );
}

/// Shares computed the way `extract` computes them — a count over a total — land exactly on the
/// published boundaries for 1/2 and 1/4, so the inclusive comparison is not defeated by float
/// representation. Cannot prove exactness for non-dyadic ratios, which are never boundaries.
#[test]
fn shares_from_integer_ratios_hit_boundaries_exactly() {
    assert_eq!(
        categorize(&shares(&[("mcp", 2.0 / 4.0), ("other", 2.0 / 4.0)])),
        CATEGORY_MCP_HEAVY
    );
    assert_eq!(
        categorize(&shares(&[("edit", 1.0 / 4.0), ("other", 3.0 / 4.0)])),
        CATEGORY_CODE_EDIT
    );
}

/// `KNOWN_MODELS` is identical to the gate's `enums.model`, in the same order, so the extractor
/// cannot emit a model id the gate would reject — and a gate change without a matching change here
/// fails, rather than shipping an envelope that refuses itself at emission time.
/// Cannot prove the gate's list is the intended allowlist.
#[test]
fn known_models_equal_gate_enum() {
    let doc: Value = serde_json::from_str(GATE_JSON).expect("gate JSON parses");
    let gate_models: Vec<&str> = doc["enums"]["model"]
        .as_array()
        .expect("enums.model is a list")
        .iter()
        .map(|m| m.as_str().expect("each model is a string"))
        .collect();
    assert_eq!(KNOWN_MODELS.to_vec(), gate_models);
}

/// An id from each allowlisted family comes back verbatim, so the payload keeps a model id usable
/// for cost rendering. Cannot prove every real provider id fits these families.
#[test]
fn allowlisted_families_pass_through_unchanged() {
    for raw in [
        "claude-sonnet-5",
        "gpt-4.1",
        "o3",
        "codex-mini-latest",
        "gemini-2.5-pro",
        "other",
    ] {
        assert_eq!(allowlist_model(Some(raw)), raw, "{raw}");
    }
}

/// An off-allowlist provider name becomes the literal `"other"` rather than egressing. This is the
/// rule that stops a locally-named model from carrying a private string onto the wire.
/// Cannot prove `"other"` is never mistaken for a real model downstream.
#[test]
fn invented_provider_name_becomes_other() {
    assert_eq!(allowlist_model(Some("fxprovider-custom-9")), MODEL_OTHER);
    assert_eq!(allowlist_model(Some("unknown")), MODEL_OTHER);
}

/// Uppercase, surrounding whitespace, a trailing newline, the empty string and a missing value all
/// map to `"other"`: the function matches the wire format exactly and never repairs its input.
/// Repairing would be how a near-miss name reaches the gate. Cannot prove a harness never reports a
/// model id in a case the allowlist rejects — if one does, it reports as `"other"`, which is the
/// intended failure.
#[test]
fn nothing_is_normalised_and_a_missing_id_is_other() {
    for raw in ["Claude-X", " claude-x", "claude-x\n", ""] {
        assert_eq!(allowlist_model(Some(raw)), MODEL_OTHER, "{raw:?}");
    }
    assert_eq!(allowlist_model(None), MODEL_OTHER);
}
