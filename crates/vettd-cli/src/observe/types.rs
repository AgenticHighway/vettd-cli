//! Closed-enum constants and the small shared value types of the passive observer.
//!
//! The reference semantics are the Python prototype under `spikes/828-passive-observer/`:
//! `prototype/sources/base.py` and `prototype/model.py` declare the closed vocabularies,
//! `prototype/aggregate.py` declares [`Stats`], and `prototype/sources/claude_code.py` plus
//! `prototype/extract.py` declare the two timestamp helpers. The repo-root
//! `telemetry-field-gate.json` is the second source of truth for the vocabularies: a value that is
//! not in one of these arrays cannot pass the gate, so the arrays and the gate must not drift.
//!
//! Only the pieces later phases already depend on live here. The rest of the data model
//! (`SessionFacts`, `RunFacts`, `AttributedRun`, …) lands with the phases that produce it.

use chrono::{DateTime, NaiveDateTime};

/// Every failure class a tool call can carry, in the order `sources/base.py:21-27` declares them.
/// The same five names are the leaves of `records[].assets[].signals.failures.*` in
/// `telemetry-field-gate.json`, which is what makes this list closed on the wire.
pub(crate) const FAILURE_CLASSES: [&str; 5] = [
    "tool_error",
    "timeout",
    "user_denied",
    "interrupted",
    "unknown",
];

/// The failure classes that count toward an asset's observed non-success rate
/// (`sources/base.py:29`). A user denial or an interruption is a fact about the operator, not
/// about the asset, so neither may move a rate the ranking shows.
pub(crate) const RATE_BEARING_FAILURES: [&str; 2] = ["tool_error", "timeout"];

/// Every asset type, in the order `model.py:14-18` declares the `ASSET_*` constants —
/// identical to the `enums.asset_type` list in `telemetry-field-gate.json`.
pub(crate) const ASSET_TYPES: [&str; 5] = ["skill", "mcp_server", "agent", "rules_file", "prompt"];

/// The asset types that can be invoked outright, so direct evidence is available for them
/// (`model.py:19`, `DIRECT_CAPABLE_TYPES`). A rules file or a prompt is only ever loaded.
pub(crate) const DIRECT_CAPABLE_TYPES: [&str; 3] = ["skill", "mcp_server", "agent"];

/// Harness built-in agent types (`attribute.py:61-64`, identical set in `claude_code.py:53-56`).
/// These are not assets: their spawns count in the run counts only, and they are kept out of the
/// dynamic forbids because as substrings they would collide with legitimate enum values.
pub(crate) const BUILTIN_AGENT_TYPES: [&str; 8] = [
    "Explore",
    "Plan",
    "general-purpose",
    "claude",
    "Bash",
    "statusline-setup",
    "claude-code-guide",
    "output-style-setup",
];

/// The zoneless fallback accepted by [`parse_ts_ms`], per the plan's "Timestamps" paragraph.
const NAIVE_TS_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";

/// A mergeable `{n, sum, min, max, sumsq}` summary over integers — the #965 rollup rule, which
/// never carries a percentile because percentiles cannot be combined across devices.
///
/// Port of `aggregate.py:52-75`. `merge` treats an `n == 0` side as absent so an empty summary can
/// never contribute a false minimum of zero, and is associative and commutative so the cloud may
/// combine rows in any order or grouping and get the same answer as summarising the whole sample.
///
/// `sum` and `sumsq` are fixed-width where the Python's integers are unbounded. Every value the
/// observer summarises is a latency in milliseconds or a token count, both bounded by the gate's
/// `numericBounds`, so a realistic sample stays many orders of magnitude below `i64::MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Stats {
    pub n: u64,
    pub sum: i64,
    pub min: i64,
    pub max: i64,
    pub sumsq: u128,
}

impl Stats {
    /// Summarise `values`, or `None` when the sample is empty.
    ///
    /// **Divergence from the Python, deliberate and named:** `Stats.from_values([])` returns the
    /// all-zeros dict rather than a sentinel (`aggregate.py:62-63`), and callers distinguish the
    /// two cases themselves — `aggregate.py:220` always writes a stats object for `latency_ms`
    /// while `aggregate.py:221` writes `null` for `tokens_attributed` when there is nothing to
    /// summarise. Rust makes "empty" unrepresentable-by-accident instead: a caller that needs the
    /// Python's zeros writes `Stats::from_values(v).unwrap_or_default()`, since `Stats::default()`
    /// *is* `{n: 0, sum: 0, min: 0, max: 0, sumsq: 0}`.
    pub(crate) fn from_values(values: &[i64]) -> Option<Stats> {
        let first = *values.first()?;
        let mut stats = Stats {
            n: 0,
            sum: 0,
            min: first,
            max: first,
            sumsq: 0,
        };
        for &value in values {
            stats.n += 1;
            stats.sum += value;
            stats.min = stats.min.min(value);
            stats.max = stats.max.max(value);
            stats.sumsq += square(value);
        }
        Some(stats)
    }

    /// Combine two summaries. An `n == 0` side is absent, not a sample of zeros.
    pub(crate) fn merge(&self, other: &Stats) -> Stats {
        if self.n == 0 {
            return *other;
        }
        if other.n == 0 {
            return *self;
        }
        Stats {
            n: self.n + other.n,
            sum: self.sum + other.sum,
            min: self.min.min(other.min),
            max: self.max.max(other.max),
            sumsq: self.sumsq + other.sumsq,
        }
    }
}

/// `value * value` widened before the multiply, so squaring any `i64` is exact and non-negative.
fn square(value: i64) -> u128 {
    let widened = i128::from(value);
    (widened * widened) as u128
}

/// Parse a harness timestamp to milliseconds since the epoch, or `None` when it is not a timestamp.
///
/// Port of `claude_code.py:599-609` (`_parse_ts`): an offset-bearing stamp is read as written, a
/// zoneless stamp is read as UTC, and sub-millisecond precision is truncated rather than rounded
/// (the Python's `microsecond // 1000`, here `timestamp_millis()`), so `.0009` is 0 ms, not 1.
///
/// **Divergence from the Python, named:** `datetime.fromisoformat` accepts more shapes than
/// RFC 3339 plus the one naive format above — most visibly a bare `"2026-08-15"`, which the
/// Python reads as midnight UTC and this returns `None` for. No Claude Code log line carries a
/// date-only timestamp; widening the accepted grammar would mean accepting stamps the gate then
/// has to reason about, so the plan pins the narrower grammar.
pub(crate) fn parse_ts_ms(raw: &str) -> Option<i64> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.timestamp_millis());
    }
    NaiveDateTime::parse_from_str(raw, NAIVE_TS_FORMAT)
        .ok()
        .map(|naive| naive.and_utc().timestamp_millis())
}

/// The UTC calendar day of `ts_ms` as `YYYY-MM-DD` — never the local day.
///
/// Port of `extract.py:287-288`. The seconds are taken with `div_euclid`, which floors, matching
/// Python's `//`: truncation toward zero would put a pre-epoch millisecond on the following day.
/// `observed_day` is a retention key the cloud indexes on, so a collector in UTC-7 and a collector
/// in UTC+9 must agree on the day a run belongs to.
///
/// Panics only for a `ts_ms` outside chrono's representable range (roughly ±262,000 years), which
/// no parsed harness timestamp can reach; the Python raises on the same inputs.
pub(crate) fn utc_day(ts_ms: i64) -> String {
    DateTime::from_timestamp(ts_ms.div_euclid(1000), 0)
        .expect("timestamp is within the representable date range")
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
