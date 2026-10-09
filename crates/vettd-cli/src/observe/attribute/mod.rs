//! Attribution: [`RunFacts`] -> [`AttributedRun`].
//!
//! Port of the attribution half of `spikes/828-passive-observer/prototype/attribute.py`
//! (`attribute`, `_basis`, `_observe`, `_key_for`, `_mtime_binding`, `_context_cost` and
//! `name_hash`); the segmentation half is [`segments`] and the on-disk index is [`fs_index`].
//! Where this file and `prototype/CONTRACTS.md` disagree, the Python code wins.
//!
//! What this layer decides is *identity*: which hash stands for the asset the harness loaded, and
//! how strongly that hash is bound to what was actually in the model's context. The precedence is
//! `attribute.py:8-14`'s and it is deliberate:
//!
//! 1. **In-band body** — the log itself carried the bytes the harness injected, so the hash is of
//!    the very thing that was loaded (`harness_log_exact`). Only skills and rules files can have
//!    one.
//! 2. **Local file or tree** — this machine holds a copy, hashed now. It is the same asset only if
//!    it has not changed since the harness listed it, which is what [`mtime_binding`] decides.
//! 3. **MCP descriptor** — a server has no content to hash, so its identity is the stripped
//!    configuration that launches it. Configuration is not loaded content, so the binding is
//!    `not_applicable`.
//! 4. **Keyed name pseudonym** — nothing local matched; `HMAC(secret, "<type>:<name>")` is a
//!    per-device pseudonym that identifies the asset across this device's runs and nowhere else.
//!
//! The consequence worth stating: an invoked skill whose body the log carried and a listed-only
//! copy of the same skill on disk get *different* `asset_id`s, because the two preimages are
//! different. That is the honest answer — the collector cannot prove they are the same bytes.
//!
//! **Privacy.** Everything crossing this boundary that could name something is local-only:
//! [`AssetKey::name`], [`AttributedRun::name_map`] and [`InvocationObs::name`] carry skill, agent
//! and MCP server names so the local report can be printed and so the gate checker can prove none
//! of them leaked. What the envelope may read is the hash, the closed-enum `asset_type`,
//! `key_basis` and `binding`, the counts and the segment timestamps.

pub(crate) mod fs_index;
pub(crate) mod segments;

use std::collections::BTreeMap;

use crate::observe::canonical::hmac_sha256_hex;
use crate::observe::claude_code::HARNESS_CLAUDE_CODE;
use crate::observe::types::{
    AssetKey, AssetObservation, AttributedRun, ContextCost, InBandAsset, InBandKind, InvocationObs,
    RunFacts, Segment, ASSET_AGENT, ASSET_MCP_SERVER, ASSET_RULES_FILE, ASSET_SKILL, BINDING_EXACT,
    BINDING_MTIME, BINDING_NA, BINDING_UNPROVEN, BUILTIN_AGENT_TYPES, KEY_CONTENT, KEY_DESCRIPTOR,
    KEY_NAME, TIER_INFERRED,
};

use fs_index::{FsIndex, LocalAsset};
use segments::{
    bom_version, end_ts_for, loaded_set_basis, segment_for, settle, SegState, BASIS_FILESYSTEM,
};

/// `context_cost_est.method` for a skill priced from its listing lines (`attribute.py:480`).
const METHOD_LISTING: &str = "listing_bytes_div4";
/// `context_cost_est.method` for a rules file priced from the bytes the log carried.
const METHOD_FILE: &str = "file_bytes_div4";
/// `context_cost_est.method` for an MCP server priced from its tool schemas.
const METHOD_SCHEMA: &str = "tool_schema_bytes_div4";

/// Bytes per token in the context-cost estimate (`attribute.py:478-484`). A crude constant, which
/// is why the method that used it travels with every estimate.
const BYTES_PER_TOKEN: i64 = 4;

/// `HMAC-SHA256(secret, "<asset_type>:<name>")` (`attribute.py:82-84`).
///
/// A pseudonym with no cross-device meaning: the secret is this device's, so the same skill on two
/// machines produces two unrelated ids and the cloud cannot join them. Keying also means the name
/// cannot be recovered by hashing a dictionary of skill names, which a bare SHA-256 would allow.
pub(crate) fn name_hash(secret: &[u8], asset_type: &str, name: &str) -> String {
    hmac_sha256_hex(secret, &format!("{asset_type}:{name}"))
}

/// Attribute one run's loaded sets, in-band assets and invocations to assets (`attribute.py:391`).
///
/// Produces one [`Segment`] per settled loaded set, one [`AssetKey`] per loaded asset per segment,
/// and one [`AssetObservation`] per key, plus the local-only `name_map` the report prints from.
pub(crate) fn attribute(run: &RunFacts, fs_index: &FsIndex, secret: &[u8]) -> AttributedRun {
    let listed = fs_index.listed();
    let basis = loaded_set_basis(
        !run.loaded_events.is_empty(),
        listed.values().any(|names| !names.is_empty()),
    );
    let mut segs = settle(&run.loaded_events, run.first_ts_ms);
    for seg in &mut segs {
        seg.mcp_corroborations = run.mcp_corroborations.clone();
    }
    if basis == BASIS_FILESYSTEM {
        for (asset_type, names) in &listed {
            for name in names {
                segs[0].add(asset_type, name, None);
            }
        }
    }
    let in_band = seed_in_band(&mut segs, &run.in_band_assets);
    let invs = seed_invocations(&mut segs, &run.invocations);
    build(run, &segs, fs_index, secret, basis, &in_band, &invs)
}

/// Every in-band asset joins the segment its timestamp falls in, keyed by `(asset_type, name)`.
///
/// First body wins (`setdefault`, `attribute.py:403`): a rules file re-injected after a compaction
/// must not change the identity the run already reported for it.
fn seed_in_band<'a>(
    segs: &mut [SegState],
    assets: &'a [InBandAsset],
) -> BTreeMap<(String, String), &'a InBandAsset> {
    let mut out: BTreeMap<(String, String), &InBandAsset> = BTreeMap::new();
    for asset in assets {
        let asset_type = match asset.kind {
            InBandKind::RulesFile => ASSET_RULES_FILE,
            InBandKind::SkillBody => ASSET_SKILL,
        };
        out.entry((asset_type.to_string(), asset.name.clone()))
            .or_insert(asset);
        let index = segment_for(segs, asset.ts_ms);
        segs[index].add(asset_type, &asset.name, None);
    }
    out
}

/// Every invocation joins the segment it happened in, whether or not a listing named the asset
/// (`attribute.py:406-411`) — a skill can be invoked that the harness never listed.
///
/// Built-in agent types are skipped: their spawns count in the run's `subagent_runs` and nowhere
/// else, because `Explore` is the harness, not something the user installed.
fn seed_invocations<'a>(
    segs: &mut [SegState],
    invocations: &'a [InvocationObs],
) -> BTreeMap<(usize, String, String), Vec<&'a InvocationObs>> {
    let mut out: BTreeMap<(usize, String, String), Vec<&InvocationObs>> = BTreeMap::new();
    for inv in invocations {
        if inv.asset_type == ASSET_AGENT && BUILTIN_AGENT_TYPES.contains(&inv.name.as_str()) {
            continue;
        }
        let index = segment_for(segs, inv.ts_ms);
        segs[index].add(&inv.asset_type, &inv.name, None);
        out.entry((segs[index].index, inv.asset_type.clone(), inv.name.clone()))
            .or_default()
            .push(inv);
    }
    out
}

/// Turn the settled [`SegState`]s into the wire-shaped [`Segment`]s and their observations
/// (`attribute.py:413-427`).
///
/// Observations are sorted by `asset_id` because that is the order `bom[].asset_ids` is emitted in
/// and the order the golden envelope is compared byte for byte against.
fn build(
    run: &RunFacts,
    segs: &[SegState],
    fs_index: &FsIndex,
    secret: &[u8],
    basis: &str,
    in_band: &BTreeMap<(String, String), &InBandAsset>,
    invs: &BTreeMap<(usize, String, String), Vec<&InvocationObs>>,
) -> AttributedRun {
    let mut segments: Vec<Segment> = Vec::new();
    let mut observations: BTreeMap<usize, Vec<AssetObservation>> = BTreeMap::new();
    let mut name_map: BTreeMap<String, String> = BTreeMap::new();
    let empty: Vec<&InvocationObs> = Vec::new();
    for (i, st) in segs.iter().enumerate() {
        let mut obs: Vec<AssetObservation> = st
            .members()
            .iter()
            .map(|(asset_type, name)| {
                let band = in_band.get(&(asset_type.clone(), name.clone())).copied();
                let key = (st.index, asset_type.clone(), name.clone());
                let inv_list = invs.get(&key).unwrap_or(&empty);
                observe(
                    &run.harness,
                    asset_type,
                    name,
                    st,
                    fs_index,
                    secret,
                    band,
                    inv_list,
                )
            })
            .collect();
        obs.sort_by(|a, b| a.key.asset_id.cmp(&b.key.asset_id));
        for o in &obs {
            name_map.insert(
                o.key.asset_id.clone(),
                format!("{}:{}", o.key.asset_type, o.key.name),
            );
        }
        segments.push(Segment {
            index: st.index,
            start_ts_ms: st.start_ts,
            end_ts_ms: end_ts_for(segs, i, run.last_ts_ms),
            loaded_set_basis: basis.to_string(),
            asset_keys: obs.iter().map(|o| o.key.clone()).collect(),
            bom_version: bom_version(obs.iter().map(|o| o.key.asset_id.as_str())),
        });
        observations.insert(st.index, obs);
    }
    AttributedRun {
        run: run.clone(),
        segments,
        observations,
        name_map,
    }
}

/// One asset's observation inside one segment (`attribute.py:438-448`).
///
/// Every row is `inferred`: this collector reads finished logs, so it can never claim it watched
/// the invocation happen. `direct_evidence_available` says only that a live collector *could* have
/// attributed this row `direct`, i.e. that the run contains an invocation of the asset.
#[allow(clippy::too_many_arguments)]
fn observe(
    harness: &str,
    asset_type: &str,
    name: &str,
    st: &SegState,
    fs_index: &FsIndex,
    secret: &[u8],
    band: Option<&InBandAsset>,
    inv_list: &[&InvocationObs],
) -> AssetObservation {
    AssetObservation {
        key: key_for(harness, asset_type, name, st, fs_index, secret, band),
        tier: TIER_INFERRED.to_string(),
        direct_evidence_available: !inv_list.is_empty(),
        invocations: inv_list.iter().map(|inv| (*inv).clone()).collect(),
        context_cost_est: context_cost(asset_type, name, st, band),
        harness_corroborations: corroborations(asset_type, name, st, inv_list),
    }
}

/// Harness-native attribution markers for this asset in this segment (`attribute.py:443-445`).
///
/// `None`, not `0`, when there is nothing to corroborate: a listed-but-never-invoked agent has no
/// spawn for the harness to have attributed, and reporting zero there would read as a *failure* to
/// corroborate rather than as an absence of evidence. For MCP servers the harness's own
/// `attributionMcpServer` count wins over the per-invocation flag, because it is the harness
/// speaking rather than an inference from the tool name.
fn corroborations(
    asset_type: &str,
    name: &str,
    st: &SegState,
    inv_list: &[&InvocationObs],
) -> Option<u64> {
    if inv_list.is_empty() {
        return None;
    }
    if asset_type == ASSET_MCP_SERVER {
        if let Some(count) = st.mcp_corroborations.get(name) {
            return Some(*count);
        }
        return None;
    }
    if asset_type != ASSET_AGENT {
        return None;
    }
    Some(inv_list.iter().filter(|inv| inv.corroborated).count() as u64)
}

/// The key-precedence rule (`attribute.py:451-467`); see the module docs for why it is this order.
#[allow(clippy::too_many_arguments)]
fn key_for(
    harness: &str,
    asset_type: &str,
    name: &str,
    st: &SegState,
    fs_index: &FsIndex,
    secret: &[u8],
    band: Option<&InBandAsset>,
) -> AssetKey {
    if let Some(band) = band {
        if asset_type == ASSET_SKILL || asset_type == ASSET_RULES_FILE {
            return AssetKey::new(
                &band.content_sha256,
                asset_type,
                KEY_CONTENT,
                name,
                BINDING_EXACT,
            );
        }
    }
    if let Some(local) = local_asset(harness, asset_type, name, fs_index) {
        let listed_ts = st
            .listed_ts
            .get(&(asset_type.to_string(), name.to_string()))
            .copied();
        let binding = mtime_binding(local, listed_ts);
        return AssetKey::new(&local.content_hash, asset_type, KEY_CONTENT, name, binding);
    }
    if asset_type == ASSET_MCP_SERVER {
        if let Some(digest) = fs_index.mcp_descriptor(name) {
            return AssetKey::new(digest, asset_type, KEY_DESCRIPTOR, name, BINDING_NA);
        }
    }
    AssetKey::new(
        &name_hash(secret, asset_type, name),
        asset_type,
        KEY_NAME,
        name,
        BINDING_NA,
    )
}

/// The local copy behind a named asset, if this harness has one to offer (`attribute.py:455-459`).
///
/// Only skills and agents have local content: a rules file's bytes reach us in band or not at all,
/// and an MCP server has a descriptor rather than content.
fn local_asset<'a>(
    harness: &str,
    asset_type: &str,
    name: &str,
    fs_index: &'a FsIndex,
) -> Option<&'a LocalAsset> {
    if harness != HARNESS_CLAUDE_CODE {
        return None;
    }
    match asset_type {
        ASSET_SKILL => fs_index.skill(name),
        ASSET_AGENT => fs_index.agent(name),
        _ => None,
    }
}

/// `mtime_proven` only when every file *and* directory behind the hash is strictly older than the
/// harness's listing timestamp (`attribute.py:470-475`).
///
/// The comparison is **strict**: a file whose mtime equals the listing millisecond could have been
/// written in the same millisecond as the listing, in either order, so it does not prove anything.
/// Loosening it to `<=` would silently relabel every same-millisecond asset as proven. Without a
/// listing timestamp at all — the filesystem basis — nothing binds the hash to what was loaded, so
/// the answer is `unproven` rather than "no opinion".
fn mtime_binding(local: &LocalAsset, listed_ts: Option<i64>) -> &'static str {
    match listed_ts {
        Some(ts) if local.max_mtime_ms < ts => BINDING_MTIME,
        _ => BINDING_UNPROVEN,
    }
}

/// Estimated context cost in tokens, with the method that produced it (`attribute.py:478-485`).
///
/// Three sources, one per asset type that has a measurable footprint: a skill's listing lines, a
/// rules file's in-band bytes, an MCP server's tool schemas. Agents have none — a spawn's prompt is
/// not in the parent's context — so they report `None`, which the envelope keeps distinct from an
/// estimate of zero.
///
/// **Division:** Python's `//` floors. `i64::div_euclid` by a positive divisor is also a floor, so
/// the two agree on every input including the negatives no real byte count produces; plain `/`
/// would truncate toward zero and disagree there. Named because the choice is invisible for the
/// only inputs that occur.
fn context_cost(
    asset_type: &str,
    name: &str,
    st: &SegState,
    band: Option<&InBandAsset>,
) -> Option<ContextCost> {
    let cost = |bytes: i64, method: &str| ContextCost {
        tokens: bytes.div_euclid(BYTES_PER_TOKEN),
        method: method.to_string(),
    };
    match asset_type {
        ASSET_SKILL => st.listing_bytes.get(name).map(|b| cost(*b, METHOD_LISTING)),
        ASSET_RULES_FILE => band.map(|band| cost(band.byte_len, METHOD_FILE)),
        ASSET_MCP_SERVER => st.schema_bytes.get(name).map(|b| cost(*b, METHOD_SCHEMA)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "attribute_tests.rs"]
mod tests;
