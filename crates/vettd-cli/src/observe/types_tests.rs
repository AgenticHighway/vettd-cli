//! Tests for [`super`] — the shared value types and the harness-neutral data model.
//!
//! Split out of `types.rs` under the `gate.rs`/`gate_tests.rs` convention, so the declarations
//! there are not buried under their assertions. The Phase 1 tests below are unchanged.
//!
//! The data-model tests cover only what the model itself owns: the two [`super::ToolCall`]
//! accessors, [`super::SessionFacts::note_forbid`]'s skip rule, and the defaulted environment
//! strings. Everything about how a session file becomes a `SessionFacts` belongs to the reader.

use super::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Proves: an empty sample yields no summary at all, so a caller must decide between the
/// Python's all-zeros object and a `null` on the wire rather than getting zeros by accident —
/// which is how a minimum of 0 could otherwise be invented for a rate nobody observed.
/// Cannot prove callers make the right choice; that is asserted where they emit.
#[test]
fn stats_from_values_is_none_for_an_empty_sample() {
    assert_eq!(Stats::from_values(&[]), None);
    assert_eq!(
        Stats::from_values(&[]).unwrap_or_default(),
        Stats {
            n: 0,
            sum: 0,
            min: 0,
            max: 0,
            sumsq: 0,
        },
        "the Python's Stats.from_values([]) value must stay reachable"
    );
}

/// Proves: a single observation is its own minimum and maximum, so a one-shot asset reports the
/// value it actually saw instead of a range seeded from a zero initialiser.
/// Cannot prove the value reached here unrounded; that is the reader's invariant.
#[test]
fn stats_from_values_of_one_value_is_that_value() {
    assert_eq!(
        Stats::from_values(&[7]),
        Some(Stats {
            n: 1,
            sum: 7,
            min: 7,
            max: 7,
            sumsq: 49,
        })
    );
}

/// Proves: `n`, `sum`, `min`, `max` and `sumsq` are computed over the whole sample, including
/// negative values, so the cloud can recover a mean and a variance from a row without ever
/// receiving the individual observations. Values are `test_aggregate.py:216`'s verbatim.
/// Cannot prove the cloud's mean/variance formulas; it proves the inputs they need are right.
#[test]
fn stats_from_values_computes_n_sum_min_max_sumsq() {
    assert_eq!(
        Stats::from_values(&[200, 300, 400]),
        Some(Stats {
            n: 3,
            sum: 900,
            min: 200,
            max: 400,
            sumsq: 290_000,
        })
    );
    assert_eq!(
        Stats::from_values(&[200, 300, 400, -5, 12, 7]),
        Some(Stats {
            n: 6,
            sum: 914,
            min: -5,
            max: 400,
            sumsq: 290_218,
        })
    );
}

/// Proves: merging is associative and commutative and agrees with summarising the whole sample
/// in one pass. This is exactly what the cloud relies on to fold many devices' rows together in
/// arrival order — if it failed, a rollup would depend on which record landed first.
/// Cannot prove the merge saturates safely on adversarial magnitudes; the gate bounds do that.
#[test]
fn stats_merge_is_associative_and_commutative() {
    let a = Stats::from_values(&[200, 300, 400]).expect("non-empty");
    let b = Stats::from_values(&[-5, 12]).expect("non-empty");
    let c = Stats::from_values(&[7]).expect("non-empty");
    let whole = Stats::from_values(&[200, 300, 400, -5, 12, 7]).expect("non-empty");

    assert_eq!(a.merge(&b).merge(&c), whole, "left fold");
    assert_eq!(a.merge(&b.merge(&c)), whole, "right fold (associativity)");
    assert_eq!(c.merge(&b.merge(&a)), whole, "reversed (commutativity)");
    assert_eq!(b.merge(&a), a.merge(&b), "pairwise commutativity");
}

/// Proves: an `n == 0` side is absent rather than a sample containing zero, so folding an
/// unobserved stratum into a real one can never drag the reported minimum down to 0 — the
/// failure mode `aggregate.py:55-56` names in its docstring.
/// Cannot prove callers never fabricate an `n > 0` summary of zeros; that is upstream.
#[test]
fn stats_merge_treats_a_zero_count_side_as_absent() {
    let empty = Stats::default();
    let real = Stats::from_values(&[-5, 12]).expect("non-empty");
    assert_eq!(empty.merge(&real), real);
    assert_eq!(real.merge(&empty), real);
    assert_eq!(empty.merge(&empty), empty);
}

/// Proves: a `Z` stamp and an offset stamp naming the same instant are the same instant, so a
/// run's `observed_day` and every latency are independent of how the harness wrote the zone.
/// Expected values from `python3 -c "from sources.claude_code import _parse_ts; ..."` in
/// `spikes/828-passive-observer/prototype`.
/// Cannot prove the harness writes the zone it means.
#[test]
fn parse_ts_ms_reads_zulu_and_offset_forms() {
    assert_eq!(parse_ts_ms("2026-08-15T10:00:00Z"), Some(1_786_788_000_000));
    assert_eq!(
        parse_ts_ms("2026-08-15T10:00:00+05:30"),
        Some(1_786_768_200_000),
        "an offset must be subtracted, not ignored"
    );
}

/// Proves: sub-millisecond precision is truncated, never rounded up. Rounding would let a
/// result stamped one microsecond before its call produce a negative latency, and would make
/// two collectors disagree about a value that is supposed to be reproducible from the log.
/// Cannot prove nanosecond-precision logs exist; it pins the behaviour if one appears.
#[test]
fn parse_ts_ms_truncates_sub_millisecond_precision() {
    assert_eq!(
        parse_ts_ms("2026-08-15T10:00:00.0009Z"),
        Some(1_786_788_000_000),
        "900 microseconds is 0 ms, not 1"
    );
    assert_eq!(
        parse_ts_ms("2026-08-15T10:00:00.123456Z"),
        Some(1_786_788_000_123)
    );
}

/// Proves: a stamp with no zone is read as UTC rather than as the collector's local time, so
/// the same log file observed in two time zones yields the same `observed_day` and the same
/// run boundaries.
/// Cannot prove the harness meant UTC when it omitted the zone; it pins one reading of it.
#[test]
fn parse_ts_ms_treats_a_zoneless_stamp_as_utc() {
    assert_eq!(parse_ts_ms("2026-08-15T10:00:00"), Some(1_786_788_000_000));
    assert_eq!(
        parse_ts_ms("2026-08-15T10:00:00"),
        parse_ts_ms("2026-08-15T10:00:00Z")
    );
}

/// Proves: an unparseable stamp is `None` rather than a silent 0. A 0 would place the line at
/// the epoch, which would both invent an `observed_day` of 1970-01-01 and make every latency
/// computed against it enormous.
/// Cannot prove every malformed stamp shape is covered; it covers the "not a date" class.
#[test]
fn parse_ts_ms_rejects_a_non_timestamp() {
    assert_eq!(parse_ts_ms("not-a-timestamp"), None);
    assert_eq!(parse_ts_ms(""), None);
    assert_eq!(parse_ts_ms("2026-08-15T10:00:00+99:99"), None);
}

/// Proves: `utc_day` is the UTC calendar day, including at exactly midnight where a local-time
/// implementation would name the previous or the next day. `observed_day` is the cloud's
/// retention key, so a run must land on the same day for every collector on earth.
/// Cannot prove the process time zone is exercised; `%Y-%m-%d` of a UTC datetime cannot
/// read it.
#[test]
fn utc_day_is_the_utc_calendar_day_including_exact_midnight() {
    assert_eq!(utc_day(1_755_252_000_000), "2025-08-15");
    assert_eq!(utc_day(1_786_752_000_000), "2026-08-15", "00:00:00.000Z");
    assert_eq!(
        utc_day(1_786_788_000_123),
        "2026-08-15",
        "same day, mid-run"
    );
    assert_eq!(utc_day(0), "1970-01-01", "the epoch instant itself");
}

/// Proves: the seconds are floored, not truncated toward zero, so a pre-1970 millisecond stays
/// on the day it belongs to. `-86_400_001` is 1 ms before 1969-12-31T00:00:00Z: flooring gives
/// 1969-12-30, truncation would give 1969-12-31 and silently shift a day boundary.
/// Expected values from `python3 -c "from extract import utc_day; ..."` in the prototype.
/// Cannot prove pre-epoch stamps occur in practice; it proves the arithmetic is total.
#[test]
fn utc_day_floors_rather_than_truncates_before_the_epoch() {
    assert_eq!(utc_day(-1), "1969-12-31");
    assert_eq!(utc_day(-1_000), "1969-12-31");
    assert_eq!(utc_day(-86_400_000), "1969-12-31");
    assert_eq!(utc_day(-86_400_001), "1969-12-30", "floor, not trunc");
}

// -- the data model's own invariants --------------------------------------------------------------

/// Proves: an unpaired call reports no latency at all rather than a zero one. A zero would enter
/// the latency [`Stats`] of whatever asset the call belongs to and drag its observed minimum and
/// mean down with a duration nobody measured — the call's result may simply not be written yet.
/// Cannot prove the reader pairs the calls it should; that is the reader's invariant.
#[test]
fn tool_call_is_unpaired_until_a_result_timestamp_arrives() {
    let mut call = ToolCall {
        tool_use_id: "toolu_1".to_string(),
        name: "Bash".to_string(),
        ts_ms: 1_000,
        ..Default::default()
    };
    assert!(!call.paired());
    assert_eq!(call.latency_ms(), None);

    call.result_ts_ms = Some(1_500);
    assert!(call.paired());
    assert_eq!(call.latency_ms(), Some(500));
}

/// Proves: a result stamped at or before its call yields 0, never a negative latency. Harness
/// clocks are adjusted, lines are written out of order, and a synthetic self-paired call carries
/// the same stamp twice; a negative value would survive into `Stats.sum`/`min` and into a rate the
/// ranking shows, where it cannot be distinguished from a real measurement.
/// Expected values from `python3 -c "from sources.base import ToolCall; ..."` in the prototype.
/// Cannot prove such transcripts exist; it proves the clamp is total when one does.
#[test]
fn tool_call_latency_clamps_a_result_before_the_call_to_zero() {
    let call = |result: i64| ToolCall {
        ts_ms: 1_000,
        result_ts_ms: Some(result),
        ..Default::default()
    };
    assert_eq!(
        call(900).latency_ms(),
        Some(0),
        "100 ms early is 0, not -100"
    );
    assert_eq!(call(1_000).latency_ms(), Some(0), "same instant");
    assert_eq!(
        ToolCall {
            ts_ms: i64::MAX,
            result_ts_ms: Some(i64::MIN),
            ..Default::default()
        }
        .latency_ms(),
        Some(0),
        "an absurd pair of stamps saturates instead of panicking"
    );
}

/// Proves: `note_forbid` drops a missing or empty name instead of storing it. The gate treats
/// every member of a bucket as a forbidden substring of the envelope, and the empty string is a
/// substring of every value there is — one empty member would fail every record and destroy the
/// only signal that says a real local name leaked.
/// Cannot prove the gate's substring rule itself; `gate_tests.rs` owns that.
#[test]
fn note_forbid_skips_a_missing_or_empty_value() {
    let mut facts = SessionFacts::default();
    facts.note_forbid("loaded_set_names", None);
    facts.note_forbid("loaded_set_names", Some(""));
    assert!(
        facts.forbids.is_empty(),
        "an absent or empty name must not even create its bucket"
    );

    facts.note_forbid("loaded_set_names", Some("fx-reviewer"));
    facts.note_forbid("loaded_set_names", Some("fx-reviewer"));
    facts.note_forbid("tool_use_ids", Some("toolu_1"));
    assert_eq!(
        facts.forbids.get("loaded_set_names"),
        Some(&BTreeSet::from(["fx-reviewer".to_string()])),
        "a repeated name is one member, not two"
    );
    assert_eq!(
        facts.forbids.keys().collect::<Vec<_>>(),
        vec!["loaded_set_names", "tool_use_ids"]
    );
}

/// Proves: fresh facts start at `"unknown"` for all four environment strings, and `mode_counts`
/// is a field of its own rather than a bucket of `forbids`. Both matter for egress: `"unknown"` is
/// the enum value the gate accepts for a session that never states its entrypoint, so a default of
/// `""` would be refused at the gate; and the Python's `_permission_modes` scratch bucket
/// (`claude_code.py:376-382`) leaks permission-mode names into the dynamic-forbid sidecar, where
/// they forbid a closed enum value the envelope is required to carry.
/// Cannot prove the reader never writes a `_`-prefixed bucket; `collect_dynamic` asserts that.
#[test]
fn fresh_facts_are_unknown_and_keep_mode_counts_out_of_forbids() {
    let facts = SessionFacts::new(SessionRef {
        path: PathBuf::from("/tmp/session.ndjson"),
        harness: "claude_code".to_string(),
        session_key: "sess-1".to_string(),
        kind: SessionKind::Main,
        ..Default::default()
    });
    assert_eq!(
        (
            facts.harness_version.as_str(),
            facts.entrypoint.as_str(),
            facts.permission_mode.as_str(),
            facts.effort.as_str()
        ),
        (UNKNOWN, UNKNOWN, UNKNOWN, UNKNOWN)
    );
    assert_eq!(facts.ref_.session_key, "sess-1");
    assert_eq!(facts.ref_.kind, SessionKind::Main);
    assert!(facts.mode_counts.is_empty());
    assert!(facts.forbids.is_empty());

    let mut facts = facts;
    facts.mode_counts.insert("plan".to_string(), 2);
    facts.permission_mode = "plan".to_string();
    assert!(
        facts.forbids.is_empty(),
        "recording a permission mode must not touch forbids"
    );
}

/// The stamp grammar must not be *wider* than the reference in either direction. A lowercase `z`
/// survives `DateTime::parse_from_rfc3339` but fails the Python's `replace("Z", "+00:00")`, and a
/// leap second is folded by chrono into the previous second's nanoseconds — so accepting `:60`
/// would report a time 60 s later than the log said and could push `observed_day`, a retention key
/// the cloud indexes on, onto the wrong day. Expectations confirmed by running the reference:
/// `python3 -c "from datetime import datetime; datetime.fromisoformat(v.replace('Z','+00:00'))"`.
#[test]
fn stamp_grammar_rejects_what_the_reference_rejects() {
    assert_eq!(parse_ts_ms("2026-01-01T00:00:00Z"), Some(1_767_225_600_000));
    assert_eq!(parse_ts_ms("2026-01-01T00:00:59Z"), Some(1_767_225_659_000));
    assert_eq!(
        parse_ts_ms("2026-01-01T00:00:00+05:30"),
        Some(1_767_225_600_000 - 19_800_000)
    );

    assert_eq!(parse_ts_ms("2026-01-01T00:00:00z"), None, "lowercase z");
    assert_eq!(parse_ts_ms("2026-01-01T00:00:60Z"), None, "leap second");
    assert_eq!(
        parse_ts_ms("2026-01-01T00:00:60+00:00"),
        None,
        "leap second"
    );
}

// -- the derived model (`model.py`) ---------------------------------------------------------------

/// Proves: the five named `ASSET_*` consts are exactly [`ASSET_TYPES`], in the same order, and the
/// three direct-capable ones are exactly [`DIRECT_CAPABLE_TYPES`]. Producers spell the consts and
/// the gate checks the arrays, so a value could otherwise be added to one and not the other and
/// only surface as a rejected record on a customer's machine.
/// Cannot prove either list matches `telemetry-field-gate.json`; `gate.rs` owns that comparison.
#[test]
fn asset_type_consts_and_arrays_cannot_drift() {
    assert_eq!(
        ASSET_TYPES,
        [
            ASSET_SKILL,
            ASSET_MCP_SERVER,
            ASSET_AGENT,
            ASSET_RULES_FILE,
            ASSET_PROMPT
        ]
    );
    assert_eq!(
        DIRECT_CAPABLE_TYPES,
        [ASSET_SKILL, ASSET_MCP_SERVER, ASSET_AGENT]
    );
}

/// Proves: a nullable token bucket starts absent and only the two buckets that are never null on
/// the wire start at zero. "Absent" and "observed zero" are different facts — a provider with no
/// cache-read bucket must not contribute a zero to a cache-read average across providers — and
/// `extract.rs` sums into this accumulator, so the distinction has to be right before the first
/// addition.
/// Cannot prove the summation preserves it; `extract.rs`'s `sum_tokens` tests do that.
#[test]
fn token_totals_start_absent_except_the_two_never_null_buckets() {
    let start = TokenTotals::zeroed_non_null();
    assert_eq!(start.input, Some(0));
    assert_eq!(start.output, Some(0));
    assert_eq!(start.cache_creation, None);
    assert_eq!(start.cache_read, None);
    assert_eq!(start.cached_input, None);
    assert_eq!(start.thinking, None);
    assert_eq!(start.reasoning, None);

    assert_eq!(
        TokenTotals::default(),
        TokenTotals {
            input: None,
            output: None,
            cache_creation: None,
            cache_read: None,
            cached_input: None,
            thinking: None,
            reasoning: None,
        },
        "the dataclass's empty-dict default is all-absent, not all-zero"
    );
}

/// Proves: [`RunFacts::default`] reports no tokens at all — `tokens_basis` is `"none"`, which is
/// what tells the envelope not to invent a per-model split for a run that recorded nothing. The
/// eleven fields the Python makes required default to empty here, which is only ever legitimate in
/// a test builder; the assertion pins that they are empty rather than plausible-looking, so a
/// producer that forgot one fails the gate loudly instead of shipping a made-up enum value.
/// Cannot prove `extract()` sets them all; that is asserted where it builds a run.
#[test]
fn run_facts_default_is_an_empty_run_with_no_token_basis() {
    let run = RunFacts::default();
    assert_eq!(run.tokens_basis, "none");
    assert_eq!(run.tokens, TokenTotals::default());
    assert_eq!(run.model, "");
    assert_eq!(run.observed_day, "");
    assert_eq!(run.run_outcome, "");
    assert_eq!(run.turns, 0);
    assert!(run.invocations.is_empty());
    assert!(run.tokens_by_model.is_empty());
    assert!(run.tool_class_shares.is_empty());
    assert!(run.forbids.is_empty());
    assert!(!run.truncated);
}

/// Proves: `binding` is the one [`AssetKey`] field with a default, and that default is
/// [`BINDING_NA`] (`model.py:91`). A key built without a binding claims nothing about how its hash
/// is tied to what the harness loaded; defaulting to an empty string would put a value outside the
/// gate's `binding` enum one forgotten argument away.
#[test]
fn asset_key_default_binding_is_not_applicable() {
    assert_eq!(AssetKey::default().binding, BINDING_NA);
    assert_eq!(AssetKey::default().asset_id, "");
}

/// Proves: [`AssetKey::new`] fills the five fields in the positional order `attribute.py`'s
/// `_key_for` uses, so porting those four call sites cannot silently transpose `key_basis` and
/// `name` — a transposition that would hash a name into the wrong preimage and still type-check.
#[test]
fn asset_key_new_keeps_the_reference_argument_order() {
    let key = AssetKey::new(
        "f".repeat(64).as_str(),
        ASSET_SKILL,
        KEY_CONTENT,
        "alpha",
        BINDING_MTIME,
    );
    assert_eq!(
        key,
        AssetKey {
            asset_id: "f".repeat(64),
            asset_type: ASSET_SKILL.to_string(),
            key_basis: KEY_CONTENT.to_string(),
            name: "alpha".to_string(),
            binding: BINDING_MTIME.to_string(),
        }
    );
}

/// Proves: observations sort by `asset_id` before anything else, because the derived `Ord` follows
/// declaration order and `asset_id` is declared first. `attribute()` sorts its rows and the
/// envelope's `bom[].asset_ids` are sorted; both are inputs to a byte-compared golden file, so the
/// ordering key must not be, say, `asset_type`.
/// Cannot prove the sort is applied; `attribute/` and `envelope.rs` assert that where they sort.
#[test]
fn asset_keys_order_by_asset_id_first() {
    let mut keys = vec![
        AssetKey::new("b", ASSET_AGENT, KEY_NAME, "zulu", BINDING_NA),
        AssetKey::new("a", ASSET_SKILL, KEY_NAME, "alpha", BINDING_NA),
    ];
    keys.sort();
    assert_eq!(
        keys.iter().map(|k| k.asset_id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
}

/// Proves: the defaulted halves of [`InvocationObs`], [`Segment`], [`AssetObservation`] and
/// [`AttributedRun`] mirror the dataclasses' defaults — an unresolved invocation carries no
/// latency, no failure and no child tokens, and an observation carries `None` corroborations
/// rather than `0`. `None` and `0` are different rows on the wire (`aggregate.py:_corroborations`
/// writes null when nothing was seen), so a `#[derive(Default)]` that produced zeros here would
/// invent evidence.
#[test]
fn derived_model_defaults_mirror_the_dataclasses() {
    let inv = InvocationObs {
        asset_type: ASSET_SKILL.to_string(),
        name: "alpha".to_string(),
        ts_ms: 1,
        ..Default::default()
    };
    assert_eq!(inv.latency_ms, None);
    assert_eq!(inv.failure_class, None);
    assert_eq!(inv.child_tokens_total, None);
    assert!(!inv.is_async && !inv.corroborated);

    let segment = Segment::default();
    assert_eq!(segment.bom_version, "");
    assert!(segment.asset_keys.is_empty());

    let obs = AssetObservation::default();
    assert_eq!(obs.harness_corroborations, None);
    assert_eq!(obs.context_cost_est, None);
    assert!(!obs.direct_evidence_available);
    assert!(obs.invocations.is_empty());

    let attributed = AttributedRun::default();
    assert!(attributed.segments.is_empty());
    assert!(attributed.observations.is_empty());
    assert!(attributed.name_map.is_empty());
}

/// Invariant: accumulating a summary saturates rather than wrapping. The release profile sets no
/// `overflow-checks`, so a bare `+=` panics in debug and silently wraps in release — and a wrapped
/// sum is a plausible-looking wrong number that the gate's bounds would happily admit. Saturating
/// pins the value at the type ceiling instead, far outside every gate bound, so such a payload is
/// refused rather than believed. Unreachable while `sumsq` stays representable (`sum^2 <= n*sumsq`),
/// but that is an argument about inputs, not a property of the code.
/// Cannot prove: that saturation is preferable to refusing at this layer — the caller's `sumsq`
/// check is what actually turns it into a refusal.
#[test]
fn stats_accumulation_saturates_rather_than_wrapping() {
    let stats = Stats::from_values(&[i64::MAX, 1]).expect("two values");
    assert_eq!(stats.n, 2);
    assert_eq!(stats.sum, i64::MAX, "a wrapped sum would be negative here");

    let low = Stats::from_values(&[i64::MIN, -1]).expect("two values");
    assert_eq!(low.sum, i64::MIN, "and positive here");

    let huge = Stats {
        n: u64::MAX,
        sum: i64::MAX,
        min: 0,
        max: i64::MAX,
        sumsq: u128::MAX,
    };
    let merged = huge.merge(&huge);
    assert_eq!(merged.n, u64::MAX);
    assert_eq!(merged.sum, i64::MAX);
    assert_eq!(merged.sumsq, u128::MAX);
}
