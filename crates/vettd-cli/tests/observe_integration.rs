//! Integration tests for `vettd observe`, spawning the real binary as a subprocess.
//!
//! These exist because the properties that matter most about this command are properties of the
//! *process*, not of any function in it: which stream a byte lands on, whether a file exists after
//! a refusal, what the exit code is, and whether any of it touched the user's home. None of that is
//! observable from a unit test that calls `run_observe` in-process.
//!
//! Every test runs against a throwaway `VETTD_HOME` seeded by [`seed_home`] and a *copy* of the fixture
//! harness home, so a test can never read (or mutate) anything under `tests/fixtures/`. The clock,
//! the day and the HMAC secret are pinned via the hidden test hooks so payload bytes are
//! reproducible.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use httpmock::Method::POST;
use httpmock::MockServer;
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_vettd");

/// The fixture harness home, and the one whose skill name collides with a free-string leaf.
const FIXTURE_HOME: &str = "claude_home";
const FIXTURE_HOME_GATE_VIOLATION: &str = "claude_home_gate_violation";

/// The pinned clock (2027-01-15T08:00:00Z) and day used to build the committed goldens. Far from
/// any checkout mtime, so nothing is reported as `truncated`.
const NOW_MS: &str = "1800000000000";
const TODAY: &str = "2027-01-15";

/// A device id pinned through the env var `resolve_scanner_uuid` reads, so the payload does not
/// depend on a uuid minted in the temp home.
const DEVICE_ID: &str = "00000000-0000-4000-8000-000000000000";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("observe")
}

/// Directory holding throwaway `$HOME`s for this test binary, kept under
/// `crates/vettd-cli/tests/` for the same reason `search_integration.rs` does: a stray leftover is
/// visible in `git status` rather than hidden in the OS temp dir.
fn tmp_homes_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("tmp-homes");
    std::fs::create_dir_all(&dir).expect("create tests/tmp-homes");
    dir
}

/// A throwaway `$HOME` with the observer secret written and telemetry set to `enabled`.
///
/// `telemetry` is a tri-state on purpose: `Some(true)` opts in, `Some(false)` writes an explicit
/// opt-out, and `None` writes no `[telemetry]` table at all — the three states the not-configured
/// path has to treat identically-but-for-`true`.
fn seed_home(telemetry: Option<bool>) -> tempfile::TempDir {
    let home = tempfile::tempdir_in(tmp_homes_dir()).expect("create temp home");
    let vettd = home.path().join(".vettd");
    std::fs::create_dir_all(&vettd).expect("create ~/.vettd");

    // The same 33-byte secret the goldens were generated with, so a payload written here is
    // comparable to the committed golden envelope.
    std::fs::copy(
        fixtures_dir().join("golden/secret.bin"),
        vettd.join("observer_secret"),
    )
    .expect("seed observer secret");

    let mut config = String::from("[access]\nmode = \"licensed\"\n");
    if let Some(enabled) = telemetry {
        config.push_str(&format!("\n[telemetry]\nenabled = {enabled}\n"));
    }
    std::fs::write(vettd.join(".vettd.toml"), config).expect("seed ~/.vettd/.vettd.toml");
    home
}

/// Copy a fixture harness home into `home`, returning the copy's path.
///
/// A copy rather than a `--root` into `tests/fixtures/` because the reader stats and (on a resume
/// path) could in principle open files there; a test that mutated a committed fixture would
/// silently corrupt every later test run.
fn copy_harness_home(home: &Path, fixture: &str) -> PathBuf {
    let dest = home.join(".claude");
    copy_tree(&fixtures_dir().join(fixture), &dest);
    dest
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create destination directory");
    for entry in std::fs::read_dir(from).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

struct CliOutput {
    status: i32,
    stdout: Vec<u8>,
    stderr: String,
}

impl CliOutput {
    fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }
}

/// Run the real binary with vettd's explicit home pointed at `home`.
fn run_vettd(args: &[&str], home: &Path) -> CliOutput {
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    cmd.env("VETTD_HOME", home);
    cmd.env("HOME", home);
    cmd.env("USERPROFILE", home);
    cmd.env_remove("HOMEDRIVE");
    cmd.env_remove("HOMEPATH");
    // A uuid minted in the temp home would differ per run; pin it so payload bytes are stable.
    cmd.env("VETTD_SCANNER_UUID", DEVICE_ID);
    cmd.env_remove("XDG_CONFIG_HOME");
    cmd.env_remove("XDG_CONFIG_DIRS");
    // The command must not inherit a developer's real endpoint or credential.
    cmd.env_remove("VETTD_API_KEY");
    cmd.env_remove("VETTD_ENDPOINT");
    // Run from the temp home so a bare `--out` filename cannot land in the repository.
    cmd.current_dir(home);
    let output = cmd.output().expect("run vettd binary");
    CliOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

/// The flags every observation test shares: pinned clock, pinned day, the whole fixture window.
fn observe_args<'a>(root: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "observe",
        "--root",
        root,
        "--now-ms",
        NOW_MS,
        "--today",
        TODAY,
        "--window-days",
        "3650",
    ];
    args.extend_from_slice(extra);
    args
}

// ---------------------------------------------------------------------------
// Opt-in
// ---------------------------------------------------------------------------

/// Invariant: with no opt-in, the command reads nothing, writes nothing to stdout, exits 3, and
/// tells the user the exact file and lines to add.
///
/// Exit 3 is distinct from 1 on purpose: "you have not configured this" is not a failure, and a
/// script that treats it as one would be wrong. The disclosure still prints, because a user asking
/// why the command refused deserves to see what it would have collected.
#[test]
fn observe_exits_3_when_telemetry_disabled() {
    for telemetry in [None, Some(false)] {
        let home = seed_home(telemetry);
        let root = copy_harness_home(home.path(), FIXTURE_HOME);
        let root = root.to_str().unwrap();
        let out = run_vettd(&observe_args(root, &["--dry-run"]), home.path());

        assert_eq!(out.status, 3, "telemetry {telemetry:?}: {}", out.stderr);
        assert!(
            out.stdout.is_empty(),
            "telemetry {telemetry:?}: stdout must be empty, got {:?}",
            out.stdout_text()
        );
        assert!(
            out.stderr.contains("This observation will include:"),
            "the disclosure must print even when refusing: {}",
            out.stderr
        );
        assert!(
            out.stderr
                .contains("Observation is not enabled. Nothing was read."),
            "must say nothing was read: {}",
            out.stderr
        );
        assert!(
            out.stderr.contains("[telemetry]") && out.stderr.contains("enabled = true"),
            "must show the TOML to add: {}",
            out.stderr
        );
        assert!(
            out.stderr.contains(".vettd.toml"),
            "must name the file to add it to: {}",
            out.stderr
        );
        // And, decisively: nothing was written anywhere.
        assert!(
            !home.path().join("vettd-observations.json").exists(),
            "--dry-run must not write a payload when telemetry is off"
        );
        assert!(
            !home.path().join(".vettd/observer").exists(),
            "the store must not be created when telemetry is off"
        );
    }
}

/// Invariant: `vettd observe enable` records the opt-in, and a subsequent run gets past the gate.
///
/// Also: it must refuse to rewrite an existing `[telemetry]` table. Silently flipping a value a
/// user deliberately set to `false` would be the worst possible behaviour for a consent flag, so
/// the second call prints instructions instead of editing.
#[test]
fn observe_enable_appends_telemetry_table() {
    let home = seed_home(None);
    let config = home.path().join(".vettd/.vettd.toml");
    let before = std::fs::read_to_string(&config).unwrap();
    assert!(!before.contains("[telemetry]"), "precondition");

    let out = run_vettd(&["observe", "enable"], home.path());
    assert_eq!(out.status, 0, "{}", out.stderr);
    let after = std::fs::read_to_string(&config).unwrap();
    assert!(
        after.starts_with(before.trim_end()),
        "the existing config must be preserved verbatim:\n{after}"
    );
    assert!(
        after.contains("[telemetry]") && after.contains("enabled = true"),
        "the opt-in must be recorded:\n{after}"
    );
    assert!(
        after.contains("mode = \"licensed\""),
        "the pre-existing [access] table must survive:\n{after}"
    );

    // Now the same command that exited 3 above proceeds.
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let proceeds = run_vettd(
        &observe_args(root.to_str().unwrap(), &["--dry-run"]),
        home.path(),
    );
    assert_eq!(proceeds.status, 0, "{}", proceeds.stderr);

    // A second enable must not touch the file.
    let idempotent = run_vettd(&["observe", "enable"], home.path());
    assert_eq!(idempotent.status, 0, "{}", idempotent.stderr);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        after,
        "a second enable must not rewrite a config that already has the table"
    );
    assert!(
        idempotent
            .stdout_text()
            .contains("already has a [telemetry] table"),
        "it must say why it did nothing: {}",
        idempotent.stdout_text()
    );
}

// ---------------------------------------------------------------------------
// Disclosure
// ---------------------------------------------------------------------------

/// Invariant: the disclosure names every category and the source directory, and it is on stderr
/// before anything is read.
///
/// "Before" is established by the refusal path above (exit 3 prints it having opened nothing); this
/// test pins the *content*, including that it names the root the command will actually read rather
/// than a hardcoded `~/.claude`.
#[test]
fn observe_prints_disclosure_to_stderr_before_reading() {
    let home = seed_home(Some(true));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let root_str = root.to_str().unwrap();
    let out = run_vettd(&observe_args(root_str, &["--dry-run"]), home.path());

    assert_eq!(out.status, 0, "{}", out.stderr);
    let disclosure_end = out
        .stderr
        .find("Source: Claude Code session logs under")
        .expect("the disclosure must name its source");
    let header = out
        .stderr
        .find("This observation will include:")
        .expect("the disclosure must print");
    assert!(
        header < disclosure_end,
        "the source line closes the disclosure"
    );
    assert!(
        out.stderr[disclosure_end..].contains(root_str),
        "the disclosure must name the root actually being read: {}",
        out.stderr
    );
    assert!(
        out.stderr[disclosure_end..].contains("never leave this machine"),
        "it must state what is withheld: {}",
        out.stderr
    );
    // The gate summary is emitted after the payload is built, so its position proves the
    // disclosure preceded the read.
    let gate = out
        .stderr
        .find("gate: OK")
        .expect("the gate summary must print");
    assert!(
        disclosure_end < gate,
        "the disclosure must precede the gate summary:\n{}",
        out.stderr
    );
    // No destination line without --submit: claiming one would be a lie about egress.
    assert!(
        !out.stderr.contains("Destination:"),
        "a run that sends nothing must not name a destination: {}",
        out.stderr
    );
}

/// Invariant: not one byte of the disclosure reaches stdout.
///
/// This is the AGENTS.md stream contract, and it is load-bearing: `vettd --json observe` pipes
/// stdout into a parser, and a stray human line there is a parse error for the caller.
#[test]
fn observe_disclosure_rendering_does_not_write_to_stdout() {
    let home = seed_home(Some(true));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let out = run_vettd(
        &observe_args(root.to_str().unwrap(), &["--dry-run"]),
        home.path(),
    );
    assert_eq!(out.status, 0, "{}", out.stderr);

    let stdout = out.stdout_text();
    for human in [
        "This observation will include:",
        "Source: Claude Code session logs",
        "never leave this machine",
        "gate: OK",
        "wrote ",
    ] {
        assert!(
            !stdout.contains(human),
            "{human:?} belongs on stderr, found on stdout:\n{stdout}"
        );
    }
    // ...and stdout is not simply empty: the report is there.
    assert!(!stdout.trim().is_empty(), "the report must be on stdout");
}

// ---------------------------------------------------------------------------
// Dry run
// ---------------------------------------------------------------------------

/// Invariant: a dry run writes the canonical payload, prints the human report, and creates no
/// store.
///
/// Cursors and the ledger are submission state. A dry run that advanced a cursor would mark the
/// session as already-read and silently starve the next real submit of it, which is the worst kind
/// of bug here: the data loss is invisible.
#[test]
fn observe_dry_run_writes_canonical_file_and_touches_no_store() {
    let home = seed_home(Some(true));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let payload = home.path().join("payload.json");
    let out = run_vettd(
        &observe_args(
            root.to_str().unwrap(),
            &["--dry-run", "--out", payload.to_str().unwrap(), "--scrub"],
        ),
        home.path(),
    );

    assert_eq!(out.status, 0, "{}", out.stderr);
    let bytes = std::fs::read(&payload).expect("the payload must be written");
    assert!(
        out.stderr.contains(&format!(
            "wrote {} ({} bytes)",
            payload.display(),
            bytes.len()
        )),
        "the write must be reported with its size: {}",
        out.stderr
    );

    // Canonical bytes: sorted keys, no spaces after separators, ASCII-only, one trailing newline.
    let text = std::str::from_utf8(&bytes).expect("canonical bytes are ASCII");
    assert!(text.is_ascii(), "canonical JSON must be pure ASCII");
    assert!(text.ends_with("}\n"), "exactly one trailing newline");
    assert!(!text.contains(", \""), "no space after a comma separator");
    assert!(!text.contains("\": "), "no space after a key separator");
    let parsed: serde_json::Value = serde_json::from_str(text).expect("the payload must parse");
    assert_eq!(parsed["envelope_version"], "0.1.0");
    assert!(
        parsed["records"].as_array().is_some_and(|r| !r.is_empty()),
        "the fixture home has a session, so records must not be empty"
    );
    // Re-serializing the parsed value must reproduce the file: that is what canonical means.
    let reserialized = serde_json::to_vec(&parsed).expect("re-serialize");
    assert_eq!(
        String::from_utf8_lossy(&reserialized),
        text.trim_end_matches('\n'),
        "the payload must already be in serde's canonical key order"
    );

    // No store, no ledger, no cursors.
    assert!(
        !home.path().join(".vettd/observer").exists(),
        "a dry run must not create the observer store directory"
    );
}

/// Invariant: repeated dry runs are identical, because a dry run stores no cursor.
///
/// The cursor mechanism only exists for submission. If `--dry-run` consumed sessions, a user could
/// not run it twice to compare, and the second run would silently report an empty payload.
#[test]
fn observe_second_dry_run_still_emits_the_session() {
    let home = seed_home(Some(true));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let root_str = root.to_str().unwrap();
    let first_path = home.path().join("first.json");
    let second_path = home.path().join("second.json");

    let first = run_vettd(
        &observe_args(
            root_str,
            &["--dry-run", "--out", first_path.to_str().unwrap()],
        ),
        home.path(),
    );
    let second = run_vettd(
        &observe_args(
            root_str,
            &["--dry-run", "--out", second_path.to_str().unwrap()],
        ),
        home.path(),
    );
    assert_eq!(first.status, 0, "{}", first.stderr);
    assert_eq!(second.status, 0, "{}", second.stderr);

    let a = std::fs::read(&first_path).unwrap();
    let b = std::fs::read(&second_path).unwrap();
    assert_eq!(
        a, b,
        "two dry runs on unchanged input must be byte-identical"
    );

    let parsed: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        parsed["coverage"]["sessions_emitted"],
        1,
        "the second run must still emit the session: {}",
        String::from_utf8_lossy(&b)
    );
    assert_eq!(
        parsed["coverage"]["cursor_state"], "fresh",
        "no cursors exist outside submit mode"
    );
}

/// Invariant: `--json` puts the canonical envelope on stdout and nothing else, byte for byte the
/// same as the file `--out` writes.
///
/// Two renderings of the same payload that could disagree would make `observe check` on the file
/// meaningless as an audit of what was sent.
#[test]
fn observe_json_prints_envelope_to_stdout_only() {
    let home = seed_home(Some(true));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let payload = home.path().join("payload.json");
    let out = run_vettd(
        &{
            let mut args = vec!["--json"];
            args.extend(observe_args(
                root.to_str().unwrap(),
                &["--dry-run", "--out", payload.to_str().unwrap()],
            ));
            args
        },
        home.path(),
    );

    assert_eq!(out.status, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        std::fs::read(&payload).unwrap(),
        "stdout must be the same bytes as the written payload"
    );
    // Nothing but JSON: the whole stream parses.
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .expect("all of stdout must be one JSON document");
    // The human report went nowhere, rather than to stdout.
    assert!(
        !out.stdout_text().contains("Recommended"),
        "the ranking table must not share stdout with --json"
    );
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// Invariant: a payload that fails the gate is never written, never printed, and the diagnostic
/// names the rule without echoing the value it caught.
///
/// The fixture's skill is named `3.4`, which is a substring of that run's own `harness_version`
/// (`3.4.5`) — the documented fail-closed substring rule. Echoing `3.4` in the error would leak
/// the very local-only name the gate refused to emit, so the assertion that the value appears
/// nowhere on stderr is the point of this test, not a detail of it.
#[test]
fn observe_gate_refusal_exits_2_and_writes_nothing() {
    let home = seed_home(Some(true));
    let root = copy_harness_home(home.path(), FIXTURE_HOME_GATE_VIOLATION);
    let payload = home.path().join("must-not-exist.json");
    let out = run_vettd(
        &observe_args(
            root.to_str().unwrap(),
            &["--dry-run", "--out", payload.to_str().unwrap()],
        ),
        home.path(),
    );

    assert_eq!(out.status, 2, "gate refusal is exit 2: {}", out.stderr);
    assert!(
        !payload.exists(),
        "a refused payload must not be written to {}",
        payload.display()
    );
    assert!(
        out.stdout.is_empty(),
        "a refused payload must not reach stdout: {:?}",
        out.stdout_text()
    );
    assert!(
        out.stderr.contains("REFUSING TO WRITE"),
        "the refusal must be loud: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("dynamic:loaded_set_names"),
        "the refusal must name the rule that fired: {}",
        out.stderr
    );
    assert_eq!(
        out.stderr.matches("3.4").count(),
        0,
        "the caught value must never be echoed:\n{}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("gate: OK"),
        "a refusal must not also claim the gate passed: {}",
        out.stderr
    );
}

/// Invariant: `observe check` distinguishes clean (0), violating (1) and unreadable (2) — and
/// treats a duplicate JSON key as unreadable rather than checking it.
///
/// `serde_json` keeps the LAST value for a duplicated key, so a payload could carry a leak in the
/// first copy and a clean value in the second and pass every rule. That is exactly what this
/// command exists to catch, so it must refuse the input rather than bless it.
#[test]
fn observe_check_exit_codes() {
    let home = seed_home(Some(true));
    let golden = fixtures_dir().join("golden/envelope.json");
    let dynamic = fixtures_dir().join("golden/dynamic.json");

    // 0 — the committed golden, with the emitter's own dynamic sets.
    let clean = run_vettd(
        &[
            "observe",
            "check",
            golden.to_str().unwrap(),
            "--dynamic",
            dynamic.to_str().unwrap(),
        ],
        home.path(),
    );
    assert_eq!(clean.status, 0, "{}{}", clean.stdout_text(), clean.stderr);
    assert!(
        clean.stdout_text().contains("gate: OK"),
        "a clean result is machine-visible on stdout: {}",
        clean.stdout_text()
    );

    // 0 — and without the dynamic sets, since a receiver auditing someone else's payload has no
    // access to the emitter's local vocabulary. Every structural and value rule still applies.
    let no_dynamic = run_vettd(&["observe", "check", golden.to_str().unwrap()], home.path());
    assert_eq!(no_dynamic.status, 0, "{}", no_dynamic.stderr);

    // 1 — an unknown key. Nothing may be emitted that the gate does not name.
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&golden).unwrap()).unwrap();
    doc["records"][0]["surprise_field"] = serde_json::json!("anything at all");
    let violating = home.path().join("violating.json");
    std::fs::write(&violating, serde_json::to_vec(&doc).unwrap()).unwrap();
    let bad = run_vettd(
        &["observe", "check", violating.to_str().unwrap()],
        home.path(),
    );
    assert_eq!(bad.status, 1, "violations are exit 1: {}", bad.stderr);
    assert!(
        bad.stderr.contains("unknown_key") && bad.stderr.contains("records[0]"),
        "the violation must name the rule and where it fired: {}",
        bad.stderr
    );
    // But NOT the key itself. An unknown key could be the content the gate exists to withhold —
    // a field someone added carrying a path or a username — so the diagnostic reports its length
    // and nothing more. This mirrors `check_field_gate.py::_walk_dict`.
    assert!(
        !bad.stderr.contains("surprise_field"),
        "an unknown key must never be echoed back: {}",
        bad.stderr
    );
    assert!(
        !bad.stdout_text().contains("gate: OK"),
        "a violating payload must not print a clean summary"
    );

    // 2 — a file that is not there.
    let missing = run_vettd(
        &[
            "observe",
            "check",
            home.path().join("nope.json").to_str().unwrap(),
        ],
        home.path(),
    );
    assert_eq!(
        missing.status, 2,
        "unreadable is exit 2: {}",
        missing.stderr
    );
    assert!(missing.stderr.contains("Cannot read"), "{}", missing.stderr);

    // 2 — a file that is not JSON.
    let garbage = home.path().join("garbage.json");
    std::fs::write(&garbage, b"{not json at all").unwrap();
    let unparseable = run_vettd(
        &["observe", "check", garbage.to_str().unwrap()],
        home.path(),
    );
    assert_eq!(unparseable.status, 2, "{}", unparseable.stderr);
    assert!(
        unparseable.stderr.contains("Cannot parse"),
        "{}",
        unparseable.stderr
    );

    // 2 — a duplicate key, which parses fine but is not checkable.
    let text = std::fs::read_to_string(&golden).unwrap();
    let duplicated = text.replacen(
        "\"envelope_version\":",
        "\"envelope_version\":\"leaked\",\"envelope_version\":",
        1,
    );
    let dup_path = home.path().join("duplicate-key.json");
    std::fs::write(&dup_path, &duplicated).unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(&duplicated).is_ok(),
        "precondition: the duplicate parses, which is exactly the problem"
    );
    let dup = run_vettd(
        &["observe", "check", dup_path.to_str().unwrap()],
        home.path(),
    );
    assert_eq!(
        dup.status, 2,
        "a duplicate key is unreadable: {}",
        dup.stderr
    );
    assert!(
        dup.stderr.contains("duplicate key"),
        "the reason must be stated: {}",
        dup.stderr
    );

    // 2 — an unreadable dynamic-set sidecar, rather than silently checking without it. Falling
    // back to no sets would weaken the substring rule with no signal to the caller.
    let bad_dynamic = run_vettd(
        &[
            "observe",
            "check",
            golden.to_str().unwrap(),
            "--dynamic",
            home.path().join("no-such-sets.json").to_str().unwrap(),
        ],
        home.path(),
    );
    assert_eq!(bad_dynamic.status, 2, "{}", bad_dynamic.stderr);
}

// ---------------------------------------------------------------------------
// status / help
// ---------------------------------------------------------------------------

/// Invariant: `observe status --json` reports the opt-in state and every path it would use, on
/// stdout, as parseable JSON — and reports "not enabled" without creating any of them.
///
/// `status` is what someone runs to answer "is this thing reading my logs?". If running it created
/// the store or the secret, the answer would change by asking the question.
#[test]
fn observe_status_json() {
    let home = seed_home(Some(true));
    let out = run_vettd(&["observe", "status", "--json"], home.path());
    assert_eq!(out.status, 0, "{}", out.stderr);

    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("status --json must be JSON on stdout");
    assert_eq!(parsed["enabled"], true);
    assert_eq!(parsed["secret_present"], true, "seed_home wrote the secret");
    assert_eq!(parsed["store_present"], false, "nothing has submitted yet");
    assert_eq!(parsed["cursor_state"], "fresh");
    assert_eq!(parsed["envelope_version"], "0.1.0");
    assert!(
        parsed["config_path"].as_str().unwrap().contains(".vettd"),
        "the config path must be reported so a user can find it: {parsed}"
    );
    assert!(
        parsed["store_path"]
            .as_str()
            .unwrap()
            .starts_with(home.path().to_str().unwrap()),
        "every reported path must be under the seeded home, not the developer's: {parsed}"
    );
    assert!(
        !home.path().join(".vettd/observer").exists(),
        "status must not create the store it reports on"
    );

    // The same state, off, in the human rendering, with the way to turn it on.
    let off_home = seed_home(Some(false));
    let off = run_vettd(&["observe", "status"], off_home.path());
    assert_eq!(off.status, 0, "{}", off.stderr);
    assert!(
        off.stdout_text().contains("observation: not enabled"),
        "{}",
        off.stdout_text()
    );
    assert!(
        off.stdout_text().contains("vettd observe enable"),
        "it must say how to opt in: {}",
        off.stdout_text()
    );
}

/// Invariant: the documented flags are in `--help`, and the three test hooks are not.
///
/// The hooks pin the clock, the day and the HMAC secret. A user who found `--secret-file` in the
/// help text could produce a payload whose `run_id` does not correspond to their device, which is
/// not a supported thing to do and would corrupt the pseudonym's meaning server-side.
#[test]
fn observe_help_lists_flags() {
    let home = seed_home(Some(true));
    let out = run_vettd(&["observe", "--help"], home.path());
    assert_eq!(out.status, 0, "{}", out.stderr);
    let help = out.stdout_text();

    for shown in [
        "--harness",
        "--root",
        "--task",
        "--window-days",
        "--model",
        "--dry-run",
        "--out",
        "--scrub",
        "--public-names",
        "--prices",
        "--submit",
        "--api-key",
        "--resend",
        "--allow-public-endpoint",
        "enable",
        "status",
        "check",
    ] {
        assert!(
            help.contains(shown),
            "--help must document {shown}:\n{help}"
        );
    }
    for hidden in ["--secret-file", "--now-ms", "--today"] {
        assert!(
            !help.contains(hidden),
            "--help must not mention {hidden}:\n{help}"
        );
    }
    assert!(
        out.stderr.is_empty(),
        "a requested --help is not an error: {}",
        out.stderr
    );
}

/// Invariant: a `--root` that is not a directory fails with exit 1 and names the path, rather than
/// reporting an empty observation.
///
/// "You have no sessions" and "I could not find your sessions" are different answers, and only one
/// of them is actionable.
#[test]
fn observe_reports_a_bad_root_rather_than_an_empty_observation() {
    let home = seed_home(Some(true));
    let absent = home.path().join("not-a-harness-home");
    let out = run_vettd(
        &observe_args(absent.to_str().unwrap(), &["--dry-run"]),
        home.path(),
    );
    assert_eq!(out.status, 1, "{}", out.stderr);
    assert!(
        out.stderr.contains(absent.to_str().unwrap()) && out.stderr.contains("--root"),
        "the error must name the path and the flag: {}",
        out.stderr
    );
    assert!(
        out.stdout.is_empty(),
        "nothing on stdout: {:?}",
        out.stdout_text()
    );
}

// ---------------------------------------------------------------------------
// Submission
//
// Against a real loopback mock server, so the assertions are about the bytes and headers that
// actually go on the wire. 127.0.0.1 is a local host, so `ensure_endpoint_allowed` permits plain
// HTTP without `--allow-public-endpoint` — which is itself covered below.
// ---------------------------------------------------------------------------

const MOCK_API_KEY: &str = "observe-mock-key-123";
const INGEST_PATH: &str = "/api/observations/ingest";

/// A throwaway `$HOME` that also carries saved credentials, as `vettd auth` would have written.
///
/// `endpoint` is written in the SCAN ingest form the CLI actually saves, so the tests exercise the
/// real `/api/scans/ingest` → `/api/observations/ingest` derivation rather than a pre-cooked URL.
fn seed_home_with_auth(scan_endpoint: &str) -> tempfile::TempDir {
    let home = seed_home(Some(true));
    let config_dir = home.path().join(".config").join("vettd");
    std::fs::create_dir_all(&config_dir).expect("create ~/.config/vettd");
    std::fs::write(
        config_dir.join("config.json"),
        json!({"endpoint": scan_endpoint, "apiKey": MOCK_API_KEY}).to_string(),
    )
    .expect("seed auth config");
    home
}

fn scan_endpoint(server: &MockServer) -> String {
    format!("{}/api/scans/ingest", server.base_url())
}

/// What a dry run on this home produces: the exact canonical bytes, and the run ids in them.
///
/// A `run_id` is `HMAC(secret, "claude_code:<session_key>")`; pinning the literal would make every
/// test below fail for the right reason but the wrong one the day the fixture or the HMAC input
/// changes. The bytes come back too, so a test can require the wire body to equal them — which
/// pins the property a user actually relies on: what `--dry-run` shows you is what gets sent.
fn learn_dry_run(home: &Path, root: &Path) -> (Vec<u8>, Vec<String>) {
    let payload = home.join("learn.json");
    let out = run_vettd(
        &observe_args(
            root.to_str().unwrap(),
            &["--dry-run", "--out", payload.to_str().unwrap()],
        ),
        home,
    );
    assert_eq!(
        out.status, 0,
        "the learning dry run must succeed: {}",
        out.stderr
    );
    let bytes = std::fs::read(&payload).expect("read the learned payload");
    let envelope: Value = serde_json::from_slice(&bytes).expect("parse the learned payload");
    let ids: Vec<String> = envelope["records"]
        .as_array()
        .expect("records")
        .iter()
        .map(|r| r["run_id"].as_str().expect("run_id").to_string())
        .collect();
    assert!(
        !ids.is_empty(),
        "the fixture home must produce at least one run"
    );
    std::fs::remove_file(&payload).ok();
    (bytes, ids)
}

/// Just the run ids, for the tests that do not care about the bytes.
fn learn_run_ids(home: &Path, root: &Path) -> Vec<String> {
    learn_dry_run(home, root).1
}

fn results_body(run_ids: &[String], status: &str) -> Value {
    json!({
        "results": run_ids
            .iter()
            .map(|id| json!({"run_id": id, "status": status}))
            .collect::<Vec<_>>()
    })
}

/// Start the binary without waiting, for the one test that has to change the server mid-flight.
fn spawn_vettd(args: &[&str], home: &Path) -> Child {
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    cmd.env("VETTD_HOME", home);
    cmd.env("HOME", home);
    cmd.env("USERPROFILE", home);
    cmd.env_remove("HOMEDRIVE");
    cmd.env_remove("HOMEPATH");
    cmd.env("VETTD_SCANNER_UUID", DEVICE_ID);
    cmd.env_remove("XDG_CONFIG_HOME");
    cmd.env_remove("XDG_CONFIG_DIRS");
    cmd.current_dir(home);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.spawn().expect("spawn vettd")
}

/// Invariant: the envelope goes out with the credential and content type the ingest route
/// requires, as the exact bytes that were gate-checked, and the runs the server confirms are
/// recorded so the next run has nothing to send.
///
/// The body assertion is the whole payload re-parsed, not a substring: a submission that dropped
/// or reshaped a field between the gate and the socket would be a payload nobody authorised.
#[test]
fn observe_submit_posts_envelope_and_updates_ledger() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let (expected_bytes, run_ids) = learn_dry_run(home.path(), &root);

    // Everything that must be true of the request is expressed as a MATCHER, so a hit is itself
    // the assertion: the credential, the content type, and the body as exact bytes — the same
    // canonical payload `--dry-run` produced. A mismatch anywhere means no mock matches, the
    // server answers 404, and the CLI fails loudly rather than the test quietly checking less.
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path(INGEST_PATH)
            .header("Authorization", format!("Bearer {MOCK_API_KEY}"))
            .header("Content-Type", "application/json")
            .body(String::from_utf8(expected_bytes.clone()).expect("canonical bytes are ASCII"));
        then.status(200)
            .header("Content-Type", "application/json")
            .json_body(results_body(&run_ids, "accepted"));
    });

    let sent = home.path().join("sent.json");
    let out = run_vettd(
        &observe_args(
            root.to_str().unwrap(),
            &["--submit", "--out", sent.to_str().unwrap()],
        ),
        home.path(),
    );

    assert_eq!(out.status, 0, "{}", out.stderr);
    mock.assert_calls(1);

    // ...and the file left behind is that same payload, so `observe check` on it audits what was
    // actually sent rather than a second rendering of it.
    assert_eq!(
        std::fs::read(&sent).expect("--out payload"),
        expected_bytes,
        "the written payload must be the bytes that went on the wire"
    );
    let posted: Value = serde_json::from_slice(&expected_bytes).expect("the body must be JSON");
    assert_eq!(posted["envelope_version"], "0.1.0");
    assert_eq!(
        posted["records"].as_array().map(Vec::len),
        Some(run_ids.len())
    );

    assert!(
        out.stderr
            .contains("Observations accepted: 1 new, 0 replaced, 0 duplicate"),
        "the outcome must be reported: {}",
        out.stderr
    );
    assert!(
        out.stderr
            .contains(&format!("Destination: {}", server.address())),
        "the disclosure must name the host the payload reached: {}",
        out.stderr
    );
    // The ledger now holds the run, so the store exists.
    assert!(
        home.path()
            .join(".vettd/observer/observer-v1.sqlite3")
            .exists(),
        "a successful submit must have written cursors and the ledger"
    );
}

/// Invariant: a second submit with nothing changed makes NO request at all.
///
/// Not "sends an empty envelope" — no request. A collector that woke a server up on every
/// invocation to say nothing would be a bad citizen on someone else's infrastructure, and the
/// user's own bandwidth.
#[test]
fn observe_second_submit_sends_nothing_new() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let run_ids = learn_run_ids(home.path(), &root);

    let mock = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(200)
            .json_body(results_body(&run_ids, "accepted"));
    });

    let args = observe_args(root.to_str().unwrap(), &["--submit"]);
    let first = run_vettd(&args, home.path());
    assert_eq!(first.status, 0, "{}", first.stderr);
    mock.assert_calls(1);

    let second = run_vettd(&args, home.path());
    assert_eq!(second.status, 0, "{}", second.stderr);
    assert!(
        second.stderr.contains("nothing new to send"),
        "the second run must say so: {}",
        second.stderr
    );
    mock.assert_calls(1);
    assert!(
        second.stdout.is_empty(),
        "and print no report for an empty submission: {:?}",
        second.stdout_text()
    );
}

/// Invariant: a run that gained turns is sent again under the SAME `run_id`.
///
/// A record is the cumulative state of one harness run, and `run_id` is its idempotency key: the
/// server replaces the row rather than storing two. If the ledger were keyed on the run alone the
/// updated record would read as already-sent and the completed run would never reach the server —
/// which is why `ledger_has` takes the record hash.
#[test]
fn observe_changed_run_is_resent_under_the_same_run_id() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let run_ids = learn_run_ids(home.path(), &root);

    // Matches only a body carrying this run id, so a hit proves the run id, not just a POST.
    let run_id = run_ids[0].clone();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path(INGEST_PATH)
            .body_includes(format!("\"run_id\":\"{run_id}\""));
        then.status(200)
            .json_body(results_body(&run_ids, "accepted"));
    });

    let args = observe_args(root.to_str().unwrap(), &["--submit"]);
    let first = run_vettd(&args, home.path());
    assert_eq!(first.status, 0, "{}", first.stderr);
    mock.assert_calls(1);

    append_a_turn(&root);

    // The record must now differ — proved independently of the wire by a dry run, which reads the
    // same transcript through the same pipeline.
    let (changed_bytes, changed_ids) = learn_dry_run(home.path(), &root);
    assert_eq!(
        changed_ids, run_ids,
        "appending a turn must not change the run id: it is keyed on the session, not its content"
    );
    assert!(
        !changed_bytes.is_empty(),
        "the changed run must still produce a payload"
    );

    let second = run_vettd(&args, home.path());
    assert_eq!(second.status, 0, "{}", second.stderr);
    assert!(
        second
            .stderr
            .contains("Observations accepted: 1 new, 0 replaced, 0 duplicate"),
        "the changed run must be sent, not suppressed by the ledger: {}",
        second.stderr
    );
    // Two hits on a mock that only matches this run id: the same run went twice.
    mock.assert_calls(2);
}

/// Append one more assistant turn to the fixture transcript, so the run's record changes.
///
/// Reuses a line already in the file rather than authoring one, so the appended line is guaranteed
/// to be a shape the reader understands. The fixture's last line is deliberately unparseable (it
/// exercises `lines_unknown_type`), so this looks for the last line that actually parses.
fn append_a_turn(root: &Path) {
    let transcript = root
        .join("projects")
        .join("-fixture-project")
        .join("0f1e2d3c-4b5a-6978-8a9b-0c1d2e3f4a5b.ndjson");
    let text = std::fs::read_to_string(&transcript).expect("read the fixture transcript");
    let mut line = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value["type"] == "user")
        .next_back()
        .expect("the fixture has at least one user line");
    line["uuid"] = json!("00000000-0000-4000-8000-0000000000f1");
    line["timestamp"] = json!("2026-08-15T10:01:30.000Z");
    let appended = format!("{}\n", serde_json::to_string(&line).unwrap());
    std::fs::write(&transcript, format!("{text}{appended}")).expect("append a turn");
}

/// Invariant: `--resend` sends a record the machine has already sent, unchanged.
///
/// This is a deliberate deviation from the port plan's step ordering, and the plan's own
/// acceptance line is the evidence: it expects `--resend` on an unchanged machine to report
/// `0 new, 0 replaced, 1 duplicate`, which is impossible if the cursor probe short-circuits the
/// group before a record is ever built. So `--resend` bypasses the cursor probe as well as the
/// ledger. Under the literal reading the flag does nothing in exactly the case a user reaches for
/// it and reports `nothing new to send`, which reads as success.
#[test]
fn observe_resend_ignores_ledger() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let run_ids = learn_run_ids(home.path(), &root);

    let accepted = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(200)
            .json_body(results_body(&run_ids, "accepted"));
    });
    let args = observe_args(root.to_str().unwrap(), &["--submit"]);
    assert_eq!(run_vettd(&args, home.path()).status, 0);
    accepted.assert_calls(1);

    // Without --resend: nothing.
    assert!(run_vettd(&args, home.path())
        .stderr
        .contains("nothing new to send"));
    accepted.assert_calls(1);

    // With it: the same record goes again, and the server's `duplicate` verdict is reported as
    // such rather than being counted as new.
    let mut accepted = accepted;
    accepted.delete();
    let duplicate = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(200)
            .json_body(results_body(&run_ids, "duplicate"));
    });
    let resent = run_vettd(
        &observe_args(root.to_str().unwrap(), &["--submit", "--resend"]),
        home.path(),
    );
    assert_eq!(resent.status, 0, "{}", resent.stderr);
    duplicate.assert_calls(1);
    assert!(
        resent
            .stderr
            .contains("Observations accepted: 0 new, 0 replaced, 1 duplicate"),
        "a resend of an unchanged record is a duplicate, not new: {}",
        resent.stderr
    );
}

/// Invariant: a 400 fails with exit 1 and writes NO ledger row, so the record is sent again next
/// time.
///
/// A rejected payload is a bug on our side, and the one thing that must not happen is for the CLI
/// to record it as delivered — the run would then be suppressed forever by a ledger row for data
/// nobody has.
#[test]
fn observe_submit_400_exits_1_without_ledger_write() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let run_ids = learn_run_ids(home.path(), &root);

    let mut rejected = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(400)
            .json_body(json!({"error": "records[0].run_outcome: invalid"}));
    });

    let args = observe_args(root.to_str().unwrap(), &["--submit"]);
    let out = run_vettd(&args, home.path());
    assert_eq!(
        out.status, 1,
        "a rejected payload is a runtime error: {}",
        out.stderr
    );
    rejected.assert_calls(1);
    assert!(
        out.stderr.contains("Server rejected payload (400)"),
        "the failure must name the status: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("telemetry-envelope.schema.json"),
        "and point at the contract it violated: {}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("Observations accepted"),
        "a rejection must not report a success line: {}",
        out.stderr
    );

    // Nothing was ledgered: the very same record goes again once the server accepts it.
    rejected.delete();
    let accepted = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(200)
            .json_body(results_body(&run_ids, "accepted"));
    });
    let retried = run_vettd(&args, home.path());
    assert_eq!(retried.status, 0, "{}", retried.stderr);
    accepted.assert_calls(1);
    assert!(
        retried
            .stderr
            .contains("Observations accepted: 1 new, 0 replaced, 0 duplicate"),
        "the run the 400 lost must still be sendable: {}",
        retried.stderr
    );
}

/// Invariant: a 200 response is not whole-batch success when a run is marked deadline-exceeded.
/// Its cursor must remain at the prior position so the next invocation rebuilds and resends it.
#[test]
fn observe_deadline_exceeded_run_is_resent() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let run_ids = learn_run_ids(home.path(), &root);
    let args = observe_args(root.to_str().unwrap(), &["--submit"]);

    let mut timed_out = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(200)
            .json_body(results_body(&run_ids, "deadline_exceeded"));
    });
    let first = run_vettd(&args, home.path());
    assert_eq!(first.status, 0, "{}", first.stderr);
    timed_out.assert_calls(1);
    assert!(first.stderr.contains("will be resent"), "{}", first.stderr);

    timed_out.delete();
    let accepted = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(200)
            .json_body(results_body(&run_ids, "accepted"));
    });
    let retried = run_vettd(&args, home.path());
    assert_eq!(retried.status, 0, "{}", retried.stderr);
    accepted.assert_calls(1);
    assert!(
        retried
            .stderr
            .contains("Observations accepted: 1 new, 0 replaced, 0 duplicate"),
        "the unconfirmed run must be rebuilt and sent again: {}",
        retried.stderr
    );
}

/// Invariant: a 429 is retried, and `Retry-After` decides when rather than the built-in backoff.
///
/// The synchronisation is deliberate and not a sleep race: the throttling mock answers with
/// `Retry-After: 1`, and the test swaps in the accepting mock as soon as it sees the first hit —
/// a ~1 second window against a 10 ms poll. The elapsed-time assertion is what proves the header
/// won: the first entry in the shared backoff schedule is 5 seconds, so a run that finishes in
/// under 4 cannot have used it.
#[test]
fn observe_submit_429_honours_retry_after() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let run_ids = learn_run_ids(home.path(), &root);

    let mut throttled = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(429)
            .header("Retry-After", "1")
            .json_body(json!({"error": "Rate limit exceeded."}));
    });

    let started = Instant::now();
    let args = observe_args(root.to_str().unwrap(), &["--submit"]);
    let child = spawn_vettd(&args, home.path());

    let deadline = Instant::now() + Duration::from_secs(30);
    while throttled.calls() == 0 {
        assert!(
            Instant::now() < deadline,
            "the CLI never reached the throttled endpoint"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Delete before adding, so the two mocks are never both live for one request and the CLI
    // cannot be served a 429 by the mock we thought we had replaced.
    throttled.delete();
    let accepted = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(200)
            .json_body(results_body(&run_ids, "accepted"));
    });

    let output = child.wait_with_output().expect("wait for vettd");
    let elapsed = started.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert_eq!(output.status.code(), Some(0), "{stderr}");
    accepted.assert_calls(1);
    assert!(
        stderr.contains("Server returned 429, retrying in 1s"),
        "the retry must say what it is waiting for: {stderr}"
    );
    assert!(
        stderr.contains("Observations accepted: 1 new, 0 replaced, 0 duplicate"),
        "the retry must succeed: {stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "took {elapsed:?}; the shared backoff's first delay is 5s, so Retry-After was ignored"
    );
}

/// Invariant: plain HTTP to a public host is refused even with `--allow-public-endpoint`.
///
/// The flag exists to permit a *public* endpoint, not an unencrypted one. A bearer token on the
/// wire in cleartext is a credential disclosure, and no flag on this command opts into it.
#[test]
fn observe_submit_refuses_public_http_endpoint() {
    let home = seed_home_with_auth("https://vettd.invented.test/api/scans/ingest");
    let root = copy_harness_home(home.path(), FIXTURE_HOME);

    for extra in [
        vec![
            "--submit",
            "http://collector.invented.test/api/observations/ingest",
        ],
        vec![
            "--submit",
            "http://collector.invented.test/api/observations/ingest",
            "--allow-public-endpoint",
        ],
    ] {
        let out = run_vettd(&observe_args(root.to_str().unwrap(), &extra), home.path());
        assert_eq!(out.status, 1, "refusing to send is exit 1: {}", out.stderr);
        assert!(
            out.stderr.contains("HTTP with a public host"),
            "the refusal must say why: {}",
            out.stderr
        );
        assert!(
            out.stdout.is_empty(),
            "nothing may be produced for a refused endpoint: {:?}",
            out.stdout_text()
        );
        // Refused before anything was read, so no store and no claimed destination. The source
        // disclosure still precedes the config/endpoint validation on every path.
        assert!(
            !home.path().join(".vettd/observer").exists(),
            "a refused endpoint must not create the store"
        );
        assert!(
            !out.stderr.contains("Destination:"),
            "and must not claim a destination it will not use: {}",
            out.stderr
        );
        assert!(out.stderr.contains("This observation will include:"));
    }
}

/// Invariant: `--submit <URL>` posts to that URL exactly, with no path rewriting.
///
/// An operator naming a route means it. Deriving from it — as the saved scan endpoint is derived —
/// would make a local or self-hosted collector unreachable, and the failure would look like the
/// server being down rather than the client editing the address.
#[test]
fn observe_explicit_submit_url_is_used_verbatim() {
    let server = MockServer::start();
    // A deliberately non-standard path: nothing about it suggests `/api/observations/ingest`.
    let explicit = format!("{}/collector/v9/drop-here", server.base_url());
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let run_ids = learn_run_ids(home.path(), &root);

    let verbatim = server.mock(|when, then| {
        when.method(POST).path("/collector/v9/drop-here");
        then.status(200)
            .json_body(results_body(&run_ids, "accepted"));
    });
    let derived = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(500);
    });

    let out = run_vettd(
        &observe_args(root.to_str().unwrap(), &["--submit", &explicit]),
        home.path(),
    );
    assert_eq!(out.status, 0, "{}", out.stderr);
    verbatim.assert_calls(1);
    derived.assert_calls(0);
}

/// Invariant: a bare `--submit` derives the observations route from the SAVED scan endpoint.
///
/// The saved endpoint points at `/api/scans/ingest` because that is what `vettd auth` configures.
/// Posting observations there would be silently wrong — a route that exists, authenticates, and
/// stores the payload as something it is not.
#[test]
fn observe_derived_url_ends_with_observations_ingest() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let run_ids = learn_run_ids(home.path(), &root);

    let observations = server.mock(|when, then| {
        when.method(POST).path(INGEST_PATH);
        then.status(200)
            .json_body(results_body(&run_ids, "accepted"));
    });
    let scans = server.mock(|when, then| {
        when.method(POST).path("/api/scans/ingest");
        then.status(500);
    });

    let out = run_vettd(
        &observe_args(root.to_str().unwrap(), &["--submit"]),
        home.path(),
    );
    assert_eq!(out.status, 0, "{}", out.stderr);
    observations.assert_calls(1);
    scans.assert_calls(0);
}

/// Invariant: `--submit` with no credential exits 3 (not configured) and reads nothing.
///
/// Distinct from 1 on purpose, and the same code as telemetry being off: both mean "you have not
/// set this up", which is not a failure a script should treat as an error. Failing before the read
/// also means nobody's transcripts are opened to build a payload that could never be sent.
#[test]
fn observe_submit_without_credentials_exits_3_before_reading() {
    let home = seed_home(Some(true));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);
    let out = run_vettd(
        &observe_args(root.to_str().unwrap(), &["--submit"]),
        home.path(),
    );

    assert_eq!(out.status, 3, "{}", out.stderr);
    assert!(
        out.stderr.contains("No API key for --submit"),
        "the guidance must name what is missing: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("vettd auth"),
        "and how to fix it: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("Nothing was read."),
        "and say nothing was read: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("This observation will include:"),
        "the disclosure must precede even a credential/configuration refusal: {}",
        out.stderr
    );
    assert!(out.stdout.is_empty());
    assert!(!home.path().join(".vettd/observer").exists());
}

/// Invariant: `--dry-run --submit` builds and checks the payload and sends nothing.
///
/// The combination is legitimate — "show me exactly what would go" — and the failure to guard
/// against is the obvious one: a dry run that actually transmits.
#[test]
fn observe_dry_run_with_submit_sends_nothing() {
    let server = MockServer::start();
    let home = seed_home_with_auth(&scan_endpoint(&server));
    let root = copy_harness_home(home.path(), FIXTURE_HOME);

    let never = server.mock(|when, then| {
        when.method(POST);
        then.status(200).json_body(json!({"results": []}));
    });

    let payload = home.path().join("would-send.json");
    let out = run_vettd(
        &observe_args(
            root.to_str().unwrap(),
            &["--submit", "--dry-run", "--out", payload.to_str().unwrap()],
        ),
        home.path(),
    );

    assert_eq!(out.status, 0, "{}", out.stderr);
    never.assert_calls(0);
    assert!(
        payload.exists(),
        "the payload must still be produced to inspect"
    );
    assert!(
        out.stderr.contains("dry run: nothing was sent"),
        "and it must say so rather than leaving the user to infer it: {}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("Observations accepted"),
        "no submission means no success line: {}",
        out.stderr
    );
    // The store IS opened (step 4 runs for any --submit, dry or not) but nothing is committed to
    // it, so the next real submit still has the run to send.
    let ledgered = run_vettd(
        &observe_args(root.to_str().unwrap(), &["--submit", "--dry-run"]),
        home.path(),
    );
    assert!(
        !ledgered.stderr.contains("nothing new to send"),
        "a dry run must not have consumed the record: {}",
        ledgered.stderr
    );
}
