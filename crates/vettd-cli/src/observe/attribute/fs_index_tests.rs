//! `FsIndex` tests, ported from the prototype's
//! `spikes/828-passive-observer/prototype/tests/test_attribute.py` (`DescriptorHash`, and the
//! filesystem half of `Binding`), plus the parity checks the port needs that the Python did not.
//!
//! Every name, path, timestamp and byte below is invented and written into a `tempfile` scratch
//! directory; nothing here reads the real `$HOME`, the real `~/.claude`, or the repository's
//! fixture tree. Each test states the invariant it protects and what it cannot prove, following
//! the prototype's habit.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use tempfile::TempDir;

use super::*;

/// Invented harness listing timestamp, in ms (`test_attribute.py:33`).
const T: i64 = 1_756_000_000_000;
/// A far-future mtime, so "the newest file wins" is unambiguous against a just-touched directory.
const FUTURE_MS: i64 = 4_000_000_000_000;

/// Built at runtime so no secret-shaped literal exists in this file (`test_attribute.py:37`).
fn flag_value() -> String {
    ["sk", "-", "invented", "0123456789", "abcdefghij"].concat()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("path has a parent")).expect("create parent");
    File::create(path)
        .expect("create file")
        .write_all(text.as_bytes())
        .expect("write file");
}

fn obj(value: Value) -> Map<String, Value> {
    value.as_object().expect("descriptor is an object").clone()
}

/// The prototype's `make_claude_home`: one two-file skill, one agent, one MCP descriptor.
fn make_claude_home(root: &Path, node_path: &str, flag: &str, env_value: &str) -> PathBuf {
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
        "command": node_path,
        "args": ["server.js", "--api-key", flag, "--port", "8080"],
        "env": {"ZZ_TOKEN": env_value},
    }}});
    write(&root.join(".claude.json"), &cfg.to_string());
    skill
}

/// Sets one path's mtime. Directories are handled by touching their contents, because opening a
/// directory as a `File` is not portable.
fn set_mtime(path: &Path, ms: i64) {
    let stamp = UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).expect("non-negative ms"));
    File::options()
        .write(true)
        .open(path)
        .expect("open for set_modified")
        .set_modified(stamp)
        .expect("set_modified");
}

/// Sets every *file* mtime under `root` (the prototype's `set_tree_mtime`, minus the directories,
/// which no stable std API can stamp).
fn set_file_mtimes(root: &Path, ms: i64) {
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.expect("walk the scratch tree");
        if entry.file_type().is_file() {
            set_mtime(entry.path(), ms);
        }
    }
}

// -- descriptor hashing ---------------------------------------------------------------------------

/// Invariant: the descriptor preimage carries no directory, no credential and no env *value*, so
/// two machines that differ only in those three ways report the same server identity.
/// Cannot prove: that every secret-carrying shape is stripped — only the three listed rules are.
#[test]
fn path_prefix_and_secret_value_do_not_change_the_hash() {
    let (home_a, home_b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    make_claude_home(home_a.path(), "/opt/tools/bin/node", &flag_value(), "x");
    let reversed: String = flag_value().chars().rev().collect();
    make_claude_home(home_b.path(), "/usr/local/lib/tools/node", &reversed, "y");
    let a = FsIndex::with_home(Some(home_a.path()), None);
    let b = FsIndex::with_home(Some(home_b.path()), None);
    let h_a = a.mcp_descriptor("srvfx").expect("descriptor indexed");
    assert_eq!(h_a, b.mcp_descriptor("srvfx").expect("descriptor indexed"));
    assert_eq!(h_a.len(), 64);
    assert!(h_a
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
}

/// Invariant: the command basename is part of the identity, so the previous test does not pass
/// because every descriptor collapses to one hash.
/// Cannot prove: that a basename is a sufficient identity for well-known servers.
#[test]
fn command_basename_changes_the_hash() {
    let node = obj(json!({"command": "/a/node", "args": ["server.js"], "env": {}}));
    let deno = obj(json!({"command": "/a/deno", "args": ["server.js"], "env": {}}));
    assert_ne!(descriptor_hash(&node), descriptor_hash(&deno));
}

/// Invariant: a standalone secret-shaped token, a path-shaped token and the value glued to a
/// secret flag are dropped, while a plain value still changes the hash — the stripping is
/// targeted, not "drop everything".
/// Cannot prove: the secret-shape heuristics catch every token format.
#[test]
fn secret_shaped_and_path_shaped_args_dropped_but_plain_args_kept() {
    let flag = flag_value();
    let plain = descriptor_hash(&obj(
        json!({"command": "node", "args": ["server.js", "--port", "8080"]}),
    ));
    let with_secret = descriptor_hash(&obj(
        json!({"command": "node", "args": ["server.js", flag, "--port", "8080"]}),
    ));
    let with_path = descriptor_hash(&obj(
        json!({"command": "node", "args": ["server.js", "/srv/data/cfg.json", "--port", "8080"]}),
    ));
    let other_port = descriptor_hash(&obj(
        json!({"command": "node", "args": ["server.js", "--port", "9090"]}),
    ));
    let glued = canonical_descriptor(&obj(json!({
        "command": "node",
        "args": ["server.js", format!("--api-key={flag}"), "--port", "8080"],
    })));
    assert_eq!(plain, with_secret);
    assert_eq!(plain, with_path);
    assert_ne!(plain, other_port);
    assert_eq!(
        glued["args"],
        json!(["server.js", "--api-key", "--port", "8080"])
    );
}

/// Invariant: a url server becomes transport `http` with the *scheme class* as its command, so two
/// hosts on https hash alike and no hostname ever enters the preimage.
/// Cannot prove: anything about headers — they are never read.
#[test]
fn url_descriptor_uses_host_class_not_host() {
    let a = canonical_descriptor(&obj(
        json!({"url": "https://one.invalid/mcp", "headers": {"Authorization": "x"}}),
    ));
    let b = canonical_descriptor(&obj(json!({"url": "https://two.invalid/mcp"})));
    assert_eq!(a, b);
    assert_eq!(a["transport"], json!("http"));
    assert_eq!(a["command"], json!("https"));
    let plain = canonical_descriptor(&obj(json!({"url": "http://three.invalid/"})));
    assert_eq!(plain["command"], json!("http"));
    let blank = canonical_descriptor(&obj(json!({"url": "   ", "command": "/bin/npx"})));
    assert_eq!(
        (&blank["transport"], &blank["command"]),
        (&json!("stdio"), &json!("npx"))
    );
}

/// Invariant: `descriptor_hash` is byte-identical to the Python prototype's, so a descriptor
/// hashed by the Rust collector is the same wire value the prototype produced.
///
/// Expected values recomputed with (from `spikes/828-passive-observer/prototype/`):
/// `python3 -c "import attribute as A; print(A.descriptor_hash({'command':'npx','args':['-y','pkg']}))"`.
/// Cannot prove: parity for argv holding non-string JSON values (see `python_str`).
#[test]
fn descriptor_hash_matches_python_prototype() {
    let npx = obj(json!({"command": "npx", "args": ["-y", "pkg"]}));
    assert_eq!(
        descriptor_hash(&npx),
        "8eb2b3d3294b90dbcc5a844f05af483a6b78de62d94f39abbd8316a2154bfd16"
    );
    assert_eq!(
        canonical_descriptor(&npx),
        json!({"transport": "stdio", "command": "npx", "args": ["-y", "pkg"], "env_names": []})
    );
    let stdio = obj(json!({
        "command": "/opt/tools/bin/node",
        "args": ["server.js", "--api-key", flag_value(), "--port", "8080"],
        "env": {"ZZ_TOKEN": "x"},
    }));
    assert_eq!(
        descriptor_hash(&stdio),
        "8cc8d0d2b9a2d8aa08a5872fcbbbfc204aaa289240d14332d8aa76bac1f26043"
    );
    let url = obj(json!({"url": "https://one.invalid/mcp", "headers": {"Authorization": "x"}}));
    assert_eq!(
        descriptor_hash(&url),
        "0b592dd05e43c6c3a41f1d466781c2f87c269f5676156c31ca2ca8114d1cfbcd"
    );
}

// -- descriptor sources ---------------------------------------------------------------------------

/// Invariant: the three descriptor sources are consulted in order and the first one that names a
/// server wins, so a project-scoped `<root>/.claude.json` is never overridden by `~/.claude.json`.
/// Cannot prove: which file Claude Code itself considered authoritative for that server.
#[test]
fn root_claude_json_wins_over_home_and_settings() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let cfg = |command: &str| json!({"mcpServers": {"srvfx": {"command": command}}}).to_string();
    write(&root.path().join(".claude.json"), &cfg("alpha"));
    write(&home.path().join(".claude.json"), &cfg("beta"));
    write(&root.path().join("settings.json"), &cfg("gamma"));
    let index = FsIndex::with_home(Some(root.path()), Some(home.path()));
    let expected = descriptor_hash(&obj(json!({"command": "alpha"})));
    assert_eq!(index.mcp_descriptor("srvfx"), Some(expected.as_str()));
}

/// Invariant (the deliberate divergence from the prototype): `~/.claude.json` — where Claude Code
/// actually writes `mcpServers` — is read, so a real machine yields real descriptor keys instead
/// of degrading every MCP server to a `name_hash`. Also: `<root>/settings.json` still ranks last.
/// Cannot prove: that `~/.claude.json` is the only place a real installation stores servers.
#[test]
fn home_claude_json_is_read_and_outranks_settings_json() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    write(
        &home.path().join(".claude.json"),
        &json!({"mcpServers": {"srvfx": {"command": "beta"}}}).to_string(),
    );
    write(
        &root.path().join("settings.json"),
        &json!({"mcpServers": {"srvfx": {"command": "gamma"}, "srvzz": {"command": "delta"}}})
            .to_string(),
    );
    let index = FsIndex::with_home(Some(root.path()), Some(home.path()));
    let beta = descriptor_hash(&obj(json!({"command": "beta"})));
    let delta = descriptor_hash(&obj(json!({"command": "delta"})));
    assert_eq!(index.mcp_descriptor("srvfx"), Some(beta.as_str()));
    assert_eq!(index.mcp_descriptor("srvzz"), Some(delta.as_str()));
    // Without a home directory the second source simply does not exist.
    let no_home = FsIndex::with_home(Some(root.path()), None);
    let gamma = descriptor_hash(&obj(json!({"command": "gamma"})));
    assert_eq!(no_home.mcp_descriptor("srvfx"), Some(gamma.as_str()));
}

/// Invariant: unreadable, non-JSON, wrong-shaped and non-object descriptor entries all degrade to
/// "no descriptor" rather than failing the index — a hand-edited config costs identity, not a run.
/// Cannot prove: that a *valid* config could not still describe a server this code misreads.
#[test]
fn malformed_descriptor_sources_cost_only_the_descriptors() {
    let root = TempDir::new().unwrap();
    write(&root.path().join(".claude.json"), "{\"mcpServers\": {,}");
    write(
        &root.path().join("settings.json"),
        &json!({"mcpServers": {"srvzz": "not-an-object", "srvfx": {"command": "npx"}}}).to_string(),
    );
    let index = FsIndex::with_home(Some(root.path()), None);
    assert_eq!(index.mcp_descriptor("srvzz"), None);
    let npx = descriptor_hash(&obj(json!({"command": "npx"})));
    assert_eq!(index.mcp_descriptor("srvfx"), Some(npx.as_str()));
    // A top-level array, and a `mcpServers` that is not an object, are both simply empty.
    write(&root.path().join(".claude.json"), "[1, 2, 3]");
    assert!(json_servers(&root.path().join(".claude.json")).is_empty());
    write(&root.path().join(".claude.json"), "{\"mcpServers\": 7}");
    assert!(json_servers(&root.path().join(".claude.json")).is_empty());
}

// -- tree hashing -----------------------------------------------------------------------------

/// The three-file skill used by the tree-hash tests; returns the skill directory.
fn make_nested_skill(root: &Path) -> PathBuf {
    let skill = root.join("skills").join("skill-alpha");
    write(
        &skill.join("SKILL.md"),
        "---\nname: skill-alpha\n---\nInvented body.\n",
    );
    write(&skill.join("reference.md"), "Invented reference text.\n");
    write(
        &skill.join("sub").join("inner.md"),
        "Invented nested note.\n",
    );
    skill
}

/// Invariant: the published tree hash really is SHA-256 over the canonical JSON of the sorted
/// `[relpath, sha256(file)]` pairs — recomputed here from the bytes on disk by a second,
/// independent route that shares no code with `tree_asset`.
/// Cannot prove: that the walk found every file (the nested-path test covers that).
#[test]
fn tree_hash_matches_independent_recomputation() {
    let root = TempDir::new().unwrap();
    let skill = make_nested_skill(root.path());
    let index = FsIndex::with_home(Some(root.path()), None);
    let mut rows: Vec<String> = Vec::new();
    for rel in ["SKILL.md", "reference.md", "sub/inner.md"] {
        let bytes = fs::read(skill.join(rel)).expect("read the file back");
        rows.push(format!("[\"{rel}\",\"{}\"]", hex_sha256(&bytes)));
    }
    rows.sort();
    let preimage = format!("[{}]", rows.join(","));
    let expected = hex_sha256(preimage.as_bytes());
    assert_eq!(index.skill("skill-alpha").unwrap().content_hash, expected);
}

/// Invariant: the tree hash is byte-identical to the Python prototype's, so an asset hashed here
/// and an asset hashed there are the same wire identity.
///
/// Recomputed with (from `spikes/828-passive-observer/prototype/`, tree as `make_nested_skill`
/// builds it): `python3 -c "import attribute as A; print(A._tree_asset(PATH).content_hash)"`.
/// Cannot prove: parity for filenames that are not valid UTF-8 (documented as a divergence).
#[test]
fn tree_hash_matches_python_prototype() {
    let root = TempDir::new().unwrap();
    make_nested_skill(root.path());
    let index = FsIndex::with_home(Some(root.path()), None);
    assert_eq!(
        index.skill("skill-alpha").unwrap().content_hash,
        "e0a3fdbb7c816688e7c412ce93396ae1331ccf6bcc4ff6b10e438a4e59c5324b"
    );
}

/// Invariant: nested files enter the preimage with `/` separators, so the same skill hashes the
/// same on Windows as on Unix; and a file in a subdirectory is not silently skipped.
/// Cannot prove: the Windows behaviour itself — only that no platform separator is used here.
#[test]
fn nested_paths_are_posix_and_contribute_to_the_hash() {
    let root = TempDir::new().unwrap();
    let skill = make_nested_skill(root.path());
    let with_nested = FsIndex::with_home(Some(root.path()), None)
        .skill("skill-alpha")
        .unwrap()
        .content_hash
        .clone();
    fs::remove_dir_all(skill.join("sub")).expect("drop the nested directory");
    let without = FsIndex::with_home(Some(root.path()), None)
        .skill("skill-alpha")
        .unwrap()
        .content_hash
        .clone();
    assert_ne!(with_nested, without);
    assert_eq!(
        relative_posix(&skill, &skill.join("sub").join("inner.md")).unwrap(),
        "sub/inner.md"
    );
}

/// Invariant: `max_mtime_ms` is a maximum over files *and* directories. A file stamped in the far
/// future wins over a just-created directory; and with every file stamped in the past, the
/// directory's own (recent) mtime still surfaces — which is what makes a *deleted* file move an
/// asset's binding to `unproven`.
/// Cannot prove: that the user's filesystem keeps mtimes monotonic.
#[test]
fn max_mtime_covers_files_and_directories() {
    let root = TempDir::new().unwrap();
    let skill = make_nested_skill(root.path());
    set_file_mtimes(&skill, FUTURE_MS);
    let newest_file = FsIndex::with_home(Some(root.path()), None);
    assert_eq!(
        newest_file.skill("skill-alpha").unwrap().max_mtime_ms,
        FUTURE_MS
    );

    set_file_mtimes(&skill, T - 60_000);
    let before = FsIndex::with_home(Some(root.path()), None);
    let hash_before = before.skill("skill-alpha").unwrap().content_hash.clone();
    assert!(before.skill("skill-alpha").unwrap().max_mtime_ms > T - 60_000);
    // A directory mtime is "now": far past the stamped files, and not from any file's stamp.
    let now_ms = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after 1970")
            .as_millis(),
    )
    .expect("now fits i64");
    let observed = before.skill("skill-alpha").unwrap().max_mtime_ms;
    assert!(
        (now_ms - observed).abs() < 60_000,
        "dir mtime {observed} vs now {now_ms}"
    );
    assert_eq!(
        hash_before,
        FsIndex::with_home(Some(root.path()), None)
            .skill("skill-alpha")
            .unwrap()
            .content_hash
    );
}

/// Invariant: a skill nested inside another skill is indexed under its own directory name *and*
/// its files still count toward the outer skill's tree hash — one file, two identities, because
/// the harness can load either.
/// Cannot prove: that a harness would in fact offer both.
#[test]
fn nested_skill_gets_its_own_entry_and_feeds_the_parent_hash() {
    let root = TempDir::new().unwrap();
    let outer = root.path().join("skills").join("skill-outer");
    write(&outer.join("SKILL.md"), "outer\n");
    let inner = outer.join("skill-inner");
    write(&inner.join("SKILL.md"), "inner\n");
    let index = FsIndex::with_home(Some(root.path()), None);
    let inner_hash = index.skill("skill-inner").unwrap().content_hash.clone();
    let only_inner =
        hex_sha256(format!("[[\"SKILL.md\",\"{}\"]]", hex_sha256(b"inner\n")).as_bytes());
    assert_eq!(inner_hash, only_inner);
    assert_ne!(index.skill("skill-outer").unwrap().content_hash, inner_hash);
}

// -- agents, listing and fail-open ---------------------------------------------------------------

/// Invariant: an agent is keyed by the `.md` stem and hashed as the file's raw bytes (not as a
/// one-file tree), so the agent preimage and the skill preimage are different by construction.
/// Cannot prove: that the harness loaded *this* copy rather than another scope's.
#[test]
fn agents_are_hashed_as_raw_file_bytes() {
    let root = TempDir::new().unwrap();
    make_claude_home(root.path(), "npx", "plain", "x");
    fs::create_dir_all(root.path().join("agents").join("nested")).expect("mkdir");
    write(&root.path().join("agents").join("notes.txt"), "ignored\n");
    let index = FsIndex::with_home(Some(root.path()), None);
    let body = fs::read(root.path().join("agents").join("agent-omega.md")).expect("read agent");
    let agent = index.agent("agent-omega").expect("agent indexed");
    assert_eq!(agent.content_hash, hex_sha256(&body));
    assert_eq!(index.agent("notes"), None);
    assert_eq!(index.agent("nested"), None);
}

/// Invariant: `listed()` reports exactly the names the index holds, grouped by the closed-enum
/// asset type — the filesystem-basis loaded set the attributor seeds segment 0 with.
/// Cannot prove: that those assets were loaded by any particular run (that is the whole point of
/// calling this basis `filesystem` rather than `harness_log`).
#[test]
fn listed_reports_every_indexed_name_by_type() {
    let root = TempDir::new().unwrap();
    make_claude_home(root.path(), "npx", "plain", "x");
    let listed = FsIndex::with_home(Some(root.path()), None).listed();
    assert_eq!(
        listed[ASSET_SKILL],
        BTreeSet::from(["skill-alpha".to_string()])
    );
    assert_eq!(
        listed[ASSET_AGENT],
        BTreeSet::from(["agent-omega".to_string()])
    );
    assert_eq!(
        listed[ASSET_MCP_SERVER],
        BTreeSet::from(["srvfx".to_string()])
    );
    let empty = FsIndex::new(None).listed();
    assert!(empty.values().all(BTreeSet::is_empty));
}

/// Invariant: an absent root, an absent `skills/`, and a `skills/` holding no `SKILL.md` all yield
/// an empty index rather than an error — the observer degrades to fewer assets, never to a
/// refusal to run.
/// Cannot prove: that every OS reports these conditions as recoverable errors.
#[test]
fn missing_and_empty_roots_degrade_to_an_empty_index() {
    let missing = TempDir::new().unwrap().path().join("gone");
    let index = FsIndex::with_home(Some(&missing), Some(&missing));
    assert!(index.listed().values().all(BTreeSet::is_empty));

    let root = TempDir::new().unwrap();
    write(
        &root.path().join("skills").join("notes").join("README.md"),
        "no skill here\n",
    );
    fs::create_dir_all(root.path().join("agents")).expect("mkdir agents");
    let sparse = FsIndex::with_home(Some(root.path()), None);
    assert_eq!(sparse.skill("notes"), None);
    assert!(sparse.listed()[ASSET_SKILL].is_empty());
}

/// Invariant: a skill holding one unreadable file is dropped whole rather than hashed from its
/// readable remainder — a partial tree hash would be a *wrong* identity, which is worse than a
/// missing one — and the neighbouring skill is untouched.
/// Cannot prove: anything when the test runs as root, where the mode bits do not deny; the test
/// says so on stderr and stops rather than asserting something it did not arrange.
#[cfg(unix)]
#[test]
fn unreadable_skill_file_drops_only_that_skill() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new().unwrap();
    let blocked = root.path().join("skills").join("skill-blocked");
    write(&blocked.join("SKILL.md"), "blocked\n");
    write(&blocked.join("secret.md"), "unreadable\n");
    write(
        &root.path().join("skills").join("skill-ok").join("SKILL.md"),
        "ok\n",
    );
    fs::set_permissions(blocked.join("secret.md"), fs::Permissions::from_mode(0o000))
        .expect("chmod 000");
    if fs::read(blocked.join("secret.md")).is_ok() {
        eprintln!("SKIPPED unreadable_skill_file_drops_only_that_skill: running with rights that ignore mode 0o000");
        return;
    }
    let index = FsIndex::with_home(Some(root.path()), None);
    assert_eq!(index.skill("skill-blocked"), None);
    assert!(index.skill("skill-ok").is_some());
}

/// Invariant: symlinks are never followed into, so a directory link that points at its own
/// ancestor cannot make the walk loop forever, and a link to a regular file is still hashed (the
/// prototype's `os.path.isfile` follows links even though `os.walk` does not).
/// Cannot prove: behaviour on filesystems without symlinks (Windows is excluded by `cfg`).
#[cfg(unix)]
#[test]
fn symlink_loop_terminates_and_file_links_are_hashed() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let skill = root.path().join("skills").join("skill-loop");
    write(&skill.join("SKILL.md"), "loop\n");
    write(&root.path().join("target.md"), "linked body\n");
    symlink("..", skill.join("self")).expect("create the loop link");
    symlink(root.path().join("target.md"), skill.join("linked.md")).expect("create the file link");
    symlink("nowhere", skill.join("broken.md")).expect("create the broken link");
    let index = FsIndex::with_home(Some(root.path()), None);
    let mut rows = [
        format!("[\"SKILL.md\",\"{}\"]", hex_sha256(b"loop\n")),
        format!("[\"linked.md\",\"{}\"]", hex_sha256(b"linked body\n")),
    ];
    rows.sort();
    let expected = hex_sha256(format!("[{}]", rows.join(",")).as_bytes());
    assert_eq!(index.skill("skill-loop").unwrap().content_hash, expected);
}

// -- the opaque-token rule --------------------------------------------------------------------

/// At least 200 strings spanning every shape an MCP argv token can plausibly take.
fn opaque_corpus() -> Vec<String> {
    let mut corpus: Vec<String> = vec![
        String::new(),
        "-".into(),
        "--port".into(),
        "8080".into(),
        "server.js".into(),
        "/srv/data/cfg.json".into(),
        "C:\\Users\\invented\\cfg.json".into(),
        "123e4567-e89b-12d3-a456-426614174000".into(),
        "0123456789abcdef0123456789abcdef".into(),
        "abcdefabcdefabcdefabcdefabcdefab".into(),
        "01234567890123456789012345678901".into(),
        "aGVsbG8gd29ybGQgaW52ZW50ZWQgcGF5bG9hZA==".into(),
        "a+b/c=d-e_f0123456789012345678901234".into(),
        "invented token with spaces and 0123456789 padding".into(),
        "тестовыйтокен0123456789012345678901".into(),
        "emoji-\u{1f600}-0123456789012345678901234".into(),
        "value.with.dots.0123456789.0123456789".into(),
        "value:with:colons:0123456789:012345678".into(),
        "trailing-newline-0123456789012345678\n".into(),
        "\n".into(),
    ];
    let alnum = "abcdefghij0123456789klmnopqrst0123456789uvwxyzABCD0123456789EFGH";
    let letters = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijkl";
    let digits = "0123456789".repeat(7);
    let digits = &digits[..64];
    for len in 0..=63usize {
        corpus.push(alnum[..len].to_string());
        corpus.push(letters[..len].to_string());
        corpus.push(digits[..len].to_string());
    }
    for len in 28..=40usize {
        corpus.push(format!("{}.", &alnum[..len]));
        corpus.push(format!("{} ", &alnum[..len]));
        corpus.push(format!("--flag={}", &alnum[..len]));
        corpus.push(format!("{}=", &alnum[..len]));
        corpus.push(format!("{}-_+/=", &alnum[..len]));
    }
    corpus
}

/// Invariant: the hand-written opaque rule and the prototype's two-lookahead regex classify every
/// corpus token identically, so replacing `_OPAQUE_RE` did not change which argv tokens the
/// descriptor preimage keeps. The Python regex is embedded here verbatim from `attribute.py:69`
/// rather than imported, so this test outlives the spike directory.
/// Cannot prove: agreement outside the corpus. One class is known to differ and is asserted
/// separately by `opaque_rule_diverges_only_on_a_trailing_newline`; those inputs are excluded here.
#[test]
fn opaque_rule_agrees_with_the_python_regex_over_a_corpus() {
    let corpus: Vec<String> = opaque_corpus()
        .into_iter()
        .filter(|s| !s.ends_with('\n'))
        .collect();
    assert!(
        corpus.len() >= 200,
        "corpus is only {} strings",
        corpus.len()
    );
    let Some(python) = python_opaque_matches(&corpus) else {
        eprintln!(
            "SKIPPED opaque_rule_agrees_with_the_python_regex_over_a_corpus: no usable python3"
        );
        return;
    };
    let mine: Vec<bool> = corpus.iter().map(|s| is_opaque_token(s)).collect();
    let disagreements: Vec<&String> = corpus
        .iter()
        .zip(python.iter().zip(mine.iter()))
        .filter(|(_, (p, m))| p != m)
        .map(|(s, _)| s)
        .collect();
    assert!(disagreements.is_empty(), "{disagreements:?}");
    assert!(
        mine.iter().any(|m| *m) && mine.iter().any(|m| !*m),
        "corpus is one-sided"
    );
}

/// Invariant: a single trailing newline does not hide a credential-shaped token from the strip.
/// Python's `$` is not end-of-string — outside `MULTILINE` it also matches just before one trailing
/// newline — so `_OPAQUE_RE` calls `"<token>\n"` opaque. Judging it *not* opaque would be fail-open:
/// the token would stay in the descriptor argv and enter the `descriptor_hash` preimage that this
/// rule exists to keep credentials out of. Two newlines are opaque to neither side.
/// Cannot prove: that the wider `_BEARER_RE`/`_JWT_RE` rules would have caught such a token anyway.
#[test]
fn a_single_trailing_newline_does_not_hide_an_opaque_token() {
    let token = "A1".repeat(16);
    assert!(is_opaque_token(&token));
    assert!(
        is_opaque_token(&format!("{token}\n")),
        "one trailing newline must still be opaque, as Python's `$` accepts it"
    );
    assert!(!is_opaque_token(&format!("{token}\n\n")));
    assert!(
        !is_opaque_token(&format!("\n{token}")),
        "`^` is string start"
    );

    let cases = [
        format!("{token}\n"),
        format!("{token}\n\n"),
        format!("\n{token}"),
        token.clone(),
    ];
    let Some(python) = python_opaque_matches(&cases) else {
        eprintln!("SKIPPED a_single_trailing_newline_does_not_hide_an_opaque_token: no python3");
        return;
    };
    let mine: Vec<bool> = cases.iter().map(|s| is_opaque_token(s)).collect();
    assert_eq!(python, mine, "newline handling must match the prototype");
}

/// Runs the prototype's `_OPAQUE_RE` over `corpus` in CPython. `None` when python3 is unavailable.
fn python_opaque_matches(corpus: &[String]) -> Option<Vec<bool>> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    const SCRIPT: &str = concat!(
        "import json,re,sys\n",
        // verbatim from spikes/828-passive-observer/prototype/attribute.py:69
        r#"RE = re.compile(r"^(?=.*[A-Za-z])(?=.*[0-9])[A-Za-z0-9_+/=-]{32,}$")"#,
        "\n",
        "print(json.dumps([RE.match(s) is not None for s in json.load(sys.stdin)]))\n",
    );
    let mut child = Command::new("python3")
        .args(["-c", SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let payload = serde_json::to_vec(corpus).expect("corpus serialises");
    child.stdin.take()?.write_all(&payload).ok()?;
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    if !child.wait().ok()?.success() {
        return None;
    }
    serde_json::from_str(out.trim()).ok()
}

/// Invariant: the two vendor-token rules survive the port unchanged — a prefixed bearer token and
/// a JWT header are secret-shaped wherever they sit in the token, while an ordinary flag or short
/// value is not.
/// Cannot prove: that these prefixes cover every credential format in the wild.
#[test]
fn secret_shaped_matches_bearer_and_jwt_shapes() {
    let bearer = ["ghp", "_", "invented0123456789"].concat();
    assert!(is_secret_shaped(&bearer));
    assert!(is_secret_shaped(&format!("Bearer:{bearer}")));
    assert!(is_secret_shaped(&["eyJ", "invented0123", "."].concat()));
    assert!(!is_secret_shaped("--api-key"));
    assert!(!is_secret_shaped("8080"));
    assert!(!is_secret_shaped(
        &["xghp", "_", "invented0123456789"].concat()
    ));
}

/// Invariant: `strip_args` reads a non-list `args` as an empty argv and renders non-string tokens
/// the way Python's `str()` does, so a hand-edited config cannot change a descriptor's identity
/// through a type the prototype would have stringified differently.
/// Cannot prove: parity for array- or object-valued argv entries (documented as a divergence).
#[test]
fn strip_args_handles_non_list_and_non_string_input() {
    assert!(strip_args(None).is_empty());
    assert!(strip_args(Some(&json!("--port"))).is_empty());
    assert!(strip_args(Some(&json!({"a": 1}))).is_empty());
    assert_eq!(
        strip_args(Some(&json!([true, false, Value::Null, 8080, "--port"]))),
        vec!["True", "False", "None", "8080", "--port"]
    );
    // A bare secret flag drops the next token; a glued one drops only its own value.
    assert_eq!(
        strip_args(Some(&json!(["-k", "abc", "--keep"]))),
        vec!["-k", "--keep"]
    );
    assert_eq!(
        strip_args(Some(&json!(["--token=abc", "--keep"]))),
        vec!["--token", "--keep"]
    );
    // A falsy `command` is the empty basename, as `str(x or "")` gives.
    let falsy = canonical_descriptor(&obj(json!({"command": 0, "args": []})));
    assert_eq!(falsy["command"], json!(""));
}

/// Invariant: a `SKILL.md` that is itself a directory, or a symlink to one, does not make its
/// parent a skill. `os.walk` splits entries into `dirnames` and `filenames` and the reference tests
/// `"SKILL.md" not in filenames`, so neither shape qualifies. Accepting them mints a skill that does
/// not exist, which adds an `asset_id` and therefore moves `bom_version` — the run reports a
/// different bill of materials than the machine actually has.
/// A broken symlink and a symlink to a file DO appear in `filenames`, so the rule cannot simply
/// reject symlinks; those cases are covered by the existing walk tests and must keep passing.
#[test]
fn a_skill_md_directory_does_not_mint_a_phantom_skill() {
    let dir = TempDir::new().expect("tempdir");
    let skills = dir.path().join("skills");

    write(&skills.join("real").join("SKILL.md"), "# real\n");
    fs::create_dir_all(skills.join("dirskill").join("SKILL.md")).expect("SKILL.md as a directory");

    let target = skills.join("target_dir");
    fs::create_dir_all(&target).expect("target dir");
    #[cfg(unix)]
    {
        fs::create_dir_all(skills.join("symskill")).expect("symskill");
        std::os::unix::fs::symlink(&target, skills.join("symskill").join("SKILL.md"))
            .expect("symlink to a directory");
    }

    let found: BTreeSet<String> = index_skills(&skills).into_keys().collect();
    assert!(found.contains("real"), "the real skill is still found");
    assert!(
        !found.contains("dirskill"),
        "a SKILL.md directory is not a skill: {found:?}"
    );
    #[cfg(unix)]
    assert!(
        !found.contains("symskill"),
        "a SKILL.md symlink to a directory is not a skill: {found:?}"
    );
}
