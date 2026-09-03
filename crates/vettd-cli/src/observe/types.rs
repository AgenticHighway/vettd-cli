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
//! [`Usage`], [`LoadedSetEvent`], [`InBandAsset`] and [`SessionFacts`] — is the middle of this
//! file and ports `sources/base.py` field for field. The derived model that `extract.rs` and
//! `attribute/` produce — [`InvocationObs`], [`TokenTotals`], [`RunFacts`], [`AssetKey`],
//! [`Segment`], [`ContextCost`], [`AssetObservation`] and [`AttributedRun`] — is the last third
//! and ports `model.py` the same way.
//!
//! The tests live in `types_tests.rs` (the `gate.rs`/`gate_tests.rs` convention). Even so this file
//! is over CONTRIBUTING.md's 400-line budget: it is one closed set of declarations that is
//! meaningless split in half, and the only way to split it is a second module file plus a
//! `pub use` re-export, which buys a shorter file at the cost of a second place to look for a
//! field. The budget is deliberately overrun here and nowhere else in `observe/`.

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

/// The five asset types as the names every producer spells (`model.py:14-18`). [`ASSET_TYPES`] is
/// the same closed set as an array; a test below pins the two together so neither can drift.
pub(crate) const ASSET_SKILL: &str = "skill";
/// See [`ASSET_SKILL`].
pub(crate) const ASSET_MCP_SERVER: &str = "mcp_server";
/// See [`ASSET_SKILL`].
pub(crate) const ASSET_AGENT: &str = "agent";
/// See [`ASSET_SKILL`].
pub(crate) const ASSET_RULES_FILE: &str = "rules_file";
/// See [`ASSET_SKILL`].
pub(crate) const ASSET_PROMPT: &str = "prompt";

/// Evidence tier of an [`AssetObservation`] (`model.py:21-23`), identical to `enums.tier` in
/// `telemetry-field-gate.json`. `Direct` is an invocation the collector saw itself, `Loaded` is a
/// harness listing, `Inferred` is a historical read reconstructed after the fact — which is every
/// row this collector produces, because it reads logs rather than watching a live run.
pub(crate) const TIER_DIRECT: &str = "direct";
/// See [`TIER_DIRECT`].
pub(crate) const TIER_LOADED: &str = "loaded";
/// See [`TIER_DIRECT`].
pub(crate) const TIER_INFERRED: &str = "inferred";

/// What an [`AssetKey::asset_id`] is a hash *of* (`model.py:25-27`), identical to
/// `enums.key_basis` in `telemetry-field-gate.json`. The precedence between them is
/// `attribute.py:29-31`'s: in-band body > local tree/file > descriptor > name.
pub(crate) const KEY_CONTENT: &str = "content_hash";
/// See [`KEY_CONTENT`].
pub(crate) const KEY_DESCRIPTOR: &str = "descriptor_hash";
/// See [`KEY_CONTENT`].
pub(crate) const KEY_NAME: &str = "name_hash";

/// How strongly an [`AssetKey`]'s hash is bound to what the harness actually loaded
/// (`model.py:29-32`), identical to `enums.binding` in `telemetry-field-gate.json`.
///
/// `harness_log_exact` hashes bytes the log itself carried; `mtime_proven` hashes files that were
/// all older than the harness's listing timestamp; `unproven` hashes files that may have changed
/// since; `not_applicable` is for keys that are not content hashes at all (descriptors, names).
pub(crate) const BINDING_EXACT: &str = "harness_log_exact";
/// See [`BINDING_EXACT`].
pub(crate) const BINDING_MTIME: &str = "mtime_proven";
/// See [`BINDING_EXACT`].
pub(crate) const BINDING_UNPROVEN: &str = "unproven";
/// See [`BINDING_EXACT`].
pub(crate) const BINDING_NA: &str = "not_applicable";

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
        // Saturating, not `+=`: the release profile sets no `overflow-checks`, so a bare `+=`
        // panics in debug and silently wraps in release — and a wrapped sum is a plausible-looking
        // wrong number on the wire. Saturating instead pins the value at the type's ceiling, which
        // is far outside every gate bound, so the payload is refused rather than believed.
        // Unreachable while `sumsq` is representable (sum^2 <= n * sumsq), but that is an argument,
        // not a guarantee.
        for &value in values {
            stats.n = stats.n.saturating_add(1);
            stats.sum = stats.sum.saturating_add(value);
            stats.min = stats.min.min(value);
            stats.max = stats.max.max(value);
            stats.sumsq = stats.sumsq.saturating_add(square(value));
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
        // Saturating for the same reason as `from_values`: a wrapped merge is silently wrong.
        Stats {
            n: self.n.saturating_add(other.n),
            sum: self.sum.saturating_add(other.sum),
            min: self.min.min(other.min),
            max: self.max.max(other.max),
            sumsq: self.sumsq.saturating_add(other.sumsq),
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

// ------------------------------------------------------------------------------------------------
// The derived model — port of `model.py:35-119`, field for field.
//
// `extract.rs` turns a [`SessionFacts`] tree into one [`RunFacts`]; `attribute/` turns that into an
// [`AttributedRun`]; `envelope.rs` is the only module that may read either onto the wire.
//
// PRIVACY INVARIANT (`model.py:1-5`): the local-only fields of `SessionFacts` do not stop being
// local-only by being derived. `RunFacts.session_key`, `RunFacts.tool_class_shares`,
// `RunFacts.mcp_corroborations`' keys, `RunFacts.forbids`, `InvocationObs.name`, `AssetKey.name`
// and `AttributedRun.name_map` all carry names, harness ids or shares that must never be written
// to the envelope: the session key is an HMAC preimage for `run_id`, an asset name is an HMAC
// preimage for `asset_id`, the shares are a `taskcat::categorize` input, and `forbids` and
// `name_map` exist precisely so the gate checker can prove none of them leaked. Every field so
// marked is annotated below; nothing else in these types is a string a session chose.
//
// Like the model above, none of these types derive `Serialize`/`Deserialize`: `envelope.rs` copies
// gate fields across by hand, so "this is local-only" stays a property of the type.
// ------------------------------------------------------------------------------------------------

/// One explicit invocation of an asset inside a run (`model.py:35-48`) — a `Skill` call, an MCP
/// tool call resolved to its server, or a sub-agent spawn resolved to its agent type.
///
/// `Default` mirrors the dataclass's defaults, so a producer writes
/// `InvocationObs { asset_type, name, ts_ms, ..Default::default() }` exactly as the Python omits
/// the trailing keywords.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InvocationObs {
    /// One of the `ASSET_*` consts; only [`DIRECT_CAPABLE_TYPES`] are ever invoked.
    pub asset_type: String,
    /// Skill, MCP server or agent-type name — **local only**, an `asset_id` HMAC preimage.
    pub name: String,
    pub ts_ms: i64,
    /// `None` for async spawns and unpaired calls (`extract.py:_latency`), because neither has a
    /// duration the harness clock can vouch for.
    pub latency_ms: Option<i64>,
    /// One of [`FAILURE_CLASSES`]; `None` when the call succeeded or was never resolved.
    pub failure_class: Option<String>,
    pub is_async: bool,
    /// A harness-native attribution marker agreed with this invocation.
    pub corroborated: bool,
    /// Exact token total of a linked child run (agents only). `None` is "no evidence", never zero.
    pub child_tokens_total: Option<i64>,
}

/// The seven envelope token buckets of one run, or of one model within it.
///
/// **Divergence from the Python, named:** the prototype carries these as `Dict[str, Optional[int]]`
/// keyed by the envelope's own key names (`extract.py:57-67` `TOKEN_BUCKETS`, `aggregate.py:38`
/// `TOKEN_KEYS` — the same seven keys in a different order, which does not matter because the
/// canonical JSON sorts keys). A struct is those same seven buckets with the key names checked by
/// the compiler, so a mistyped bucket is a build failure rather than a silently missing number in a
/// byte-compared golden envelope.
///
/// `None` means the provider never reported the bucket, which the envelope keeps distinct from an
/// observed zero (`aggregate.py:192-202`) — except for `input` and `output`, the two buckets that
/// are never null on the wire and are written as `0` when absent.
///
/// [`Self::default`] is all-absent, which is the dataclass's `field(default_factory=dict)`: an
/// empty dict and an all-`None` struct produce the same envelope and the same total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TokenTotals {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_creation: Option<i64>,
    pub cache_read: Option<i64>,
    pub cached_input: Option<i64>,
    pub thinking: Option<i64>,
    pub reasoning: Option<i64>,
}

impl TokenTotals {
    /// The accumulator `extract.py:174` starts summing from: the two never-null buckets at zero,
    /// every nullable bucket absent.
    ///
    /// The distinction is the point — a provider that reports no `cache_read` must stay "absent"
    /// rather than claim an observed zero, or the cloud would average a cache-read rate over
    /// providers that have no such bucket at all.
    pub(crate) fn zeroed_non_null() -> TokenTotals {
        TokenTotals {
            input: Some(0),
            output: Some(0),
            ..Default::default()
        }
    }
}

/// Per-run derived facts, harness-neutral — the output of `extract.rs` (`model.py:51-81`).
///
/// Scope of "the run" is `extract.py:6-18`: counts, tokens, invocations, in-band assets and
/// `forbids` merge over the whole transcript tree, while `run_outcome`, `turns`, `loaded_events`
/// and `truncated` describe the main transcript only.
///
/// **Widths:** counters are `u64` and timestamps, byte lengths and token counts are `i64`, matching
/// the [`SessionFacts`] fields they are summed from. `bytes_read` stays `u64` for that reason even
/// though it is a byte length.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RunFacts {
    /// The harness's own session id — **local only**; `envelope.rs` HMACs it into `run_id`.
    pub session_key: String,
    pub harness: String,
    pub harness_version: String,
    /// Closed enum, `extract.py:entrypoint_class`.
    pub entrypoint_class: String,
    /// Closed enum, `extract.py:effort_class`.
    pub effort: String,
    /// Closed enum, `extract.py:permission_mode`.
    pub permission_mode: String,
    /// An allowlisted model id or `"other"` (`taskcat::allowlist_model`).
    pub model: String,
    /// UTC day of `first_ts_ms` (`utc_day`), the cloud's retention key.
    pub observed_day: String,
    pub first_ts_ms: i64,
    pub last_ts_ms: i64,
    /// Closed enum, `extract.py:run_outcome`.
    pub run_outcome: String,
    pub turns: u64,
    pub tool_calls: u64,
    /// Calls whose failure class is in [`RATE_BEARING_FAILURES`].
    pub tool_failures: u64,
    pub user_denials: u64,
    pub subagent_runs: u64,
    pub compactions: u64,
    pub unpaired_tool_uses: u64,
    pub repeated_tool_calls: u64,
    pub tokens: TokenTotals,
    /// `"harness_usage"` or `"none"` (`extract.py:104`); the gate also allows `"estimated"`.
    pub tokens_basis: String,
    /// Allowlisted model id -> that model's buckets. Sub-agents may run on another model.
    pub tokens_by_model: BTreeMap<String, TokenTotals>,
    /// MCP server name -> harness attribution markers. Keys are **local only**.
    pub mcp_corroborations: BTreeMap<String, u64>,
    /// Tool class -> share of all calls, every class present. **Local only**: it is the input to
    /// `taskcat::categorize`, and only the resulting category egresses.
    pub tool_class_shares: BTreeMap<String, f64>,
    pub invocations: Vec<InvocationObs>,
    /// Main-transcript loaded-set events only — the set the segments are cut from.
    pub loaded_events: Vec<LoadedSetEvent>,
    pub in_band_assets: Vec<InBandAsset>,
    pub lines_seen: u64,
    pub lines_unknown_type: u64,
    pub bytes_read: u64,
    pub parse_errors: u64,
    /// Main-transcript truncation only; a child records its own.
    pub truncated: bool,
    /// Merged [`SessionFacts::forbids`] of the whole tree — **local only**, fed to the gate checker.
    pub forbids: BTreeMap<String, BTreeSet<String>>,
}

impl Default for RunFacts {
    /// The dataclass defaults of `model.py:66-81`.
    ///
    /// **Named divergence:** the Python's first eleven fields are required arguments and have no
    /// default at all. Rust cannot express "required" on a struct literal, so they default to empty
    /// strings and zeros here. That is a construction convenience for tests (`test_attribute.py:74`
    /// builds a run from three arguments); `extract()` sets every field explicitly and must never
    /// spread this default, because an empty `model`, `observed_day` or `run_outcome` is not a
    /// value the gate accepts.
    fn default() -> Self {
        RunFacts {
            session_key: String::new(),
            harness: String::new(),
            harness_version: String::new(),
            entrypoint_class: String::new(),
            effort: String::new(),
            permission_mode: String::new(),
            model: String::new(),
            observed_day: String::new(),
            first_ts_ms: 0,
            last_ts_ms: 0,
            run_outcome: String::new(),
            turns: 0,
            tool_calls: 0,
            tool_failures: 0,
            user_denials: 0,
            subagent_runs: 0,
            compactions: 0,
            unpaired_tool_uses: 0,
            repeated_tool_calls: 0,
            tokens: TokenTotals::default(),
            tokens_basis: "none".to_string(),
            tokens_by_model: BTreeMap::new(),
            mcp_corroborations: BTreeMap::new(),
            tool_class_shares: BTreeMap::new(),
            invocations: Vec::new(),
            loaded_events: Vec::new(),
            in_band_assets: Vec::new(),
            lines_seen: 0,
            lines_unknown_type: 0,
            bytes_read: 0,
            parse_errors: 0,
            truncated: false,
            forbids: BTreeMap::new(),
        }
    }
}

/// The identity one loaded asset has inside one segment (`model.py:84-91`).
///
/// Ordering is derived and therefore by `asset_id` first, which is the order `attribute.py:387`
/// sorts observations into and the order the envelope's `bom[].asset_ids` must be in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AssetKey {
    /// hex64: an HMAC over `"<asset_type>:<name>"`, or a sha256 of content or of a descriptor,
    /// depending on `key_basis`.
    pub asset_id: String,
    /// One of the `ASSET_*` consts.
    pub asset_type: String,
    /// One of [`KEY_CONTENT`], [`KEY_DESCRIPTOR`], [`KEY_NAME`].
    pub key_basis: String,
    /// The asset's own name — **local only** (display and the dynamic forbids); never egresses.
    pub name: String,
    /// One of the `BINDING_*` consts.
    pub binding: String,
}

impl Default for AssetKey {
    /// Empty strings with `binding` at [`BINDING_NA`], which is the dataclass's only defaulted
    /// field (`model.py:91`). Nothing but a test builds a key this way.
    fn default() -> Self {
        AssetKey {
            asset_id: String::new(),
            asset_type: String::new(),
            key_basis: String::new(),
            name: String::new(),
            binding: BINDING_NA.to_string(),
        }
    }
}

impl AssetKey {
    /// Build a key from the five borrowed values, in the positional order `attribute.py:_key_for`
    /// passes them.
    pub(crate) fn new(
        asset_id: &str,
        asset_type: &str,
        key_basis: &str,
        name: &str,
        binding: &str,
    ) -> AssetKey {
        AssetKey {
            asset_id: asset_id.to_string(),
            asset_type: asset_type.to_string(),
            key_basis: key_basis.to_string(),
            name: name.to_string(),
            binding: binding.to_string(),
        }
    }
}

/// A stretch of a run with one loaded set (`model.py:94-104`). A new segment starts only when the
/// settle rule in `attribute/segments.rs` says the loaded set genuinely changed, so a segment
/// boundary is evidence of a configuration change and not of an async connect completing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Segment {
    pub index: usize,
    pub start_ts_ms: i64,
    pub end_ts_ms: i64,
    /// Closed enum: `"harness_log"`, `"filesystem"` or `"none"` (`attribute.py:57-59`).
    pub loaded_set_basis: String,
    pub asset_keys: Vec<AssetKey>,
    /// sha256 of the sorted, de-duplicated `asset_id`s of `asset_keys`; empty until computed.
    pub bom_version: String,
}

/// An estimated context cost in tokens and the method that produced it — the port of
/// `AssetObservation.context_cost_est`'s `Tuple[int, str]` (`model.py:114`), which the envelope
/// writes as `{"tokens": …, "method": …}` (`aggregate.py:222-223`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ContextCost {
    pub tokens: i64,
    /// Closed enum `enums.context_cost_method`: one of `listing_bytes_div4`, `file_bytes_div4`,
    /// `tool_schema_bytes_div4`, `none`.
    pub method: String,
}

/// What was observed about one asset in one segment (`model.py:107-115`).
///
/// Mutable rather than frozen because `aggregate.py:120-147` merges an asset's observations across
/// segments in place; the merge is Phase 4's, the mutability it needs is declared here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AssetObservation {
    pub key: AssetKey,
    /// One of [`TIER_DIRECT`], [`TIER_LOADED`], [`TIER_INFERRED`]. Every row this collector
    /// produces is `inferred`: it reads finished logs (`attribute.py:_observe`).
    pub tier: String,
    /// Whether a live collector could have attributed this asset `direct` — i.e. whether the run
    /// contains an invocation of it. It is not a claim that this row is direct evidence.
    pub direct_evidence_available: bool,
    pub invocations: Vec<InvocationObs>,
    pub context_cost_est: Option<ContextCost>,
    /// Harness-native attribution markers for this asset in this segment. `None` (not `0`) when
    /// there is nothing to corroborate, because the harness may simply emit no such marker.
    pub harness_corroborations: Option<u64>,
}

/// A run with its segments and per-segment observations — the output of `attribute()`
/// (`model.py:118-123`) and the only input `envelope.rs` accepts.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AttributedRun {
    pub run: RunFacts,
    pub segments: Vec<Segment>,
    /// Segment index -> that segment's observations, sorted by `asset_id`.
    pub observations: BTreeMap<usize, Vec<AssetObservation>>,
    /// `asset_id` -> `"<asset_type>:<name>"` — **local only**. It is what the ranking prints and
    /// what `collect_dynamic` turns into the gate's forbidden needles; it never egresses.
    pub name_map: BTreeMap<String, String>,
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
