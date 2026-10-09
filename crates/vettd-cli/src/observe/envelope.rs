//! Wire envelope assembly — the only module in `observe/` that produces egress.
//!
//! Port of `spikes/828-passive-observer/prototype/aggregate.py`. It turns [`AttributedRun`]s into
//! the payload `telemetry-envelope.schema.json` describes — one record per run — and nothing else:
//! every key written here is a gate field, every number is an integer, every string is an enum
//! value, a hex64 hash, a UTC day or a version. Names and harness session keys enter only as HMAC
//! preimages.
//!
//! **Determinism (D3).** Same runs + same secret + same `today` → byte-identical
//! [`crate::observe::canonical::to_json_bytes`] output. Records are sorted by
//! `(observed_day, run_id)`, assets by `asset_id`, `bom` by `bom_version`, and the encoding is
//! canonical (sorted keys, no whitespace, ASCII), so neither input order nor wall-clock survives.
//!
//! The per-run half — records, assets, token buckets and stats objects — lives in the sibling
//! `envelope_record.rs`; the split is the 400-line file budget and nothing else.
//!
//! [`collect_dynamic`] is the local-only counterpart: it gathers every session-derived string the
//! runs carried so [`crate::observe::gate::Gate::check`] can prove none of them is a substring of
//! any string leaf of the payload.
//!
//! # Exact sums of squares
//!
//! [`Stats::sumsq`](crate::observe::types::Stats::sumsq) is `u128`, while JSON numbers are not
//! reliably exact in JavaScript above 2^53. Envelope v0.2 encodes the bounded value as a canonical
//! decimal string, preserving all digits through validation, persistence, and later aggregation.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::observe::canonical::{hex_sha256, hmac_sha256_hex};
use crate::observe::types::{AttributedRun, UNKNOWN};

#[path = "envelope_record.rs"]
mod envelope_record;

use envelope_record::record;

/// Wire format version, cross-checked against `telemetry-field-gate.json` and
/// `telemetry-envelope.schema.json` by `scripts/check-telemetry-field-gate.sh`.
pub(crate) const ENVELOPE_VERSION: &str = "0.2.0";

/// Which gate ruleset this emitter was written against (`telemetry-field-gate.json.gateVersion`).
pub(crate) const GATE_VERSION: i64 = 1;

/// Extractor identity: the projection version, then the task-category rule-set version, because a
/// re-extraction under a different rule set is a different observation (D2).
///
/// It carries **no letters at all**, and that is not an accident of drafting. Every free-string
/// leaf on the wire is a dynamic-forbid collision surface: `collect_dynamic` feeds the gate every
/// local asset name — installed, not merely invoked — and a *substring* hit anywhere in the payload
/// is a violation that refuses the whole payload. A value like `"vettd-cli-0.9.3"` would be blocked
/// on any machine with a skill named `vettd`, so the collector's own name and version live in
/// `resource.collector` / `resource.collector_version` and never here.
///
/// **Divergence from `docs/vettd-observe-port-plan.md`, deliberate.** The plan specifies
/// `"1+taskcat-1"`, on the stated rationale of "deliberately minimal alphabetic content". That
/// value fails its own test: it contains `cat`, `ask`, `task` and `taskcat`, every one of them a
/// plausible skill, agent or MCP server name, and any of them would permanently refuse all
/// telemetry from that machine on a leaf that carries no local data whatsoever. The digits keep the
/// plan's intent — the leading `1` is the projection version and the trailing digit is
/// [`taskcat::RULES_VERSION`]'s ordinal, so a rule-set change still yields a different
/// `extractor_version` and a re-extraction is still a distinguishable observation (D2).
/// `extractor_version_tracks_the_taskcat_rules_version` keeps the two from drifting.
///
/// This shrinks the surface; it does not remove it. `resource.harness_version` defaults to
/// `"unknown"`, whose substrings include `now`, `own` and `know`, and those are on the same
/// fail-closed footing. Fixing that needs a gate change — exempting producer-controlled leaves from
/// the dynamic rule the way `gate.rs` already exempts enum leaves — which is a contract decision,
/// not one to take here.
pub(crate) const EXTRACTOR_VERSION: &str = "1+1";

/// A harness version reduced to the gate's `semver_or_unknown` format.
///
/// The reduction is the prototype's (`observe.py:39-45`): a plain `MAJOR.MINOR.PATCH` passes
/// through, and anything else — a prerelease, build metadata, a two-part version, a `v` prefix —
/// becomes `"unknown"` entirely rather than being truncated to its numeric core.
/// The gate enforces the same shape (`^(?:[0-9]+\.[0-9]+\.[0-9]+|unknown)$`),
/// but the value is the one wire leaf parsed straight out of a local transcript, so the emitter must
/// not simply forward it. Two reasons. A build or prerelease suffix can carry a hostname or a commit
/// — the gate's own stated rationale for dropping them. And a harness that reports `"2.0.14-rc.1"`
/// or `"1.0"` would otherwise fail the gate and refuse *all* telemetry from that machine, turning a
/// cosmetic version string into a total outage.
fn semver_or_unknown(raw: &str) -> String {
    let value = raw.trim();
    let mut parts = value.split('.');
    let numeric = |part: Option<&str>| {
        // ASCII digits only. The prototype's `\d` also matches Unicode decimals, which would
        // produce a value the gate's `[0-9]` then rejects — a latent bug there, not one to port.
        part.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    };
    let plain = numeric(parts.next())
        && numeric(parts.next())
        && numeric(parts.next())
        && parts.next().is_none();
    if plain {
        value.to_string()
    } else {
        UNKNOWN.to_string()
    }
}

/// The dynamic set every asset and invocation name is merged into (`aggregate.py:41`).
const DYNAMIC_NAMES_SET: &str = "loaded_set_names";

/// The collector and machine identity block (`aggregate.py:35`, `RESOURCE_KEYS`).
///
/// **Divergence from the Python, deliberate:** `build_envelope` there copies six named keys out of
/// a caller-supplied dict so a stray extra key cannot egress. A struct makes the extra key
/// unrepresentable, which is the same guarantee checked by the compiler instead of at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resource {
    pub device_id: String,
    pub device_id_source: String,
    pub harness: String,
    pub harness_version: String,
    pub collector: String,
    pub collector_version: String,
}

/// What the collector saw while reading, as `aggregate.py:36-37` (`COVERAGE_INT_KEYS`) plus
/// `cursor_state`. See [`Resource`] for why this is a struct and not a map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Coverage {
    pub sessions_seen: u64,
    pub sessions_emitted: u64,
    pub sessions_skipped_unparseable: u64,
    pub lines_seen: u64,
    pub lines_unknown_type: u64,
    pub bytes_read: u64,
    pub truncated_sessions: u64,
    pub window_days: u64,
    /// Closed enum: `"fresh"` or `"resumed"`.
    pub cursor_state: String,
}

/// Everything [`build_envelope`] needs that is not derived from the runs themselves.
///
/// `secret` never egresses: it is only ever an HMAC key. `extractor_version` is a parameter rather
/// than [`EXTRACTOR_VERSION`] outright so the prototype's own `proto-0.1.0+taskcat-1` can be
/// reproduced byte for byte by the golden-parity test.
#[derive(Debug, Clone)]
pub(crate) struct EnvelopeMeta<'a> {
    pub resource: Resource,
    pub coverage: Coverage,
    /// The emission day, `YYYY-MM-DD` UTC.
    pub today: String,
    pub secret: &'a [u8],
    /// Closed enum: `"device_secret"` or `"test_secret"`.
    pub run_id_basis: String,
    pub extractor_version: String,
}

/// `HMAC-SHA256(secret, "{harness}:{session_key}")` (`aggregate.py:78-85`).
///
/// Deterministic locally, so re-extraction is idempotent and the cloud can use it as an
/// idempotency key; unlinkable remotely, because the secret never egresses (D2).
pub(crate) fn run_id_for(secret: &[u8], harness: &str, session_key: &str) -> String {
    hmac_sha256_hex(secret, &format!("{harness}:{session_key}"))
}

/// `sha256` over the sorted, de-duplicated asset ids of a loaded set (`aggregate.py:88-90`).
pub(crate) fn bom_version_for(asset_ids: &BTreeSet<String>) -> String {
    let joined: Vec<&str> = asset_ids.iter().map(String::as_str).collect();
    hex_sha256(joined.join(",").as_bytes())
}

/// Build the envelope: one record per run, every segment's loaded set in `bom[]`.
///
/// The record carries the *session-start* loaded set as `bom_version` and the number of settled
/// changes as `counts.loaded_set_changes`, and its assets are merged across segments, so run-level
/// tokens and counts appear exactly once (`aggregate.py:93-118`). A run with no segments is
/// skipped: it has no loaded set to describe.
///
/// `Err` only ever names an out-of-contract `sumsq`; see the module docs.
pub(crate) fn build_envelope(runs: &[AttributedRun], meta: &EnvelopeMeta) -> Result<Value, String> {
    let mut records: Vec<Value> = Vec::new();
    let mut bom: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for attributed in runs {
        if attributed.segments.is_empty() {
            continue;
        }
        let mut versions: Vec<String> = Vec::new();
        for segment in &attributed.segments {
            let ids: BTreeSet<String> = segment
                .asset_keys
                .iter()
                .map(|key| key.asset_id.clone())
                .collect();
            let version = if segment.bom_version.is_empty() {
                bom_version_for(&ids)
            } else {
                segment.bom_version.clone()
            };
            bom.entry(version.clone())
                .or_insert_with(|| ids.into_iter().collect());
            versions.push(version);
        }
        records.push(record(attributed, &versions[0], meta.secret)?);
    }
    records.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    Ok(json!({
        "envelope_version": ENVELOPE_VERSION,
        "extractor_version": meta.extractor_version,
        "gate_version": GATE_VERSION,
        "emitted_day": meta.today,
        "resource": resource(&meta.resource),
        "records": records,
        "bom": bom_list(&bom),
        "coverage": coverage(&meta.coverage, &meta.run_id_basis),
    }))
}

/// `(observed_day, run_id)` — the record ordering that keeps file order and wall-clock out of the
/// payload. Both keys are always present because [`record`] writes them.
fn sort_key(value: &Value) -> (&str, &str) {
    (
        value["observed_day"].as_str().unwrap_or_default(),
        value["run_id"].as_str().unwrap_or_default(),
    )
}

fn resource(resource: &Resource) -> Value {
    json!({
        "device_id": resource.device_id,
        "device_id_source": resource.device_id_source,
        "harness": resource.harness,
        "harness_version": semver_or_unknown(&resource.harness_version),
        "collector": resource.collector,
        "collector_version": resource.collector_version,
    })
}

fn coverage(coverage: &Coverage, run_id_basis: &str) -> Value {
    json!({
        "sessions_seen": coverage.sessions_seen,
        "sessions_emitted": coverage.sessions_emitted,
        "sessions_skipped_unparseable": coverage.sessions_skipped_unparseable,
        "lines_seen": coverage.lines_seen,
        "lines_unknown_type": coverage.lines_unknown_type,
        "bytes_read": coverage.bytes_read,
        "truncated_sessions": coverage.truncated_sessions,
        "window_days": coverage.window_days,
        "cursor_state": coverage.cursor_state,
        "run_id_basis": run_id_basis,
    })
}

fn bom_list(bom: &BTreeMap<String, Vec<String>>) -> Vec<Value> {
    bom.iter()
        .map(|(version, ids)| json!({"bom_version": version, "asset_ids": ids}))
        .collect()
}

/// Merged local-only forbid sets for the gate checker (`aggregate.py:250-269`).
///
/// Every source bucket is carried over and `loaded_set_names` receives every asset and invocation
/// name the runs mention, in both the `"<asset_type>:<name>"` display form and the bare form. The
/// inputs are never mutated.
///
/// **Second line of defence:** a bucket whose name starts with `_` is dropped. The prototype leaks
/// a `_permission_modes` bucket here (`claude_code.py:376-382`), whose members are closed enum
/// values that appear on the wire by design — forbidding them would make every payload unsendable.
/// Phases 2-3 already keep that bucket out of `SessionFacts::forbids`; this drop means a future
/// source cannot reintroduce the class of bug by writing an underscore bucket of its own.
pub(crate) fn collect_dynamic(runs: &[AttributedRun]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    for attributed in runs {
        for (bucket, values) in &attributed.run.forbids {
            if bucket.starts_with('_') {
                continue;
            }
            let target = out.entry(bucket.clone()).or_default();
            target.extend(values.iter().filter(|v| !v.is_empty()).cloned());
        }
        for display in attributed.name_map.values() {
            names.insert(display.clone());
            names.insert(bare_name(display).to_string());
        }
        for segment in &attributed.segments {
            names.extend(segment.asset_keys.iter().map(|key| key.name.clone()));
        }
        for observations in attributed.observations.values() {
            for obs in observations {
                names.insert(obs.key.name.clone());
                names.extend(obs.invocations.iter().map(|inv| inv.name.clone()));
            }
        }
    }
    names.remove("");
    out.entry(DYNAMIC_NAMES_SET.to_string())
        .or_default()
        .extend(names);
    out
}

/// `name_map` values are `"<asset_type>:<name>"`; the bare name is the stronger forbid needle
/// (`aggregate.py:245-247`).
fn bare_name(display: &str) -> &str {
    display.split_once(':').map_or(display, |(_, rest)| rest)
}

/// Keep only the records `keep` accepts and rebuild `bom[]` from the survivors.
///
/// The submit path drops records the ledger already holds; a `bom` entry no surviving record names
/// must go with them. **This is deliberately lossy for a surviving run's later segments:** the
/// envelope records only a run's *session-start* `bom_version`, so an entry describing a mid-run
/// loaded-set change is unreferenced and cannot be attributed by the cloud to any record it is
/// being sent. Dropping it is the conservative direction — never ship a loaded set without the run
/// it belongs to. Anything the input did not have (an absent `records` key) stays absent.
pub(crate) fn filter_records(envelope: &Value, keep: impl Fn(&Value) -> bool) -> Value {
    let mut out = envelope.clone();
    let Some(records) = envelope["records"].as_array() else {
        return out;
    };
    let survivors: Vec<Value> = records.iter().filter(|r| keep(r)).cloned().collect();
    let referenced: BTreeSet<&str> = survivors
        .iter()
        .filter_map(|record| record["bom_version"].as_str())
        .collect();
    let bom: Vec<Value> = envelope["bom"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter(|e| {
                    e["bom_version"]
                        .as_str()
                        .is_some_and(|v| referenced.contains(v))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    out["records"] = Value::Array(survivors);
    out["bom"] = Value::Array(bom);
    out
}

#[cfg(test)]
#[path = "envelope_tests.rs"]
mod tests;
