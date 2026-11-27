//! Runs the built binary on the hand-written logs under `tests/data/`.

use std::path::Path;
use std::process::Command;

// Reading a large history takes seconds and shows nothing on stdout, so
// stderr must say it's working. Stdout stays machine-readable: the notice
// never leaks into the export.
#[test]
fn local_export_says_it_is_reading_on_stderr() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data");
    // A relay file that doesn't exist, so a real one on this machine is never read.
    let relay = std::env::temp_dir().join("lens-startup-test-no-relay.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_vettd-lens-preview"))
        .args(["--source", "local", "--json", "--relay-file"])
        .arg(&relay)
        .env("CLAUDE_CONFIG_DIR", data.join("claude"))
        .env("CODEX_HOME", data.join("codex"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.starts_with("Reading local logs…"), "{stderr}");
    let export: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(export.is_object());
    assert!(!relay.exists());
}

/// Now as RFC 3339 UTC, so a test line falls in the run's own period.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // Howard Hinnant's civil_from_days.
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let rest = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn run(home: &Path, args: &[&str]) -> std::process::Output {
    let relay = std::env::temp_dir().join("lens-startup-test-no-relay.jsonl");
    Command::new(env!("CARGO_BIN_EXE_vettd-lens-preview"))
        .args(args)
        .arg("--relay-file")
        .arg(&relay)
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .output()
        .unwrap()
}

// Plain `--json` must cost what it did before History: a file untouched
// for 30 days is never opened, even when (as here, by hand) it holds a
// request from today that would show if it were. `--history` opts in to
// the full read, finds it, and changes nothing else in the export.
#[test]
fn plain_export_skips_old_files_and_history_export_reads_them() {
    let home = std::env::temp_dir().join(format!("lens-startup-history-{}", std::process::id()));
    let project = home.join("claude").join("projects").join("p");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(home.join("codex").join("sessions")).unwrap();
    let path = project.join("s-old.jsonl");
    std::fs::write(
        &path,
        format!(
            r#"{{"type":"assistant","sessionId":"s-old","timestamp":"{}","cwd":"/w/old","requestId":"rq-old","message":{{"id":"m-old","model":"claude-opus-5-5","usage":{{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"content":[]}}}}"#,
            utc_now()
        ),
    )
    .unwrap();
    let month = std::time::Duration::from_secs(30 * 86_400);
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() - month)
        .unwrap();
    drop(file);

    let plain = run(&home, &["--source", "local", "--json"]);
    let full = run(&home, &["--source", "local", "--json", "--history"]);
    std::fs::remove_dir_all(&home).unwrap();
    let mut plain: serde_json::Value = serde_json::from_slice(&plain.stdout).unwrap();
    let mut full: serde_json::Value = serde_json::from_slice(&full.stdout).unwrap();
    assert_eq!(
        plain["sessions"].as_array().unwrap().len(),
        0,
        "old file opened"
    );
    assert!(plain.get("history").is_none());
    let history = full.as_object_mut().unwrap().remove("history").unwrap();
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["workspace"], "old");
    // "now" is the run's own clock; everything else must match.
    plain.as_object_mut().unwrap().remove("now");
    full.as_object_mut().unwrap().remove("now");
    assert_eq!(plain, full);
}

// `--history` alone would print nothing useful, so it fails loudly.
#[test]
fn history_flag_without_json_exits_2() {
    let out = Command::new(env!("CARGO_BIN_EXE_vettd-lens-preview"))
        .arg("--history")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "vettd-lens-preview: --history needs --json\n"
    );
}
