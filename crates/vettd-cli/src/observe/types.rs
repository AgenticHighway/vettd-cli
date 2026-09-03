//! Closed-enum constants and the small shared value types of the passive observer.
//!
//! The reference semantics are the Python prototype under `spikes/828-passive-observer/`:
//! `prototype/sources/base.py` and `prototype/model.py` declare the closed vocabularies,
//! `prototype/aggregate.py` declares [`Stats`], and `prototype/sources/claude_code.py` plus
//! `prototype/extract.py` declare the two timestamp helpers. The repo-root
//! `telemetry-field-gate.json` is the second source of truth for the vocabularies: a value that is
//! not in one of these arrays cannot pass the gate, so the arrays and the gate must not drift.
//!
//! The harness-neutral data model a source produces — [`SessionRef`], [`Cursor`], [`ToolCall`],
//! [`Usage`], [`LoadedSetEvent`], [`InBandAsset`] and [`SessionFacts`] — is the second half of
//! this file and ports `sources/base.py` field for field. The derived types (`RunFacts`,
//! `AttributedRun`, …) land with the phases that produce them.
//!
//! The tests live in `types_tests.rs` (the `gate.rs`/`gate_tests.rs` convention). Even so this file
//! is over CONTRIBUTING.md's 400-line budget: the model is one closed set of declarations that is
//! meaningless split in half, and the only way to split it is a `pub use` re-export, which cannot
//! compile warning-free until Phase 3 gives every type a caller.

use chrono::{DateTime, NaiveDateTime, Timelike};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

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
///
/// The divergence is not purely one-directional, so two shapes RFC 3339 allows and the Python
/// rejects are rejected here too. A lowercase `z` fails the Python's
/// `value.replace("Z", "+00:00")`, which only rewrites the uppercase form. A leap second
/// (`:60`) fails `datetime`'s `second must be in 0..59`; chrono instead folds it into the
/// previous second's nanoseconds, so accepting it would silently report a stamp 60 s later and
/// could move `observed_day` across a day boundary. Neither shape is emitted by any harness —
/// `Date.prototype.toISOString()` always produces an uppercase `Z` — but a silent minute of
/// drift is not something to leave to chance.
pub(crate) fn parse_ts_ms(raw: &str) -> Option<i64> {
    if raw.ends_with('z') {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        // chrono represents a leap second as second 59 with nanoseconds at or past 1e9.
        if dt.nanosecond() >= 1_000_000_000 {
            return None;
        }
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

// ------------------------------------------------------------------------------------------------
// The harness-neutral data model — port of `sources/base.py:30-168`, field for field.
//
// PRIVACY INVARIANT (`base.py:3-7`): no field below may hold free text from a session. Message
// text, tool inputs, tool results, attachment bodies and file contents are hashed, counted or
// turned into booleans by the reader's projection, and the raw line is then dropped. Asset and
// harness names survive only in local-only fields — `SessionRef.session_key`, `child_meta`,
// `ToolCall.{server,skill,agent_type}`, `InBandAsset.name`, `LoadedSetEvent`'s name lists and
// `SessionFacts.forbids` — none of which reach the envelope: `aggregate.py` turns them into
// hashes, and the gate's dynamic sets exist to prove no unhashed one slipped through.
//
// None of these types derive `Serialize`/`Deserialize`, deliberately: nothing serialises them, and
// not being serialisable is what makes "this is local-only" a property of the type rather than of
// a reviewer's memory.
// ------------------------------------------------------------------------------------------------

/// The value the four environment strings of [`SessionFacts`] carry until a log line names them
/// (`base.py:139-142`). It is also a legal value on the wire, so a session that never states its
/// entrypoint is reported as unknown rather than omitted.
pub(crate) const UNKNOWN: &str = "unknown";

/// Whether a discovered session file is a main transcript or a sub-agent child of one.
///
/// The Python carries the strings `"main"`/`"child"` in `SessionRef.kind` (`base.py:38`); the port
/// closes that vocabulary in the type so a typo cannot invent a third kind. `Main` is the `Default`
/// because a file found directly under a project directory is a main transcript; children are only
/// ever constructed explicitly, under `<stem>/subagents/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum SessionKind {
    #[default]
    Main,
    Child,
}

/// A discoverable session file (`base.py:30-40`).
///
/// `session_key` is the harness's own session identifier. It is local-only: `aggregate.py:78-85`
/// HMACs it into `run_id` and the key itself never egresses. `child_meta` holds the sub-agent
/// sidecar's `agentType`, `toolUseId` and `spawnDepth`, the reader's own `agentId`, and
/// `corroborated` once an assistant line confirms the spawn — local-only for the same reason.
///
/// **Divergence from the Python, named:** the dataclass is `frozen=True` and `claude_code.py:145`
/// uses `dataclasses.replace` to add `corroborated`. Rust keeps the struct plainly mutable and lets
/// the reader insert into `child_meta` in place; the observable result is identical and a frozen
/// wrapper would buy nothing here, since the value is owned by one reader at a time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionRef {
    pub path: PathBuf,
    pub harness: String,
    pub session_key: String,
    pub kind: SessionKind,
    pub parent_key: Option<String>,
    pub child_meta: BTreeMap<String, String>,
}

/// Byte-offset cursor for resumable, non-blocking reads (`base.py:43-50`).
///
/// `byte_offset` always points one byte past a `\n`, because `iter_lines` never yields a partial
/// trailing line: a session file being appended to while it is read must resume at a line boundary
/// on the next run, not in the middle of a JSON object.
///
/// `inode` is `Some` on Unix and `None` elsewhere; a cursor whose inode no longer matches the file
/// at `path` is discarded rather than trusted, which is what stops a rotated or recreated file from
/// being resumed at an offset that means nothing in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub path: PathBuf,
    pub byte_offset: u64,
    pub inode: Option<u64>,
}

/// One `tool_use` block, paired with its `tool_result` when the transcript contains one
/// (`base.py:53-82`).
///
/// `input_fingerprint` is a sha256 of the canonicalised input, used only for the local
/// repeated-call indicator; the input itself is discarded during projection. `server`, `skill` and
/// `agent_type` are local-only names, kept because attribution needs them and because the gate
/// consumes them as dynamic forbids.
///
/// `Default` exists so the reader can mirror the dataclass's defaults with
/// `ToolCall { tool_use_id, name, ts_ms, ..Default::default() }`; a defaulted call is not a
/// meaningful call on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolCall {
    pub tool_use_id: String,
    pub name: String,
    pub ts_ms: i64,
    pub message_id: Option<String>,
    pub result_ts_ms: Option<i64>,
    pub is_error: Option<bool>,
    pub interrupted: bool,
    pub is_async: bool,
    /// One of [`FAILURE_CLASSES`]; `None` while the call is unpaired or when it succeeded.
    pub failure_class: Option<String>,
    pub input_fingerprint: String,
    /// MCP server name for `mcp__*` tools (local only).
    pub server: Option<String>,
    /// Skill name for `Skill` invocations (local only).
    pub skill: Option<String>,
    /// Sub-agent type for `Agent` spawns (local only).
    pub agent_type: Option<String>,
    /// Linked child session key, when the result named one (local only).
    pub child_key: Option<String>,
}

impl ToolCall {
    /// Whether a `tool_result` was seen for this call (`base.py:74-76`).
    ///
    /// An unpaired call is a call whose outcome the transcript never records — it is not a failure
    /// and must not be counted as one.
    pub(crate) fn paired(&self) -> bool {
        self.result_ts_ms.is_some()
    }

    /// Harness-clock duration of the call in milliseconds, or `None` when it is unpaired
    /// (`base.py:78-82`).
    ///
    /// Clamped at 0: a transcript can stamp a result at or before its call (clock adjustment,
    /// out-of-order writes, equal stamps for a synthetic self-paired call), and a negative latency
    /// would poison every [`Stats`] summary it reached. The subtraction saturates for the same
    /// reason the clamp exists — an absurd pair of stamps must not panic a read.
    pub(crate) fn latency_ms(&self) -> Option<i64> {
        self.result_ts_ms
            .map(|result| result.saturating_sub(self.ts_ms).max(0))
    }
}

/// Token usage of one API response (`base.py:85-99`), keyed by the provider message id so a
/// response split over several log lines counts once.
///
/// The `Option` fields are absent, not zero, when the provider does not report them: `cache_read`
/// of `None` means "not reported", which the envelope must keep distinct from an observed zero.
/// `cached_input` and `reasoning` are only ever set by the Codex source; the Claude Code reader
/// leaves them `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    pub message_id: String,
    pub model: String,
    pub ts_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation: Option<i64>,
    pub cache_read: Option<i64>,
    pub cached_input: Option<i64>,
    pub thinking: Option<i64>,
    pub reasoning: Option<i64>,
}

/// Whether a [`LoadedSetEvent`] is the session-start listing or a later change (`base.py:104-105`).
///
/// The distinction is load-bearing downstream: segments start at an `Initial` event, and only a
/// `Delta` can fold into the segment before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum LoadedSetKind {
    #[default]
    Initial,
    Delta,
}

/// What the harness said was loaded, at a harness timestamp (`base.py:102-118`).
///
/// Every list holds names, which are local-only. `rules_files` holds basenames only — never a path,
/// which would carry a directory layout out of the machine. `listing_bytes` maps a skill name to
/// the byte length of its line in the listing and `tool_schema_bytes` maps an MCP server to the
/// byte length of its tool lines; both are lengths, never the text they measure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LoadedSetEvent {
    pub ts_ms: i64,
    pub kind: LoadedSetKind,
    pub skills: Vec<String>,
    pub tool_names: Vec<String>,
    pub agent_types: Vec<String>,
    /// Basenames only (local).
    pub rules_files: Vec<String>,
    pub pending_mcp: Vec<String>,
    pub failed_mcp: Vec<String>,
    pub removed: Vec<String>,
    pub readded: Vec<String>,
    /// Name -> bytes of its listing line.
    pub listing_bytes: BTreeMap<String, i64>,
    /// MCP server -> bytes of its tool lines.
    pub tool_schema_bytes: BTreeMap<String, i64>,
}

/// Which kind of asset [`InBandAsset`] carries (`base.py:124`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum InBandKind {
    #[default]
    RulesFile,
    SkillBody,
}

/// An asset whose content appeared in the log itself, so it can be hashed exactly without touching
/// the filesystem — rules files via `nested_memory`, and invoked skill bodies (`base.py:121-130`).
///
/// `content_sha256` and `byte_len` are the whole of what is kept about the content: the bytes are
/// hashed and measured during projection and then dropped. `name` is a basename or a skill name,
/// local-only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InBandAsset {
    pub kind: InBandKind,
    pub name: String,
    pub content_sha256: String,
    pub byte_len: i64,
    pub ts_ms: i64,
}

/// Everything extracted from one session file, plus its children (`base.py:133-167`).
///
/// Local-only names and ids live here and go no further: `extract.rs` turns this tree into
/// `RunFacts` and `attribute.rs` hashes every name before anything is built for egress.
///
/// The four environment strings default to [`UNKNOWN`]. For `harness_version`, `entrypoint` and
/// `effort` the reader only ever overwrites an `UNKNOWN` (`claude_code.py:368-373`), so the first
/// line that states a value wins; `permission_mode` is re-derived from [`Self::mode_counts`] on
/// every line that declares one, because a session can enter and leave plan mode.
#[derive(Debug, Clone)]
pub(crate) struct SessionFacts {
    /// The file these facts came from. Named `ref_` because `ref` is a Rust keyword; the Python
    /// field is `SessionFacts.ref` and every later phase spells it `ref_`.
    pub ref_: SessionRef,
    pub harness_version: String,
    pub entrypoint: String,
    pub permission_mode: String,
    pub effort: String,
    /// Model name -> number of API responses attributed to it.
    pub models: BTreeMap<String, u64>,
    pub first_ts_ms: Option<i64>,
    pub last_ts_ms: Option<i64>,
    pub user_turns: u64,
    pub tool_calls: Vec<ToolCall>,
    /// Provider message id -> its usage, so a response split over several lines counts once.
    ///
    /// Within one file the **first** line for a message id wins. `claude_code.py:395-403` looks
    /// like it keeps the fullest usage instead, but its enclosing guard is
    /// `mid not in state.seen_message_ids`, so the entry it compares against is always absent and
    /// the branch is unreachable. Choosing the largest `output_tokens` is a tree-wide rule that
    /// belongs to `extract.rs`'s `dedupe_usages`, not to a single file's read.
    pub usages: BTreeMap<String, Usage>,
    pub loaded_events: Vec<LoadedSetEvent>,
    pub in_band_assets: Vec<InBandAsset>,
    pub compactions: u64,
    pub last_stop_reason: Option<String>,
    pub children: Vec<SessionFacts>,
    pub lines_seen: u64,
    pub lines_unknown_type: u64,
    pub bytes_read: u64,
    pub parse_errors: u64,
    pub truncated: bool,
    /// Harness-native attribution markers per MCP server name — assistant lines that name the
    /// server that produced the response. Local names, counts only.
    pub mcp_attribution_counts: BTreeMap<String, u64>,
    /// Permission mode -> how many lines declared it.
    ///
    /// **Prototype defect fixed here, not copied** (`claude_code.py:376-382`): the Python keeps
    /// this tally in `facts._mode_counts` and additionally writes the mode names into a
    /// `_permission_modes` bucket of `forbids`, where they leak into the envelope's dynamic
    /// forbids sidecar. A permission mode is a closed enum value on the wire, so forbidding it
    /// there is both wrong and self-defeating. It is a real field here and never a forbids bucket.
    ///
    /// The winner is the most frequent mode. The Python's docstring claims ties keep the earlier
    /// mode, but `max(sorted(counts.items()), key=count)` returns the **alphabetically smallest**
    /// among ties; a `BTreeMap` iterated in key order with a strictly-greater comparison reproduces
    /// that, and the port keeps the code's behaviour rather than the docstring's.
    pub mode_counts: BTreeMap<String, u64>,
    /// Local-only strings harvested while parsing, by bucket — `slugs`, `cwd_and_branches`,
    /// `harness_session_ids`, `agent_ids`, `loaded_set_names`, `message_ids`, `tool_use_ids`.
    ///
    /// These never egress. They are fed to the gate checker as dynamic forbids, which is how the
    /// port proves that no local name reached the envelope unhashed. Nothing may write a bucket
    /// whose name starts with `_`.
    pub forbids: BTreeMap<String, BTreeSet<String>>,
}

impl Default for SessionFacts {
    /// The dataclass defaults of `base.py:138-163`, including the four [`UNKNOWN`] strings.
    fn default() -> Self {
        SessionFacts {
            ref_: SessionRef::default(),
            harness_version: UNKNOWN.to_string(),
            entrypoint: UNKNOWN.to_string(),
            permission_mode: UNKNOWN.to_string(),
            effort: UNKNOWN.to_string(),
            models: BTreeMap::new(),
            first_ts_ms: None,
            last_ts_ms: None,
            user_turns: 0,
            tool_calls: Vec::new(),
            usages: BTreeMap::new(),
            loaded_events: Vec::new(),
            in_band_assets: Vec::new(),
            compactions: 0,
            last_stop_reason: None,
            children: Vec::new(),
            lines_seen: 0,
            lines_unknown_type: 0,
            bytes_read: 0,
            parse_errors: 0,
            truncated: false,
            mcp_attribution_counts: BTreeMap::new(),
            mode_counts: BTreeMap::new(),
            forbids: BTreeMap::new(),
        }
    }
}

impl SessionFacts {
    /// Empty facts for `ref_`, matching the Python's `SessionFacts(ref=...)`.
    pub(crate) fn new(ref_: SessionRef) -> Self {
        SessionFacts {
            ref_,
            ..Default::default()
        }
    }

    /// Record a local-only name under `bucket` (`base.py:165-167`).
    ///
    /// A `None` or empty value is skipped, exactly like the Python's `if value:`. That skip is the
    /// reason the gate can treat every member of a bucket as a real name: an empty string is a
    /// substring of every value on the wire, so admitting one would make the dynamic-forbid check
    /// fail on every record and destroy the signal it exists to give.
    pub(crate) fn note_forbid(&mut self, bucket: &str, value: Option<&str>) {
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            return;
        };
        self.forbids
            .entry(bucket.to_string())
            .or_default()
            .insert(value.to_string());
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
