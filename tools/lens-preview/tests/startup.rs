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
