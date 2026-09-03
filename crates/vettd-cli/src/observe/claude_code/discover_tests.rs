//! Discovery tests, ported from the layout the prototype's fixtures exercise
//! (`spikes/828-passive-observer/prototype/sources/claude_code.py`, discovery section).
//!
//! Every tree here is built at test time under a `tempfile` scratch directory from invented names;
//! the one exception is `fixture_home_discovers_the_main_and_its_one_child`, which reads the
//! checked-in fixture home read-only and compares against values dumped from the Python prototype.
//!
//! The expected refs for that test came from running the oracle directly:
//!
//! ```text
//! cd spikes/828-passive-observer/prototype && python3 -c "
//! import sys, json, dataclasses; sys.path.insert(0,'.')
//! from sources.claude_code import ClaudeCodeSource
//! refs = ClaudeCodeSource('x').discover(
//!     'crates/vettd-cli/tests/fixtures/observe/claude_home', 3650, 1800000000000)
//! print(json.dumps([dataclasses.asdict(r) for r in refs], indent=2))"
//! ```

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use tempfile::TempDir;

use super::*;

/// The golden run's clock: 2027-01-15T08:00Z, far from any checkout mtime.
const NOW_MS: i64 = 1_800_000_000_000;

/// The golden run's window, wide enough that every fixture file is inside it.
const WINDOW_DAYS: u32 = 3650;

const HARNESS: &str = "claude_code";

/// Creates `path`'s parent directories and writes one invented ndjson line into it.
fn write_line(path: &Path) {
    fs::create_dir_all(path.parent().expect("path has a parent")).expect("create parent");
    fs::write(path, b"{\"type\":\"summary\"}\n").expect("write session line");
}

/// Sets `path`'s mtime to `ms` milliseconds after the epoch.
fn set_mtime(path: &Path, ms: u64) {
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for set_modified");
    file.set_modified(UNIX_EPOCH + Duration::from_millis(ms))
        .expect("set mtime");
}

/// `<tmp>/projects/<project>` for a scratch home.
fn project_dir(home: &Path, project: &str) -> PathBuf {
    home.join("projects").join(project)
}

/// The discovered paths, relative to `home`, as forward-slashed strings.
fn relative_paths(refs: &[SessionRef], home: &Path) -> Vec<String> {
    refs.iter()
        .map(|r| {
            r.path
                .strip_prefix(home)
                .expect("ref is under the home")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

/// Discovery on a scratch home with the golden clock and window.
fn discover_home(home: &Path) -> Vec<SessionRef> {
    discover(home, HARNESS, WINDOW_DAYS, NOW_MS)
}

/// The harness writes `.jsonl` and the repository's `.gitignore` swallows that suffix, so the
/// fixtures are `.ndjson`; a reader that knew only one of the two would either be untestable in
/// this repo or blind on a real machine.
#[test]
fn both_transcript_suffixes_are_discovered() {
    let tmp = TempDir::new().expect("tempdir");
    let pdir = project_dir(tmp.path(), "proj");
    write_line(&pdir.join("alpha.jsonl"));
    write_line(&pdir.join("beta.ndjson"));

    let refs = discover_home(tmp.path());

    let keys: Vec<&str> = refs.iter().map(|r| r.session_key.as_str()).collect();
    assert_eq!(keys, ["alpha", "beta"]);
    assert!(refs.iter().all(|r| r.kind == SessionKind::Main));
}

/// Workflow sub-agents live one directory deeper than ordinary ones. Stopping the walk at
/// `subagents/` would silently attribute a workflow's tool calls to nobody, so the extra level is
/// part of the layout contract, not an optimisation.
#[test]
fn workflow_children_one_level_deeper_are_discovered() {
    let tmp = TempDir::new().expect("tempdir");
    let pdir = project_dir(tmp.path(), "proj");
    write_line(&pdir.join("s1.ndjson"));
    write_line(&pdir.join("s1/subagents/agent-plain.ndjson"));
    write_line(&pdir.join("s1/subagents/workflows/wf-b/agent-w2.ndjson"));
    write_line(&pdir.join("s1/subagents/workflows/wf-a/agent-w1.ndjson"));

    let refs = discover_home(tmp.path());

    assert_eq!(
        relative_paths(&refs, tmp.path()),
        [
            "projects/proj/s1.ndjson",
            "projects/proj/s1/subagents/agent-plain.ndjson",
            "projects/proj/s1/subagents/workflows/wf-a/agent-w1.ndjson",
            "projects/proj/s1/subagents/workflows/wf-b/agent-w2.ndjson",
        ]
    );
    let workflow_child = &refs[2];
    assert_eq!(workflow_child.kind, SessionKind::Child);
    assert_eq!(workflow_child.parent_key.as_deref(), Some("s1"));
    assert_eq!(workflow_child.session_key, "w1");
}

/// The window is what bounds how much history a single run reports. A file exactly on the cutoff is
/// inside it (`>=`), so a daily run cannot drop a session by landing one millisecond late.
#[test]
fn the_window_cutoff_is_inclusive_and_excludes_older_files() {
    let tmp = TempDir::new().expect("tempdir");
    let pdir = project_dir(tmp.path(), "proj");
    let cutoff_ms = NOW_MS - i64::from(WINDOW_DAYS) * 86_400_000;
    for (name, ms) in [
        ("on-cutoff", cutoff_ms),
        ("inside", cutoff_ms + 1),
        ("outside", cutoff_ms - 1),
    ] {
        let path = pdir.join(format!("{name}.ndjson"));
        write_line(&path);
        set_mtime(&path, u64::try_from(ms).expect("post-epoch fixture mtime"));
    }

    let keys: Vec<String> = discover_home(tmp.path())
        .into_iter()
        .map(|r| r.session_key)
        .collect();

    assert_eq!(keys, ["inside", "on-cutoff"]);
}

/// The window guard covers only the main's own ref. A parent transcript that went quiet outside the
/// window must not hide sub-agent runs that happened inside it, or a long-lived session would erase
/// its own recent children.
#[test]
fn children_are_discovered_even_when_the_main_is_out_of_window() {
    let tmp = TempDir::new().expect("tempdir");
    let pdir = project_dir(tmp.path(), "proj");
    let main = pdir.join("s1.ndjson");
    write_line(&main);
    set_mtime(&main, 1_000_000_000_000);
    write_line(&pdir.join("s1/subagents/agent-c1.ndjson"));

    let refs = discover_home(tmp.path());

    assert_eq!(refs.len(), 1, "only the child survives the window");
    assert_eq!(refs[0].kind, SessionKind::Child);
    assert_eq!(refs[0].parent_key.as_deref(), Some("s1"));
}

/// `vettd observe` runs on machines that have never used the harness. A home with no `projects/`
/// must be an empty result, not an error, or the first run on a fresh laptop would fail.
#[test]
fn a_home_without_a_projects_directory_yields_no_refs() {
    let tmp = TempDir::new().expect("tempdir");
    assert!(discover_home(tmp.path()).is_empty());
    assert!(discover_home(&tmp.path().join("does-not-exist")).is_empty());
    assert!(listdir(&tmp.path().join("nope")).is_empty());
}

/// The sidecar is the harness's own bookkeeping, not the session. A truncated, non-object or
/// unreadable `.meta.json` must degrade to "no metadata" so the transcript is still read; only
/// `agentId`, which discovery derives itself, is guaranteed.
#[test]
fn a_malformed_or_unreadable_sidecar_yields_only_the_derived_agent_id() {
    let tmp = TempDir::new().expect("tempdir");
    let subagents = project_dir(tmp.path(), "proj").join("s1/subagents");
    for name in ["garbage", "notobject", "missing", "unreadable"] {
        write_line(&subagents.join(format!("agent-{name}.ndjson")));
    }
    fs::write(subagents.join("agent-garbage.meta.json"), b"{not json").expect("write garbage");
    fs::write(subagents.join("agent-notobject.meta.json"), b"[1, 2]").expect("write array");
    // A directory in the sidecar's place is the portable stand-in for an unreadable file.
    fs::create_dir_all(subagents.join("agent-unreadable.meta.json")).expect("create dir sidecar");

    let refs = discover_children(&subagents, "s1", HARNESS, 0);

    assert_eq!(refs.len(), 4);
    for r in &refs {
        assert_eq!(
            r.child_meta.keys().collect::<Vec<_>>(),
            ["agentId"],
            "{} kept more than the derived id",
            r.session_key
        );
    }
}

/// The sidecar carries free text (`description` in the fixtures). Only the three allowlisted keys
/// may reach `child_meta`, and only as strings or non-boolean integers — an allowlist is what keeps
/// a future harness field from leaking session content into a local-only map.
#[test]
fn only_allowlisted_scalar_sidecar_keys_survive() {
    let tmp = TempDir::new().expect("tempdir");
    let sidecar = tmp.path().join("agent-x.meta.json");
    fs::write(
        &sidecar,
        br#"{"agentType":"reviewer","toolUseId":"toolu_1","spawnDepth":2,
             "description":"free text","extra":"also free text"}"#,
    )
    .expect("write sidecar");

    let meta = read_child_meta(&sidecar);

    assert_eq!(meta.get("agentType").map(String::as_str), Some("reviewer"));
    assert_eq!(meta.get("toolUseId").map(String::as_str), Some("toolu_1"));
    assert_eq!(meta.get("spawnDepth").map(String::as_str), Some("2"));
    assert_eq!(meta.len(), 3, "no unallowlisted key survived");

    fs::write(
        &sidecar,
        br#"{"agentType":true,"toolUseId":1.5,"spawnDepth":null}"#,
    )
    .expect("rewrite sidecar");
    assert!(
        read_child_meta(&sidecar).is_empty(),
        "booleans, fractions and nulls are not scalars the sidecar may carry"
    );
}

/// A child's `session_key` is the id *after* the `agent-` prefix, and files under `subagents/` that
/// do not carry it are not children at all. Treating any transcript there as a child would invent
/// sessions whose ids collide with their parent's.
#[test]
fn a_stem_without_the_agent_prefix_is_not_a_child() {
    let tmp = TempDir::new().expect("tempdir");
    let subagents = tmp.path().join("subagents");
    write_line(&subagents.join("agent-real.ndjson"));
    write_line(&subagents.join("stray.ndjson"));
    write_line(&subagents.join("agents-typo.ndjson"));

    let refs = discover_children(&subagents, "parent", HARNESS, 0);

    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].session_key, "real");
    assert_eq!(
        refs[0].child_meta.get("agentId").map(String::as_str),
        Some("real")
    );
}

/// `read_dir` is unordered, but the order of the returned refs is the order the pipeline groups
/// children under their parent in. Sorting every listing by the raw entry name is what makes two
/// runs over the same unchanged home produce the same refs in the same order.
#[test]
fn listings_are_sorted_by_raw_entry_name() {
    let tmp = TempDir::new().expect("tempdir");
    for name in ["zeta", "Alpha", "beta", "-dash", "10", "2"] {
        File::create(tmp.path().join(name)).expect("create entry");
    }

    let names: Vec<String> = listdir(tmp.path())
        .into_iter()
        .map(|n| n.to_string_lossy().into_owned())
        .collect();

    assert_eq!(names, ["-dash", "10", "2", "Alpha", "beta", "zeta"]);
}

/// Projects, sessions and workflows are each sorted independently, and children follow their own
/// main. This whole-tree ordering is the contract downstream grouping reads.
#[test]
fn refs_come_back_in_project_then_session_then_child_order() {
    let tmp = TempDir::new().expect("tempdir");
    let a = project_dir(tmp.path(), "proj-a");
    write_line(&a.join("zbeta.ndjson"));
    write_line(&a.join("salpha.ndjson"));
    write_line(&a.join("salpha/subagents/agent-b2.ndjson"));
    write_line(&a.join("salpha/subagents/agent-a1.ndjson"));
    write_line(&a.join("salpha/subagents/workflows/wf-a/agent-w1.ndjson"));
    write_line(&project_dir(tmp.path(), "proj-b").join("other.ndjson"));

    let refs = discover_home(tmp.path());

    assert_eq!(
        relative_paths(&refs, tmp.path()),
        [
            "projects/proj-a/salpha.ndjson",
            "projects/proj-a/salpha/subagents/agent-a1.ndjson",
            "projects/proj-a/salpha/subagents/agent-b2.ndjson",
            "projects/proj-a/salpha/subagents/workflows/wf-a/agent-w1.ndjson",
            "projects/proj-a/zbeta.ndjson",
            "projects/proj-b/other.ndjson",
        ]
    );
}

/// A bare `.jsonl` is a dotfile, not a session whose key is the empty string; an empty
/// `session_key` would HMAC into a run identity shared by every such file on the machine.
#[test]
fn a_bare_suffix_is_not_a_session_stem() {
    assert_eq!(session_stem(OsStr::new("s.ndjson")), Some("s"));
    assert_eq!(session_stem(OsStr::new(".jsonl")), None);
    assert_eq!(session_stem(OsStr::new(".ndjson")), None);
    assert_eq!(session_stem(OsStr::new("session")), None);
    assert_eq!(session_stem(OsStr::new("notes.json")), None);
}

/// Directories share the project namespace with transcripts (`<stem>/` sits beside `<stem>.ndjson`)
/// and a directory is never a session. Anything that cannot be stat'ed is out of window rather than
/// an error, so one unreadable entry cannot abort the whole discovery.
#[test]
fn only_regular_files_are_in_window() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path().join("s1");
    fs::create_dir_all(&dir).expect("create dir");
    let file = tmp.path().join("s1.ndjson");
    write_line(&file);

    assert!(within_window(&file, 0));
    assert!(!within_window(&dir, 0), "a directory is not a session file");
    assert!(
        !within_window(&tmp.path().join("gone.ndjson"), 0),
        "a missing file is out of window, not an error"
    );
}

/// Parity with the Python oracle on the real fixture home: the same two refs, in the same order,
/// with the same keys and the same sidecar values. This is the check that stops the port from
/// drifting from the prototype the goldens were generated with.
#[test]
fn fixture_home_discovers_the_main_and_its_one_child() {
    let home = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/observe/claude_home")
        .canonicalize()
        .expect("fixture home exists");

    let refs = discover(&home, HARNESS, WINDOW_DAYS, NOW_MS);

    assert_eq!(
        relative_paths(&refs, &home),
        [
            "projects/-fixture-project/0f1e2d3c-4b5a-6978-8a9b-0c1d2e3f4a5b.ndjson",
            "projects/-fixture-project/0f1e2d3c-4b5a-6978-8a9b-0c1d2e3f4a5b/subagents/agent-fx1.ndjson",
        ]
    );

    let main = &refs[0];
    assert_eq!(main.harness, HARNESS);
    assert_eq!(main.kind, SessionKind::Main);
    assert_eq!(main.session_key, "0f1e2d3c-4b5a-6978-8a9b-0c1d2e3f4a5b");
    assert_eq!(main.parent_key, None);
    assert!(main.child_meta.is_empty());

    let child = &refs[1];
    assert_eq!(child.kind, SessionKind::Child);
    assert_eq!(child.session_key, "fx1");
    assert_eq!(
        child.parent_key.as_deref(),
        Some("0f1e2d3c-4b5a-6978-8a9b-0c1d2e3f4a5b")
    );
    let meta: Vec<(&str, &str)> = child
        .child_meta
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        meta,
        [
            ("agentId", "fx1"),
            ("agentType", "fx-reviewer"),
            ("spawnDepth", "1"),
            ("toolUseId", "toolu_fx00000005"),
        ],
        "the sidecar's free-text `description` must not survive"
    );
}
