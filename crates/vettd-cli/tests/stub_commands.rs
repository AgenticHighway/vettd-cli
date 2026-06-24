//! Fail-loud assertions for the web-facing command stubs scaffolded in #149.
//!
//! `not_implemented` calls `std::process::exit`, so it cannot be exercised
//! in-process. These tests launch the real `vettd` binary and assert that each
//! representative stub: exits with code 2, keeps stdout (the machine channel)
//! empty, and prints the not-implemented notice to stderr. If a stub ever
//! silently exits 0, stops printing the notice, or collapses onto exit(1),
//! these tests fail.

use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vettd"))
        .args(args)
        .output()
        .expect("failed to launch vettd binary")
}

fn assert_stub(args: &[&str]) {
    let out = run(args);
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit code 2 for {args:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "stub must not write to stdout for {args:?}, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not yet implemented"),
        "stub must print not-implemented notice to stderr for {args:?}, got: {stderr}"
    );
}

#[test]
fn auth_status_is_not_implemented() {
    assert_stub(&["auth", "status"]);
}

#[test]
fn contract_status_is_not_implemented() {
    assert_stub(&["contract", "status"]);
}

#[test]
fn directory_list_is_not_implemented() {
    assert_stub(&["directory", "list"]);
}
