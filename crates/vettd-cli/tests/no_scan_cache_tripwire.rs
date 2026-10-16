//! Regression tripwire for the removed per-file scan cache (vettd-cli#293).
//!
//! `vettd` used to memoize per-file detector results in a SQLite database under
//! `~/.vettd/scan-cache/` (`scan-v1.sqlite3` / `scan-v2.sqlite3`), wired into
//! `scan.rs` and reachable through a `--no-cache` flag on `cli.rs`. The feature
//! was deleted deliberately: it added a second, silently-staleable source of
//! truth for scan results and a per-run write side effect on the user's home
//! directory, in exchange for a speedup the single-pass detectors did not need.
//!
//! Deleting the code is not the same as guaranteeing it stays deleted, and we
//! deliberately do *not* clean up leftover cache directories on users'
//! machines — nothing should ever touch them again. These tests are the
//! tripwire that pins both halves of that decision:
//!
//! 1. A real `vettd scan quick` run against an isolated `$HOME` must not create
//!    `~/.vettd/scan-cache/`.
//! 2. The same run must leave pre-existing legacy cache files byte-for-byte
//!    untouched, and must not leave a SQLite `-wal`/`-shm` sidecar beside them.
//!    Note this pins "does not mutate", not "does not open": the sentinels are
//!    not valid databases, and a read-only open need not change them at all.
//!    Point 3 is what rules out references to the legacy cache entirely, reads
//!    included, so it — not this — is the primary guard.
//! 3. No `.rs` file under `crates/vettd-cli/src/` may mention `scan_cache`,
//!    `scan-cache`, `scan-v1.sqlite`, `scan-v2.sqlite` or `--no-cache`, nor
//!    contain `no_cache` as a whole identifier (the clap field that used to
//!    back `--no-cache`). Case-insensitive, and deliberately without an
//!    allowlist: the goal is zero occurrences. This test file itself does
//!    contain those strings, which is why the walk is scoped to `src/` (and
//!    docs are out of its scope).
//!
//! The contract cache (`contract_sync.rs`, `~/.vettd/contract/`) is a separate
//! feature, unaffected by the removal, and nothing here asserts about it.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The real binary under test, built by cargo.
const BIN: &str = env!("CARGO_BIN_EXE_vettd");

/// Generous but bounded upper limit for one `scan quick` run. Against a
/// throwaway `$HOME` there is nothing to walk, so anything close to this means
/// the process hung; the bound turns a hang into a loud failure instead of a
/// CI job that never finishes.
const SCAN_TIMEOUT: Duration = Duration::from_secs(120);

/// Poll interval for the bounded wait in [`wait_with_timeout`].
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Environment variables that would relocate the scan away from the throwaway
/// `$HOME`: four harnesses document their own home override, and the config-dir
/// lookups honour XDG. Clearing them keeps the run pinned to the temp home
/// rather than the developer's or CI machine's real agent directories.
const RELOCATING_ENV_VARS: [&str; 6] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "HERMES_HOME",
    "OPENCLAW_STATE_DIR",
    "XDG_CONFIG_HOME",
    "XDG_CONFIG_DIRS",
];

/// The legacy per-file cache databases the removed feature wrote, with sentinel
/// contents — any write, truncation, or SQLite open is detected by comparing
/// snapshots around the scan.
const LEGACY_CACHE_FILES: [(&str, &str); 2] = [
    ("scan-v1.sqlite3", "legacy scan-v1 cache sentinel"),
    ("scan-v2.sqlite3", "legacy scan-v2 cache sentinel"),
];

/// Case-insensitive needles that may not appear anywhere under
/// `crates/vettd-cli/src/`. Each is specific enough that a hit really does mean
/// the removed per-file scan cache is coming back: the module name, the cache
/// directory name, the legacy database filenames, and the `--no-cache` flag
/// written with its leading dashes — so an unrelated `Cache-Control: no-cache`
/// HTTP header cannot trip the guard. Deliberately no allowlist.
const FORBIDDEN_SOURCE_REFERENCES: [&str; 5] = [
    "scan_cache",
    "scan-cache",
    "scan-v1.sqlite",
    "scan-v2.sqlite",
    "--no-cache",
];

/// Matched as a whole identifier rather than a substring, so a future
/// `no_cache_headers()` helper is not mistaken for the clap field that backed
/// `--no-cache`. Case-insensitive, like the needles above.
const FORBIDDEN_SOURCE_TOKENS: [&str; 1] = ["no_cache"];

// ---------------------------------------------------------------------------
// Test 1 — the scan never creates the cache directory
// ---------------------------------------------------------------------------

#[test]
fn scan_quick_does_not_create_the_scan_cache_directory() {
    let home = new_temp_home();

    let out = run_scan_quick(home.path());

    assert!(
        out.status.success(),
        "`vettd scan quick --stdout` exited with {:?}\nstderr:\n{}",
        out.status,
        out.stderr
    );
    assert!(
        !out.stdout.trim().is_empty(),
        "`vettd scan quick --stdout` produced no report on stdout, so this \
         tripwire proved nothing"
    );

    let cache_dir = scan_cache_dir(home.path());
    assert!(
        !cache_dir.exists(),
        "`vettd scan quick` created the removed per-file cache directory {}\n\
         (nothing may recreate the legacy scan cache — see vettd-cli#293)",
        cache_dir.display()
    );
}

// ---------------------------------------------------------------------------
// Test 2 — the scan never touches pre-existing legacy cache files
// ---------------------------------------------------------------------------

#[test]
fn scan_quick_leaves_preexisting_legacy_cache_files_untouched() {
    let home = new_temp_home();
    let cache_dir = scan_cache_dir(home.path());
    let expected = seed_legacy_cache(&cache_dir);

    let out = run_scan_quick(home.path());
    assert!(
        out.status.success(),
        "`vettd scan quick --stdout` exited with {:?}\nstderr:\n{}",
        out.status,
        out.stderr
    );

    assert_eq!(
        snapshot_dir(&cache_dir),
        expected,
        "`vettd scan quick` changed the contents of {} — the removed per-file \
         cache must never be opened or rewritten (vettd-cli#293)",
        cache_dir.display()
    );
    assert_no_sqlite_sidecars(&cache_dir);
}

// ---------------------------------------------------------------------------
// Test 3 — static source tripwire
// ---------------------------------------------------------------------------

#[test]
fn source_tree_has_no_reference_to_the_removed_scan_cache() {
    // Derived from the crate manifest, not the current directory, so the test
    // behaves the same however cargo is invoked.
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    collect_rust_sources(&src_dir, &mut sources);
    sources.sort();

    assert!(
        !sources.is_empty(),
        "found no .rs files under {} — this tripwire would pass vacuously",
        src_dir.display()
    );

    let mut hits = Vec::new();
    for path in &sources {
        let contents = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (index, line) in contents.lines().enumerate() {
            let lowered = line.to_ascii_lowercase();
            let referenced = FORBIDDEN_SOURCE_REFERENCES
                .iter()
                .any(|needle| lowered.contains(needle))
                || FORBIDDEN_SOURCE_TOKENS
                    .iter()
                    .any(|token| contains_identifier(&lowered, token));
            if referenced {
                hits.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
            }
        }
    }

    assert!(
        hits.is_empty(),
        "the removed per-file scan cache is still referenced in {} source \
         file(s) (searched {} .rs files):\n{}",
        hits.len(),
        sources.len(),
        hits.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Fixtures and helpers
// ---------------------------------------------------------------------------

/// Whether `haystack` contains `needle` as a whole identifier — that is, with a
/// non-identifier character on both sides — rather than as part of a longer one.
fn contains_identifier(haystack: &str, needle: &str) -> bool {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        if before.is_none_or(|c| !is_word(c)) && after.is_none_or(|c| !is_word(c)) {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Directory holding throwaway `$HOME`s for this test binary, kept inside
/// `crates/vettd-cli/tests/` beside the other test fixtures rather than in the
/// OS temp dir. Each `TempDir` removes its own subtree when it drops, on
/// success and on failure alike.
fn tmp_homes_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("tmp-homes");
    std::fs::create_dir_all(&dir).expect("create tests/tmp-homes");
    dir
}

/// A throwaway `$HOME`, so these tests never touch the developer's real vettd
/// state.
fn new_temp_home() -> tempfile::TempDir {
    tempfile::tempdir_in(tmp_homes_dir()).expect("create temp home")
}

/// Where the removed feature kept its databases: `$HOME/.vettd/scan-cache/`.
fn scan_cache_dir(home: &Path) -> PathBuf {
    home.join(".vettd").join("scan-cache")
}

struct CliOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Run the real `vettd scan quick --stdout` with `$HOME` pinned to `home`.
///
/// The invocation is hermetic: both `$HOME` and `USERPROFILE` point at the
/// scratch home, every relocating environment variable is removed, and stdin is
/// closed, so the scan finds nothing to walk, cannot prompt, and needs no
/// network (contract sync belongs to `submit` / `contract status`, not to a
/// local scan).
///
/// Output is drained on reader threads so a full pipe buffer can never deadlock
/// the wait, and the wait itself is bounded — a hung scan panics here rather
/// than blocking CI forever.
fn run_scan_quick(home: &Path) -> CliOutput {
    let mut cmd = Command::new(BIN);
    cmd.args(["scan", "quick", "--stdout"]);
    cmd.env("HOME", home);
    cmd.env("USERPROFILE", home);
    for key in RELOCATING_ENV_VARS {
        cmd.env_remove(key);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn the vettd binary");
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let stdout_reader = std::thread::spawn(move || read_all(stdout));
    let stderr_reader = std::thread::spawn(move || read_all(stderr));

    let status = wait_with_timeout(&mut child).unwrap_or_else(|| {
        panic!(
            "`vettd scan quick --stdout` did not exit within {SCAN_TIMEOUT:?}; killed it. \
             A scan of a throwaway $HOME has nothing to walk, so this is a hang."
        )
    });

    CliOutput {
        status,
        stdout: String::from_utf8_lossy(&stdout_reader.join().unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default()).into_owned(),
    }
}

/// Wait for `child`, killing and reaping it (returning `None`) if it outlives
/// [`SCAN_TIMEOUT`].
fn wait_with_timeout(child: &mut Child) -> Option<ExitStatus> {
    let deadline = Instant::now() + SCAN_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("wait on the scan child") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(WAIT_POLL_INTERVAL);
    }
}

fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    buffer
}

/// Top-level `name → bytes` listing of `dir`, keyed by name so two snapshots
/// compare deterministically regardless of `read_dir` order.
fn snapshot_dir(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut snapshot = BTreeMap::new();
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("entry of {}: {e}", dir.display()));
        if !entry.file_type().expect("entry file type").is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let bytes = std::fs::read(entry.path()).unwrap_or_else(|e| panic!("read {name}: {e}"));
        snapshot.insert(name, bytes);
    }
    snapshot
}

/// Seed `dir` with the legacy cache sentinels, returning the snapshot the
/// directory must still match after a scan runs against the same `$HOME`.
///
/// The returned map is asserted against the freshly seeded directory here, so a
/// broken snapshot helper cannot make the caller's comparison pass vacuously.
fn seed_legacy_cache(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::create_dir_all(dir).expect("create the legacy scan-cache directory");
    let mut expected = BTreeMap::new();
    for (name, contents) in LEGACY_CACHE_FILES {
        std::fs::write(dir.join(name), contents).expect("write legacy cache sentinel");
        expected.insert(name.to_string(), contents.as_bytes().to_vec());
    }
    let seeded = snapshot_dir(dir);
    assert_eq!(
        seeded, expected,
        "seed sanity check failed: the seeded legacy cache files should be \
         visible, otherwise the comparisons below would pass vacuously"
    );
    expected
}

/// Fail if a SQLite sidecar appeared beside the seeded legacy databases. A
/// reintroduced cache would reopen these files for writing, and SQLite leaves a
/// `-wal`/`-shm` beside a database it writes to — a side effect the caller's
/// byte snapshot would otherwise miss if the write never flushed.
fn assert_no_sqlite_sidecars(dir: &Path) {
    for (name, _) in LEGACY_CACHE_FILES {
        for suffix in ["-wal", "-shm"] {
            let sidecar = dir.join(format!("{name}{suffix}"));
            assert!(
                !sidecar.exists(),
                "`vettd scan quick` wrote to the legacy cache file {name} ({} appeared)",
                sidecar.display()
            );
        }
    }
}

/// Recursively collect every `.rs` file under `dir` into `out`.
fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("entry of {}: {e}", dir.display()));
        let path = entry.path();
        if entry.file_type().expect("entry file type").is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}
