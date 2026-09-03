//! Local state for resumable reads and send-once submission.
//!
//! Replaces `spikes/828-passive-observer/prototype/cursor_store.py`'s JSON file with SQLite, and
//! adds the submission ledger the prototype had no need for. Everything here is local-only
//! bookkeeping: the paths and run ids it holds never egress.
//!
//! # Why this is not the scan cache
//!
//! `scan_cache.rs` opens with a plain `Connection::open` — no WAL, no busy timeout — and orphans
//! its rows whenever `CACHE_SCHEMA_VERSION` or the crate version changes. That is right for a
//! cache, where a stale row costs a re-scan. It is wrong for cursors: dropping one silently
//! re-reads a whole transcript, and dropping a ledger row silently re-sends a run. So this is a
//! separate file with its own pragmas and no version-based orphaning.
//!
//! # Two behaviours that matter more than they look
//!
//! [`Store::ensure_secret_fingerprint`] clears **both** tables when the observer secret changes. A
//! rotated secret changes every `run_id`, so a surviving cursor would attribute new bytes to a
//! pseudonym the server has never seen, and a surviving ledger row would suppress a run the server
//! does not have.
//!
//! Opening **fails open**. A corrupt database is renamed aside and recreated rather than returned
//! as an error, because the observer must never block a user's run over its own bookkeeping. The
//! corrupt file is *preserved*, not deleted — losing the ledger is bad, losing the evidence of why
//! is worse.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::observe::canonical::hex_sha256;
use crate::observe::types::Cursor;

/// Cursor rows kept before the oldest are evicted.
pub(crate) const MAX_CURSOR_ROWS: usize = 10_000;

const SECRET_FINGERPRINT_KEY: &str = "secret_fingerprint";

/// One row of the submission ledger: this run, at this endpoint, with this record hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerRow {
    pub run_id: String,
    pub endpoint_host: String,
    pub harness: String,
    pub record_sha256: String,
    pub emitted_day: String,
}

/// `~/.vettd/observer/observer-v1.sqlite3`.
pub(crate) fn default_store_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| {
            home.join(".vettd")
                .join("observer")
                .join("observer-v1.sqlite3")
        })
        .ok_or_else(|| {
            "Unable to determine home directory — cannot resolve the observer store".to_string()
        })
}

pub(crate) struct Store {
    conn: Connection,
}

impl Store {
    pub(crate) fn open_default() -> Result<Store, String> {
        Store::open_at(&default_store_path()?)
    }

    /// Open (or create) the store at `path`, recovering from a corrupt file rather than failing.
    pub(crate) fn open_at(path: &Path) -> Result<Store, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create observer store directory: {e}"))?;
        }
        match Store::connect(path) {
            Ok(store) => Ok(store),
            Err(_) => {
                // Fail open: move the unusable file aside, keep it for diagnosis, and start clean.
                let aside = path.with_extension(format!("sqlite3.corrupt-{}", unix_seconds()));
                let _ = fs::rename(path, &aside);
                Store::connect(path).map_err(|e| {
                    format!(
                        "Failed to open the observer store {} even after moving the previous file to {}: {e}",
                        path.display(),
                        aside.display()
                    )
                })
            }
        }
    }

    fn connect(path: &Path) -> Result<Store, String> {
        let conn = Connection::open(path)
            .map_err(|e| format!("Failed to open observer store {}: {e}", path.display()))?;
        // WAL and a busy timeout because a second `vettd observe` may be running; NORMAL synchronous
        // because a lost cursor costs a re-read, not correctness.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             PRAGMA synchronous=NORMAL;",
        )
        .map_err(|e| format!("Failed to configure the observer store: {e}"))?;
        let store = Store { conn };
        store.ensure_schema()?;
        // A file can open and configure cleanly and still not be our schema; a read proves it.
        store
            .has_any_cursor()
            .map_err(|e| format!("Observer store is not usable: {e}"))?;
        Ok(store)
    }

    fn ensure_schema(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS observer_meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS observer_cursors (
                    path TEXT PRIMARY KEY,
                    harness TEXT NOT NULL,
                    byte_offset INTEGER NOT NULL,
                    inode INTEGER,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS observer_ledger (
                    run_id TEXT NOT NULL,
                    endpoint_host TEXT NOT NULL,
                    harness TEXT NOT NULL,
                    record_sha256 TEXT NOT NULL,
                    emitted_day TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (run_id, endpoint_host)
                );
                ",
            )
            .map_err(|e| format!("Failed to create the observer store schema: {e}"))
    }

    /// Record the secret's fingerprint, clearing both tables when it changed.
    ///
    /// Returns `true` when state was cleared, so the caller can report a rotation rather than
    /// leaving the user wondering why every session came back.
    pub(crate) fn ensure_secret_fingerprint(&self, secret: &[u8]) -> Result<bool, String> {
        let fingerprint = hex_sha256(secret);
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM observer_meta WHERE key = ?1",
                params![SECRET_FINGERPRINT_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read the observer secret fingerprint: {e}"))?;
        if stored.as_deref() == Some(fingerprint.as_str()) {
            return Ok(false);
        }
        let rotated = stored.is_some();
        self.conn
            .execute_batch("DELETE FROM observer_cursors; DELETE FROM observer_ledger;")
            .map_err(|e| format!("Failed to clear observer state after a secret change: {e}"))?;
        self.conn
            .execute(
                "INSERT INTO observer_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![SECRET_FINGERPRINT_KEY, fingerprint],
            )
            .map_err(|e| format!("Failed to record the observer secret fingerprint: {e}"))?;
        Ok(rotated)
    }

    pub(crate) fn load_cursor(&self, path: &Path) -> Result<Option<Cursor>, String> {
        let key = path.to_string_lossy().to_string();
        self.conn
            .query_row(
                "SELECT byte_offset, inode FROM observer_cursors WHERE path = ?1",
                params![key],
                |row| {
                    let offset: i64 = row.get(0)?;
                    let inode: Option<i64> = row.get(1)?;
                    Ok(Cursor {
                        path: path.to_path_buf(),
                        byte_offset: offset.max(0) as u64,
                        inode: inode.map(|i| i as u64),
                    })
                },
            )
            .optional()
            .map_err(|e| format!("Failed to read a cursor: {e}"))
    }

    pub(crate) fn has_any_cursor(&self) -> Result<bool, String> {
        self.conn
            .query_row("SELECT EXISTS(SELECT 1 FROM observer_cursors)", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|found| found != 0)
            .map_err(|e| format!("Failed to count cursors: {e}"))
    }

    /// Whether this exact record was already accepted at this endpoint.
    ///
    /// Keyed on the record hash as well as the run: a run whose record changed — a truncated run
    /// that later completed, say — must NOT read as already-sent, or the replacement would never
    /// reach the server.
    pub(crate) fn ledger_has(
        &self,
        run_id: &str,
        endpoint_host: &str,
        record_sha256: &str,
    ) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM observer_ledger
                     WHERE run_id = ?1 AND endpoint_host = ?2 AND record_sha256 = ?3
                 )",
                params![run_id, endpoint_host, record_sha256],
                |row| row.get::<_, i64>(0),
            )
            .map(|found| found != 0)
            .map_err(|e| format!("Failed to read the observer ledger: {e}"))
    }

    /// Persist staged cursors and ledger rows in ONE transaction.
    ///
    /// All or nothing on purpose: a cursor advanced without its ledger row would drop a run that
    /// was never sent, and a ledger row without its cursor would re-read bytes already accounted
    /// for. Either is silent, so neither is allowed to happen alone.
    pub(crate) fn commit(
        &mut self,
        cursors: &[(String, Cursor)],
        ledger_rows: &[LedgerRow],
    ) -> Result<(), String> {
        self.commit_inner(cursors, ledger_rows, None)
    }

    /// [`Store::commit`] with an injected failure after `fail_after` statements.
    ///
    /// A test-only seam into the *real* commit path, rather than a reimplementation of it: proving
    /// rollback needs a failure partway through, and no typed argument to `commit` can produce one
    /// (every column is populated and every insert is an upsert, so there is no constraint left to
    /// violate). Injecting it here means the test exercises the transaction the product uses.
    #[cfg(test)]
    pub(crate) fn commit_failing_after(
        &mut self,
        cursors: &[(String, Cursor)],
        ledger_rows: &[LedgerRow],
        fail_after: usize,
    ) -> Result<(), String> {
        self.commit_inner(cursors, ledger_rows, Some(fail_after))
    }

    fn commit_inner(
        &mut self,
        cursors: &[(String, Cursor)],
        ledger_rows: &[LedgerRow],
        fail_after: Option<usize>,
    ) -> Result<(), String> {
        let mut written = 0usize;
        let now = Utc::now().to_rfc3339();
        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("Failed to begin an observer store transaction: {e}"))?;
        for (harness, cursor) in cursors {
            tx.execute(
                "INSERT INTO observer_cursors (path, harness, byte_offset, inode, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(path) DO UPDATE SET
                     harness = excluded.harness,
                     byte_offset = excluded.byte_offset,
                     inode = excluded.inode,
                     updated_at = excluded.updated_at",
                params![
                    cursor.path.to_string_lossy().to_string(),
                    harness,
                    cursor.byte_offset as i64,
                    cursor.inode.map(|i| i as i64),
                    now,
                ],
            )
            .map_err(|e| format!("Failed to stage a cursor: {e}"))?;
            written += 1;
            if fail_after == Some(written) {
                return Err("injected failure after a staged cursor".to_string());
            }
        }
        for row in ledger_rows {
            tx.execute(
                "INSERT INTO observer_ledger
                     (run_id, endpoint_host, harness, record_sha256, emitted_day, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(run_id, endpoint_host) DO UPDATE SET
                     harness = excluded.harness,
                     record_sha256 = excluded.record_sha256,
                     emitted_day = excluded.emitted_day,
                     updated_at = excluded.updated_at",
                params![
                    row.run_id,
                    row.endpoint_host,
                    row.harness,
                    row.record_sha256,
                    row.emitted_day,
                    now,
                ],
            )
            .map_err(|e| format!("Failed to record a ledger row: {e}"))?;
            written += 1;
            if fail_after == Some(written) {
                return Err("injected failure after a ledger row".to_string());
            }
        }
        evict_cursors(&tx)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit observer state: {e}"))
    }

    /// One pragma value, for the tests that assert the connection is configured as intended.
    #[cfg(test)]
    fn pragma(&self, name: &str) -> Result<String, String> {
        self.conn
            .query_row(&format!("PRAGMA {name}"), [], |row| {
                row.get::<_, rusqlite::types::Value>(0)
            })
            .map(|value| match value {
                rusqlite::types::Value::Text(text) => text,
                rusqlite::types::Value::Integer(int) => int.to_string(),
                other => format!("{other:?}"),
            })
            .map_err(|e| format!("Failed to read PRAGMA {name}: {e}"))
    }

    #[cfg(test)]
    fn cursor_paths(&self) -> Vec<String> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM observer_cursors ORDER BY path")
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        rows.map(|r| r.expect("row")).collect()
    }

    #[cfg(test)]
    fn ledger_len(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM observer_ledger", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count") as usize
    }
}

/// Keep the newest [`MAX_CURSOR_ROWS`] cursors, dropping the oldest by `updated_at`.
///
/// Runs inside the caller's transaction, and after the inserts, so a cursor staged in this same
/// commit is among the newest and cannot be the one evicted.
fn evict_cursors(tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.execute(
        "DELETE FROM observer_cursors WHERE path IN (
             SELECT path FROM observer_cursors
             ORDER BY updated_at DESC, path ASC
             LIMIT -1 OFFSET ?1
         )",
        params![MAX_CURSOR_ROWS as i64],
    )
    .map(|_| ())
    .map_err(|e| format!("Failed to evict old cursors: {e}"))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
