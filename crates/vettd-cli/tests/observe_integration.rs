//! Integration tests for `vettd observe`, spawning the real binary as a subprocess.
//!
//! These exist because the properties that matter most about this command are properties of the
//! *process*, not of any function in it: which stream a byte lands on, whether a file exists after
//! a refusal, what the exit code is, and whether any of it touched the user's home. None of that is
//! observable from a unit test that calls `run_observe` in-process.
//!
//! Every test runs against a throwaway `$HOME` seeded by [`seed_home`] and a *copy* of the fixture
//! harness home, so a test can never read (or mutate) anything under `tests/fixtures/`. The clock,
//! the day and the HMAC secret are pinned via the hidden test hooks so payload bytes are
//! reproducible.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Run the real binary with `$HOME` pointed at `home` and every ambient influence removed.
fn run_vettd(args: &[&str], home: &Path) -> CliOutput {
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    cmd.env("HOME", home);
    // Windows resolves the home directory from these, not from HOME.
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
