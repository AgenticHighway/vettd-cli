//! `attribute()` tests, ported from `NameHash`, `Settle`, `Binding` and `Observations` in
//! `spikes/828-passive-observer/prototype/tests/test_attribute.py`. The `DescriptorHash` and
//! `BomVersion` classes are ported next to the code they exercise, in `fs_index_tests.rs` and
//! `segments_tests.rs`.
//!
//! Every name, path, timestamp and byte below is invented and written into a `tempfile` scratch
//! directory; nothing here reads the real `$HOME` or the repository's fixture tree. Each test
//! states the invariant it protects and what it cannot prove, following the prototype's habit.
//!
//! **Divergence from the Python, named:** the prototype's listing timestamp `T` is in the past and
//! its `set_tree_mtime` stamps every file *and directory* older than it. No stable std API can
//! stamp a directory, so `T` here is in the future instead: every just-created scratch file and
//! directory is older than it for the same reason, and the tests that need "newer than the
//! listing" stamp a single regular file past `T`.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::observe::canonical::{hex_sha256, hmac_sha256_hex};
use crate::observe::types::{LoadedSetEvent, LoadedSetKind};

/// Invented harness listing timestamp (ms) — see the module docs for why it is in the future.
const T: i64 = 4_000_000_000_000;
/// Invented device secrets; two of them, so "keyed" is provable.
const SECRET_A: &[u8] = b"invented-observer-secret-aaaa";
const SECRET_B: &[u8] = b"invented-observer-secret-bbbb";

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("path has a parent")).expect("create parent");
    File::create(path)
        .expect("create file")
        .write_all(text.as_bytes())
        .expect("write file");
}

/// The prototype's `make_claude_home`: one two-file skill, one agent, one MCP descriptor.
fn make_claude_home(root: &Path) -> PathBuf {
    let skill = root.join("skills").join("skill-alpha");
    write(
        &skill.join("SKILL.md"),
        "---\nname: skill-alpha\n---\nInvented body.\n",
    );
    write(&skill.join("reference.md"), "Invented reference text.\n");
    write(
        &root.join("agents").join("agent-omega.md"),
        "---\nname: agent-omega\n---\nInvented agent.\n",
    );
    let cfg = json!({"mcpServers": {"srvfx": {
        "command": "/opt/tools/bin/node",
        "args": ["server.js", "--port", "8080"],
        "env": {"ZZ_TOKEN": "x"},
    }}});
    write(&root.join(".claude.json"), &cfg.to_string());
    skill
}

/// The prototype's `make_run`: a run whose only interesting fields are the three passed in.
fn make_run(
    events: Vec<LoadedSetEvent>,
    invocations: Vec<InvocationObs>,
    in_band: Vec<InBandAsset>,
) -> RunFacts {
    RunFacts {
        session_key: "sess-invented-01".to_string(),
        harness: HARNESS_CLAUDE_CODE.to_string(),
        harness_version: "0.0.0".to_string(),
        entrypoint_class: "cli".to_string(),
        effort: "medium".to_string(),
        permission_mode: "default".to_string(),
        model: "other".to_string(),
        observed_day: "2025-08-24".to_string(),
        first_ts_ms: T - 1_000,
        last_ts_ms: T + 100_000,
        run_outcome: "completed".to_string(),
        invocations,
        loaded_events: events,
        in_band_assets: in_band,
        ..RunFacts::default()
    }
}

/// The prototype's `Binding._listing`: one initial event listing `skill-alpha` at 121 bytes.
fn listing() -> Vec<LoadedSetEvent> {
    vec![LoadedSetEvent {
        ts_ms: T,
        kind: LoadedSetKind::Initial,
        skills: vec!["skill-alpha".to_string()],
        listing_bytes: BTreeMap::from([("skill-alpha".to_string(), 121)]),
        ..LoadedSetEvent::default()
    }]
}

fn invocation(asset_type: &str, name: &str, ts_ms: i64) -> InvocationObs {
    InvocationObs {
        asset_type: asset_type.to_string(),
        name: name.to_string(),
        ts_ms,
        ..InvocationObs::default()
    }
}

/// The prototype's `obs_by_name`: one segment's observations keyed by their `"<type>:<name>"`.
fn obs_by_name(ar: &AttributedRun, segment: usize) -> BTreeMap<String, AssetObservation> {
    ar.observations[&segment]
        .iter()
        .map(|o| (ar.name_map[&o.key.asset_id].clone(), o.clone()))
        .collect()
}

fn stamp(path: &Path, ms: i64) {
    let at = UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).expect("non-negative ms"));
    File::options()
        .write(true)
        .open(path)
        .expect("open for set_modified")
        .set_modified(at)
        .expect("set_modified");
}

// -- name_hash -------------------------------------------------------------------------------------

/// Invariant: `asset_id` for a name-keyed asset is an HMAC under the device secret, not a hash of
/// the name — so the cloud cannot recover a skill name by hashing a dictionary of skill names, and
/// two devices report unrelated ids for the same skill. The asset type is in the preimage, so a
/// skill and an agent of one name are different assets.
/// Cannot prove: that the caller manages the secret well (that is `identity.rs`).
#[test]
fn name_hash_is_keyed_hmac_not_sha256() {
    let a = name_hash(SECRET_A, ASSET_SKILL, "skill-ghost");
    assert_ne!(a, name_hash(SECRET_B, ASSET_SKILL, "skill-ghost"));
    assert_ne!(a, hex_sha256(b"skill-ghost"));
    assert_ne!(a, hex_sha256(b"skill:skill-ghost"));
    assert_eq!(a, hmac_sha256_hex(SECRET_A, "skill:skill-ghost"));
    assert_ne!(a, name_hash(SECRET_A, ASSET_AGENT, "skill-ghost"));
}

// -- segmentation, end to end ------------------------------------------------------------------

/// Invariant: a delta that removes a name cuts a segment at the delta's timestamp, and the
/// attributor wires that through: segment 0 ends where segment 1 starts, the removed server is an
/// observed member of the first segment only, and the two `bom_version`s therefore differ. This is
/// the join between `segments.rs` and the emitted rows, which neither module can test alone.
/// Cannot prove: when the removal actually took effect beyond the harness's own timestamp.
#[test]
fn removal_splits_the_segment() {
    let events = vec![
        LoadedSetEvent {
            ts_ms: T,
            kind: LoadedSetKind::Initial,
            tool_names: vec!["mcp__srvfx__list".to_string()],
            ..LoadedSetEvent::default()
        },
        LoadedSetEvent {
            ts_ms: T + 5_000,
            kind: LoadedSetKind::Delta,
            removed: vec!["mcp__srvfx__list".to_string()],
            ..LoadedSetEvent::default()
        },
    ];
    let root = TempDir::new().unwrap();
    let index = FsIndex::with_home(Some(root.path()), None);
    let ar = attribute(&make_run(events, vec![], vec![]), &index, SECRET_A);
    assert_eq!(
        ar.segments.iter().map(|s| s.index).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        (ar.segments[0].end_ts_ms, ar.segments[1].start_ts_ms),
        (T + 5_000, T + 5_000)
    );
    assert!(obs_by_name(&ar, 0).contains_key("mcp_server:srvfx"));
    assert!(!obs_by_name(&ar, 1).contains_key("mcp_server:srvfx"));
    assert_ne!(ar.segments[0].bom_version, ar.segments[1].bom_version);
}

// -- key precedence and binding ------------------------------------------------------------------

/// Invariant: a listed skill whose local tree — every file and the directory entry — is strictly
/// older than the listing timestamp is keyed by its tree hash with binding `mtime_proven`, and
/// that hash is the published preimage (sorted `[relpath, sha256]` pairs).
/// Cannot prove: that mtime is monotonic on the user's filesystem — a copy tool can preserve an old
/// mtime over new content, which is exactly why `mtime_proven` is weaker than `harness_log_exact`.
#[test]
fn mtime_proven_when_whole_tree_is_older_than_listing() {
    let root = TempDir::new().unwrap();
    let skill = make_claude_home(root.path());
    let index = FsIndex::with_home(Some(root.path()), None);
    let ar = attribute(&make_run(listing(), vec![], vec![]), &index, SECRET_A);
    let key = obs_by_name(&ar, 0)["skill:skill-alpha"].key.clone();
    assert_eq!(
        (key.key_basis.as_str(), key.binding.as_str()),
        (KEY_CONTENT, BINDING_MTIME)
    );
    let mut rows: Vec<String> = ["SKILL.md", "reference.md"]
        .iter()
        .map(|fname| {
            let bytes = fs::read(skill.join(fname)).expect("read the file back");
            format!("[\"{fname}\",\"{}\"]", hex_sha256(&bytes))
        })
        .collect();
    rows.sort();
    assert_eq!(
        key.asset_id,
        hex_sha256(format!("[{}]", rows.join(",")).as_bytes())
    );
}

/// Invariant: one file touched after the listing flips the binding to `unproven` while the
/// `content_hash` is unchanged — the hash is about content, the binding is about time. The
/// comparison is strict, so this is what an equal-millisecond mtime would also report.
/// Cannot prove: which file changed, or whether the change was meaningful.
#[test]
fn unproven_when_any_file_is_newer_than_listing() {
    let root = TempDir::new().unwrap();
    let skill = make_claude_home(root.path());
    let before = attribute(
        &make_run(listing(), vec![], vec![]),
        &FsIndex::with_home(Some(root.path()), None),
        SECRET_A,
    );
    stamp(&skill.join("SKILL.md"), T + 60_000);
    let after = attribute(
        &make_run(listing(), vec![], vec![]),
        &FsIndex::with_home(Some(root.path()), None),
        SECRET_A,
    );
    let (before, after) = (obs_by_name(&before, 0), obs_by_name(&after, 0));
    assert_eq!(after["skill:skill-alpha"].key.binding, BINDING_UNPROVEN);
    assert_eq!(
        after["skill:skill-alpha"].key.asset_id,
        before["skill:skill-alpha"].key.asset_id
    );
}

/// Invariant: an in-band rules file and an in-band skill body are keyed by the hash the log itself
/// carried, with binding `harness_log_exact` — no filesystem read is involved, so the id is of the
/// bytes that really entered the context.
/// Cannot prove: that the source hashed the right bytes (the source tests own that).
#[test]
fn in_band_rules_file_and_skill_body_bind_exactly() {
    let rules_sha = hex_sha256(b"invented rules text");
    let body_sha = hex_sha256(b"invented skill body");
    let in_band = vec![
        InBandAsset {
            kind: InBandKind::RulesFile,
            name: "RULES.md".to_string(),
            content_sha256: rules_sha.clone(),
            byte_len: 57,
            ts_ms: T + 3,
        },
        InBandAsset {
            kind: InBandKind::SkillBody,
            name: "skill-beta".to_string(),
            content_sha256: body_sha.clone(),
            byte_len: 90,
            ts_ms: T + 4_000,
        },
    ];
    let invs = vec![invocation(ASSET_SKILL, "skill-beta", T + 4_000)];
    let root = TempDir::new().unwrap();
    let index = FsIndex::with_home(Some(root.path()), None);
    let ar = attribute(&make_run(listing(), invs, in_band), &index, SECRET_A);
    let rows = obs_by_name(&ar, 0);
    for (label, sha) in [
        ("rules_file:RULES.md", rules_sha),
        ("skill:skill-beta", body_sha),
    ] {
        let key = &rows[label].key;
        assert_eq!(
            (
                key.key_basis.as_str(),
                key.binding.as_str(),
                key.asset_id.as_str()
            ),
            (KEY_CONTENT, BINDING_EXACT, sha.as_str()),
            "{label}"
        );
    }
}

/// Invariant: a listed skill with no local tree and no in-band body falls back to the keyed name
/// pseudonym with binding `not_applicable`, and the id is not a bare hash of the name.
/// Cannot prove: that the row has any cross-device meaning — by design it has none.
#[test]
fn listed_skill_without_local_dir_gets_name_hash() {
    let root = TempDir::new().unwrap();
    let index = FsIndex::with_home(Some(root.path()), None);
    let ar = attribute(&make_run(listing(), vec![], vec![]), &index, SECRET_A);
    let key = obs_by_name(&ar, 0)["skill:skill-alpha"].key.clone();
    assert_eq!(
        (key.key_basis.as_str(), key.binding.as_str()),
        (KEY_NAME, BINDING_NA)
    );
    assert_eq!(
        key.asset_id,
        name_hash(SECRET_A, ASSET_SKILL, "skill-alpha")
    );
    assert_ne!(key.asset_id, hex_sha256(b"skill-alpha"));
}

// -- observations ----------------------------------------------------------------------------------

/// The prototype's `Observations.setUp`: one home, three initial events, one in-band rules file
/// and four invocations (one of them a built-in agent type).
fn observations_fixture(root: &Path) -> AttributedRun {
    make_claude_home(root);
    let events = vec![
        LoadedSetEvent {
            ts_ms: T,
            kind: LoadedSetKind::Initial,
            skills: vec!["skill-alpha".to_string(), "skill-ghost".to_string()],
            listing_bytes: BTreeMap::from([
                ("skill-alpha".to_string(), 121),
                ("skill-ghost".to_string(), 83),
            ]),
            ..LoadedSetEvent::default()
        },
        LoadedSetEvent {
            ts_ms: T + 1,
            kind: LoadedSetKind::Initial,
            tool_names: vec!["Bash".to_string(), "mcp__srvfx__list".to_string()],
            tool_schema_bytes: BTreeMap::from([("srvfx".to_string(), 402)]),
            ..LoadedSetEvent::default()
        },
        LoadedSetEvent {
            ts_ms: T + 2,
            kind: LoadedSetKind::Initial,
            agent_types: vec!["Explore".to_string(), "agent-omega".to_string()],
            ..LoadedSetEvent::default()
        },
    ];
    let in_band = vec![InBandAsset {
        kind: InBandKind::RulesFile,
        name: "RULES.md".to_string(),
        content_sha256: hex_sha256(b"invented rules text"),
        byte_len: 57,
        ts_ms: T + 3,
    }];
    let invocations = vec![
        invocation(ASSET_SKILL, "skill-alpha", T + 10_000),
        InvocationObs {
            latency_ms: Some(250),
            ..invocation(ASSET_MCP_SERVER, "srvfx", T + 11_000)
        },
        InvocationObs {
            corroborated: true,
            child_tokens_total: Some(500),
            ..invocation(ASSET_AGENT, "agent-omega", T + 12_000)
        },
        invocation(ASSET_AGENT, "Explore", T + 13_000),
    ];
    let index = FsIndex::with_home(Some(root), None);
    attribute(&make_run(events, invocations, in_band), &index, SECRET_A)
}

/// Invariant: every row is `inferred` — a log reader can never claim it watched the call — while
/// `direct_evidence_available` is true exactly for the assets the run invoked, and each asset
/// carries its own invocations. `harness_corroborations` is `None` rather than `0` for an asset
/// with nothing to corroborate, so "no marker exists" is not read as "the marker disagreed".
/// Cannot prove: that a live collector would reach `direct` (that is vettd#965's work).
#[test]
fn every_row_inferred_and_direct_evidence_only_for_invoked() {
    let root = TempDir::new().unwrap();
    let ar = observations_fixture(root.path());
    assert!(ar.observations[&0].iter().all(|o| o.tier == TIER_INFERRED));
    let rows = obs_by_name(&ar, 0);
    let direct: Vec<&String> = rows
        .iter()
        .filter(|(_, o)| o.direct_evidence_available)
        .map(|(label, _)| label)
        .collect();
    assert_eq!(
        direct,
        vec!["agent:agent-omega", "mcp_server:srvfx", "skill:skill-alpha"]
    );
    assert_eq!(
        rows["mcp_server:srvfx"].invocations[0].latency_ms,
        Some(250)
    );
    assert_eq!(rows["agent:agent-omega"].harness_corroborations, Some(1));
    assert_eq!(rows["skill:skill-alpha"].harness_corroborations, None);
}

/// Invariant: each asset type is priced from the only byte count that describes its footprint —
/// skills from their listing lines, rules files from the bytes the log carried, MCP servers from
/// their tool schemas — and agents report nothing, because a spawn's prompt never enters the
/// parent's context. The method travels with the number so the cloud can tell the three apart.
/// Cannot prove: that bytes/4 approximates any real tokenizer.
#[test]
fn context_cost_methods_per_type() {
    let root = TempDir::new().unwrap();
    let rows = obs_by_name(&observations_fixture(root.path()), 0);
    let cost = |tokens: i64, method: &str| {
        Some(ContextCost {
            tokens,
            method: method.to_string(),
        })
    };
    assert_eq!(
        rows["skill:skill-alpha"].context_cost_est,
        cost(30, METHOD_LISTING)
    );
    assert_eq!(
        rows["skill:skill-ghost"].context_cost_est,
        cost(20, METHOD_LISTING)
    );
    assert_eq!(
        rows["rules_file:RULES.md"].context_cost_est,
        cost(14, METHOD_FILE)
    );
    assert_eq!(
        rows["mcp_server:srvfx"].context_cost_est,
        cost(100, METHOD_SCHEMA)
    );
    assert_eq!(rows["agent:agent-omega"].context_cost_est, None);
}

/// Invariant: a built-in agent type is not an asset — listing it and invoking it produces no
/// observation, no key and no `name_map` entry — so the cloud never accumulates evidence about the
/// harness's own sub-agents under a customer's name.
/// Cannot prove: that the run-level `subagent_runs` count still includes it (that is `extract`'s).
#[test]
fn builtin_agent_types_are_not_assets() {
    let root = TempDir::new().unwrap();
    let ar = observations_fixture(root.path());
    let rows = obs_by_name(&ar, 0);
    assert!(!rows.contains_key("agent:Explore"));
    assert!(rows.contains_key("agent:agent-omega"));
    assert!(!ar.name_map.values().any(|v| v.ends_with(":Explore")));
}

/// Invariant: the second and third rungs of the precedence ladder are reached — an agent with a
/// local `agents/<type>.md` is keyed by that file's bytes, and an MCP server with a configured
/// descriptor is keyed by the descriptor hash with binding `not_applicable`, because configuration
/// is not loaded content and the mtime rule has nothing to say about it.
/// Cannot prove: that the harness loaded *that* agent file rather than another scope's copy.
#[test]
fn local_agent_file_and_descriptor_keys() {
    let root = TempDir::new().unwrap();
    let rows = obs_by_name(&observations_fixture(root.path()), 0);
    let agent_key = &rows["agent:agent-omega"].key;
    let bytes = fs::read(root.path().join("agents").join("agent-omega.md")).expect("read agent");
    assert_eq!(agent_key.asset_id, hex_sha256(&bytes));
    assert_eq!(agent_key.key_basis, KEY_CONTENT);
    assert!([BINDING_MTIME, BINDING_UNPROVEN].contains(&agent_key.binding.as_str()));
    let mcp_key = &rows["mcp_server:srvfx"].key;
    assert_eq!(
        (mcp_key.key_basis.as_str(), mcp_key.binding.as_str()),
        (KEY_DESCRIPTOR, BINDING_NA)
    );
}

/// Invariant: `name_map` covers every emitted `asset_id` in the `"<type>:<name>"` form, every id is
/// hex64, and a segment's `bom_version` is the SHA-256 of its sorted ids — so the local report can
/// name every row the envelope carries, and the bom the cloud stores matches the keys beside it.
/// Cannot prove: that `name_map` never egresses (that is the gate's and `envelope.rs`'s job).
#[test]
fn name_map_contains_every_asset_id() {
    let root = TempDir::new().unwrap();
    let ar = observations_fixture(root.path());
    for seg in &ar.segments {
        let mut ids: Vec<&str> = seg.asset_keys.iter().map(|k| k.asset_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(seg.bom_version, hex_sha256(ids.join(",").as_bytes()));
        for key in &seg.asset_keys {
            assert_eq!(key.asset_id.len(), 64);
            assert!(key
                .asset_id
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert_eq!(
                ar.name_map[&key.asset_id],
                format!("{}:{}", key.asset_type, key.name)
            );
        }
        let mut observed: Vec<&str> = ar.observations[&seg.index]
            .iter()
            .map(|o| o.key.asset_id.as_str())
            .collect();
        observed.sort_unstable();
        assert_eq!(ids, observed);
    }
    assert_eq!(ar.name_map.len(), ar.segments[0].asset_keys.len());
}

/// Invariant: a run whose log never says what was loaded still reports assets — segment 0 is seeded
/// from the filesystem index and the basis says so, so the cloud can tell a listing-derived loaded
/// set from a filesystem-derived guess. With nothing on disk either, the basis is `none` and the
/// single segment is empty rather than absent.
///
/// **Adapted, not dropped:** the prototype exercised this through a Codex home, and Codex is out of
/// scope for v1 by ruling 2 of `docs/vettd-observe-port-plan.md`; the branch under test is the
/// harness-neutral one, so a Claude Code run with no loaded-set events takes it unchanged.
/// Cannot prove: that the bom was stable over the whole run — without a listing, nothing can.
#[test]
fn run_without_listing_uses_filesystem_basis() {
    let root = TempDir::new().unwrap();
    make_claude_home(root.path());
    let index = FsIndex::with_home(Some(root.path()), None);
    let invs = vec![invocation(ASSET_MCP_SERVER, "srvfx", T + 500)];
    let ar = attribute(&make_run(vec![], invs, vec![]), &index, SECRET_A);
    assert_eq!(
        ar.segments
            .iter()
            .map(|s| s.loaded_set_basis.as_str())
            .collect::<Vec<_>>(),
        vec![BASIS_FILESYSTEM]
    );
    let rows = obs_by_name(&ar, 0);
    assert_eq!(rows["mcp_server:srvfx"].key.key_basis, KEY_DESCRIPTOR);
    assert!(rows.contains_key("skill:skill-alpha"));

    let bare = TempDir::new().unwrap();
    let empty = attribute(
        &make_run(vec![], vec![], vec![]),
        &FsIndex::with_home(Some(bare.path()), None),
        SECRET_A,
    );
    assert_eq!(empty.segments[0].loaded_set_basis, segments::BASIS_NONE);
    assert_eq!(empty.observations, BTreeMap::from([(0, vec![])]));
}

/// Invariant: the read → extract → attribute chain reproduces the prototype's own output for the
/// committed fixture home, down to the hex. `run_id`, `bom_version` and the six `asset_id`s are
/// wire-visible and are what the cloud joins on: if this collector computes them even slightly
/// differently, its rows are silently incomparable with every row the prototype produced rather
/// than visibly wrong. The expected values are read from
/// `tests/fixtures/observe/golden/envelope.json`, which the Python prototype generated (see the
/// commit that added it for the exact command), so this is a comparison against the reference and
/// not against itself.
///
/// The index is built with an explicit `None` home so the test never reads the developer's real
/// `~/.claude.json`; the fixture home carries no local skills or agents, so every key here is a
/// name pseudonym or an in-band content hash.
/// Cannot prove: the envelope layout around these values — that is `envelope.rs` and its own golden
/// byte-comparison in Phase 4.
#[test]
fn fixture_home_reproduces_the_prototype_run_id_bom_and_asset_ids() {
    use crate::observe::claude_code::{link_children, ClaudeCodeSource};
    use crate::observe::extract::extract;
    use crate::observe::source::Source;

    const NOW_MS: i64 = 1_800_000_000_000;
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("tests/fixtures/observe/claude_home");
    let golden: serde_json::Value = serde_json::from_slice(
        &fs::read(manifest.join("tests/fixtures/observe/golden/envelope.json"))
            .expect("the golden envelope is committed"),
    )
    .expect("the golden envelope parses");
    let secret = fs::read(manifest.join("tests/fixtures/observe/golden/secret.bin"))
        .expect("the golden secret is committed");

    let source = ClaudeCodeSource::with_now_ms(root.clone(), NOW_MS);
    let refs = source.discover(&root, 3650, NOW_MS).expect("discover");
    let facts: Vec<_> = refs
        .iter()
        .map(|r| source.read(r, None).expect("read").0)
        .collect();
    let mains = link_children(facts);
    assert_eq!(mains.len(), 1, "the fixture home holds exactly one run");

    let run = extract(&mains[0], NOW_MS);
    let index = FsIndex::with_home(Some(&root), None);
    let attributed = attribute(&run, &index, &secret);

    let record = &golden["records"][0];
    assert!(
        !attributed.run.session_key.is_empty(),
        "the run must carry its local session key"
    );
    assert_eq!(
        hmac_sha256_hex(
            &secret,
            &format!("{}:{}", attributed.run.harness, attributed.run.session_key)
        ),
        record["run_id"].as_str().expect("golden run_id"),
        "run_id preimage is `{{harness}}:{{session_key}}`"
    );

    assert_eq!(attributed.segments.len(), 1, "one segment in this fixture");
    assert_eq!(
        attributed.segments[0].bom_version,
        record["bom_version"].as_str().expect("golden bom_version")
    );

    let mut ours: Vec<&str> = attributed.segments[0]
        .asset_keys
        .iter()
        .map(|key| key.asset_id.as_str())
        .collect();
    ours.sort_unstable();
    ours.dedup();
    let expected: Vec<&str> = golden["bom"][0]["asset_ids"]
        .as_array()
        .expect("golden bom asset_ids")
        .iter()
        .map(|id| id.as_str().expect("asset id is a string"))
        .collect();
    assert_eq!(
        ours, expected,
        "the bill of materials must match the prototype"
    );
}
