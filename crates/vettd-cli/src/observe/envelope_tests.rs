//! Tests for [`super`], re-expressing every invariant of
//! `spikes/828-passive-observer/prototype/tests/test_aggregate.py` (19 tests) plus the parity and
//! arithmetic tests the Rust port owes on its own.
//!
//! **The one dropped port, named:** `test_validates_against_envelope_schema` walks the envelope
//! against `telemetry-envelope.schema.json` with a hand-written mini-validator. The port does not
//! reimplement JSON Schema; the same coupling is proved by `gate_tests.rs`'s
//! `schema_and_gate_leaf_paths_agree` (schema leaves == gate fields, modulo three known gate-only
//! containers) together with `every_leaf_path_is_in_the_gate_and_every_gate_path_is_emitted` below,
//! which pins this module's output to the gate. Schema semantics themselves are validated by the
//! vettd repo's Ajv test.
//!
//! `test_rejects_non_integers` has no direct runtime analogue either — [`Stats::from_values`] takes
//! `&[i64]`, so a float or a bool cannot be passed at all. Its invariant (no float may ever reach a
//! stats object, because a float breaks byte determinism across encoders) is re-expressed at the
//! only boundary where one could still appear: `stats_object_carrying_a_float_cannot_be_encoded`.
//!
//! Every name, count and day below is invented; asset ids are sha256 of fixture labels and the
//! secrets are built at runtime so no secret-shaped literal exists in the file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::{
    bom_version_for, build_envelope, collect_dynamic, filter_records, run_id_for,
    semver_or_unknown, Coverage, EnvelopeMeta, Resource, EXTRACTOR_VERSION,
};
use crate::observe::attribute::attribute;
use crate::observe::attribute::fs_index::FsIndex;
use crate::observe::canonical::{canonical_json, hex_sha256, hmac_sha256_hex, to_json_bytes};
use crate::observe::claude_code::{link_children, ClaudeCodeSource};
use crate::observe::extract::extract;
use crate::observe::gate::{Dynamic, GATE};
use crate::observe::source::Source;
use crate::observe::types::{
    AssetKey, AssetObservation, AttributedRun, ContextCost, InvocationObs, RunFacts, Segment,
    Stats, TokenTotals, ASSET_AGENT, ASSET_MCP_SERVER, ASSET_PROMPT, ASSET_RULES_FILE, ASSET_SKILL,
    BINDING_EXACT, BINDING_MTIME, BINDING_NA, KEY_CONTENT, KEY_DESCRIPTOR, KEY_NAME, TIER_INFERRED,
};

/// The repo-root allowlist, read here rather than through `gate.rs`'s private const.
const GATE_JSON: &str = include_str!("../../../../telemetry-field-gate.json");

const NULL_UUID: &str = "00000000-0000-4000-8000-000000000000";
const TODAY: &str = "2026-03-06";
/// An invented harness-clock ms value; it never reaches the wire.
const T0: i64 = 1_772_000_000_000;

fn secret_a() -> Vec<u8> {
    format!("invented-observer-{}", "material-a-".repeat(2)).into_bytes()
}

fn secret_b() -> Vec<u8> {
    format!("invented-observer-{}", "material-b-".repeat(2)).into_bytes()
}

fn hex64(label: &str) -> String {
    hex_sha256(format!("fixture:{label}").as_bytes())
}

fn key(asset_type: &str, name: &str, basis: &str, binding: &str) -> AssetKey {
    AssetKey::new(&hex64(name), asset_type, basis, name, binding)
}

fn inv(
    asset_type: &str,
    name: &str,
    ts_ms: i64,
    latency_ms: Option<i64>,
    failure_class: Option<&str>,
    child_tokens_total: Option<i64>,
    corroborated: bool,
) -> InvocationObs {
    InvocationObs {
        asset_type: asset_type.to_string(),
        name: name.to_string(),
        ts_ms,
        latency_ms,
        failure_class: failure_class.map(str::to_string),
        is_async: latency_ms.is_none(),
        corroborated,
        child_tokens_total,
    }
}

fn shares(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

/// `test_aggregate.py:run_facts` — the shared base, with the two runs' differences applied by the
/// caller.
fn run_facts(session_key: &str, day: &str, first_ts: i64, tokens: TokenTotals) -> RunFacts {
    let mut forbids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    forbids.insert(
        "harness_session_ids".to_string(),
        BTreeSet::from([session_key.to_string()]),
    );
    forbids.insert(
        "slugs".to_string(),
        BTreeSet::from([format!("invented-slug-{session_key}")]),
    );
    RunFacts {
        session_key: session_key.to_string(),
        harness: "claude_code".to_string(),
        harness_version: "1.2.3".to_string(),
        entrypoint_class: "cli".to_string(),
        effort: "medium".to_string(),
        permission_mode: "default".to_string(),
        model: "claude-sonnet-5".to_string(),
        observed_day: day.to_string(),
        first_ts_ms: first_ts,
        last_ts_ms: first_ts + 60_000,
        run_outcome: "completed".to_string(),
        turns: 2,
        tool_calls: 6,
        tool_failures: 1,
        user_denials: 1,
        subagent_runs: 1,
        tokens,
        tokens_basis: "harness_usage".to_string(),
        tool_class_shares: shares(&[
            ("edit", 0.5),
            ("read", 0.5),
            ("shell", 0.0),
            ("mcp", 0.0),
            ("other", 0.0),
        ]),
        forbids,
        ..RunFacts::default()
    }
}

fn segment(index: usize, keys: &[AssetKey], start: i64, basis: &str) -> Segment {
    Segment {
        index,
        start_ts_ms: start,
        end_ts_ms: start + 30_000,
        loaded_set_basis: basis.to_string(),
        asset_keys: keys.to_vec(),
        bom_version: String::new(),
    }
}

fn obs(
    k: &AssetKey,
    invocations: Vec<InvocationObs>,
    cost: Option<ContextCost>,
    corroborations: Option<u64>,
) -> AssetObservation {
    AssetObservation {
        key: k.clone(),
        tier: TIER_INFERRED.to_string(),
        direct_evidence_available: !invocations.is_empty(),
        invocations,
        context_cost_est: cost,
        harness_corroborations: corroborations,
    }
}

fn alpha() -> AssetKey {
    key(
        ASSET_SKILL,
        "alpha-invented-skill",
        KEY_CONTENT,
        BINDING_MTIME,
    )
}
fn beta() -> AssetKey {
    key(
        ASSET_MCP_SERVER,
        "beta-invented-server",
        KEY_DESCRIPTOR,
        BINDING_NA,
    )
}
fn gamma() -> AssetKey {
    key(ASSET_AGENT, "gamma-invented-agent", KEY_NAME, BINDING_NA)
}
fn delta() -> AssetKey {
    key(
        ASSET_RULES_FILE,
        "delta-invented-rules",
        KEY_CONTENT,
        BINDING_EXACT,
    )
}
fn epsilon() -> AssetKey {
    key(
        ASSET_PROMPT,
        "epsilon-invented-prompt",
        KEY_NAME,
        BINDING_NA,
    )
}

fn cost(tokens: i64, method: &str) -> Option<ContextCost> {
    Some(ContextCost {
        tokens,
        method: method.to_string(),
    })
}

/// `test_aggregate.py:fixture_runs`. Two runs: A (day 03-05) has two segments because a removal
/// split the loaded set; B (day 03-04) has one. Between them every nullable object is exercised
/// both null and populated.
fn fixture_runs() -> Vec<AttributedRun> {
    let (a, b, g, d, e) = (alpha(), beta(), gamma(), delta(), epsilon());
    let run_a = run_facts(
        "session-invented-a",
        "2026-03-05",
        T0 + 86_400_000,
        TokenTotals {
            input: Some(1000),
            output: Some(800),
            cache_creation: Some(200),
            cache_read: Some(5000),
            cached_input: None,
            thinking: Some(100),
            reasoning: None,
        },
    );
    let seg0 = segment(
        0,
        &[a.clone(), b.clone(), g.clone(), d.clone()],
        run_a.first_ts_ms,
        "harness_log",
    );
    let seg1 = segment(
        1,
        &[a.clone(), b.clone()],
        run_a.first_ts_ms + 40_000,
        "harness_log",
    );
    let obs_a0 = vec![
        obs(
            &a,
            vec![
                inv(
                    ASSET_SKILL,
                    &a.name,
                    T0 + 1,
                    Some(200),
                    Some("tool_error"),
                    None,
                    false,
                ),
                inv(ASSET_SKILL, &a.name, T0 + 2, Some(300), None, None, false),
                inv(ASSET_SKILL, &a.name, T0 + 3, Some(400), None, None, false),
            ],
            cost(120, "listing_bytes_div4"),
            None,
        ),
        obs(
            &b,
            vec![
                inv(
                    ASSET_MCP_SERVER,
                    &b.name,
                    T0 + 4,
                    Some(1000),
                    Some("timeout"),
                    None,
                    false,
                ),
                inv(
                    ASSET_MCP_SERVER,
                    &b.name,
                    T0 + 5,
                    Some(1500),
                    None,
                    None,
                    false,
                ),
            ],
            cost(3400, "tool_schema_bytes_div4"),
            None,
        ),
        obs(
            &g,
            vec![
                inv(ASSET_AGENT, &g.name, T0 + 6, None, None, Some(5000), true),
                inv(
                    ASSET_AGENT,
                    &g.name,
                    T0 + 7,
                    None,
                    Some("interrupted"),
                    Some(7000),
                    true,
                ),
            ],
            None,
            Some(2),
        ),
        obs(&d, vec![], cost(800, "file_bytes_div4"), None),
    ];
    let obs_a1 = vec![
        obs(
            &a,
            vec![inv(
                ASSET_SKILL,
                &a.name,
                T0 + 8,
                Some(250),
                Some("user_denied"),
                None,
                false,
            )],
            None,
            None,
        ),
        obs(&b, vec![], None, None),
    ];
    let mut run_b = run_facts(
        "session-invented-b",
        "2026-03-04",
        T0,
        TokenTotals {
            input: Some(300),
            output: Some(90),
            cache_creation: None,
            cache_read: None,
            cached_input: Some(120),
            thinking: None,
            reasoning: Some(40),
        },
    );
    run_b.harness = "codex".to_string();
    run_b.model = "gpt-5-mini".to_string();
    run_b.tool_class_shares = shares(&[
        ("edit", 0.0),
        ("read", 0.0),
        ("shell", 1.0),
        ("mcp", 0.0),
        ("other", 0.0),
    ]);
    let seg_b = segment(0, &[a.clone(), e.clone()], run_b.first_ts_ms, "filesystem");
    let obs_b = vec![
        obs(
            &a,
            vec![inv(
                ASSET_SKILL,
                &a.name,
                T0 + 9,
                Some(700),
                Some("some-future-class"),
                None,
                false,
            )],
            None,
            None,
        ),
        obs(&e, vec![], None, None),
    ];
    let names: BTreeMap<String, String> = [&a, &b, &g, &d, &e]
        .iter()
        .map(|k| (k.asset_id.clone(), format!("{}:{}", k.asset_type, k.name)))
        .collect();
    vec![
        AttributedRun {
            run: run_a,
            segments: vec![seg0, seg1],
            observations: BTreeMap::from([(0, obs_a0), (1, obs_a1)]),
            name_map: names.clone(),
        },
        AttributedRun {
            run: run_b,
            segments: vec![seg_b],
            observations: BTreeMap::from([(0, obs_b)]),
            name_map: names,
        },
    ]
}

fn fixture_resource() -> Resource {
    Resource {
        device_id: NULL_UUID.to_string(),
        device_id_source: "placeholder".to_string(),
        harness: "claude_code".to_string(),
        harness_version: "1.2.3".to_string(),
        collector: "prototype".to_string(),
        collector_version: "0.1.0".to_string(),
    }
}

fn fixture_coverage() -> Coverage {
    Coverage {
        sessions_seen: 2,
        sessions_emitted: 2,
        sessions_skipped_unparseable: 0,
        lines_seen: 40,
        lines_unknown_type: 1,
        bytes_read: 8192,
        truncated_sessions: 0,
        window_days: 30,
        cursor_state: "fresh".to_string(),
    }
}

fn meta(secret: &[u8]) -> EnvelopeMeta<'_> {
    EnvelopeMeta {
        resource: fixture_resource(),
        coverage: fixture_coverage(),
        today: TODAY.to_string(),
        secret,
        run_id_basis: "test_secret".to_string(),
        extractor_version: EXTRACTOR_VERSION.to_string(),
    }
}

/// `test_aggregate.py:build`.
fn build(runs: &[AttributedRun], secret: &[u8]) -> Value {
    build_envelope(runs, &meta(secret)).expect("the fixture envelope is representable")
}

fn fixture_envelope() -> Value {
    build(&fixture_runs(), &secret_a())
}

/// Gate path syntax: dot-joined keys, array elements as `[]`. Null is a leaf.
fn leaf_paths(value: &Value, path: &str, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let next = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                leaf_paths(v, &next, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                leaf_paths(item, &format!("{path}[]"), out);
            }
        }
        _ => {
            out.insert(path.to_string());
        }
    }
}

fn walk_numbers(value: &Value, out: &mut Vec<serde_json::Number>) {
    match value {
        Value::Object(map) => map.values().for_each(|v| walk_numbers(v, out)),
        Value::Array(items) => items.iter().for_each(|v| walk_numbers(v, out)),
        Value::Number(n) => out.push(n.clone()),
        _ => {}
    }
}

fn record_with_run_id<'a>(env: &'a Value, run_id: &str) -> &'a Value {
    env["records"]
        .as_array()
        .expect("records is an array")
        .iter()
        .find(|r| r["run_id"] == json!(run_id))
        .expect("the fixture emitted this run")
}

fn signals<'a>(record: &'a Value, asset_id: &str) -> &'a Value {
    &record["assets"]
        .as_array()
        .expect("assets is an array")
        .iter()
        .find(|a| a["asset_id"] == json!(asset_id))
        .expect("the fixture emitted this asset")["signals"]
}

// ---- Stats ---------------------------------------------------------------------------------

/// `Stats::from_values` computes n/sum/min/max/sumsq, and an empty sample summarises as all zeros —
/// the schema has no null inside a stats object. Cannot prove numeric behaviour beyond `i64`.
#[test]
fn stats_from_values_computes_the_five_fields_and_empty_is_zeros() {
    assert_eq!(
        Stats::from_values(&[200, 300, 400]),
        Some(Stats {
            n: 3,
            sum: 900,
            min: 200,
            max: 400,
            sumsq: 290_000
        })
    );
    assert_eq!(Stats::from_values(&[]), None);
    assert_eq!(
        Stats::from_values(&[]).unwrap_or_default(),
        Stats {
            n: 0,
            sum: 0,
            min: 0,
            max: 0,
            sumsq: 0
        }
    );
}

/// A float can never reach a stats object: `Stats` is integer-typed so one cannot be constructed,
/// and the canonical encoder refuses one even if some other producer built the JSON by hand —
/// which is what keeps the bytes identical across encoders. Cannot prove every call site filters
/// `None` before summarising.
#[test]
fn stats_object_carrying_a_float_cannot_be_encoded() {
    let honest = json!({"n": 1, "sum": 1, "min": 1, "max": 1, "sumsq": 1});
    assert!(canonical_json(&honest).is_ok());
    let smuggled = json!({"n": 1, "sum": 1.5, "min": 1, "max": 1, "sumsq": 1});
    assert!(
        canonical_json(&smuggled).is_err(),
        "a float inside a stats object must be fatal, not rounded"
    );
}

/// Deterministic xorshift64, seeded 828 — the port of the Python property test's `random.Random`
/// without adding a `rand` dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform enough for a shuffle and a sample size; the property under test does not depend on
    /// the quality of the distribution, only on the partitions being varied.
    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// Folding partition summaries in any grouping or order equals summarising the whole sample, and an
/// empty partition is the identity — which is what lets the cloud merge per-run rows without ever
/// seeing a per-call value, and what stops an empty side leaking a false minimum of zero. Cannot
/// prove overflow behaviour: `sum`/`sumsq` are fixed-width here where Python's ints are unbounded,
/// so this holds for samples within `i64`/`u128`, which the gate's bounds guarantee.
#[test]
fn stats_merge_is_associative_and_commutative_on_random_partitions() {
    let mut rng = XorShift64(828);
    for _ in 0..50 {
        let len = 1 + rng.below(40);
        let values: Vec<i64> = (0..len).map(|_| rng.below(5001) as i64).collect();
        let mut cuts: Vec<usize> = Vec::new();
        for _ in 0..rng.below(5).min(len.saturating_sub(1)) {
            let cut = 1 + rng.below(len.max(2) - 1);
            if !cuts.contains(&cut) {
                cuts.push(cut);
            }
        }
        cuts.sort_unstable();
        let mut parts: Vec<&[i64]> = Vec::new();
        let mut start = 0;
        for &cut in &cuts {
            parts.push(&values[start..cut]);
            start = cut;
        }
        parts.push(&values[start..]);
        let mut summaries: Vec<Stats> = parts
            .iter()
            .map(|p| Stats::from_values(p).unwrap_or_default())
            .collect();
        summaries.insert(rng.below(summaries.len() + 1), Stats::default());

        let expected = Stats::from_values(&values).expect("the sample is non-empty");
        let left = summaries
            .iter()
            .fold(Stats::default(), |acc, s| acc.merge(s));
        let right = summaries
            .iter()
            .rev()
            .fold(Stats::default(), |acc, s| s.merge(&acc));
        let mut shuffled = summaries.clone();
        for i in (1..shuffled.len()).rev() {
            shuffled.swap(i, rng.below(i + 1));
        }
        let mut tree = shuffled[0];
        for s in &shuffled[1..] {
            tree = if rng.next_u64() % 2 == 0 {
                s.merge(&tree)
            } else {
                tree.merge(s)
            };
        }
        assert_eq!(left, expected);
        assert_eq!(right, expected);
        assert_eq!(tree, expected);
    }
}

// ---- envelope shape ------------------------------------------------------------------------

/// Every leaf path this module writes is in the gate's `fields`, and every required gate field is
/// emitted at least once by the fixture — a nullable object counts as emitted when it appears as
/// null or when any child leaf appears. Cannot prove a payload from a different fixture covers
/// every path; coverage rests on this fixture exercising each nullable both ways, which it does.
#[test]
fn every_leaf_path_is_in_the_gate_and_every_gate_path_is_emitted() {
    let env = fixture_envelope();
    let gate: Value = serde_json::from_str(GATE_JSON).expect("the gate is valid JSON");
    let fields = gate["fields"]
        .as_object()
        .expect("gate.fields is an object");
    let mut emitted = BTreeSet::new();
    leaf_paths(&env, "", &mut emitted);

    let unknown: Vec<&String> = emitted
        .iter()
        .filter(|p| !fields.contains_key(*p))
        .collect();
    assert!(
        unknown.is_empty(),
        "leaf paths not in the gate: {unknown:?}"
    );

    let missing: Vec<&String> = fields
        .iter()
        .filter(|(_, spec)| spec["required"].as_bool().unwrap_or(true))
        .filter(|(path, spec)| {
            let object = spec["type"] == json!("object");
            let child = |p: &String| p.starts_with(&format!("{path}."));
            !(emitted.contains(*path) || object && emitted.iter().any(child))
        })
        .map(|(path, _)| path)
        .collect();
    assert!(
        missing.is_empty(),
        "required gate paths never emitted: {missing:?}"
    );
}

/// The real gate checker accepts the envelope when handed the names, session keys and slugs the
/// runs carried — i.e. none of those strings is a substring of any string leaf. Cannot prove that a
/// name shorter than the checker's minimum needle length would be caught.
#[test]
fn passes_the_real_gate_checker_with_dynamic_forbids() {
    let dynamic = Dynamic::normalize(&collect_dynamic(&fixture_runs()));
    assert_eq!(
        GATE.check(&fixture_envelope(), &dynamic),
        Vec::<String>::new()
    );
}

/// Every numeric leaf is an integer — `tool_class_shares`, the only float upstream, never egresses;
/// `task_category` is emitted instead. Cannot prove the same for runs whose facts already carry a
/// float in an integer field: Rust's types make that unrepresentable rather than a runtime error.
#[test]
fn no_floats_anywhere() {
    let env = fixture_envelope();
    let mut numbers = Vec::new();
    walk_numbers(&env, &mut numbers);
    assert!(!numbers.is_empty());
    assert!(
        numbers.iter().all(|n| n.is_i64() || n.is_u64()),
        "a non-integer number reached the envelope"
    );
    assert!(!env.to_string().contains("tool_class_shares"));
}

/// `resource` and `coverage` carry exactly the gate's keys and `run_id_basis` comes from the meta,
/// so a local-only bookkeeping value cannot egress by accident. Cannot prove the caller passes
/// correct values for the keys that are kept.
///
/// **Divergence from the Python, named:** its `test_resource_and_coverage_carry_only_gate_keys`
/// hands `build_envelope` a coverage dict with an extra `extra_local_only_key` and asserts it is
/// dropped. [`Coverage`] and [`Resource`] are structs here, so the extra key cannot be constructed;
/// the assertion below is on the emitted key sets, which is the property the Python test was really
/// after.
#[test]
fn resource_and_coverage_carry_only_gate_keys() {
    let env = fixture_envelope();
    let keys = |v: &Value| -> BTreeSet<String> {
        v.as_object().expect("object").keys().cloned().collect()
    };
    assert_eq!(
        keys(&env["resource"]),
        BTreeSet::from([
            "device_id".to_string(),
            "device_id_source".to_string(),
            "harness".to_string(),
            "harness_version".to_string(),
            "collector".to_string(),
            "collector_version".to_string(),
        ])
    );
    assert_eq!(
        keys(&env["coverage"]),
        BTreeSet::from([
            "sessions_seen".to_string(),
            "sessions_emitted".to_string(),
            "sessions_skipped_unparseable".to_string(),
            "lines_seen".to_string(),
            "lines_unknown_type".to_string(),
            "bytes_read".to_string(),
            "truncated_sessions".to_string(),
            "window_days".to_string(),
            "cursor_state".to_string(),
            "run_id_basis".to_string(),
        ])
    );
    assert_eq!(env["coverage"]["run_id_basis"], json!("test_secret"));
    assert_eq!(env["extractor_version"], json!(EXTRACTOR_VERSION));
}

// ---- record content ------------------------------------------------------------------------

/// `run_id` is exactly `HMAC-SHA256(secret, "harness:session_key")` — one record per run — and a
/// different secret yields disjoint run ids for the same runs, so pseudonymity rests on the secret
/// and not on the hash. Cannot prove the secret file itself never egresses.
#[test]
fn run_id_is_the_contract_hmac_and_changes_with_the_secret() {
    let env = fixture_envelope();
    let expected = hmac_sha256_hex(&secret_a(), "claude_code:session-invented-a");
    let ids: BTreeSet<String> = env["records"]
        .as_array()
        .expect("records")
        .iter()
        .map(|r| r["run_id"].as_str().expect("run_id").to_string())
        .collect();
    assert!(ids.contains(&expected));
    assert_eq!(
        expected,
        run_id_for(&secret_a(), "claude_code", "session-invented-a")
    );

    let other = build(&fixture_runs(), &secret_b());
    let other_ids: BTreeSet<String> = other["records"]
        .as_array()
        .expect("records")
        .iter()
        .map(|r| r["run_id"].as_str().expect("run_id").to_string())
        .collect();
    assert!(ids.is_disjoint(&other_ids));
}

/// A run the settle rule split emits ONE record — run-level tokens and counts are never duplicated
/// — carrying the session-start loaded set as `bom_version` and the change as
/// `counts.loaded_set_changes`, while `bom` still holds every segment's set once. Cannot prove the
/// settle rule split the run correctly; that is `attribute/segments.rs`.
#[test]
fn two_segments_yield_one_record_with_a_change_count() {
    let env = fixture_envelope();
    let run_id = run_id_for(&secret_a(), "claude_code", "session-invented-a");
    let matching: Vec<&Value> = env["records"]
        .as_array()
        .expect("records")
        .iter()
        .filter(|r| r["run_id"] == json!(run_id))
        .collect();
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0]["counts"]["loaded_set_changes"], json!(1));
    assert_eq!(env["records"].as_array().expect("records").len(), 2);
    assert_eq!(env["bom"].as_array().expect("bom").len(), 3);
    let versions: BTreeSet<&str> = env["bom"]
        .as_array()
        .expect("bom")
        .iter()
        .map(|b| b["bom_version"].as_str().expect("bom_version"))
        .collect();
    assert!(versions.contains(matching[0]["bom_version"].as_str().expect("bom_version")));
}

/// Records are in `(observed_day, run_id)` order, assets in `asset_id` order, and `bom` in
/// `bom_version` order with unique sorted asset ids whose hash is the entry's own version — so file
/// order carries no information and two collectors reading in different orders agree. Cannot prove
/// tie ordering beyond `run_id`.
#[test]
fn records_assets_and_bom_are_sorted_regardless_of_input_order() {
    let env = fixture_envelope();
    let mut reversed_runs = fixture_runs();
    reversed_runs.reverse();
    assert_eq!(build(&reversed_runs, &secret_a()), env);

    let records = env["records"].as_array().expect("records");
    let days_ids: Vec<(&str, &str)> = records
        .iter()
        .map(|r| {
            (
                r["observed_day"].as_str().expect("day"),
                r["run_id"].as_str().expect("run_id"),
            )
        })
        .collect();
    let mut sorted = days_ids.clone();
    sorted.sort_unstable();
    assert_eq!(days_ids, sorted);
    assert_eq!(days_ids[0].0, "2026-03-04");
    for record in records {
        let ids: Vec<&str> = record["assets"]
            .as_array()
            .expect("assets")
            .iter()
            .map(|a| a["asset_id"].as_str().expect("asset_id"))
            .collect();
        let mut want = ids.clone();
        want.sort_unstable();
        assert_eq!(ids, want);
    }
    let versions: Vec<&str> = env["bom"]
        .as_array()
        .expect("bom")
        .iter()
        .map(|b| b["bom_version"].as_str().expect("bom_version"))
        .collect();
    let mut want = versions.clone();
    want.sort_unstable();
    assert_eq!(versions, want);
    assert_eq!(
        versions.iter().collect::<BTreeSet<_>>().len(),
        versions.len()
    );
    for entry in env["bom"].as_array().expect("bom") {
        let ids: Vec<String> = entry["asset_ids"]
            .as_array()
            .expect("asset_ids")
            .iter()
            .map(|v| v.as_str().expect("asset_id").to_string())
            .collect();
        let unique: BTreeSet<String> = ids.iter().cloned().collect();
        assert_eq!(ids, unique.iter().cloned().collect::<Vec<String>>());
        assert_eq!(entry["bom_version"], json!(bom_version_for(&unique)));
    }
}

/// `tokens_attributed` is null unless an invocation carried an exact child total, and
/// `context_cost_est` is null unless the attributor supplied an estimate; both have the contract
/// shape when present. Cannot prove the attributor chose the right estimate method.
#[test]
fn nullable_objects_are_null_when_absent_and_stats_when_present() {
    let env = fixture_envelope();
    let rec_a = record_with_run_id(
        &env,
        &run_id_for(&secret_a(), "claude_code", "session-invented-a"),
    );
    assert_eq!(
        signals(rec_a, &alpha().asset_id)["tokens_attributed"],
        Value::Null
    );
    assert_eq!(
        signals(rec_a, &gamma().asset_id)["tokens_attributed"],
        json!({"n": 2, "sum": 12000, "min": 5000, "max": 7000, "sumsq": "74000000"})
    );
    assert_eq!(
        signals(rec_a, &alpha().asset_id)["context_cost_est"],
        json!({"tokens": 120, "method": "listing_bytes_div4"})
    );
    assert_eq!(
        signals(rec_a, &gamma().asset_id)["context_cost_est"],
        Value::Null
    );

    let rec_b = record_with_run_id(
        &env,
        &run_id_for(&secret_a(), "codex", "session-invented-b"),
    );
    assert_eq!(
        signals(rec_b, &epsilon().asset_id)["tokens_attributed"],
        Value::Null
    );
    assert_eq!(
        signals(rec_b, &epsilon().asset_id)["context_cost_est"],
        Value::Null
    );
}

/// Failures are counted per closed class with an unknown class folded into `unknown`; latency stats
/// include only paired invocations, so async spawns contribute `n = 0`; `harness_corroborations` is
/// the attributor's count, or the invocation markers, or null. Everything merges across segments,
/// because one run is one record. Cannot prove the source classified the failures correctly.
#[test]
fn failure_classes_latency_and_corroborations() {
    let env = fixture_envelope();
    let rec_a = record_with_run_id(
        &env,
        &run_id_for(&secret_a(), "claude_code", "session-invented-a"),
    );
    assert_eq!(
        signals(rec_a, &alpha().asset_id)["failures"],
        json!({"tool_error": 1, "timeout": 0, "user_denied": 1, "interrupted": 0, "unknown": 0})
    );
    assert_eq!(
        signals(rec_a, &alpha().asset_id)["latency_ms"],
        json!({"n": 4, "sum": 1150, "min": 200, "max": 400, "sumsq": "352500"})
    );
    assert_eq!(
        signals(rec_a, &gamma().asset_id)["latency_ms"]["n"],
        json!(0)
    );
    assert_eq!(
        signals(rec_a, &gamma().asset_id)["failures"]["interrupted"],
        json!(1)
    );
    assert_eq!(
        signals(rec_a, &gamma().asset_id)["harness_corroborations"],
        json!(2)
    );
    assert_eq!(
        signals(rec_a, &alpha().asset_id)["harness_corroborations"],
        Value::Null
    );

    let rec_b = record_with_run_id(
        &env,
        &run_id_for(&secret_a(), "codex", "session-invented-b"),
    );
    assert_eq!(
        signals(rec_b, &alpha().asset_id)["failures"]["unknown"],
        json!(1)
    );
    assert_eq!(
        signals(rec_b, &alpha().asset_id)["invocations"]["n"],
        json!(1)
    );
}

/// Token buckets pass through with absent nullable buckets as null and the two never-null buckets
/// as integers, and `task_category` is the rule over the local shares (edit 0.5 → `code_edit`,
/// shell 1.0 → `shell_ops`). Cannot prove taskcat's boundaries; that is `taskcat_tests.rs`.
#[test]
fn tokens_and_task_category_per_record() {
    let env = fixture_envelope();
    let rec_a = record_with_run_id(
        &env,
        &run_id_for(&secret_a(), "claude_code", "session-invented-a"),
    );
    assert_eq!(
        rec_a["tokens"],
        json!({"input": 1000, "cache_creation": 200, "cache_read": 5000, "cached_input": null,
               "output": 800, "thinking": 100, "reasoning": null, "basis": "harness_usage"})
    );
    assert_eq!(rec_a["task_category"], json!("code_edit"));
    let rec_b = record_with_run_id(
        &env,
        &run_id_for(&secret_a(), "codex", "session-invented-b"),
    );
    assert_eq!(rec_b["tokens"]["cached_input"], json!(120));
    assert_eq!(rec_b["tokens"]["cache_creation"], Value::Null);
    assert_eq!(rec_b["task_category"], json!("shell_ops"));
    assert_eq!(rec_b["loaded_set_basis"], json!("filesystem"));
    assert_eq!(
        rec_b["tokens_by_model"],
        json!([{"model": "gpt-5-mini", "input": 300, "output": 90, "cache_creation": null,
                "cache_read": null, "cached_input": 120, "thinking": null, "reasoning": 40}])
    );
}

// ---- serialization -------------------------------------------------------------------------

/// Same runs + secret + `today` → byte-identical payload, including when the runs arrive in another
/// order. Cannot prove determinism across serde_json releases.
#[test]
fn two_builds_give_identical_bytes() {
    let first = to_json_bytes(&fixture_envelope()).expect("encodes");
    let mut reversed_runs = fixture_runs();
    reversed_runs.reverse();
    let second = to_json_bytes(&build(&reversed_runs, &secret_a())).expect("encodes");
    assert_eq!(first, second);
    assert_eq!(hex_sha256(&first), hex_sha256(&second));
}

/// The bytes are canonical: sorted keys, no whitespace outside strings, ASCII only, exactly one
/// trailing newline, and they parse back to the same document. Cannot prove canonical float
/// formatting — there are no floats, by `no_floats_anywhere`.
#[test]
fn json_is_canonical() {
    let env = fixture_envelope();
    let raw = to_json_bytes(&env).expect("encodes");
    assert!(raw.ends_with(b"\n") && !raw.ends_with(b"\n\n"));
    assert!(raw.is_ascii());
    let text = std::str::from_utf8(&raw[..raw.len() - 1]).expect("ascii");
    assert_eq!(serde_json::from_str::<Value>(text).expect("parses"), env);
    assert!(!text.contains(": "));
    assert!(!text.contains(", "));
}

/// The local-only strings the fixture carries — asset names, session keys, slugs — and the
/// harness-clock millisecond value never appear in the serialized payload. Cannot prove the same
/// for strings the fixture did not include; that is what the dynamic forbids are for.
#[test]
fn no_name_session_key_or_timestamp_in_the_bytes() {
    let raw =
        String::from_utf8(to_json_bytes(&fixture_envelope()).expect("encodes")).expect("ascii");
    for needle in [
        "alpha-invented",
        "beta-invented",
        "session-invented",
        "invented-slug",
        &T0.to_string(),
    ] {
        assert!(!raw.contains(needle), "{needle} leaked into the payload");
    }
}

// ---- collect_dynamic -----------------------------------------------------------------------

/// Every forbids bucket of every run is merged, `loaded_set_names` holds each `name_map` value in
/// both display and bare form plus every asset-key and invocation name, and the runs' own forbids
/// are untouched. Cannot prove the sources populated `forbids` completely.
#[test]
fn collect_dynamic_merges_forbids_and_names_without_mutating_inputs() {
    let runs = fixture_runs();
    let before = runs[0].run.forbids.clone();
    let dynamic = collect_dynamic(&runs);
    assert_eq!(
        dynamic["harness_session_ids"],
        BTreeSet::from([
            "session-invented-a".to_string(),
            "session-invented-b".to_string()
        ])
    );
    assert_eq!(
        dynamic["slugs"],
        BTreeSet::from([
            "invented-slug-session-invented-a".to_string(),
            "invented-slug-session-invented-b".to_string(),
        ])
    );
    let names = &dynamic["loaded_set_names"];
    for k in [alpha(), beta(), gamma(), delta(), epsilon()] {
        assert!(names.contains(&k.name), "bare name {} missing", k.name);
        assert!(names.contains(&format!("{}:{}", k.asset_type, k.name)));
    }
    assert!(!names.contains(""));
    assert_eq!(runs[0].run.forbids, before);
}

/// With no runs the checker still receives a (empty) `loaded_set_names` set, so a caller can rely on
/// the key existing. Cannot prove anything about non-empty behaviour.
#[test]
fn collect_dynamic_on_empty_runs_still_yields_the_names_set() {
    assert_eq!(
        collect_dynamic(&[]),
        BTreeMap::from([("loaded_set_names".to_string(), BTreeSet::new())])
    );
}

/// An underscore-prefixed bucket never reaches the gate checker. The prototype leaks
/// `_permission_modes`, whose members are closed enum values that appear on the wire by design, so
/// forbidding them would make every payload unsendable. Cannot prove no *other* bucket holds a
/// value that legitimately appears on the wire.
#[test]
fn collect_dynamic_never_emits_an_underscore_bucket() {
    let mut runs = fixture_runs();
    runs[0].run.forbids.insert(
        "_permission_modes".to_string(),
        BTreeSet::from(["acceptEdits".to_string(), "default".to_string()]),
    );
    let dynamic = collect_dynamic(&runs);
    assert!(
        dynamic.keys().all(|bucket| !bucket.starts_with('_')),
        "an underscore bucket reached the gate: {:?}",
        dynamic.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        GATE.check(&fixture_envelope(), &Dynamic::normalize(&dynamic)),
        Vec::<String>::new(),
        "the payload's own permission_mode enum value must survive the dynamic check"
    );
}

// ---- filter_records ------------------------------------------------------------------------

/// Dropping a record drops the bom entries no surviving record names, so a loaded set is never sent
/// without the run it belongs to; keeping everything keeps every referenced entry. Cannot prove the
/// ledger picks the right records to drop — that is the submit path's job.
#[test]
fn filter_records_rebuilds_bom_from_the_survivors() {
    let env = fixture_envelope();
    let keep_id = run_id_for(&secret_a(), "codex", "session-invented-b");
    let filtered = filter_records(&env, |r| r["run_id"] == json!(keep_id));
    let records = filtered["records"].as_array().expect("records");
    assert_eq!(records.len(), 1);
    let boms: Vec<&str> = filtered["bom"]
        .as_array()
        .expect("bom")
        .iter()
        .map(|b| b["bom_version"].as_str().expect("bom_version"))
        .collect();
    assert_eq!(
        boms,
        vec![records[0]["bom_version"].as_str().expect("bom_version")]
    );
    assert_eq!(filtered["coverage"], env["coverage"]);

    let none = filter_records(&env, |_| false);
    assert_eq!(none["records"], json!([]));
    assert_eq!(none["bom"], json!([]));
}

// ---- the sumsq decision --------------------------------------------------------------------

/// A sum of squares above `u64::MAX` is emitted as an exact decimal string, while a value outside
/// the envelope's 1e21 bound fails loudly with its field named.
#[test]
fn sumsq_above_u64_max_is_exact_and_the_envelope_bound_is_enforced() {
    let huge = 5_000_000_000i64;
    let mut runs = fixture_runs();
    let invocations: Vec<InvocationObs> = (0..4)
        .map(|i| {
            inv(
                ASSET_AGENT,
                &gamma().name,
                T0 + i,
                None,
                None,
                Some(huge),
                false,
            )
        })
        .collect();
    let summary = Stats::from_values(&[huge; 4]).expect("non-empty");
    assert!(
        summary.sumsq > u128::from(u64::MAX),
        "the fixture must actually exceed the representable range"
    );
    runs[0]
        .observations
        .insert(0, vec![obs(&gamma(), invocations, None, None)]);
    let envelope = build_envelope(&runs, &meta(&secret_a())).expect("must encode exactly");
    let record = record_with_run_id(
        &envelope,
        &run_id_for(&secret_a(), "claude_code", "session-invented-a"),
    );
    assert_eq!(
        signals(record, &gamma().asset_id)["tokens_attributed"]["sumsq"],
        json!(summary.sumsq.to_string())
    );

    let too_large = 40_000_000_000i64;
    runs[0].observations.insert(
        0,
        vec![obs(
            &gamma(),
            vec![inv(
                ASSET_AGENT,
                &gamma().name,
                T0,
                None,
                None,
                Some(too_large),
                false,
            )],
            None,
            None,
        )],
    );
    let error = build_envelope(&runs, &meta(&secret_a())).expect_err("must enforce the bound");
    assert!(
        error.starts_with("records[].assets[].signals.tokens_attributed.sumsq:"),
        "the error must name the field, got: {error}"
    );
    assert!(
        error.contains("1600000000000000000000"),
        "and the rejected value: {error}"
    );
}

// ---- golden parity -------------------------------------------------------------------------

fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/observe")
}

/// Run the whole chain over the committed fixture home, exactly as the prototype's `observe.py`
/// does, and return the envelope plus the runs it was built from.
fn golden_envelope() -> (Value, Vec<AttributedRun>) {
    const NOW_MS: i64 = 1_800_000_000_000;
    let root = fixtures_dir().join("claude_home");
    let secret =
        fs::read(fixtures_dir().join("golden/secret.bin")).expect("the secret is committed");

    let source = ClaudeCodeSource::with_now_ms(root.clone(), NOW_MS);
    let refs = source.discover(&root, 3650, NOW_MS).expect("discover");
    let facts: Vec<_> = refs
        .iter()
        .map(|r| source.read(r, None).expect("read").0)
        .collect();
    let mains = link_children(facts);
    assert_eq!(mains.len(), 1, "the fixture home holds exactly one run");
    let run = extract(&mains[0], NOW_MS);
    let runs = vec![attribute(
        &run,
        &FsIndex::with_home(Some(&root), None),
        &secret,
    )];

    let meta = EnvelopeMeta {
        resource: Resource {
            device_id: NULL_UUID.to_string(),
            device_id_source: "placeholder".to_string(),
            harness: "claude_code".to_string(),
            harness_version: run.harness_version.clone(),
            collector: "prototype".to_string(),
            collector_version: "0.1.0".to_string(),
        },
        coverage: Coverage {
            sessions_seen: 1,
            sessions_emitted: 1,
            sessions_skipped_unparseable: 0,
            lines_seen: run.lines_seen,
            lines_unknown_type: run.lines_unknown_type,
            bytes_read: run.bytes_read,
            truncated_sessions: u64::from(run.truncated),
            window_days: 3650,
            cursor_state: "fresh".to_string(),
        },
        today: "2027-01-15".to_string(),
        secret: &secret,
        run_id_basis: "test_secret".to_string(),
        extractor_version: "proto-0.1.0+taskcat-1".to_string(),
    };
    let envelope = build_envelope(&runs, &meta).expect("the golden envelope is representable");
    (envelope, runs)
}

/// The Rust chain reproduces the committed v0.2 envelope byte for byte from the same fixture home,
/// secret and clock. The one intentional divergence from the prototype is the lossless decimal
/// string encoding for `sumsq`; every hash, count, null, key order and escape remains pinned.
#[test]
fn golden_envelope_bytes_match_committed_v2_contract() {
    let (envelope, _) = golden_envelope();
    let ours = to_json_bytes(&envelope).expect("encodes");
    let golden =
        fs::read(fixtures_dir().join("golden/envelope.json")).expect("golden is committed");
    if ours != golden {
        let theirs: Value = serde_json::from_slice(&golden).expect("golden parses");
        panic!(
            "golden mismatch\n  ours:   {}\n  golden: {}",
            canonical_json(&envelope).expect("encodes"),
            canonical_json(&theirs).expect("encodes")
        );
    }
}

/// The golden envelope passes the real gate with the sidecar dynamic set the prototype emitted
/// beside it. That sidecar contains the prototype's `_permission_modes` leak plus `hostname`,
/// `home_dir` and `current_username` from the author's machine; extra sets only ever make the gate
/// stricter, so a pass here is a strictly stronger statement than a pass with our own sets. Cannot
/// prove the gate lists the right fields.
#[test]
fn golden_envelope_passes_the_gate_with_its_dynamic_set() {
    let (envelope, runs) = golden_envelope();
    let sidecar: Value = serde_json::from_slice(
        &fs::read(fixtures_dir().join("golden/dynamic.json")).expect("dynamic is committed"),
    )
    .expect("dynamic parses");
    let dynamic = Dynamic::from_json(&sidecar).expect("the sidecar is a dynamic set");
    assert_eq!(GATE.check(&envelope, &dynamic), Vec::<String>::new());
    assert_eq!(
        GATE.check(&envelope, &Dynamic::normalize(&collect_dynamic(&runs))),
        Vec::<String>::new(),
        "our own collected sets must also pass"
    );
}

/// The committed worked example — a real payload from the author's machine — passes the gate with
/// no dynamic sets, which is what a receiver can check without the emitter's local vocabulary.
/// Cannot prove it would pass with that vocabulary; the emitter proves that at emission time.
#[test]
fn worked_example_envelope_is_gate_clean() {
    let raw = fs::read(fixtures_dir().join("worked-example/observations.example.json"))
        .expect("the worked example is committed");
    let payload: Value = serde_json::from_slice(&raw).expect("the worked example parses");
    assert_eq!(
        GATE.check(&payload, &Dynamic::empty()),
        Vec::<String>::new()
    );
}

/// `EXTRACTOR_VERSION` must keep encoding the task-category rule-set ordinal, or a rule change
/// would produce observations indistinguishable from ones made under the old rules — which is the
/// whole reason D2 puts the rule version in `extractor_version`. The value carries digits rather
/// than the word `taskcat` because every letter in a free-string leaf is a name a user's asset
/// might collide with; this test is what keeps the two from drifting apart now that the coupling is
/// no longer spelled out in the string itself.
/// Cannot prove: that the cloud interprets the ordinal the same way.
#[test]
fn extractor_version_tracks_the_taskcat_rules_version() {
    let ordinal = crate::observe::taskcat::RULES_VERSION
        .rsplit('-')
        .next()
        .expect("the rules version ends in an ordinal");
    assert_eq!(
        EXTRACTOR_VERSION,
        format!("1+{ordinal}"),
        "extractor_version must carry the taskcat rule-set ordinal"
    );
    assert!(
        !EXTRACTOR_VERSION.bytes().any(|b| b.is_ascii_alphabetic()),
        "a letter here is a substring an asset name can collide with"
    );
}

/// `resource.harness_version` is the one wire leaf read straight out of a local transcript, so the
/// emitter reduces it to the gate's format rather than forwarding it. Two failures are prevented: a
/// build or prerelease suffix can carry a hostname or a commit (the gate's own stated reason for
/// dropping them), and a harness reporting `"1.0"` would otherwise fail the gate and refuse *all*
/// telemetry from that machine — a cosmetic version string turned into a total outage.
/// Cannot prove: that a harness never reports a version this reduction discards meaningfully.
#[test]
fn harness_version_is_reduced_to_the_gate_format() {
    for (raw, want) in [
        ("3.4.5", "3.4.5"),
        ("0.0.0", "0.0.0"),
        ("2.0.14-rc.1", "unknown"),
        ("2.0.14+build.MacBook-Pro.local", "unknown"),
        ("1.0", "unknown"),
        ("1.2.3.4", "unknown"),
        ("v1.2.3", "unknown"),
        ("unknown", "unknown"),
        ("", "unknown"),
        ("1.2.x", "unknown"),
    ] {
        assert_eq!(semver_or_unknown(raw), want, "{raw:?}");
    }

    // And the reduced value must actually satisfy the gate, not merely look like it does.
    let (mut envelope, runs) = golden_envelope();
    envelope["resource"]["harness_version"] =
        Value::String(semver_or_unknown("2.0.14+build.host.local"));
    assert_eq!(
        GATE.check(&envelope, &Dynamic::normalize(&collect_dynamic(&runs))),
        Vec::<String>::new()
    );
}

/// The documented fail-closed behaviour, end to end: an installed asset whose name is a substring
/// of a free string leaf refuses the whole payload, and the refusal names the *set* and never the
/// value — reporting the value would leak the very local name the rule exists to protect.
///
/// The fixture is `claude_home` with one skill renamed `3.4`, a substring of that fixture's own
/// `harness_version` `"3.4.5"`. It was previously named `taskcat` to collide with the plan's
/// `EXTRACTOR_VERSION`; that constant no longer carries letters, and the collision is now anchored
/// to a value the fixture itself controls rather than to a constant that may change.
/// Cannot prove: that a real user's asset names collide — only that the mechanism fires when they do.
#[test]
fn an_asset_named_after_a_version_substring_refuses_the_payload() {
    const NOW_MS: i64 = 1_800_000_000_000;
    let root = fixtures_dir().join("claude_home_gate_violation");
    let secret = fs::read(fixtures_dir().join("golden/secret.bin")).expect("secret");

    let source = ClaudeCodeSource::with_now_ms(root.clone(), NOW_MS);
    let refs = source.discover(&root, 3650, NOW_MS).expect("discover");
    let facts: Vec<_> = refs
        .iter()
        .map(|r| source.read(r, None).expect("read").0)
        .collect();
    let mains = link_children(facts);
    let run = extract(&mains[0], NOW_MS);
    let runs = vec![attribute(
        &run,
        &FsIndex::with_home(Some(&root), None),
        &secret,
    )];
    assert_eq!(
        run.harness_version, "3.4.5",
        "the collision is with this fixture's own harness_version"
    );

    let meta = EnvelopeMeta {
        resource: Resource {
            device_id: NULL_UUID.to_string(),
            device_id_source: "placeholder".to_string(),
            harness: "claude_code".to_string(),
            harness_version: run.harness_version.clone(),
            collector: "vettd-cli".to_string(),
            collector_version: "0.0.0".to_string(),
        },
        coverage: Coverage {
            sessions_seen: 1,
            sessions_emitted: 1,
            sessions_skipped_unparseable: 0,
            lines_seen: run.lines_seen,
            lines_unknown_type: run.lines_unknown_type,
            bytes_read: run.bytes_read,
            truncated_sessions: u64::from(run.truncated),
            window_days: 3650,
            cursor_state: "fresh".to_string(),
        },
        today: "2027-01-15".to_string(),
        secret: &secret,
        run_id_basis: "test_secret".to_string(),
        extractor_version: EXTRACTOR_VERSION.to_string(),
    };
    let envelope = build_envelope(&runs, &meta).expect("the envelope builds; the gate refuses it");
    let violations = GATE.check(&envelope, &Dynamic::normalize(&collect_dynamic(&runs)));

    assert!(
        violations
            .iter()
            .any(|v| v.contains("dynamic:loaded_set_names")),
        "expected the fail-closed refusal, got {violations:?}"
    );
    for violation in &violations {
        assert!(
            !violation.contains("3.4"),
            "a refusal must never echo the local name it caught: {violation}"
        );
    }
}
