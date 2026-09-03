//! Ranked, confidence-tagged view of one envelope for a stated task.
//!
//! Port of the ranking half of `spikes/828-passive-observer/prototype/rank.py`; the rendering half
//! is [`crate::observe::render`]. Everything here is display logic over the wire envelope plus the
//! local name map: nothing is stored and nothing egresses. [`rank`] reads no files.
//!
//! The rules are presented as display *floors* rather than as statistics, which is what keeps a
//! two-call asset from outranking a two-hundred-call one:
//!
//! - An observed non-success rate counts `tool_error` and `timeout` only. Denials, interruptions
//!   and unknowns never count — a user saying no is not the asset failing.
//! - The rate is *shown* with its 95% Wilson interval from [`FLOOR_RATE_SHOW`] calls and *ordered*
//!   only from [`FLOOR_RATE_ORDER`], ascending by the interval's UPPER bound. Ordering on the upper
//!   bound is the conservative choice: it penalises small samples rather than rewarding them.
//! - Below the show floor an asset appears in a separate list, sorted by calls seen, never
//!   interleaved with the ranked one. Insufficient evidence is a *state*, not a low rank.
//! - Rules files and prompts can never be Direct, so they are listed apart with a context-cost
//!   estimate and no non-success figure at all.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::observe::taskcat::CATEGORY_UNSPECIFIED;
use crate::observe::types::{Stats, DIRECT_CAPABLE_TYPES, RATE_BEARING_FAILURES};

/// Minimum invocations before a count is reported as observed.
pub(crate) const FLOOR_COUNT: u64 = 1;
/// Minimum exactly-attributed runs before a child-token mean is reported.
pub(crate) const FLOOR_TOKENS: u64 = 3;
/// Minimum paired calls before a latency mean is reported.
pub(crate) const FLOOR_LATENCY: u64 = 5;
/// Minimum calls before a non-success rate is shown at all.
pub(crate) const FLOOR_RATE_SHOW: u64 = 20;
/// Minimum calls before a non-success rate is used for ordering.
pub(crate) const FLOOR_RATE_ORDER: u64 = 50;

pub(crate) const OBSERVED: &str = "observed";
pub(crate) const EARLY: &str = "early_evidence";
pub(crate) const INSUFFICIENT: &str = "insufficient_evidence";
pub(crate) const NOT_APPLICABLE: &str = "not_applicable";
pub(crate) const NO_COVERAGE: &str = "no_coverage";

/// The signals an evidence state can be asked about.
///
/// The Python takes a signal *name* and raises `ValueError` for an unknown one; an enum makes that
/// unrepresentable, so [`evidence_state`] is infallible here. Same states, one fewer failure mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    Count,
    Tokens,
    Latency,
    Rate,
}

impl Signal {
    fn floor(self) -> u64 {
        match self {
            Signal::Count => FLOOR_COUNT,
            Signal::Tokens => FLOOR_TOKENS,
            Signal::Latency => FLOOR_LATENCY,
            // The rate has two floors and is handled in `evidence_state`; this is the lower one.
            Signal::Rate => FLOOR_RATE_SHOW,
        }
    }
}

/// The first category whose keyword appears as a whole word in the stated task wins.
const TASK_KEYWORDS: [(&str, &[&str]); 4] = [
    (
        "mcp_heavy",
        &["mcp", "connector", "connectors", "integration"],
    ),
    (
        "code_edit",
        &[
            "edit",
            "fix",
            "implement",
            "refactor",
            "write",
            "add",
            "change",
            "migrate",
            "patch",
        ],
    ),
    (
        "code_explore",
        &[
            "explore",
            "understand",
            "explain",
            "review",
            "audit",
            "find",
            "read",
            "investigate",
        ],
    ),
    (
        "shell_ops",
        &[
            "shell", "deploy", "build", "run", "install", "test", "tests",
        ],
    ),
];

/// Tier precedence: the strongest tier a run reported for an asset wins when rows merge.
const TIER_ORDER: [&str; 3] = ["direct", "loaded", "inferred"];

/// The token buckets a price table charges for, in the order the cost sum walks them.
pub(crate) const PRICED_BUCKETS: [&str; 4] = ["input", "cache_creation", "cache_read", "output"];

/// Wilson score interval for `k` non-successes in `n` calls. `n == 0` is the whole range.
///
/// The interval, not the point estimate, is what the ranking orders on: 0 failures in 2 calls and 0
/// in 200 have the same rate and very different upper bounds, and only the second is evidence.
pub(crate) fn wilson(k: u64, n: u64) -> (f64, f64) {
    const Z: f64 = 1.96;
    if n == 0 {
        return (0.0, 1.0);
    }
    let (k, n) = (k as f64, n as f64);
    let p = k / n;
    let z2 = Z * Z;
    let denom = 1.0 + z2 / n;
    let centre = p + z2 / (2.0 * n);
    let margin = Z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();
    (
        f64::max(0.0, (centre - margin) / denom),
        f64::min(1.0, (centre + margin) / denom),
    )
}

/// State of one signal given how many observations back it.
///
/// `n = None` means the signal was never observable in this stratum, which is not the same as
/// `n = 0`: no coverage versus nothing seen. `applicable = false` is the caller saying the signal
/// does not exist for this asset type — a non-success rate for a rules file.
pub(crate) fn evidence_state(signal: Signal, n: Option<u64>, applicable: bool) -> &'static str {
    if !applicable {
        return NOT_APPLICABLE;
    }
    let Some(n) = n else {
        return NO_COVERAGE;
    };
    if signal == Signal::Rate {
        if n >= FLOOR_RATE_ORDER {
            return OBSERVED;
        }
        return if n >= FLOOR_RATE_SHOW {
            EARLY
        } else {
            INSUFFICIENT
        };
    }
    if n >= signal.floor() {
        OBSERVED
    } else {
        INSUFFICIENT
    }
}

/// The task category read from the stated task with the published keyword table.
pub(crate) fn task_category_for(task: &str) -> &'static str {
    let lowered = task.to_lowercase();
    // The Python is `re.findall(r"[a-z]+", task.lower())`, so a non-ASCII letter is a separator.
    let words: BTreeSet<&str> = lowered
        .split(|c: char| !c.is_ascii_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    for (category, keywords) in TASK_KEYWORDS {
        if keywords.iter().any(|kw| words.contains(kw)) {
            return category;
        }
    }
    CATEGORY_UNSPECIFIED
}

/// One asset accumulated across every run of the stratum.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AssetRow {
    pub asset_id: String,
    pub asset_type: String,
    pub tier: String,
    pub direct_evidence_available: bool,
    /// Runs with at least one invocation of this asset.
    pub runs: u64,
    /// Runs the asset was loaded in, invoked or not.
    pub loaded_runs: u64,
    pub n: u64,
    /// Rate-bearing non-successes: `tool_error` + `timeout` only.
    pub k: u64,
    pub user_denied: u64,
    pub interrupted: u64,
    pub unknown: u64,
    pub latency: Stats,
    pub tokens_attributed: Option<Stats>,
    pub context_cost_tokens: Option<i64>,
    pub context_cost_methods: Vec<String>,
    pub context_cost_runs: u64,
    pub lo: f64,
    pub hi: f64,
    pub rate_state: &'static str,
    pub needs: u64,
}

impl AssetRow {
    fn new(asset_id: &str, asset_type: &str, tier: &str) -> AssetRow {
        AssetRow {
            asset_id: asset_id.to_string(),
            asset_type: asset_type.to_string(),
            tier: tier.to_string(),
            direct_evidence_available: false,
            runs: 0,
            loaded_runs: 0,
            n: 0,
            k: 0,
            user_denied: 0,
            interrupted: 0,
            unknown: 0,
            // The Python's `Stats.from_values([])` is all zeros, not absent: the schema has no null
            // inside a stats object, and `merge` treats an n=0 side as absent anyway.
            latency: Stats::default(),
            tokens_attributed: None,
            context_cost_tokens: None,
            context_cost_methods: Vec::new(),
            context_cost_runs: 0,
            lo: 0.0,
            hi: 1.0,
            rate_state: INSUFFICIENT,
            needs: FLOOR_RATE_SHOW,
        }
    }
}

/// The whole ranked view of one stratum.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RankResult {
    pub task: String,
    pub harness: String,
    pub model: Option<String>,
    pub task_category: &'static str,
    /// asset_id to local display name. **Local only** — never rendered when scrubbing.
    pub names: BTreeMap<String, String>,
    pub run_count: usize,
    pub day_count: usize,
    pub ranked: Vec<AssetRow>,
    pub early: Vec<AssetRow>,
    pub insufficient: Vec<AssetRow>,
    pub loaded_only: Vec<AssetRow>,
    /// Task categories present in the harness but not in the stratum, with their run counts.
    pub context: Vec<(String, u64)>,
    /// Model to runs in the stratum.
    pub models: BTreeMap<String, u64>,
    pub tokens_by_model: BTreeMap<String, BTreeMap<String, i64>>,
    /// True only when the matched task category had no runs and every category was pooled.
    pub pooled_categories: bool,
    /// Rows skipped because their counts were inconsistent.
    pub invalid_rows: u64,
}

/// Envelope plus local names to a ranked view. Pure: reads no files.
pub(crate) fn rank(
    envelope: &Value,
    name_map: &BTreeMap<String, String>,
    task: &str,
    harness: &str,
    model: Option<&str>,
) -> RankResult {
    let category = task_category_for(task);
    let harness_matches = envelope["resource"]["harness"].as_str() == Some(harness);
    let empty = Vec::new();
    let records = if harness_matches {
        envelope["records"].as_array().unwrap_or(&empty)
    } else {
        &empty
    };
    let in_model: Vec<&Value> = records
        .iter()
        .filter(|r| model.is_none_or(|m| models_of(r).contains(m)))
        .collect();

    let matched: Vec<&Value> = in_model
        .iter()
        .copied()
        .filter(|r| {
            category == CATEGORY_UNSPECIFIED || r["task_category"].as_str() == Some(category)
        })
        .collect();
    // Nothing observed in the matched category: pool every category rather than show an empty
    // view — but say so in the header, and never merge silently when the matched stratum has runs.
    let pooled_categories =
        (category == CATEGORY_UNSPECIFIED && !in_model.is_empty()) || matched.is_empty();
    let stratum = if matched.is_empty() && !in_model.is_empty() {
        in_model.clone()
    } else {
        matched
    };

    let mut others: BTreeMap<String, u64> = BTreeMap::new();
    for record in &in_model {
        if let Some(other) = record["task_category"].as_str().filter(|c| *c != category) {
            *others.entry(other.to_string()).or_insert(0) += 1;
        }
    }

    let mut result = RankResult {
        task: task.to_string(),
        harness: harness.to_string(),
        model: model.map(str::to_string),
        task_category: category,
        names: name_map.clone(),
        run_count: stratum.len(),
        day_count: stratum
            .iter()
            .filter_map(|r| r["observed_day"].as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        ranked: Vec::new(),
        early: Vec::new(),
        insufficient: Vec::new(),
        loaded_only: Vec::new(),
        context: others.into_iter().collect(),
        models: runs_per_model(&stratum),
        tokens_by_model: tokens_by_model(&stratum),
        pooled_categories: pooled_categories && !in_model.is_empty(),
        invalid_rows: 0,
    };
    for row in accumulate(&stratum).into_values() {
        classify(row, &mut result);
    }
    // Ascending by the interval's upper bound, then more calls first, then asset_id for a total
    // order — without the last key two equally-evidenced assets could swap between runs.
    result.ranked.sort_by(|a, b| {
        a.hi.total_cmp(&b.hi)
            .then(b.n.cmp(&a.n))
            .then(a.asset_id.cmp(&b.asset_id))
    });
    result.early.sort_by(|a, b| a.asset_id.cmp(&b.asset_id));
    result
        .insufficient
        .sort_by(|a, b| b.n.cmp(&a.n).then(a.asset_id.cmp(&b.asset_id)));
    result
        .loaded_only
        .sort_by(|a, b| a.asset_id.cmp(&b.asset_id));
    result
}

fn models_of(record: &Value) -> BTreeSet<&str> {
    let mut out = BTreeSet::new();
    if let Some(model) = record["model"].as_str() {
        out.insert(model);
    }
    for entry in record["tokens_by_model"].as_array().into_iter().flatten() {
        if let Some(model) = entry["model"].as_str() {
            out.insert(model);
        }
    }
    out
}

/// Runs in which each model produced tokens; a run with sub-agents on another model counts for
/// both. Falls back to the run's dominant model when no per-model split was recorded.
fn runs_per_model(records: &[&Value]) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for record in records {
        if record["tokens"]["basis"].as_str() == Some("none") {
            continue;
        }
        for model in split_models(record) {
            *counts.entry(model).or_insert(0) += 1;
        }
    }
    counts
}

/// The distinct models a record attributes tokens to, or its dominant model when unsplit.
fn split_models(record: &Value) -> BTreeSet<String> {
    let entries = record["tokens_by_model"].as_array();
    match entries.filter(|e| !e.is_empty()) {
        Some(entries) => entries
            .iter()
            .filter_map(|e| e["model"].as_str().map(str::to_string))
            .collect(),
        None => record["model"]
            .as_str()
            .map(|m| BTreeSet::from([m.to_string()]))
            .unwrap_or_default(),
    }
}

fn tokens_by_model(records: &[&Value]) -> BTreeMap<String, BTreeMap<String, i64>> {
    let mut out: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for record in records {
        // No usage evidence is not zero tokens, so a basis-less run contributes nothing.
        if record["tokens"]["basis"].as_str() == Some("none") {
            continue;
        }
        let unsplit = [record["tokens"].clone()];
        let entries: Vec<(&str, &Value)> = match record["tokens_by_model"]
            .as_array()
            .filter(|e| !e.is_empty())
        {
            Some(entries) => entries
                .iter()
                .filter_map(|e| e["model"].as_str().map(|m| (m, e)))
                .collect(),
            None => record["model"]
                .as_str()
                .map(|m| vec![(m, &unsplit[0])])
                .unwrap_or_default(),
        };
        for (model, tokens) in entries {
            let bucket = out.entry(model.to_string()).or_insert_with(|| {
                PRICED_BUCKETS
                    .iter()
                    .map(|b| ((*b).to_string(), 0))
                    .collect()
            });
            for name in PRICED_BUCKETS {
                *bucket.entry(name.to_string()).or_insert(0) += tokens[name].as_i64().unwrap_or(0);
            }
        }
    }
    out
}

/// Merge every asset row of the stratum by `asset_id` using the mergeable stats.
fn accumulate(records: &[&Value]) -> BTreeMap<String, AssetRow> {
    let mut rows: BTreeMap<String, AssetRow> = BTreeMap::new();
    for record in records {
        for asset in record["assets"].as_array().into_iter().flatten() {
            let (Some(id), Some(kind), Some(tier)) = (
                asset["asset_id"].as_str(),
                asset["asset_type"].as_str(),
                asset["tier"].as_str(),
            ) else {
                continue;
            };
            let row = rows
                .entry(id.to_string())
                .or_insert_with(|| AssetRow::new(id, kind, tier));
            fold(row, asset);
        }
    }
    rows
}

fn stats_from(value: &Value) -> Stats {
    Stats {
        n: value["n"].as_u64().unwrap_or(0),
        sum: value["sum"].as_i64().unwrap_or(0),
        min: value["min"].as_i64().unwrap_or(0),
        max: value["max"].as_i64().unwrap_or(0),
        sumsq: value["sumsq"].as_u64().map(u128::from).unwrap_or(0),
    }
}

fn fold(row: &mut AssetRow, asset: &Value) {
    let signals = &asset["signals"];
    let failures = &signals["failures"];
    row.loaded_runs += 1;
    let invocations = signals["invocations"]["n"].as_u64().unwrap_or(0);
    if invocations > 0 {
        row.runs += 1;
    }
    row.n += invocations;
    for class in RATE_BEARING_FAILURES {
        row.k += failures[class].as_u64().unwrap_or(0);
    }
    row.user_denied += failures["user_denied"].as_u64().unwrap_or(0);
    row.interrupted += failures["interrupted"].as_u64().unwrap_or(0);
    row.unknown += failures["unknown"].as_u64().unwrap_or(0);
    row.latency = row.latency.merge(&stats_from(&signals["latency_ms"]));
    if !signals["tokens_attributed"].is_null() {
        let incoming = stats_from(&signals["tokens_attributed"]);
        row.tokens_attributed = Some(match &row.tokens_attributed {
            Some(current) => current.merge(&incoming),
            None => incoming,
        });
    }
    let cost = &signals["context_cost_est"];
    if !cost.is_null() {
        row.context_cost_tokens =
            Some(row.context_cost_tokens.unwrap_or(0) + cost["tokens"].as_i64().unwrap_or(0));
        row.context_cost_runs += 1;
        if let Some(method) = cost["method"].as_str() {
            if !row.context_cost_methods.iter().any(|m| m == method) {
                row.context_cost_methods.push(method.to_string());
                row.context_cost_methods.sort();
            }
        }
    }
    if tier_rank(asset["tier"].as_str().unwrap_or("")) < tier_rank(&row.tier) {
        row.tier = asset["tier"].as_str().unwrap_or("").to_string();
    }
    row.direct_evidence_available |= asset["direct_evidence_available"]
        .as_bool()
        .unwrap_or(false);
}

fn tier_rank(tier: &str) -> usize {
    TIER_ORDER.iter().position(|t| *t == tier).unwrap_or(99)
}

fn classify(mut row: AssetRow, result: &mut RankResult) {
    if row.k > row.n {
        // More non-successes than calls is an inconsistent record, never displayed.
        result.invalid_rows += 1;
        return;
    }
    let applicable = DIRECT_CAPABLE_TYPES.contains(&row.asset_type.as_str());
    row.rate_state = evidence_state(Signal::Rate, Some(row.n), applicable);
    let (lo, hi) = wilson(row.k, row.n);
    row.lo = lo;
    row.hi = hi;
    row.needs = FLOOR_RATE_SHOW.saturating_sub(row.n);
    if !applicable {
        result.loaded_only.push(row);
    } else if row.rate_state == OBSERVED {
        result.ranked.push(row);
    } else if row.rate_state == EARLY {
        result.early.push(row);
    } else {
        result.insufficient.push(row);
    }
}

// `pub(crate)` so `render_tests` can reuse the `populated()` stratum builder: the renderer's tests
// need exactly the fixture the ranking's tests build, and two copies would drift.
#[cfg(test)]
#[path = "rank_tests.rs"]
pub(crate) mod tests;
