//! Segmentation: cutting a run into stretches that share one loaded set.
//!
//! Ports the segmentation half of
//! `spikes/828-passive-observer/prototype/attribute.py` — `_SegState`, `folds`,
//! `settle`, `_segment_for`, `_basis`, the `end_ts` rule and `bom_version`. The
//! Python is the reference semantics; where this file and `CONTRACTS.md`
//! disagree, the Python code wins.
//!
//! A harness emits one `initial` loaded-set listing and then `delta` events. Not
//! every delta is a change to *what is loaded*: Claude Code reports an MCP server
//! as `pending` while it connects and then emits a delta carrying that server's
//! tools once the connection lands. That delta is the same loaded set finishing
//! its arrival, so folding it into the current segment is what keeps
//! `counts.loaded_set_changes` a count of configuration changes rather than a
//! count of async connects. [`folds`] is that rule, and it is deliberately
//! narrow: any removal, re-add, skill, agent or rules-file change, or any added
//! tool whose server was never announced pending, cuts a new segment.
//!
//! **Privacy.** [`SegState`] is local-only in its entirety: `names`, `mcp_tools`,
//! `listed_ts`, `listing_bytes`, `schema_bytes` and `mcp_corroborations` are all
//! keyed by asset and server *names*. Nothing here egresses; `attribute/mod.rs`
//! turns every name into a hash before an envelope is built. The one value this
//! module produces that does reach the wire is [`bom_version`], which is a
//! SHA-256 over already-hashed asset ids.

use std::collections::{BTreeMap, BTreeSet};

use crate::observe::canonical::hex_sha256;
use crate::observe::types::{
    LoadedSetEvent, LoadedSetKind, ASSET_AGENT, ASSET_MCP_SERVER, ASSET_RULES_FILE, ASSET_SKILL,
    BUILTIN_AGENT_TYPES,
};

/// The harness itself listed what was loaded (`attribute.py:57`).
pub(super) const BASIS_HARNESS_LOG: &str = "harness_log";
/// No listing in the log; the loaded set is what the filesystem knows (`attribute.py:58`).
pub(super) const BASIS_FILESYSTEM: &str = "filesystem";
/// Neither a listing nor any local asset (`attribute.py:59`).
pub(super) const BASIS_NONE: &str = "none";

/// `mcp__<server>__<tool>` -> `server`; `None` for anything else (`attribute.py:93-98`).
///
/// The Python requires the `mcp__` prefix, at least three `__`-separated parts and a non-empty
/// second part, so `mcp____tool` and `mcp__srv` are both `None` while `mcp__srv__a__b` is `srv`.
pub(super) fn mcp_server_of(tool_name: &str) -> Option<&str> {
    if !tool_name.starts_with("mcp__") {
        return None;
    }
    let parts: Vec<&str> = tool_name.split("__").collect();
    match parts.get(1) {
        Some(server) if parts.len() >= 3 && !server.is_empty() => Some(server),
        _ => None,
    }
}

/// SHA-256 over the sorted, de-duplicated asset ids joined by `,` (`attribute.py:87-91`).
///
/// Identical to `aggregate.bom_version_for`, so a segment's `bom_version` always equals the hash
/// of the `bom[]` entry that is emitted for it. Python sorts `str` by code point; `BTreeSet<&str>`
/// orders by UTF-8 bytes, which is the same order, so the two agree byte for byte. The empty set
/// hashes the empty string (`e3b0c442…`), which is a legal value: a segment can load nothing.
pub(crate) fn bom_version<'a, I>(asset_ids: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    let unique: BTreeSet<&str> = asset_ids.into_iter().collect();
    let joined = unique.into_iter().collect::<Vec<&str>>().join(",");
    hex_sha256(joined.as_bytes())
}

/// Which loaded set a segment's membership was derived from (`attribute.py:430-435`).
///
/// Split out of the Python's `_basis` so this module need not know about `FsIndex`: the caller
/// passes the two booleans. A harness listing always wins; the filesystem is the fallback for a
/// harness that never says what it loaded.
pub(super) fn loaded_set_basis(has_loaded_events: bool, has_listed_assets: bool) -> &'static str {
    if has_loaded_events {
        BASIS_HARNESS_LOG
    } else if has_listed_assets {
        BASIS_FILESYSTEM
    } else {
        BASIS_NONE
    }
}

/// One segment under construction (`attribute.py:296-357`).
///
/// `names` holds the loaded names by asset type, and MCP membership is tracked at *tool*
/// granularity in `mcp_tools`: a server is a member while it has at least one live tool, so a
/// delta that removes a server's last tool drops the server from the next segment. `listed_ts`
/// records the harness timestamp each name was first listed at — the mtime rule in
/// `attribute/mod.rs` compares against it — and the two byte maps carry the counts behind the
/// context-cost estimates.
///
/// **Local-only.** Every map here is keyed by a name; see the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SegState {
    /// Position of this segment in the run, and its `Segment.index` on the wire.
    pub index: usize,
    /// Harness timestamp the segment opens at (ms).
    pub start_ts: i64,
    /// Asset type -> names loaded in this segment, for everything but MCP servers.
    names: BTreeMap<String, BTreeSet<String>>,
    /// MCP server -> its live tool names. A server with no tools is not a member.
    mcp_tools: BTreeMap<String, BTreeSet<String>>,
    /// `(asset_type, name)` -> the timestamp of the *first* listing that named it.
    pub listed_ts: BTreeMap<(String, String), i64>,
    /// Skill name -> bytes of its listing lines, summed across the segment's events.
    pub listing_bytes: BTreeMap<String, i64>,
    /// MCP server -> bytes of its tool-schema lines, summed across the segment's events.
    pub schema_bytes: BTreeMap<String, i64>,
    /// MCP server -> assistant lines the harness itself attributed to it. Not derived here:
    /// `attribute.py:394-395` copies `RunFacts.mcp_corroborations` onto every segment after
    /// [`settle`], and the assembler does the same.
    pub mcp_corroborations: BTreeMap<String, u64>,
}

impl SegState {
    /// An empty segment numbered `index`, opening at `start_ts`.
    pub(super) fn new(index: usize, start_ts: i64) -> SegState {
        SegState {
            index,
            start_ts,
            ..SegState::default()
        }
    }

    /// The next segment, carrying this one's membership forward (`attribute.py:311-319`).
    ///
    /// A segment boundary is a *change* to the loaded set, not a reset of it: everything still
    /// loaded stays loaded, and the delta that caused the split is then absorbed into the fork.
    pub(super) fn fork(&self, index: usize, start_ts: i64) -> SegState {
        SegState {
            index,
            start_ts,
            names: self.names.clone(),
            mcp_tools: self.mcp_tools.clone(),
            listed_ts: self.listed_ts.clone(),
            listing_bytes: self.listing_bytes.clone(),
            schema_bytes: self.schema_bytes.clone(),
            mcp_corroborations: self.mcp_corroborations.clone(),
        }
    }

    /// Add `name` to the segment, recording `ts` as its listing timestamp if it has none yet.
    ///
    /// First listing wins (`setdefault`, `attribute.py:321-324`): a later re-listing must not
    /// move the timestamp the mtime binding is proven against.
    pub(super) fn add(&mut self, asset_type: &str, name: &str, ts: Option<i64>) {
        self.names
            .entry(asset_type.to_string())
            .or_default()
            .insert(name.to_string());
        if let Some(ts) = ts {
            self.listed_ts
                .entry((asset_type.to_string(), name.to_string()))
                .or_insert(ts);
        }
    }

    /// Fold one loaded-set event into the segment (`attribute.py:326-341`).
    ///
    /// Built-in agent types are skipped here rather than filtered later: they are not assets, so
    /// they must never become segment members.
    pub(super) fn absorb(&mut self, ev: &LoadedSetEvent) {
        for name in &ev.skills {
            self.add(ASSET_SKILL, name, Some(ev.ts_ms));
        }
        for (name, bytes) in &ev.listing_bytes {
            *self.listing_bytes.entry(name.clone()).or_insert(0) += bytes;
        }
        for name in &ev.rules_files {
            self.add(ASSET_RULES_FILE, name, Some(ev.ts_ms));
        }
        for name in &ev.agent_types {
            if !BUILTIN_AGENT_TYPES.contains(&name.as_str()) {
                self.add(ASSET_AGENT, name, Some(ev.ts_ms));
            }
        }
        for name in &ev.removed {
            self.mcp_tool(name, ev.ts_ms, false);
        }
        for name in ev.tool_names.iter().chain(ev.readded.iter()) {
            self.mcp_tool(name, ev.ts_ms, true);
        }
        for (server, bytes) in &ev.tool_schema_bytes {
            *self.schema_bytes.entry(server.clone()).or_insert(0) += bytes;
        }
    }

    /// Track one tool's presence for its MCP server (`attribute.py:343-352`). Non-MCP tool names
    /// are ignored: `Bash` is a harness built-in, not an asset.
    fn mcp_tool(&mut self, tool_name: &str, ts: i64, present: bool) {
        let Some(server) = mcp_server_of(tool_name) else {
            return;
        };
        let tools = self.mcp_tools.entry(server.to_string()).or_default();
        if present {
            tools.insert(tool_name.to_string());
            self.listed_ts
                .entry((ASSET_MCP_SERVER.to_string(), server.to_string()))
                .or_insert(ts);
        } else {
            tools.remove(tool_name);
        }
    }

    /// The segment's `(asset_type, name)` members, sorted (`attribute.py:354-357`).
    ///
    /// Sorted because the order decides the order observations are built in, and `Segment` rows
    /// are compared byte for byte against the golden envelope downstream.
    pub(super) fn members(&self) -> Vec<(String, String)> {
        let mut out: BTreeSet<(String, String)> = BTreeSet::new();
        for (asset_type, names) in &self.names {
            for name in names {
                out.insert((asset_type.clone(), name.clone()));
            }
        }
        for (server, tools) in &self.mcp_tools {
            if !tools.is_empty() {
                out.insert((ASSET_MCP_SERVER.to_string(), server.clone()));
            }
        }
        out.into_iter().collect()
    }
}

/// The settle rule (`attribute.py:360-366`).
///
/// A delta folds into the current segment — rather than opening a new one — when it removes and
/// re-adds nothing, touches no skill, agent or rules file, and *every* added tool is
/// `mcp__<S>__*` for a server `S` the harness had already reported pending. That is the shape of
/// an async MCP connect completing, which is not a change to what is loaded.
///
/// Two quantifier details the Python fixes and this port keeps: it is **every** added tool, not
/// any (a delta that adds one pending server's tools *and* one unannounced server's tools splits);
/// and `all()` over an empty list is true, so a delta that carries nothing at all folds.
pub(super) fn folds(ev: &LoadedSetEvent, prior_pending: &BTreeSet<String>) -> bool {
    if !ev.removed.is_empty()
        || !ev.readded.is_empty()
        || !ev.skills.is_empty()
        || !ev.agent_types.is_empty()
        || !ev.rules_files.is_empty()
    {
        return false;
    }
    ev.tool_names
        .iter()
        .all(|name| mcp_server_of(name).is_some_and(|server| prior_pending.contains(server)))
}

/// Cut `events` into segments (`attribute.py:369-378`). The result is never empty.
///
/// An `initial` event never splits — a harness may re-announce the full listing — while a `delta`
/// splits unless [`folds`] says otherwise. `prior_pending` is **cumulative**: the Python seeds an
/// empty set before the loop and only ever `update`s it, so a server announced pending by any
/// earlier event still licenses a fold many events later. It is also strictly *prior*: an event's
/// own `pending_mcp` is added after that event has been judged, so a delta cannot license itself.
pub(super) fn settle(events: &[LoadedSetEvent], first_ts: i64) -> Vec<SegState> {
    let mut segs = vec![SegState::new(0, first_ts)];
    let mut pending: BTreeSet<String> = BTreeSet::new();
    for ev in events {
        if ev.kind != LoadedSetKind::Initial && !folds(ev, &pending) {
            let next = segs[segs.len() - 1].fork(segs.len(), ev.ts_ms);
            segs.push(next);
        }
        if let Some(current) = segs.last_mut() {
            current.absorb(ev);
        }
        pending.extend(ev.pending_mcp.iter().cloned());
    }
    segs
}

/// Index of the segment a `ts_ms` falls in (`attribute.py:381-385`).
///
/// The last segment that had already opened; segment 0 for anything before the first boundary,
/// which is why a tool call whose timestamp precedes the first listing is still attributed rather
/// than dropped. `segs` must be non-empty — [`settle`] guarantees that.
pub(super) fn segment_for(segs: &[SegState], ts_ms: i64) -> usize {
    segs.iter()
        .rposition(|seg| seg.start_ts <= ts_ms)
        .unwrap_or(0)
}

/// End timestamp of segment `i` (`attribute.py:417`).
///
/// A segment ends where the next one starts; the last ends at the run's last timestamp, floored
/// at its own start so a run whose final event arrives after `last_ts_ms` cannot produce a
/// negative-length segment.
pub(super) fn end_ts_for(segs: &[SegState], i: usize, run_last_ts_ms: i64) -> i64 {
    match segs.get(i + 1) {
        Some(next) => next.start_ts,
        None => segs[i].start_ts.max(run_last_ts_ms),
    }
}

#[cfg(test)]
#[path = "segments_tests.rs"]
mod tests;
