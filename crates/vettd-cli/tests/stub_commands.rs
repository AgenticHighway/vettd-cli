//! Fail-loud assertions for web-facing command stubs that remain unimplemented.
//!
//! `not_implemented` calls `std::process::exit`, so it cannot be exercised
//! in-process. These tests launch the real `vettd` binary and assert that each
//! remaining stub: exits with code 2, keeps stdout (the machine channel)
//! empty, and prints the not-implemented notice to stderr. If a stub ever
//! silently exits 0, stops printing the notice, or collapses onto exit(1),
//! these tests fail.
//!
//! Commands removed from this file are now fully implemented:
//!   auth status, contract status, directory search, directory list,
//!   directory view, directory findings, directory compare.
//!
//! Remaining stubs (blocked on vettd#631 backend work):
//!   directory trending, directory random.

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
fn directory_trending_is_not_implemented() {
    assert_stub(&["directory", "trending"]);
}

#[test]
fn directory_random_is_not_implemented() {
    assert_stub(&["directory", "random"]);
}
