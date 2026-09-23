//! Record and asset construction for [`super`] — the per-run half of `aggregate.py`.
//!
//! Split out of `envelope.rs` for the 400-line file budget (CONTRIBUTING.md "File size limits");
//! every item here is `aggregate.py`'s and is used only by [`super::build_envelope`], so the module
//! boundary is a file boundary and carries no meaning of its own.
//!
//! This is where the run-level and asset-level numbers are chosen. Nothing here reads a name: the
//! only local strings it touches are `run.harness` and `run.session_key`, and both go straight into
//! [`super::run_id_for`]'s HMAC preimage.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::super::taskcat;
use super::super::types::{
    AssetObservation, AttributedRun, RunFacts, Stats, TokenTotals, FAILURE_CLASSES, TIER_INFERRED,
};
use super::run_id_for;

/// One record (`aggregate.py:150-172`).
pub(super) fn record(
    attributed: &AttributedRun,
    bom_version: &str,
    secret: &[u8],
) -> Result<Value, String> {
    let run = &attributed.run;
    let mut assets = Vec::new();
    for observation in merged_observations(attributed) {
        assets.push(asset(&observation)?);
    }
    Ok(json!({
        "run_id": run_id_for(secret, &run.harness, &run.session_key),
        "observed_day": run.observed_day,
        "model": run.model,
        "entrypoint_class": run.entrypoint_class,
        "effort": run.effort,
        "permission_mode": run.permission_mode,
        "task_category": taskcat::categorize(&run.tool_class_shares),
        "bom_version": bom_version,
        "loaded_set_basis": attributed.segments[0].loaded_set_basis,
        "run_outcome": run.run_outcome,
        "counts": {
            "turns": run.turns,
            "tool_calls": run.tool_calls,
            "tool_failures": run.tool_failures,
            "user_denials": run.user_denials,
            "subagent_runs": run.subagent_runs,
            "compactions": run.compactions,
            "unpaired_tool_uses": run.unpaired_tool_uses,
            "repeated_tool_calls": run.repeated_tool_calls,
            "loaded_set_changes": attributed.segments.len() - 1,
        },
        "tokens": tokens(&run.tokens, Some(&run.tokens_basis)),
        "tokens_by_model": tokens_by_model(run),
        "assets": assets,
    }))
}

/// Observations of the same asset across segments merge into one (`aggregate.py:121-147`):
/// invocations concatenated, direct evidence OR-ed, the first context-cost estimate kept,
/// corroborations summed when any side has them, and a tier disagreement demoted to `inferred` —
/// never promoted. Sorted by `asset_id`, which is the order the envelope requires.
fn merged_observations(attributed: &AttributedRun) -> Vec<AssetObservation> {
    let mut merged: BTreeMap<String, AssetObservation> = BTreeMap::new();
    for segment in &attributed.segments {
        let Some(observations) = attributed.observations.get(&segment.index) else {
            continue;
        };
        for obs in observations {
            let Some(current) = merged.get_mut(&obs.key.asset_id) else {
                merged.insert(obs.key.asset_id.clone(), obs.clone());
                continue;
            };
            current.invocations.extend(obs.invocations.iter().cloned());
            current.direct_evidence_available |= obs.direct_evidence_available;
            if current.context_cost_est.is_none() {
                current.context_cost_est = obs.context_cost_est.clone();
            }
            if let Some(extra) = obs.harness_corroborations {
                // Saturating: a bare `+` wraps in release (no `overflow-checks` in the release
                // profile) and would put a small wrong count on the wire. `count` is bounded at
                // 1e7 by the gate, so a saturated value is refused rather than believed.
                current.harness_corroborations = Some(
                    current
                        .harness_corroborations
                        .unwrap_or(0)
                        .saturating_add(extra),
                );
            }
            if current.tier != obs.tier {
                current.tier = TIER_INFERRED.to_string();
            }
        }
    }
    merged.into_values().collect()
}

/// One entry per model id, sorted (`aggregate.py:175-186`). When the run recorded totals but no
/// per-model split, the totals are attributed to the run's own model so the per-model view is never
/// silently empty.
///
/// **Precondition, and it matters:** the keys of `run.tokens_by_model` are already allowlisted
/// model ids — `extract_tally::sum_tokens_by_model` groups by [`taskcat::allowlist_model`] before
/// summing, and `run.model` is allowlisted at `extract.rs:123`. That is what makes this array
/// sorted and unique *in the key it emits*. Were a raw local model id ever to reach here, the
/// array's length would count the machine's non-allowlisted models and its order would encode
/// their lexicographic rank — a channel the field gate cannot see, because the gate inspects
/// values and this would be in the ordering. `extract_tokens_by_model_keys_are_always_allowlisted`
/// pins the precondition upstream.
fn tokens_by_model(run: &RunFacts) -> Vec<Value> {
    let mut by_model: BTreeMap<&str, &TokenTotals> = run
        .tokens_by_model
        .iter()
        .map(|(model, totals)| (model.as_str(), totals))
        .collect();
    if by_model.is_empty() && run.tokens_basis != "none" {
        by_model.insert(run.model.as_str(), &run.tokens);
    }
    by_model
        .into_iter()
        .map(|(model, totals)| {
            let mut entry = Map::new();
            entry.insert("model".into(), json!(taskcat::allowlist_model(Some(model))));
            if let Value::Object(buckets) = tokens(totals, None) {
                entry.extend(buckets);
            }
            Value::Object(entry)
        })
        .collect()
}

/// The seven token buckets, plus `basis` when one is given (`aggregate.py:189-201`).
///
/// A nullable bucket stays `null` when the provider never reported it — "absent" is not "zero", or
/// the cloud would average a cache-read rate over providers that have no such bucket. `input` and
/// `output` are the two buckets that are never null on the wire and default to 0.
fn tokens(totals: &TokenTotals, basis: Option<&str>) -> Value {
    let mut out = Map::new();
    out.insert("input".into(), json!(totals.input.unwrap_or(0)));
    out.insert("output".into(), json!(totals.output.unwrap_or(0)));
    out.insert("cache_creation".into(), json!(totals.cache_creation));
    out.insert("cache_read".into(), json!(totals.cache_read));
    out.insert("cached_input".into(), json!(totals.cached_input));
    out.insert("thinking".into(), json!(totals.thinking));
    out.insert("reasoning".into(), json!(totals.reasoning));
    if let Some(basis) = basis {
        out.insert("basis".into(), json!(basis));
    }
    Value::Object(out)
}

/// One asset row and its signals (`aggregate.py:204-227`).
fn asset(obs: &AssetObservation) -> Result<Value, String> {
    let mut failures: BTreeMap<&str, u64> = FAILURE_CLASSES.iter().map(|c| (*c, 0)).collect();
    for class in obs
        .invocations
        .iter()
        .filter_map(|i| i.failure_class.as_ref())
    {
        let bucket = if failures.contains_key(class.as_str()) {
            class.as_str()
        } else {
            FAILURE_CLASSES[4]
        };
        *failures.get_mut(bucket).expect("bucket exists") += 1;
    }
    let latency: Vec<i64> = obs
        .invocations
        .iter()
        .filter_map(|i| i.latency_ms)
        .collect();
    let attributed: Vec<i64> = obs
        .invocations
        .iter()
        .filter_map(|i| i.child_tokens_total)
        .collect();
    Ok(json!({
        "asset_id": obs.key.asset_id,
        "asset_type": obs.key.asset_type,
        "key_basis": obs.key.key_basis,
        "tier": obs.tier,
        "binding": obs.key.binding,
        "direct_evidence_available": obs.direct_evidence_available,
        "signals": {
            "invocations": {"n": obs.invocations.len()},
            "failures": failures,
            "harness_corroborations": corroborations(obs),
            "latency_ms": stats(&Stats::from_values(&latency).unwrap_or_default(), LATENCY_SUMSQ)?,
            "tokens_attributed": match Stats::from_values(&attributed) {
                Some(summary) => stats(&summary, ATTRIBUTED_SUMSQ)?,
                None => Value::Null,
            },
            "context_cost_est": obs.context_cost_est.as_ref().map(|cost| {
                json!({"tokens": cost.tokens, "method": cost.method})
            }),
        },
    }))
}

/// The attributor's explicit count wins; otherwise the invocations' own markers are counted, and
/// the result is `null` (not `0`) when no marker was seen, because the harness may simply emit
/// none (`aggregate.py:230-236`).
fn corroborations(obs: &AssetObservation) -> Value {
    if let Some(count) = obs.harness_corroborations {
        return json!(count);
    }
    match obs.invocations.iter().filter(|i| i.corroborated).count() {
        0 => Value::Null,
        marked => json!(marked),
    }
}

/// Gate path of the two `sumsq` leaves, used only in the error message.
const LATENCY_SUMSQ: &str = "records[].assets[].signals.latency_ms.sumsq";
/// See [`LATENCY_SUMSQ`].
const ATTRIBUTED_SUMSQ: &str = "records[].assets[].signals.tokens_attributed.sumsq";

/// A `{n, sum, min, max, sumsq}` object, or an error naming `field` when `sumsq` is too large.
fn stats(summary: &Stats, field: &str) -> Result<Value, String> {
    Ok(json!({
        "n": summary.n,
        "sum": summary.sum,
        "min": summary.min,
        "max": summary.max,
        "sumsq": stats_sumsq(summary.sumsq, field)?,
    }))
}

/// Render the bounded `u128` as a decimal string so every JSON consumer receives it exactly.
fn stats_sumsq(sumsq: u128, field: &str) -> Result<String, String> {
    const MAX_SUMSQ: u128 = 1_000_000_000_000_000_000_000;
    if sumsq > MAX_SUMSQ {
        return Err(format!(
            "{field}: sum of squares {sumsq} exceeds the envelope maximum {MAX_SUMSQ}"
        ));
    }
    Ok(sumsq.to_string())
}
