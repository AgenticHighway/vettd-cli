//! Task-category rule set and model allowlist.
//!
//! Port of `spikes/828-passive-observer/prototype/taskcat.py`.
//!
//! [`categorize`] is the published rule set: a closed category derived from tool-mix shares alone.
//! It reads no content, and the shares it consumes never egress. The boundaries here *are* the
//! published rules — changing any of them is a different rule set and must bump [`RULES_VERSION`],
//! because `extractor_version` carries it and a re-extraction under different rules is a different
//! observation, not a correction of the same one.
//!
//! [`allowlist_model`] mirrors the gate's `enums.model`: anything not on the list — a custom
//! provider name, a build string, a missing value — becomes the literal `"other"`.

use std::collections::BTreeMap;

/// Bumped whenever any boundary or class in this module changes.
pub(crate) const RULES_VERSION: &str = "taskcat-1";

pub(crate) const CATEGORY_UNSPECIFIED: &str = "unspecified";
pub(crate) const CATEGORY_MCP_HEAVY: &str = "mcp_heavy";
pub(crate) const CATEGORY_CODE_EDIT: &str = "code_edit";
pub(crate) const CATEGORY_SHELL_OPS: &str = "shell_ops";
pub(crate) const CATEGORY_CODE_EXPLORE: &str = "code_explore";
pub(crate) const CATEGORY_MIXED: &str = "mixed";

// Published boundaries. Inclusive: a share exactly on the boundary is inside the category.
pub(crate) const MCP_HEAVY_MIN: f64 = 0.5;
pub(crate) const CODE_EDIT_MIN: f64 = 0.25;
pub(crate) const SHELL_OPS_MIN: f64 = 0.5;
pub(crate) const CODE_EXPLORE_MIN: f64 = 0.5;

/// The literal emitted for any model id not on the allowlist.
pub(crate) const MODEL_OTHER: &str = "other";

/// Identical to the gate's `enums.model`, in the same order.
///
/// A closed list rather than a prefix pattern, deliberately: a user-named model such as
/// `claude-<org>-<project>` would satisfy a pattern and carry a private name onto the wire. A new
/// model id reports as `"other"` until this list and the gate are versioned forward together, which
/// `known_models_equal_gate_enum` enforces.
pub(crate) const KNOWN_MODELS: [&str; 23] = [
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-opus-5",
    "claude-sonnet-5",
    "claude-haiku-4-5-20251001",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-1",
    "claude-sonnet-4-5",
    "claude-sonnet-4",
    "gpt-5",
    "gpt-5-mini",
    "gpt-5-codex",
    "gpt-5.1",
    "gpt-5.1-codex",
    "gpt-5.2",
    "gpt-4.1",
    "o3",
    "o4-mini",
    "codex-mini-latest",
    "gemini-2.5-pro",
    "gemini-3-pro",
    "other",
];

/// Tool-class shares (class to fraction of tool calls) to a task category. Pure.
///
/// The rule order *is* the precedence: the first boundary met wins, so a run that is both mcp-heavy
/// and edit-heavy is `mcp_heavy`. A zero total is `unspecified` rather than `mixed`, which is what
/// keeps "nothing to classify" distinguishable from "genuinely mixed".
pub(crate) fn categorize(shares: &BTreeMap<String, f64>) -> &'static str {
    let total: f64 = shares.values().sum();
    if total == 0.0 {
        return CATEGORY_UNSPECIFIED;
    }
    let share = |class: &str| shares.get(class).copied().unwrap_or(0.0);
    if share("mcp") >= MCP_HEAVY_MIN {
        CATEGORY_MCP_HEAVY
    } else if share("edit") >= CODE_EDIT_MIN {
        CATEGORY_CODE_EDIT
    } else if share("shell") >= SHELL_OPS_MIN {
        CATEGORY_SHELL_OPS
    } else if share("read") >= CODE_EXPLORE_MIN {
        CATEGORY_CODE_EXPLORE
    } else {
        CATEGORY_MIXED
    }
}

/// The harness-reported model id if it is on the allowlist, else `"other"`. Pure.
///
/// Deliberately does not normalise: no trimming, no case folding, no repair. The wire format is an
/// exact enum, so an id that needs repairing is one this collector does not recognise, and guessing
/// would be how a private name reaches the gate.
pub(crate) fn allowlist_model(raw: Option<&str>) -> &'static str {
    let Some(raw) = raw else {
        return MODEL_OTHER;
    };
    KNOWN_MODELS
        .iter()
        .find(|known| **known == raw)
        .copied()
        .unwrap_or(MODEL_OTHER)
}

#[cfg(test)]
#[path = "taskcat_tests.rs"]
mod tests;
