//! The Claude Code session source: discovery, streaming read, and parent/child linkage.
//!
//! Port of `spikes/828-passive-observer/prototype/sources/claude_code.py` — the class
//! `ClaudeCodeSource` (lines 78-153) and `link_children` (lines 156-165). The three halves it
//! delegates to live beside this file: [`discover`] walks the harness home, [`project`] reduces one
//! raw line to hashes, lengths and booleans, and [`apply`] folds the result into [`SessionFacts`].
//!
//! **Privacy invariant.** [`ClaudeCodeSource::read`] is the only place a session's bytes are held,
//! and it holds them for exactly as long as it takes to parse one line and project it: the
//! [`serde_json::Value`] is dropped before the next line is read, and nothing it contained survives
//! except hashes, byte lengths, booleans, counts and the local-only names the gate consumes as
//! dynamic forbids. `no_content_string_survives_parse` is the executable form of that claim.
//!
//! **Named divergence from the Python.** `ClaudeCodeSource.discover` assigns `self._now_ms = now_ms`
//! so a later `read` can reuse it (`claude_code.py:88`). Here `discover` takes `&self` — a source
//! that rewrote itself during discovery would make the truncation verdict depend on call order — so
//! `now_ms` is fixed when the source is constructed and `discover`'s argument is used only for the
//! window. The pipeline builds the source with the same `now_ms` it passes to `discover`, so the two
//! agree; a source built without one falls back to the wall clock at read time, exactly as
//! `_is_truncated` does (`claude_code.py:151`).

#[path = "apply.rs"]
mod apply;
#[path = "discover.rs"]
mod discover;
#[path = "project.rs"]
mod project;

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::source::{inode_of, resume_offset, Line, LineReader, Source};
use super::types::{Cursor, SessionFacts, SessionKind, SessionRef};

/// The harness identifier this source reports and stamps on every [`SessionRef`]
/// (`base.py:17`, `HARNESS_CLAUDE_CODE`). It is also the `harness` value the envelope carries, so
/// the string is closed by `telemetry-field-gate.json`.
pub(crate) const HARNESS_CLAUDE_CODE: &str = "claude_code";

/// How close to `now` a session file's mtime must be for the session to count as still running
/// (`claude_code.py:45`, `TRUNCATION_GRACE_MS`).
const TRUNCATION_GRACE_MS: i64 = 120_000;

/// The `stop_reason` of a response that ended a turn cleanly. A read whose last one is anything
/// else, on a file the harness touched moments ago, is a read of a session still in flight.
const END_TURN: &str = "end_turn";

/// The `child_meta` key naming the sub-agent type a child session's sidecar declared, which an
/// assistant line's `attributionAgent` may corroborate (`claude_code.py:130`).
const AGENT_TYPE_KEY: &str = "agentType";

/// The `child_meta` flag set when an assistant line in the child's own transcript names the agent
/// type its sidecar declared (`claude_code.py:146-147`). Harness-native corroboration: it is the
/// difference between "the path says this is a sub-agent run" and "the harness said so too".
const CORROBORATED_KEY: &str = "corroborated";

/// Reads Claude Code's on-disk session transcripts under one harness home.
///
/// `root` is the harness home (`~/.claude` in production, a fixture tree in tests) and is used only
/// as the default the caller passes back to [`Source::discover`]. `now_ms` pins the collector's
/// notion of "now" for the truncation verdict; `None` reads the wall clock at read time.
pub(crate) struct ClaudeCodeSource {
    root: PathBuf,
    now_ms: Option<i64>,
}

impl ClaudeCodeSource {
    /// A source rooted at `root`, taking "now" from the wall clock (`claude_code.py:81-83`).
    pub(crate) fn new(root: PathBuf) -> Self {
        ClaudeCodeSource { root, now_ms: None }
    }

    /// A source rooted at `root` with "now" pinned to `now_ms`.
    ///
    /// The pipeline uses this so every session in one run is judged against a single instant, and
    /// the tests use it so the truncation verdict is a function of the fixture rather than of when
    /// the suite happens to run.
    pub(crate) fn with_now_ms(root: PathBuf, now_ms: i64) -> Self {
        ClaudeCodeSource {
            root,
            now_ms: Some(now_ms),
        }
    }

    /// The harness home this source was built for.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Whether a session is still being written (`_is_truncated`, `claude_code.py:150-153`).
    ///
    /// Both halves are required: a file the harness touched within [`TRUNCATION_GRACE_MS`] whose
    /// last response ended the turn is a finished session that merely happens to be recent, and an
    /// unfinished-looking transcript nobody has touched for hours is an abandoned session, not a
    /// live one. The comparison is absolute so a collector clock behind the harness clock does not
    /// silently turn every live session into a settled one.
    fn is_truncated(&self, facts: &SessionFacts, meta: &Metadata) -> bool {
        let now_ms = self.now_ms.unwrap_or_else(wall_clock_ms);
        let recent = discover::mtime_ms_of(meta).is_some_and(|mtime_ms| {
            now_ms.saturating_sub(mtime_ms).saturating_abs() <= TRUNCATION_GRACE_MS
        });
        recent && facts.last_stop_reason.as_deref() != Some(END_TURN)
    }
}

impl Source for ClaudeCodeSource {
    fn harness(&self) -> &'static str {
        HARNESS_CLAUDE_CODE
    }

    fn discover(
        &self,
        root: &Path,
        window_days: u32,
        now_ms: i64,
    ) -> Result<Vec<SessionRef>, String> {
        Ok(discover::discover(
            root,
            HARNESS_CLAUDE_CODE,
            window_days,
            now_ms,
        ))
    }

    /// Stream one session file into [`SessionFacts`], resuming from `cursor` when it is still valid.
    ///
    /// Port of `claude_code.py:125-148`, line for line: every complete line bumps `lines_seen` and
    /// `bytes_read`; a line that is not a JSON object — malformed, or a bare scalar, or longer than
    /// the reader's ceiling — is one `parse_errors` and nothing else; a line whose `type` is outside
    /// `CONSUMED_TYPES` is one `lines_unknown_type` and is never interpreted; everything else is
    /// projected and applied.
    ///
    /// The returned [`Cursor`] is one byte past the last **complete** line, which equals the offset
    /// the read started at when the file held nothing new. A partial trailing line is left for the
    /// next read (see [`LineReader`]).
    fn read(
        &self,
        r: &SessionRef,
        cursor: Option<&Cursor>,
    ) -> Result<(SessionFacts, Cursor), String> {
        let meta =
            std::fs::metadata(&r.path).map_err(|e| format!("stat {}: {e}", r.path.display()))?;
        let start = resume_offset(cursor, &r.path, &meta);
        let mut facts = SessionFacts::new(r.clone());
        let mut state = apply::ReadState::new();
        let expected_agent = expected_agent(r);
        let mut reader = LineReader::open(&r.path, start)?;
        while let Some(item) = reader.next_line() {
            consume(&mut facts, &mut state, item?, expected_agent.as_deref());
        }
        facts.truncated = self.is_truncated(&facts, &meta);
        if r.kind == SessionKind::Child && state.corroborated {
            facts
                .ref_
                .child_meta
                .insert(CORROBORATED_KEY.to_string(), "true".to_string());
        }
        Ok((
            facts,
            Cursor {
                path: r.path.clone(),
                byte_offset: reader.offset(),
                inode: inode_of(&meta),
            },
        ))
    }
}

/// The `agentType` a child session's sidecar declared, or `None` for a main transcript
/// (`claude_code.py:130`).
fn expected_agent(r: &SessionRef) -> Option<String> {
    if r.kind != SessionKind::Child {
        return None;
    }
    r.child_meta.get(AGENT_TYPE_KEY).cloned()
}

/// Fold one line from the reader into the growing facts (`claude_code.py:133-144`).
///
/// The raw [`Value`] is dropped before this returns — that drop is the privacy boundary, and it is
/// why nothing below the projection ever sees a transcript string.
fn consume(
    facts: &mut SessionFacts,
    state: &mut apply::ReadState,
    line: Line,
    expected_agent: Option<&str>,
) {
    let (line_len, bytes) = match line {
        Line::Complete { bytes, .. } => (bytes.len() as u64, bytes),
        // A line past the reader's ceiling was drained rather than assembled. It is counted like
        // any other line the parser could not turn into an object: seen, measured, one parse error.
        Line::Oversized { byte_len, .. } => {
            facts.lines_seen += 1;
            facts.bytes_read += byte_len;
            facts.parse_errors += 1;
            return;
        }
    };
    facts.lines_seen += 1;
    facts.bytes_read += line_len;
    // `_decode` (`claude_code.py:591-596`): anything that is not a JSON *object* is a parse error,
    // including a well-formed bare scalar or array.
    let Ok(raw) = serde_json::from_slice::<Value>(&bytes) else {
        facts.parse_errors += 1;
        return;
    };
    if !raw.is_object() {
        facts.parse_errors += 1;
        return;
    }
    match project::project(&raw, line_len, expected_agent) {
        Some(projected) => {
            drop(raw);
            apply::apply(facts, state, projected);
        }
        None => facts.lines_unknown_type += 1,
    }
}

/// The collector's wall clock in milliseconds, for a source built without a pinned `now_ms`.
///
/// A clock before the epoch yields 0, which makes every session look settled rather than every
/// session look live: an unreadable clock must not manufacture truncation warnings.
fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// Attach each child's facts to its parent's `children`, and return the mains
/// (`link_children`, `claude_code.py:156-165`).
///
/// The caller reads every discovered ref — main and child alike — separately, so each file keeps its
/// own cursor; this is what puts the tree back together afterwards. A child whose parent is not
/// among `all` is dropped: its parent's transcript fell outside the window or failed to read, and a
/// sub-agent run with no run to belong to has nothing to be attributed to.
///
/// Mains keep the order they arrived in, and a repeated `session_key` keeps the first main's
/// position while the later facts win — Python dict semantics, reproduced because the pipeline's
/// grouping reads this order.
pub(crate) fn link_children(all: Vec<SessionFacts>) -> Vec<SessionFacts> {
    let mut mains: Vec<SessionFacts> = Vec::new();
    let mut index: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut children: Vec<SessionFacts> = Vec::new();
    for facts in all {
        if facts.ref_.kind != SessionKind::Main {
            children.push(facts);
            continue;
        }
        match index.get(&facts.ref_.session_key) {
            Some(&at) => mains[at] = facts,
            None => {
                index.insert(facts.ref_.session_key.clone(), mains.len());
                mains.push(facts);
            }
        }
    }
    for child in children {
        let parent = child
            .ref_
            .parent_key
            .as_deref()
            .and_then(|key| index.get(key));
        if let Some(&at) = parent {
            mains[at].children.push(child);
        }
    }
    mains
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
