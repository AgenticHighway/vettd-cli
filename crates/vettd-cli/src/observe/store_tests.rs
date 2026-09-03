//! Tests for [`super`], covering the invariants the prototype's `cursor_store.py` guaranteed plus
//! the ledger it had no need for.
//!
//! Every path below is a `tempfile` scratch directory; nothing here touches the real `~/.vettd`.

use tempfile::TempDir;

use super::*;

fn cursor(path: &Path, offset: u64, inode: Option<u64>) -> Cursor {
    Cursor {
        path: path.to_path_buf(),
        byte_offset: offset,
        inode,
    }
}

fn ledger(run: &str) -> LedgerRow {
    LedgerRow {
        run_id: run.to_string(),
        endpoint_host: "app.invented.test".to_string(),
        harness: "claude_code".to_string(),
        record_sha256: format!("{run}-hash"),
        emitted_day: "2026-09-03".to_string(),
    }
}

/// Invariant: the connection carries the pragmas the plan specifies. WAL and a busy timeout are
/// what let a second `vettd observe` run without either process failing on a lock; without them a
/// concurrent run would abort on `SQLITE_BUSY` rather than wait.
/// Cannot prove SQLite honours them under every filesystem — a network mount may refuse WAL.
#[test]
fn wal_and_busy_timeout_are_set() {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open_at(&dir.path().join("observer-v1.sqlite3")).expect("opens");
    assert_eq!(store.pragma("journal_mode").expect("journal_mode"), "wal");
    assert_eq!(store.pragma("busy_timeout").expect("busy_timeout"), "5000");
    // synchronous=NORMAL is 1.
    assert_eq!(store.pragma("synchronous").expect("synchronous"), "1");
}

/// Invariant: opening fails OPEN. A missing file is created, and a corrupt one is moved aside and
/// recreated rather than returned as an error — the observer must never block a user's run over
/// its own bookkeeping. The corrupt file is preserved, because losing the ledger is bad and losing
/// the evidence of why is worse.
/// Cannot prove recovery from every kind of corruption SQLite can produce.
#[test]
fn store_open_tolerates_missing_and_corrupt_db() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("observer-v1.sqlite3");

    // Missing: created.
    let store = Store::open_at(&path).expect("creates a missing store");
    drop(store);
    assert!(path.exists());

    // Corrupt: recovered, and the bad file kept.
    fs::write(&path, b"this is not a sqlite database at all").expect("write garbage");
    let store = Store::open_at(&path).expect("recovers from a corrupt store");
    assert!(!store.has_any_cursor().expect("usable after recovery"));
    let kept: Vec<_> = fs::read_dir(dir.path())
        .expect("read dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
        .collect();
    assert_eq!(
        kept.len(),
        1,
        "the corrupt file must be preserved, not deleted"
    );
    assert_eq!(
        fs::read(kept[0].path()).expect("read kept"),
        b"this is not a sqlite database at all",
        "and preserved byte for byte"
    );
}

/// Invariant: a rotated observer secret clears BOTH tables. The secret keys every `run_id`, so a
/// surviving cursor would attribute new bytes to a pseudonym the server has never seen, and a
/// surviving ledger row would suppress a run the server does not have. The same secret must clear
/// nothing — that is the case a naive "fingerprint differs" check gets right and a
/// "write it every time" implementation gets wrong.
#[test]
fn secret_rotation_clears_cursors_and_ledger() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("observer-v1.sqlite3");
    let session = dir.path().join("session.ndjson");

    let mut store = Store::open_at(&path).expect("opens");
    assert!(
        !store
            .ensure_secret_fingerprint(b"first-invented-secret")
            .expect("records"),
        "the first secret is not a rotation"
    );
    store
        .commit(
            &[("claude_code".to_string(), cursor(&session, 42, Some(7)))],
            &[ledger("run-a")],
        )
        .expect("commits");
    assert!(store.has_any_cursor().expect("has"));
    assert_eq!(store.ledger_len(), 1);

    // Same secret: nothing cleared.
    assert!(!store
        .ensure_secret_fingerprint(b"first-invented-secret")
        .expect("unchanged"));
    assert!(store.has_any_cursor().expect("still has"));
    assert_eq!(store.ledger_len(), 1);

    // Different secret: both cleared, and the rotation is reported.
    assert!(store
        .ensure_secret_fingerprint(b"second-invented-secret")
        .expect("rotates"));
    assert!(!store.has_any_cursor().expect("cursors cleared"));
    assert_eq!(store.ledger_len(), 0, "ledger cleared");
}

/// Invariant: `commit` is one transaction over both tables. A cursor advanced without its ledger
/// row would drop a run that was never sent; a ledger row without its cursor would re-read bytes
/// already accounted for. Both are silent, so a partial commit must leave NEITHER table changed.
/// The failure is injected inside the real commit path rather than simulated: every column is
/// populated and every insert is an upsert, so no typed argument can produce a constraint
/// violation to fail on.
#[test]
fn commit_is_atomic_across_cursors_and_ledger() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("observer-v1.sqlite3");
    let session = dir.path().join("session.ndjson");
    let mut store = Store::open_at(&path).expect("opens");

    store
        .commit(
            &[("claude_code".to_string(), cursor(&session, 10, None))],
            &[ledger("run-a"), ledger("run-b")],
        )
        .expect("a well-formed batch commits");
    assert_eq!(store.cursor_paths().len(), 1);
    assert_eq!(store.ledger_len(), 2);

    // Now fail partway: one cursor and one ledger row succeed, then the injected failure fires.
    // The seam is inside the real commit path, so this is the product's transaction rolling back.
    let before_cursors = store.cursor_paths();
    let before_ledger = store.ledger_len();
    let result = store.commit_failing_after(
        &[(
            "claude_code".to_string(),
            cursor(&dir.path().join("other.ndjson"), 99, None),
        )],
        &[ledger("run-c")],
        2,
    );
    assert!(result.is_err(), "the injected failure must surface");
    assert_eq!(
        store.cursor_paths(),
        before_cursors,
        "no cursor may survive a failed commit"
    );
    assert_eq!(
        store.ledger_len(),
        before_ledger,
        "no ledger row may survive a failed commit"
    );
}

/// Invariant: the store keeps the newest [`MAX_CURSOR_ROWS`] cursors and drops the oldest, so a
/// long-lived machine cannot grow the file without bound. Eviction runs after the inserts, so a
/// cursor staged in the same commit is among the newest and can never be the one evicted — the
/// opposite order would silently discard the run being recorded.
/// Cannot prove the cap is the right number.
#[test]
fn cursor_store_evicts_oldest_beyond_cap() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("observer-v1.sqlite3");
    let mut store = Store::open_at(&path).expect("opens");

    // Fill past the cap in one batch; `updated_at` ties, so the tiebreak is the path.
    let over = 5;
    let staged: Vec<(String, Cursor)> = (0..MAX_CURSOR_ROWS + over)
        .map(|i| {
            (
                "claude_code".to_string(),
                cursor(&dir.path().join(format!("s{i:06}.ndjson")), i as u64, None),
            )
        })
        .collect();
    store.commit(&staged, &[]).expect("commits");
    assert_eq!(
        store.cursor_paths().len(),
        MAX_CURSOR_ROWS,
        "the cap is enforced"
    );

    // A later commit's cursor survives, and the table stays at the cap.
    let fresh = dir.path().join("zz-newest.ndjson");
    store
        .commit(&[("claude_code".to_string(), cursor(&fresh, 1, None))], &[])
        .expect("commits");
    let paths = store.cursor_paths();
    assert_eq!(paths.len(), MAX_CURSOR_ROWS);
    assert!(
        paths.iter().any(|p| p.ends_with("zz-newest.ndjson")),
        "the cursor staged last must not be the one evicted"
    );
}

/// Invariant: the ledger is keyed on the record hash as well as the run. A run whose record changed
/// — a truncated run that later completed — must NOT read as already-sent, or the replacement
/// would never reach the server and the run would stay truncated there forever.
#[test]
fn ledger_is_keyed_on_the_record_hash_not_just_the_run() {
    let dir = TempDir::new().expect("tempdir");
    let mut store = Store::open_at(&dir.path().join("observer-v1.sqlite3")).expect("opens");
    let row = ledger("run-a");
    store.commit(&[], &[row.clone()]).expect("commits");

    assert!(store
        .ledger_has(&row.run_id, &row.endpoint_host, &row.record_sha256)
        .expect("hit"));
    assert!(
        !store
            .ledger_has(&row.run_id, &row.endpoint_host, "a-different-hash")
            .expect("miss"),
        "a changed record must be resendable"
    );
    assert!(
        !store
            .ledger_has(&row.run_id, "other.invented.test", &row.record_sha256)
            .expect("miss"),
        "another endpoint has not seen it"
    );
}

/// Invariant: a cursor round-trips exactly, including an absent inode. The offset always names a
/// line boundary, so a wrong value does not merely re-read — it resumes mid-line and drops a
/// record. `None` for the inode is the Windows case, where validity rests on size alone.
#[test]
fn cursors_round_trip_including_an_absent_inode() {
    let dir = TempDir::new().expect("tempdir");
    let mut store = Store::open_at(&dir.path().join("observer-v1.sqlite3")).expect("opens");
    let with_inode = dir.path().join("a.ndjson");
    let without = dir.path().join("b.ndjson");
    store
        .commit(
            &[
                (
                    "claude_code".to_string(),
                    cursor(&with_inode, 4096, Some(99)),
                ),
                ("claude_code".to_string(), cursor(&without, 0, None)),
            ],
            &[],
        )
        .expect("commits");

    assert_eq!(
        store.load_cursor(&with_inode).expect("read"),
        Some(cursor(&with_inode, 4096, Some(99)))
    );
    assert_eq!(
        store.load_cursor(&without).expect("read"),
        Some(cursor(&without, 0, None))
    );
    assert_eq!(
        store
            .load_cursor(&dir.path().join("never-seen.ndjson"))
            .expect("read"),
        None
    );
}
