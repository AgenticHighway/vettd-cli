//! Discovery of Claude Code session files under a harness home.
//!
//! Port of the "discovery" section of
//! `spikes/828-passive-observer/prototype/sources/claude_code.py` (lines 85-121 and the
//! `_listdir` / `_session_stem` / `_within_window` / `_read_child_meta` helpers at 546-580).
//!
//! The layout read (verified by the prototype against harness 2.1.258) is:
//!
//! ```text
//! <root>/projects/<project>/<session>.{jsonl,ndjson}                                  main
//! <root>/projects/<project>/<session>/subagents/agent-<id>.{jsonl,ndjson}             child
//! <root>/projects/<project>/<session>/subagents/agent-<id>.meta.json                  child sidecar
//! <root>/projects/<project>/<session>/subagents/workflows/<wf>/agent-<id>.{jsonl,ndjson}  child
//! ```
//!
//! Both suffixes are discovered: the harness writes `.jsonl`, and the repository's `.gitignore`
//! ignores `*.jsonl`, so the checked-in fixtures use `.ndjson`.
//!
//! **Discovery never fails.** Every listing, stat and sidecar read that errors is treated as
//! "nothing here" rather than propagated, exactly as the Python's `try/except OSError` helpers do.
//! A home with no `projects/` directory yields no refs, which is what lets `vettd observe` run on a
//! machine that has never used the harness without turning an empty disk into an error.
//!
//! **Order is part of the contract.** The Python sorts `os.listdir` at every level, and the order
//! of the returned refs is what the pipeline's grouping of children under their parent depends on.
//! [`std::fs::read_dir`] is unordered, so every listing here is collected and sorted explicitly, by
//! the raw entry name — the same key `sorted(os.listdir(...))` uses.
//!
//! Nothing in this module reads a session file's contents; it only names paths and reads the tiny
//! `.meta.json` sidecar, whose three allowlisted keys are local-only (`SessionRef.child_meta`).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, Metadata};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;

use crate::observe::types::{SessionKind, SessionRef};

/// Suffixes a session transcript may carry (`claude_code.py:44`, `SESSION_SUFFIXES`).
///
/// Order matters only in that the Python tries `.jsonl` first; no name can end with both.
const SESSION_SUFFIXES: [&str; 2] = [".jsonl", ".ndjson"];

/// Milliseconds in a day, the unit `window_days` is expressed in.
const MS_PER_DAY: i64 = 86_400_000;

/// The stem prefix that marks a sub-agent transcript (`claude_code.py:110`).
const CHILD_PREFIX: &str = "agent-";

/// The sidecar keys kept from `<stem>.meta.json` (`claude_code.py:576`).
///
/// An allowlist, not a filter: any other key the harness writes into the sidecar — the fixture's
/// `description`, for instance, which is free text — cannot reach [`SessionRef::child_meta`].
const CHILD_META_KEYS: [&str; 3] = ["agentType", "toolUseId", "spawnDepth"];

/// Every session file under `root` whose mtime falls inside the window.
///
/// `cutoff = now_ms - window_days * 86_400_000`; a file is in-window when it is a regular file and
/// its mtime in milliseconds is `>= cutoff` (see [`within_window`]).
///
/// Refs come back in the Python's order: projects sorted by name, and within each project, for each
/// transcript entry in sorted order, the main (when in window), then its `subagents/` children,
/// then the children under each sorted `subagents/workflows/<wf>/`.
///
/// Two behaviours of the Python are load-bearing and are reproduced rather than tidied away:
///
/// * A main that is *out* of window still has its children discovered — the window guard covers
///   only the main's own ref. A long-running parent whose transcript went quiet must not hide the
///   sub-agent runs that happened inside the window.
/// * If a session has transcripts under *both* suffixes, its children are enumerated once per
///   transcript, so duplicate child refs are returned. That pairing is not a real harness state,
///   and de-duplicating here would silently drop one of the two mains' children.
pub(super) fn discover(
    root: &Path,
    harness: &'static str,
    window_days: u32,
    now_ms: i64,
) -> Vec<SessionRef> {
    let cutoff = now_ms.saturating_sub(i64::from(window_days).saturating_mul(MS_PER_DAY));
    let projects = root.join("projects");
    let mut refs: Vec<SessionRef> = Vec::new();
    for project in listdir(&projects) {
        let pdir = projects.join(&project);
        for entry in listdir(&pdir) {
            let Some(stem) = session_stem(&entry) else {
                continue;
            };
            let path = pdir.join(&entry);
            if within_window(&path, cutoff) {
                refs.push(SessionRef {
                    path,
                    harness: harness.to_string(),
                    session_key: stem.to_string(),
                    kind: SessionKind::Main,
                    parent_key: None,
                    child_meta: BTreeMap::new(),
                });
            }
            let subagents = pdir.join(stem).join("subagents");
            refs.extend(discover_children(&subagents, stem, harness, cutoff));
            let workflows = subagents.join("workflows");
            for wf in listdir(&workflows) {
                refs.extend(discover_children(
                    &workflows.join(&wf),
                    stem,
                    harness,
                    cutoff,
                ));
            }
        }
    }
    refs
}

/// The in-window sub-agent transcripts directly inside `dir` (`claude_code.py:104-118`).
///
/// `dir` is either a `<stem>/subagents` directory or one `<stem>/subagents/workflows/<wf>`
/// directory; the walk is never deeper than that, because the harness does not nest further.
///
/// An entry is a child only when its stem parses as a transcript *and* starts with `agent-`; the
/// `session_key` is the part after that prefix, and the same id is echoed into `child_meta` under
/// `agentId` so a later pass can link a `Task` tool call to the child it spawned without re-parsing
/// the path.
pub(super) fn discover_children(
    dir: &Path,
    parent_key: &str,
    harness: &'static str,
    cutoff_ms: i64,
) -> Vec<SessionRef> {
    let mut refs: Vec<SessionRef> = Vec::new();
    for entry in listdir(dir) {
        let Some(stem) = session_stem(&entry) else {
            continue;
        };
        let Some(agent_id) = stem.strip_prefix(CHILD_PREFIX) else {
            continue;
        };
        let path = dir.join(&entry);
        if !within_window(&path, cutoff_ms) {
            continue;
        }
        let mut child_meta = read_child_meta(&dir.join(format!("{stem}.meta.json")));
        child_meta.insert("agentId".to_string(), agent_id.to_string());
        refs.push(SessionRef {
            path,
            harness: harness.to_string(),
            session_key: agent_id.to_string(),
            kind: SessionKind::Child,
            parent_key: Some(parent_key.to_string()),
            child_meta,
        });
    }
    refs
}

/// The names in `dir`, sorted; an empty list when `dir` cannot be listed (`claude_code.py:548`).
///
/// Swallowing the error is deliberate and matches the Python: a project directory that has been
/// removed, or a home that has no `projects/` at all, means "no sessions here", not a failed run.
///
/// The sort key is the raw entry name, as [`OsString`]. On Unix that compares the underlying
/// bytes, which for valid UTF-8 is the same order as Python's code-point comparison of the decoded
/// `str`, so the two implementations agree on every name a harness actually writes.
pub(super) fn listdir(dir: &Path) -> Vec<OsString> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<OsString> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
        .collect();
    names.sort();
    names
}

/// The session stem of `entry`, or `None` when it is not a transcript (`claude_code.py:555`).
///
/// The suffix must be a strict suffix: a file named exactly `.jsonl` is not a session whose key is
/// the empty string, it is a dotfile, and `len(entry) > len(suffix)` in the Python says so.
///
/// **Named divergence:** a name that is not valid UTF-8 is skipped instead of being lossily
/// decoded. The stem becomes `session_key`, which is HMAC'd into `run_id`; a `U+FFFD` substitution
/// would silently produce a *different* run identity for the same session, so refusing the file is
/// the honest choice. Harness session names are UUIDs, so this cannot fire in practice.
pub(super) fn session_stem(entry: &OsStr) -> Option<&str> {
    let name = entry.to_str()?;
    SESSION_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .filter(|stem| !stem.is_empty())
}

/// Whether `path` is a regular file whose mtime is at or after `cutoff_ms` (`claude_code.py:562`).
///
/// Any failure — a missing file, a broken symlink, a directory, an unreadable stat, an mtime the
/// platform cannot express — answers "not in window" rather than raising, mirroring the Python's
/// `except OSError: return False`. Discovery must not abort because one entry in a project
/// directory happens to be unreadable.
pub(super) fn within_window(path: &Path, cutoff_ms: i64) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    // A pre-epoch mtime cannot be expressed as a positive `Duration`. It is older than any window
    // the caller can express, so it is out of window — which is also what the Python computes.
    mtime_ms_of(&meta).is_some_and(|mtime_ms| mtime_ms >= cutoff_ms)
}

/// `int(os.stat(path).st_mtime * 1000)` for an already-stat'd file, or `None` when the platform
/// cannot express the mtime as a post-epoch instant.
///
/// Shared with the reader, which needs the same number for the truncation verdict
/// (`claude_code.py:152`); the two must agree, so there is one conversion and not two.
pub(super) fn mtime_ms_of(meta: &Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(mtime_ms(since_epoch))
}

/// `int(st_mtime * 1000)` — the Python's truncating conversion, reproduced.
///
/// `os.stat().st_mtime` is a C double built as `seconds + 1e-9 * nanoseconds`, and the Python
/// multiplies *that* by 1000 before truncating. Computing the milliseconds exactly from the integer
/// nanoseconds instead would disagree with the prototype by one millisecond on timestamps whose
/// fractional part is not representable in binary, so the double is reproduced deliberately.
fn mtime_ms(since_epoch: Duration) -> i64 {
    let seconds = since_epoch.as_secs() as f64 + f64::from(since_epoch.subsec_nanos()) * 1e-9;
    (seconds * 1000.0).trunc() as i64
}

/// The three allowlisted values from a `<stem>.meta.json` sidecar (`claude_code.py:568`).
///
/// A missing, unreadable, non-object or malformed sidecar yields an empty map — a sub-agent
/// transcript is still worth reading when its sidecar is gone, and the sidecar is the harness's
/// bookkeeping, not the session.
///
/// A value survives only when it is a string or a non-boolean integer, and is stringified. JSON
/// `true` is an `int` in Python's `isinstance` sense, which is why the Python excludes `bool`
/// explicitly; serde_json types it separately, so the exclusion here is structural. A fractional
/// number is dropped by both (`isinstance(1.0, (str, int))` is false).
pub(super) fn read_child_meta(path: &Path) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let Ok(bytes) = fs::read(path) else {
        return out;
    };
    let Ok(Value::Object(meta)) = serde_json::from_slice::<Value>(&bytes) else {
        return out;
    };
    for key in CHILD_META_KEYS {
        match meta.get(key) {
            Some(Value::String(value)) => {
                out.insert(key.to_string(), value.clone());
            }
            Some(Value::Number(value)) if value.is_i64() || value.is_u64() => {
                out.insert(key.to_string(), value.to_string());
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
#[path = "discover_tests.rs"]
mod tests;
