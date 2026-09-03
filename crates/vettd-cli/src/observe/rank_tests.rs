//! Tests for [`super`], ported from the ranking half of
//! `spikes/828-passive-observer/prototype/tests/test_rank.py`.
//!
//! Every asset, count, model and price below is invented; asset ids are sha256 of fixture labels
//! and envelopes are built directly so these tests do not depend on the aggregation fixture.
//!
//! None of these can prove D5 chose the *right* floors. They prove the code implements the
//! published floors and the Wilson ordering rule exactly as the contract states them.

use serde_json::json;

use super::*;
use crate::observe::canonical::hex_sha256;

const HARNESS: &str = "claude_code";
const MODEL: &str = "claude-sonnet-5";

fn hex64(label: &str) -> String {
    hex_sha256(format!("fixture:{label}").as_bytes())
}

/// A builder mirroring the reference's `asset()` helper.
#[derive(Default)]
struct Asset {
    label: &'static str,
    asset_type: &'static str,
    n: u64,
    tool_error: u64,
    timeout: u64,
    user_denied: u64,
    interrupted: u64,
    unknown: u64,
    latency: Vec<i64>,
    child_tokens: Vec<i64>,
    cost: Option<i64>,
    tier: &'static str,
}

fn stats_json(values: &[i64]) -> Value {
    json!({
        "n": values.len(),
        "sum": values.iter().sum::<i64>(),
        "min": values.iter().min().copied().unwrap_or(0),
        "max": values.iter().max().copied().unwrap_or(0),
        "sumsq": values.iter().map(|v| (v * v) as u64).sum::<u64>(),
    })
}

fn asset(label: &'static str) -> Asset {
    Asset {
        label,
        asset_type: "mcp_server",
        tier: "inferred",
        ..Asset::default()
    }
}

impl Asset {
    fn kind(mut self, kind: &'static str) -> Self {
        self.asset_type = kind;
        self
    }
    fn n(mut self, n: u64) -> Self {
        self.n = n;
        self
    }
    fn tool_error(mut self, k: u64) -> Self {
        self.tool_error = k;
        self
    }
    fn timeout(mut self, k: u64) -> Self {
        self.timeout = k;
        self
    }
    fn denied(mut self, k: u64) -> Self {
        self.user_denied = k;
        self
    }
    fn interrupted(mut self, k: u64) -> Self {
        self.interrupted = k;
        self
    }
    fn unknown(mut self, k: u64) -> Self {
        self.unknown = k;
        self
    }
    fn latency(mut self, value: i64, count: usize) -> Self {
        self.latency = vec![value; count];
        self
    }
    fn child_tokens(mut self, value: i64, count: usize) -> Self {
        self.child_tokens = vec![value; count];
        self
    }
    fn cost(mut self, tokens: i64) -> Self {
        self.cost = Some(tokens);
        self
    }

    fn build(&self) -> Value {
        json!({
            "asset_id": hex64(self.label),
            "asset_type": self.asset_type,
            "key_basis": "name_hash",
            "tier": self.tier,
            "binding": "not_applicable",
            "direct_evidence_available": self.n > 0,
            "signals": {
                "invocations": {"n": self.n},
                "failures": {
                    "tool_error": self.tool_error,
                    "timeout": self.timeout,
                    "user_denied": self.user_denied,
                    "interrupted": self.interrupted,
                    "unknown": self.unknown,
                },
                "harness_corroborations": Value::Null,
                "latency_ms": stats_json(&self.latency),
                "tokens_attributed": if self.child_tokens.is_empty() {
                    Value::Null
                } else {
                    stats_json(&self.child_tokens)
                },
                "context_cost_est": match self.cost {
                    None => Value::Null,
                    Some(tokens) => json!({"tokens": tokens, "method": "file_bytes_div4"}),
                },
            },
        })
    }
}

fn record(label: &str, assets: Vec<Value>) -> Value {
    record_full(label, assets, "2026-03-05", MODEL, "code_edit")
}

fn record_full(label: &str, assets: Vec<Value>, day: &str, model: &str, category: &str) -> Value {
    json!({
        "run_id": hex64(&format!("run-{label}")),
        "observed_day": day,
        "model": model,
        "entrypoint_class": "cli",
        "effort": "medium",
        "permission_mode": "default",
        "task_category": category,
        "bom_version": hex64("bom"),
        "loaded_set_basis": "harness_log",
        "run_outcome": "completed",
        "counts": {"turns": 1, "tool_calls": 1, "tool_failures": 0, "user_denials": 0,
                   "subagent_runs": 0, "compactions": 0, "unpaired_tool_uses": 0,
                   "repeated_tool_calls": 0},
        "tokens": {"input": 1000, "cache_creation": Value::Null, "cache_read": Value::Null,
                   "cached_input": Value::Null, "output": 500, "thinking": Value::Null,
                   "reasoning": Value::Null, "basis": "harness_usage"},
        "assets": assets,
    })
}

fn envelope(records: Vec<Value>, harness: &str) -> Value {
    json!({
        "envelope_version": "0.1.0",
        "extractor_version": "proto-0.1.0+taskcat-1",
        "gate_version": 1,
        "emitted_day": "2026-03-06",
        "resource": {"device_id": "00000000-0000-4000-8000-000000000000",
                     "device_id_source": "placeholder", "harness": harness,
                     "harness_version": "1.0.0", "collector": "prototype",
                     "collector_version": "0.1.0"},
        "records": records,
        "bom": [],
        "coverage": {},
    })
}

/// A stratum with one asset in every list: two ranked so ordering shows, one early, two
/// insufficient so sorting shows, two loaded-only, and an agent with exactly attributed tokens.
pub(crate) fn populated() -> Value {
    envelope(
        vec![
            record(
                "r1",
                vec![
                    asset("zeta-invented").n(50).latency(100, 50).build(),
                    asset("eta-invented")
                        .n(1000)
                        .tool_error(6)
                        .timeout(4)
                        .denied(3)
                        .latency(400, 200)
                        .build(),
                    asset("theta-invented").n(25).tool_error(1).build(),
                    asset("iota-invented").n(7).build(),
                    asset("kappa-invented").n(12).tool_error(2).build(),
                    asset("lambda-invented")
                        .kind("rules_file")
                        .cost(800)
                        .build(),
                    asset("mu-invented").kind("prompt").build(),
                    asset("nu-invented")
                        .kind("agent")
                        .n(60)
                        .child_tokens(5000, 4)
                        .build(),
                ],
            ),
            record_full(
                "r2",
                vec![asset("iota-invented").n(0).build()],
                "2026-03-04",
                MODEL,
                "code_edit",
            ),
        ],
        HARNESS,
    )
}

/// `wilson(0, 20)` is `[0, 0.161]` — twenty clean calls really are informative. Hand value:
/// `z^2 = 3.8416`; centre `3.8416/40 = 0.09604`; margin `1.96*sqrt(3.8416/1600) = 0.09604`;
/// denominator `1.19208`; hi `0.19208/1.19208 = 0.1611`.
/// Cannot prove the interval's coverage properties, only the arithmetic.
#[test]
fn zero_of_twenty_upper_bound_is_point_one_six_one() {
    let (lo, hi) = wilson(0, 20);
    assert_eq!(lo, 0.0);
    assert!((hi - 0.161).abs() < 5e-4, "hi was {hi}");
}

/// `wilson(5, 50)` is `[0.043, 0.214]`. Hand value: `p = 0.1`; centre `0.138416`;
/// margin `1.96*sqrt(0.09/50 + 3.8416/10000) = 0.091601`; denominator `1.076832`.
#[test]
fn five_of_fifty_matches_hand_value() {
    let (lo, hi) = wilson(5, 50);
    assert!((lo - 0.043).abs() < 5e-4, "lo was {lo}");
    assert!((hi - 0.214).abs() < 5e-4, "hi was {hi}");
}

/// `n = 0` is the whole range — no information, not a clean record — and `k = n` clamps hi to
/// exactly 1.0 with lo below it, so a row can never show an impossible interval.
/// Cannot prove numerical stability for huge n.
#[test]
fn zero_calls_is_the_whole_range_and_bounds_are_clamped() {
    assert_eq!(wilson(0, 0), (0.0, 1.0));
    let (lo, hi) = wilson(30, 30);
    assert_eq!(hi, 1.0);
    assert!(lo < 1.0);
}

/// The floors are exactly D5's. A change here is a product decision and must fail a test rather
/// than quietly reclassify every asset on every machine. Cannot prove the floors are right.
#[test]
fn floors_are_the_published_ones() {
    assert_eq!(
        (
            FLOOR_COUNT,
            FLOOR_TOKENS,
            FLOOR_LATENCY,
            FLOOR_RATE_SHOW,
            FLOOR_RATE_ORDER
        ),
        (1, 3, 5, 20, 50)
    );
}

/// A rate is insufficient below 20, early evidence from 20 to 49, observed from 50 — inclusive at
/// each floor. Getting a boundary wrong by one either hides evidence or promotes a thin sample to
/// the ranked list. Cannot prove what the display does with each state.
#[test]
fn rate_bands_are_inclusive_at_each_floor() {
    let state = |n| evidence_state(Signal::Rate, Some(n), true);
    assert_eq!(state(19), INSUFFICIENT);
    assert_eq!(state(20), EARLY);
    assert_eq!(state(49), EARLY);
    assert_eq!(state(50), OBSERVED);
}

/// Count, tokens and latency are observed at their floor and insufficient below it; `None` is no
/// coverage, which is not the same as zero seen; and an inapplicable signal is `not_applicable`
/// rather than a misleading zero rate. The Python raises on an unknown signal name — [`Signal`] is
/// an enum here, so that failure mode does not exist.
#[test]
fn count_tokens_latency_floors_and_special_states() {
    assert_eq!(evidence_state(Signal::Count, Some(0), true), INSUFFICIENT);
    assert_eq!(evidence_state(Signal::Count, Some(1), true), OBSERVED);
    assert_eq!(evidence_state(Signal::Tokens, Some(2), true), INSUFFICIENT);
    assert_eq!(evidence_state(Signal::Tokens, Some(3), true), OBSERVED);
    assert_eq!(evidence_state(Signal::Latency, Some(4), true), INSUFFICIENT);
    assert_eq!(evidence_state(Signal::Latency, Some(5), true), OBSERVED);
    assert_eq!(evidence_state(Signal::Latency, None, true), NO_COVERAGE);
    assert_eq!(
        evidence_state(Signal::Rate, Some(500), false),
        NOT_APPLICABLE
    );
}

/// Ordering is by the interval's UPPER bound, so `k=0/n=50` ranks BELOW `k=10/n=1000`: a thin clean
/// record does not beat a thick record with a few non-successes. Ordering by the point rate would
/// invert this and recommend the asset nobody has exercised.
/// Cannot prove that users read the order this way.
#[test]
fn upper_bound_ordering_punishes_low_n_not_high_n() {
    let env = envelope(
        vec![record(
            "r",
            vec![
                asset("clean-thin").n(50).build(),
                asset("thick-invented").n(1000).tool_error(10).build(),
            ],
        )],
        HARNESS,
    );
    let result = rank(
        &env,
        &BTreeMap::new(),
        "fix the invented thing",
        HARNESS,
        None,
    );
    let ids: Vec<&str> = result.ranked.iter().map(|r| r.asset_id.as_str()).collect();
    assert_eq!(ids, vec![hex64("thick-invented"), hex64("clean-thin")]);
    assert!(result.ranked[0].hi < result.ranked[1].hi);
}

/// Equal upper bounds order by larger n first, then asset_id ascending — the key is
/// `(hi, -n, asset_id)` exactly. Without the last key two equally-evidenced assets could swap
/// between runs, which would make the report look unstable for no reason.
#[test]
fn tiebreak_is_more_calls_then_asset_id() {
    let mut tied = [hex64("same-a"), hex64("same-b")];
    tied.sort();
    let env = envelope(
        vec![record(
            "r",
            vec![
                asset("same-b").n(60).build(),
                asset("same-a").n(60).build(),
                asset("bigger-invented").n(120).build(),
            ],
        )],
        HARNESS,
    );
    let result = rank(&env, &BTreeMap::new(), "fix", HARNESS, None);
    let ids: Vec<&str> = result.ranked.iter().map(|r| r.asset_id.as_str()).collect();
    assert_eq!(ids[0], hex64("bigger-invented"));
    assert_eq!(ids[1..], tied[..]);
}

/// The lists are separated by floor: 50+ ranked, 20-49 early, below 20 insufficient with
/// `needs = 20 - n` and sorted by calls descending. Rules files and prompts go to loaded-only
/// whatever their n, with state `not_applicable` — a non-success rate for a rules file would be a
/// figure about nothing. Cannot prove the floors themselves.
#[test]
fn lists_are_separated_by_floor_and_insufficient_sorts_by_calls_descending() {
    let env = populated();
    let result = rank(&env, &BTreeMap::new(), "fix", HARNESS, None);

    let ranked: BTreeSet<&str> = result.ranked.iter().map(|r| r.asset_id.as_str()).collect();
    assert_eq!(
        ranked,
        BTreeSet::from([
            hex64("zeta-invented").as_str(),
            hex64("eta-invented").as_str(),
            hex64("nu-invented").as_str()
        ])
        .iter()
        .copied()
        .collect::<BTreeSet<&str>>()
    );
    assert_eq!(
        result
            .early
            .iter()
            .map(|r| r.asset_id.clone())
            .collect::<Vec<_>>(),
        vec![hex64("theta-invented")]
    );
    assert_eq!(
        result
            .insufficient
            .iter()
            .map(|r| (r.asset_id.clone(), r.n, r.needs))
            .collect::<Vec<_>>(),
        vec![
            (hex64("kappa-invented"), 12, 8),
            (hex64("iota-invented"), 7, 13),
        ]
    );
    let mut loaded = vec![hex64("lambda-invented"), hex64("mu-invented")];
    loaded.sort();
    assert_eq!(
        result
            .loaded_only
            .iter()
            .map(|r| r.asset_id.clone())
            .collect::<Vec<_>>(),
        loaded
    );
    assert!(result
        .loaded_only
        .iter()
        .all(|r| r.rate_state == NOT_APPLICABLE));
    assert_eq!(result.early[0].rate_state, EARLY);
}

/// The same asset across records merges, and `k` counts `tool_error` + `timeout` only. Denials,
/// interruptions and unknowns are kept apart and never raise the rate: a user declining a tool is
/// not the tool failing, and folding those in would make cautious users' assets look broken.
#[test]
fn aggregation_across_records_counts_only_rate_bearing_failures() {
    let records: Vec<Value> = (0..3)
        .map(|i| {
            record(
                &format!("r{i}"),
                vec![asset("split-invented")
                    .n(20)
                    .tool_error(1)
                    .timeout(1)
                    .denied(2)
                    .interrupted(1)
                    .unknown(1)
                    .build()],
            )
        })
        .collect();
    let result = rank(
        &envelope(records, HARNESS),
        &BTreeMap::new(),
        "fix",
        HARNESS,
        None,
    );
    let row = &result.ranked[0];
    assert_eq!(
        (
            row.n,
            row.k,
            row.user_denied,
            row.interrupted,
            row.unknown,
            row.runs
        ),
        (60, 6, 6, 3, 3, 3)
    );
    assert_eq!(result.run_count, 3);
}

/// Only records matching the stated task's category enter the rows; other categories appear as
/// context counts and are never merged in silently. A model filter narrows further, and a harness
/// mismatch yields nothing at all rather than another harness's data.
/// Cannot prove the keyword table maps every real task well — it is a published placeholder.
#[test]
fn stratum_filters_by_task_category_model_and_harness() {
    let env = envelope(
        vec![
            record_full(
                "edit1",
                vec![asset("a-invented").n(60).build()],
                "2026-03-05",
                MODEL,
                "code_edit",
            ),
            record_full(
                "explore1",
                vec![asset("a-invented").n(60).build()],
                "2026-03-05",
                MODEL,
                "code_explore",
            ),
            record_full(
                "edit2",
                vec![asset("a-invented").n(60).build()],
                "2026-03-05",
                "gpt-5-mini",
                "code_edit",
            ),
        ],
        HARNESS,
    );
    let all = rank(
        &env,
        &BTreeMap::new(),
        "fix the invented bug",
        HARNESS,
        None,
    );
    assert_eq!(all.task_category, "code_edit");
    assert_eq!(all.ranked[0].n, 120);
    assert_eq!(all.context, vec![("code_explore".to_string(), 1)]);
    assert_eq!(
        all.models,
        BTreeMap::from([(MODEL.to_string(), 1), ("gpt-5-mini".to_string(), 1)])
    );

    let narrowed = rank(
        &env,
        &BTreeMap::new(),
        "fix the invented bug",
        HARNESS,
        Some(MODEL),
    );
    assert_eq!(narrowed.ranked[0].n, 60);

    let pooled = rank(
        &env,
        &BTreeMap::new(),
        "something without keywords",
        HARNESS,
        None,
    );
    assert_eq!((pooled.task_category, pooled.run_count), ("unspecified", 3));

    assert_eq!(
        rank(&env, &BTreeMap::new(), "fix", "codex", None).run_count,
        0
    );
}

/// An empty matched category pools every category and SAYS SO, rather than showing an empty view
/// that reads as "nothing to report". A populated matched category never pools — silently merging
/// other categories into real evidence would be the worse failure.
#[test]
fn an_empty_matched_stratum_pools_visibly_and_a_populated_one_never_pools() {
    let env = envelope(
        vec![record_full(
            "only",
            vec![asset("a-invented").n(60).build()],
            "2026-03-05",
            MODEL,
            "shell_ops",
        )],
        HARNESS,
    );
    // "fix" matches code_edit, which has no runs here.
    let pooled = rank(&env, &BTreeMap::new(), "fix", HARNESS, None);
    assert_eq!(pooled.task_category, "code_edit");
    assert!(pooled.pooled_categories, "the fallback must be visible");
    assert_eq!(pooled.run_count, 1);

    let matched = rank(&env, &BTreeMap::new(), "run the build", HARNESS, None);
    assert_eq!(matched.task_category, "shell_ops");
    assert!(
        !matched.pooled_categories,
        "a populated matched stratum must not pool"
    );
}

/// The task category comes from the published keyword table, first match wins in precedence order,
/// and a task with no keyword is `unspecified` rather than guessed.
#[test]
fn task_category_reads_the_keyword_table_in_precedence_order() {
    assert_eq!(task_category_for("wire up the mcp connector"), "mcp_heavy");
    assert_eq!(task_category_for("fix the parser"), "code_edit");
    assert_eq!(task_category_for("review the parser"), "code_explore");
    assert_eq!(task_category_for("run the tests"), "shell_ops");
    // mcp_heavy precedes code_edit, so a task naming both is mcp_heavy.
    assert_eq!(task_category_for("fix the mcp server"), "mcp_heavy");
    assert_eq!(
        task_category_for("exercise passive observer resume"),
        "unspecified"
    );
    assert_eq!(task_category_for(""), "unspecified");
    // Whole words only: "prefix" must not match "fix".
    assert_eq!(task_category_for("prefix the output"), "unspecified");
}

/// A record claiming more non-successes than calls is inconsistent and is counted, not displayed.
/// Rendering it would put an impossible rate in front of a user; dropping it silently would hide
/// that the collector produced nonsense.
#[test]
fn an_inconsistent_row_is_counted_and_never_displayed() {
    let env = envelope(
        vec![record(
            "r",
            vec![asset("broken-invented").n(5).tool_error(9).build()],
        )],
        HARNESS,
    );
    let result = rank(&env, &BTreeMap::new(), "fix", HARNESS, None);
    assert_eq!(result.invalid_rows, 1);
    assert!(result.ranked.is_empty() && result.early.is_empty());
    assert!(result.insufficient.is_empty() && result.loaded_only.is_empty());
}

/// The strongest tier any run reported for an asset wins when rows merge, so one run seeing direct
/// evidence is not erased by another that only saw it loaded.
#[test]
fn merging_keeps_the_strongest_tier() {
    let mut direct = asset("t-invented").n(60);
    direct.tier = "direct";
    let env = envelope(
        vec![
            record("r1", vec![asset("t-invented").n(60).build()]),
            record("r2", vec![direct.build()]),
        ],
        HARNESS,
    );
    let result = rank(&env, &BTreeMap::new(), "fix", HARNESS, None);
    assert_eq!(result.ranked[0].tier, "direct");
    assert!(result.ranked[0].direct_evidence_available);
}
