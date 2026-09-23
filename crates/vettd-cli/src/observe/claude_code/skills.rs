//! Skills the harness injects in band, and the two-line dance that delivers their bodies.
//!
//! Split out of `apply.rs` for the file-length budget; port of the meta-line branch of
//! `_apply_user` plus `_record_skill_invocation` (`claude_code.py:453-490`).

use serde_json::json;

use super::ReadState;
use crate::observe::claude_code::project::{sha256_json, Digest, ProjectedCommand};
use crate::observe::types::{InBandAsset, InBandKind, SessionFacts, ToolCall};

/// A `<command-name>` line that declared `<skill-format>true</skill-format>` and is waiting for the
/// meta line that carries its body (`claude_code.py:73`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingSkill {
    pub name: String,
    /// The timestamp of the command line. The Python records it and never reads it — the invocation
    /// is stamped with the *body* line's timestamp — and the port keeps that behaviour.
    pub ts_ms: i64,
}

/// The two-line skill dance on meta lines (`claude_code.py:453-459`).
///
/// A `<command-name>` line that declares `<skill-format>true</skill-format>` carries no body: the
/// harness injects it as its own meta line immediately afterwards, so the command parks a
/// [`PendingSkill`] and the *next* meta line's text digest becomes the skill body. A meta line that
/// names a command never consumes a pending one.
pub(crate) fn apply_meta(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    command: Option<ProjectedCommand>,
    meta_text: Option<Digest>,
    ts_ms: i64,
) {
    if let Some(command) = command {
        if command.skill_format {
            state.pending_skill = Some(PendingSkill {
                name: command.name,
                ts_ms,
            });
        } else {
            record_skill_invocation(facts, state, &command.name, &command.digest, ts_ms);
        }
    } else if let Some(digest) = meta_text {
        if let Some(pending) = state.pending_skill.take() {
            record_skill_invocation(facts, state, &pending.name, &digest, ts_ms);
        }
    }
}

/// Record a skill the harness injected in band (`_record_skill_invocation`,
/// `claude_code.py:478-490`).
///
/// The synthetic call is paired with itself and marked async. That is how the shared model spells
/// "there is no latency to measure": the harness injects the body without a tool round-trip, and
/// `extract` nulls the latency of an async call. Its `tool_use_id` is synthetic, so — unlike a real
/// call's — it is deliberately not forbidden: it is not an identifier the harness ever wrote down.
fn record_skill_invocation(
    facts: &mut SessionFacts,
    state: &mut ReadState,
    name: &str,
    digest: &Digest,
    ts_ms: i64,
) {
    facts.in_band_assets.push(InBandAsset {
        kind: InBandKind::SkillBody,
        name: name.to_string(),
        content_sha256: digest.sha256.clone(),
        byte_len: digest.byte_len,
        ts_ms,
    });
    state.synthetic += 1;
    facts.tool_calls.push(ToolCall {
        tool_use_id: format!("synthetic-skill-{}", state.synthetic),
        name: "Skill".to_string(),
        ts_ms,
        result_ts_ms: Some(ts_ms),
        is_error: Some(false),
        is_async: true,
        skill: Some(name.to_string()),
        input_fingerprint: sha256_json(&json!({ "skill": name })),
        ..Default::default()
    });
    facts.note_forbid("loaded_set_names", Some(name));
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
