//! Tests for [`super`], ported from `spikes/828-passive-observer/prototype/tests/test_observe.py`.
//!
//! These drive the resume machinery directly — `read_group`, `probe_group`, `group_sessions` — so
//! they need no `$HOME` and no subprocess. The end-to-end behaviour that depends on the real
//! environment (the opt-in file, stdout/stderr split, exit codes) is asserted against the built
//! binary in `crates/vettd-cli/tests/observe_integration.rs`.
//!
//! Every fixture is a copy of the committed one in a `tempfile` directory: a test that appended to
//! `tests/fixtures` would corrupt the baseline every other test compares against.

use std::fs;

use tempfile::TempDir;

use super::*;

const NOW_MS: i64 = 1_800_000_000_000;
const SECRET: &[u8] = b"invented-observer-secret-material";

/// A writable copy of the committed fixture home.
fn fixture_home() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/observe/claude_home");
    let dst = dir.path().join("claude_home");
    copy_tree(&src, &dst);
    (dir, dst)
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dir");
    for entry in fs::read_dir(src).expect("read dir") {
        let entry = entry.expect("entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

fn discover(root: &Path) -> (ClaudeCodeSource, Vec<Group>) {
    let source = ClaudeCodeSource::with_now_ms(root.to_path_buf(), NOW_MS);
    let refs = source.discover(root, 3650, NOW_MS).expect("discover");
    (source, group_sessions(refs))
}

fn blank_coverage() -> Coverage {
    Coverage {
        sessions_seen: 0,
        sessions_emitted: 0,
        sessions_skipped_unparseable: 0,
        lines_seen: 0,
        lines_unknown_type: 0,
        bytes_read: 0,
        truncated_sessions: 0,
        window_days: 3650,
        cursor_state: "fresh".to_string(),
    }
}

/// Read every file of every group from byte 0 and commit the resulting cursors, so the store is in
/// the state a completed submission would leave it in.
fn seed_cursors(store: &mut Store, source: &ClaudeCodeSource, groups: &[Group]) {
    let mut staged = Vec::new();
    for group in groups {
        for r in group.refs() {
            let (_, cursor) = source.read(r, None).expect("read");
            staged.push((source.harness().to_string(), cursor));
        }
    }
    store.commit(&staged, &[]).expect("commit");
}

/// Invariant: a group whose files have not grown emits NOTHING and stages a cursor for EVERY file
/// in it. Emitting would re-send an identical record; staging only some files would make the group
/// partially cursored, and a partially cursored group is read whole next time — so the silence
/// would not persist and every later run would re-read the whole transcript.
#[test]
fn unchanged_resume_emits_silence_and_stages_cursors_every_file() {
    let (dir, root) = fixture_home();
    let (source, groups) = discover(&root);
    let mut store = Store::open_at(&dir.path().join("store.sqlite3")).expect("store");
    store
        .ensure_secret_fingerprint(SECRET)
        .expect("fingerprint");
    seed_cursors(&mut store, &source, &groups);

    let file_count: usize = groups.iter().map(|g| g.refs().count()).sum();
    let mut coverage = blank_coverage();
    let mut staged = Vec::new();
    for group in &groups {
        let emitted = read_group(&source, group, Some(&store), &mut coverage, &mut staged)
            .expect("read group");
        assert!(emitted.is_none(), "an unchanged group must emit nothing");
    }
    assert_eq!(
        staged.len(),
        file_count,
        "every file in the group must be staged, not just the main"
    );
    assert_eq!(coverage.lines_seen, 0, "nothing was re-read");
    assert_eq!(coverage.sessions_skipped_unparseable, 0);
}

/// Invariant: when the MAIN transcript grows, the whole run is rebuilt from byte zero — a record is
/// the cumulative state of one run and `run_id` is its idempotency key, so a partial replacement
/// would report a run missing everything before the cursor. The probe's bytes stay in coverage on
/// top of the full re-read: coverage reports what was READ, not what was emitted, and hiding the
/// probe would understate the work done.
#[test]
fn changed_main_rebuilds_the_complete_run_and_double_counts_probe_bytes() {
    let (dir, root) = fixture_home();
    let (source, groups) = discover(&root);
    let mut store = Store::open_at(&dir.path().join("store.sqlite3")).expect("store");
    seed_cursors(&mut store, &source, &groups);

    // A full read from byte 0, for comparison.
    let mut baseline = blank_coverage();
    let mut discard = Vec::new();
    let full = read_group(&source, &groups[0], None, &mut baseline, &mut discard)
        .expect("read")
        .expect("emits");
    let full_lines = full.lines_seen + full.children.iter().map(|c| c.lines_seen).sum::<u64>();

    // Append one complete line to the main transcript.
    let appended = "{\"type\":\"summary\",\"timestamp\":\"2026-08-15T10:02:00.000Z\"}\n";
    let mut existing = fs::read(&groups[0].main.path).expect("read main");
    existing.extend_from_slice(appended.as_bytes());
    fs::write(&groups[0].main.path, &existing).expect("append");

    let (source, groups) = discover(&root);
    let mut coverage = blank_coverage();
    let mut staged = Vec::new();
    let facts = read_group(
        &source,
        &groups[0],
        Some(&store),
        &mut coverage,
        &mut staged,
    )
    .expect("read group")
    .expect("a changed group must emit");

    let rebuilt = facts.lines_seen + facts.children.iter().map(|c| c.lines_seen).sum::<u64>();
    assert_eq!(
        rebuilt,
        full_lines + 1,
        "the rebuild is the whole run plus the new line, not just the new line"
    );
    assert_eq!(
        coverage.lines_seen,
        rebuilt + 1,
        "the one probed line is counted on top of the full re-read"
    );
}

/// Invariant: when a CHILD transcript grows, the parent run is rebuilt complete. A sub-agent's
/// tokens and failures belong to its parent's record, so emitting the child's delta alone — or the
/// parent without the child — would report a run whose sub-agent evidence does not add up.
#[test]
fn changed_child_rebuilds_the_complete_parent_run() {
    let (dir, root) = fixture_home();
    let (source, groups) = discover(&root);
    assert!(
        !groups[0].children.is_empty(),
        "the fixture must have a child for this test to mean anything"
    );
    let mut store = Store::open_at(&dir.path().join("store.sqlite3")).expect("store");
    seed_cursors(&mut store, &source, &groups);

    let child_path = groups[0].children[0].path.clone();
    let mut existing = fs::read(&child_path).expect("read child");
    existing
        .extend_from_slice(b"{\"type\":\"summary\",\"timestamp\":\"2026-08-15T10:01:30.000Z\"}\n");
    fs::write(&child_path, &existing).expect("append");

    let (source, groups) = discover(&root);
    let mut coverage = blank_coverage();
    let mut staged = Vec::new();
    let facts = read_group(
        &source,
        &groups[0],
        Some(&store),
        &mut coverage,
        &mut staged,
    )
    .expect("read group")
    .expect("a changed child must rebuild the parent");

    assert!(
        facts.lines_seen > 0,
        "the parent was re-read, not just the child"
    );
    assert_eq!(facts.children.len(), groups[0].children.len());
    assert!(
        facts.children.iter().any(|c| c.compactions > 0),
        "the appended child line is in the rebuilt record"
    );
}

/// Invariant: a group whose main transcript cannot be read stages NO cursor and is counted
/// unparseable. Advancing a cursor over bytes that were never parsed would skip them forever — the
/// failure has to be retried, so the prior cursor must survive.
#[test]
fn failed_rebuild_does_not_advance_the_probe_cursor() {
    let (dir, root) = fixture_home();
    let (source, groups) = discover(&root);
    let mut store = Store::open_at(&dir.path().join("store.sqlite3")).expect("store");
    seed_cursors(&mut store, &source, &groups);
    let before = store
        .load_cursor(&groups[0].main.path)
        .expect("read")
        .expect("seeded");

    // Replace the main transcript with a directory: a read that cannot succeed at all.
    fs::remove_file(&groups[0].main.path).expect("remove");
    fs::create_dir(&groups[0].main.path).expect("directory in its place");

    let (source, groups) = discover(&root);
    let mut coverage = blank_coverage();
    let mut staged = Vec::new();
    // Discovery skips a non-file, so the group may vanish entirely; either way no cursor moves.
    if let Some(group) = groups.first() {
        let _ = read_group(&source, group, Some(&store), &mut coverage, &mut staged);
    }
    assert!(
        !staged.iter().any(|(_, c)| c.path == before.path),
        "a failed read must not stage a cursor for the file it failed on"
    );
    assert_eq!(
        store.load_cursor(&before.path).expect("read"),
        Some(before),
        "the committed cursor is untouched until a commit says otherwise"
    );
}

/// Invariant: a group whose CHILD cannot be read emits nothing at all, so the prior complete record
/// stays authoritative on the server. Emitting the parent alone would replace a correct record with
/// one that silently lost its sub-agent, under the same `run_id`.
#[test]
fn failed_child_rebuild_preserves_the_complete_parent_record() {
    let (_dir, root) = fixture_home();
    let (source, groups) = discover(&root);
    let child_path = groups[0].children[0].path.clone();

    // A directory where the child transcript was: discovery still lists nothing for it, so drive
    // `read_group` with the original refs to exercise the child-failure branch directly.
    fs::remove_file(&child_path).expect("remove");
    fs::create_dir(&child_path).expect("directory in its place");

    let mut coverage = blank_coverage();
    let mut staged = Vec::new();
    let emitted =
        read_group(&source, &groups[0], None, &mut coverage, &mut staged).expect("read group");
    assert!(
        emitted.is_none(),
        "a group with an unreadable child must emit nothing"
    );
    assert_eq!(coverage.sessions_skipped_unparseable, 1);
    assert!(
        staged.is_empty(),
        "the whole group's cursors are abandoned, not just the child's"
    );
}

/// Invariant: an explicitly supplied clock of zero is used, not silently replaced by the wall
/// clock. `unwrap_or_else` on an `Option` gets this right and a `now_ms == 0` sentinel would not —
/// and a test hook that quietly ignored its value would make every golden irreproducible.
#[test]
fn explicit_zero_now_is_not_replaced_by_wall_clock() {
    let dir = TempDir::new().expect("tempdir");
    let secret_path = dir.path().join("secret.bin");
    fs::write(&secret_path, SECRET).expect("write secret");

    let args = observe_args(&secret_path, Some(0), None);
    let context = resolve_context(&args, dir.path().to_path_buf()).expect("resolves");
    assert_eq!(context.now_ms, 0, "an explicit zero must survive");
    assert_eq!(
        context.today, "1970-01-01",
        "and the day is derived from it, not from today"
    );
    assert_eq!(context.run_id_basis, "test_secret");

    // An explicit --today still wins over the derived one.
    let pinned = observe_args(&secret_path, Some(NOW_MS), Some("2027-01-15"));
    let context = resolve_context(&pinned, dir.path().to_path_buf()).expect("resolves");
    assert_eq!(context.today, "2027-01-15");
}

fn observe_args(secret: &Path, now_ms: Option<i64>, today: Option<&str>) -> ObserveArgs {
    ObserveArgs {
        harness: "claude_code".to_string(),
        root: None,
        task: None,
        window_days: 30,
        model: None,
        dry_run: false,
        out: None,
        scrub: false,
        public_names: None,
        prices: None,
        submit: None,
        api_key: None,
        allow_public_endpoint: false,
        resend: false,
        secret_file: Some(secret.to_path_buf()),
        now_ms,
        today: today.map(str::to_string),
    }
}

/// Invariant: a main transcript is grouped with its own children, sorted by path, and a child whose
/// parent was not discovered is dropped rather than promoted to a run of its own. A sub-agent
/// transcript is not a run; reporting one as a run would invent a record with no user turn in it.
#[test]
fn groups_pair_mains_with_their_children_in_path_order() {
    let (_dir, root) = fixture_home();
    let (_source, groups) = discover(&root);
    assert_eq!(groups.len(), 1, "the fixture home holds one run");
    assert_eq!(groups[0].children.len(), 1);
    assert_eq!(
        groups[0].children[0].parent_key.as_deref(),
        Some(groups[0].main.session_key.as_str()),
        "the child is attached to its own parent"
    );

    let orphan = SessionRef {
        path: root.join("orphan.ndjson"),
        harness: "claude_code".to_string(),
        session_key: "agent-nobody".to_string(),
        kind: SessionKind::Child,
        parent_key: Some("a-main-that-was-not-discovered".to_string()),
        child_meta: BTreeMap::new(),
    };
    let grouped = group_sessions(vec![orphan]);
    assert!(
        grouped.is_empty(),
        "a child without its parent is not a run"
    );
}
