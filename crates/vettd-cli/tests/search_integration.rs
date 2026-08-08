//! Integration tests for `vettd directory search` / `vettd inventory search`
//! against a real (mocked) HTTP endpoint.
//!
//! Unlike the unit tests in `directory.rs`/`inventory.rs`/`network.rs`,
//! these spawn the actual `vettd` binary as a subprocess and serve real
//! HTTP responses via `httpmock`'s loopback server, exercising the full
//! path: CLI arg parsing → endpoint derivation → HTTP request → allow-list
//! deserialization → stdout rendering.
//!
//! Covers both shapes described in `docs/SEARCH_INTERFACE.md`:
//! - "old" shape: `GET {base}/directory?search=...` (SEARCH_BETA_TESTING unset).
//! - "new" shape: `POST {base}/directory` with a JSON body carrying
//!   `--language`/`--agent-compatibility`/`--rankings` (SEARCH_BETA_TESTING=1),
//!   plus the dual raw-json + formatted-table dump it triggers.
//!
//! Every JSON assertion compares a *whole* `serde_json::Value` (request body
//! or CLI stdout) against one literal built with `serde_json::json!` — no
//! chains of `payload["skills"][0]["field"]` indexing. A mismatch anywhere
//! in the object fails the `assert_eq!`/`mock.assert()` with the full diff.

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use serde_json::{json, Value};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vettd");
const MOCK_API_KEY: &str = "mock-api-key-123";

/// Derive the ingest-style endpoint URL `derive_api_url()` expects, pointing
/// at an `httpmock::MockServer`.
fn ingest_endpoint(server: &MockServer) -> String {
    format!("{}/api/scans/ingest", server.base_url())
}

// ---------------------------------------------------------------------------
// Canned server bodies. The server always returns `internalRiskScore` (not
// part of the CLI's allow-list `DirectoryCard` struct) to prove over-exposed
// fields are dropped rather than forwarded to stdout.
// ---------------------------------------------------------------------------

/// What a plain (pre-beta) directory server returns: no `language`/
/// `agentCompatibility`/`rankings` fields at all.
fn directory_response_basic() -> Value {
    json!({
        "skills": [{
            "slug": "pdf-summarizer",
            "name": "PDF Summarizer",
            "description": "Extracts and summarizes long PDF reports.",
            "version": "2.3.0",
            "author": "acme-labs",
            "category": "productivity",
            "badgeStatus": "verified",
            "overallGrade": "A",
            "sourceType": "github",
            "scannerRunCount": 12,
            "internalRiskScore": 0.02
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1
    })
}

/// What the exact same CLI call prints to stdout for `directory_response_basic()`:
/// allow-listed fields only. `language`/`agentCompatibility`/`rankings` are
/// skipped entirely (not even `null`) since the server didn't send them —
/// `--json` output must be byte-identical to the pre-beta shape.
fn expected_directory_output_basic() -> Value {
    json!({
        "skills": [{
            "slug": "pdf-summarizer",
            "name": "PDF Summarizer",
            "description": "Extracts and summarizes long PDF reports.",
            "version": "2.3.0",
            "author": "acme-labs",
            "category": "productivity",
            "badgeStatus": "verified",
            "overallGrade": "A",
            "sourceType": "github",
            "scannerRunCount": 12
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1
    })
}

/// What a `SEARCH_BETA_TESTING` directory server returns: includes the new
/// `language`/`agentCompatibility`/`rankings` fields with real values.
fn directory_response_beta() -> Value {
    json!({
        "skills": [{
            "slug": "pdf-summarizer",
            "name": "PDF Summarizer",
            "description": "Extracts and summarizes long PDF reports.",
            "version": "2.3.0",
            "author": "acme-labs",
            "category": "productivity",
            "badgeStatus": "verified",
            "overallGrade": "A",
            "sourceType": "github",
            "scannerRunCount": 12,
            "internalRiskScore": 0.02,
            "language": "python",
            "agentCompatibility": ["claude-code", "cursor"],
            "rankings": {
                "stars": 812,
                "skillsShLeaderboardRank": 14,
                "numberOfAggregators": 3,
                "officialClaudeMarketplace": true
            }
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1
    })
}

/// What the exact same CLI call prints to stdout for `directory_response_beta()`:
/// allow-listed fields only (`internalRiskScore` dropped), everything else
/// forwarded as-is.
fn expected_directory_output_beta() -> Value {
    json!({
        "skills": [{
            "slug": "pdf-summarizer",
            "name": "PDF Summarizer",
            "description": "Extracts and summarizes long PDF reports.",
            "version": "2.3.0",
            "author": "acme-labs",
            "category": "productivity",
            "badgeStatus": "verified",
            "overallGrade": "A",
            "sourceType": "github",
            "scannerRunCount": 12,
            "language": "python",
            "agentCompatibility": ["claude-code", "cursor"],
            "rankings": {
                "stars": 812,
                "skillsShLeaderboardRank": 14,
                "numberOfAggregators": 3,
                "officialClaudeMarketplace": true
            }
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1
    })
}

fn inventory_response_basic() -> Value {
    json!({
        "skills": [{
            "slug": "my-private-notes-skill",
            "name": "My Private Notes Skill",
            "description": "Formats personal notes into a daily digest.",
            "version": "0.1.0",
            "author": "you",
            "category": "personal",
            "badgeStatus": "unlisted",
            "overallGrade": "B",
            "sourceType": "local",
            "scannerRunCount": 1
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1
    })
}

fn expected_inventory_output_basic() -> Value {
    json!({
        "skills": [{
            "slug": "my-private-notes-skill",
            "name": "My Private Notes Skill",
            "description": "Formats personal notes into a daily digest.",
            "version": "0.1.0",
            "author": "you",
            "category": "personal",
            "badgeStatus": "unlisted",
            "overallGrade": "B",
            "sourceType": "local",
            "scannerRunCount": 1
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1
    })
}

// ---------------------------------------------------------------------------
// Mount helpers
// ---------------------------------------------------------------------------

/// Mount `GET /api/directory` (old shape), mirroring `crate::read_client`
/// (no auth header required).
fn mount_directory_get(server: &MockServer, response_body: &Value) {
    server.mock(|when, then| {
        when.method(GET).path("/api/directory");
        then.status(200).json_body(response_body.clone());
    });
}

/// Mount `POST /api/directory` (new, `SEARCH_BETA_TESTING` shape), asserting
/// the request body matches `expected_request_body` exactly. Returns the
/// `Mock` handle so the caller can `.assert()` after invoking the CLI.
fn mount_directory_post<'a>(
    server: &'a MockServer,
    expected_request_body: &Value,
    response_body: &Value,
) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/api/directory")
            .json_body(expected_request_body.clone());
        then.status(200).json_body(response_body.clone());
    })
}

/// Mount `GET /api/inventory` (old shape), mirroring
/// `crate::inventory_client`: requires `Authorization: Bearer
/// <MOCK_API_KEY>`, otherwise 401.
fn mount_inventory_get(server: &MockServer, response_body: &Value) {
    server.mock(|when, then| {
        when.method(GET)
            .path("/api/inventory")
            .header("authorization", format!("Bearer {MOCK_API_KEY}"));
        then.status(200).json_body(response_body.clone());
    });
    server.mock(|when, then| {
        when.method(GET).path("/api/inventory");
        then.status(401)
            .json_body(json!({"error": "invalid or missing bearer token"}));
    });
}

/// Mount `POST /api/inventory` (new shape), requiring both the bearer token
/// and an exact JSON body match.
fn mount_inventory_post<'a>(
    server: &'a MockServer,
    expected_request_body: &Value,
    response_body: &Value,
) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/api/inventory")
            .header("authorization", format!("Bearer {MOCK_API_KEY}"))
            .json_body(expected_request_body.clone());
        then.status(200).json_body(response_body.clone());
    })
}

/// The default POST body the CLI sends when no `--language`/
/// `--agent-compatibility`/`--rankings` flags are given.
fn default_search_body(query: &str) -> Value {
    json!({
        "search": query,
        "page": 1,
        "sort": "newest",
        "reverse": false,
        "languages": [],
        "agentCompatibility": [],
        "rankings": null
    })
}

// ---------------------------------------------------------------------------
// $HOME fixture helpers
// ---------------------------------------------------------------------------

/// Directory holding throwaway `$HOME`s for this test binary, kept inside
/// `crates/vettd-cli/tests/` (rather than the OS temp dir) so all seeded
/// test fixtures live under version control's eye — see `.gitignore` for
/// why nothing here ends up committed.
fn tmp_homes_dir() -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("tmp-homes");
    std::fs::create_dir_all(&dir).expect("create tests/tmp-homes");
    dir
}

/// Create a throwaway `$HOME` under `crates/vettd-cli/tests/tmp-homes/`, so
/// tests never touch the developer's real vettd config.
fn new_temp_home() -> tempfile::TempDir {
    tempfile::tempdir_in(tmp_homes_dir()).expect("create temp home")
}

/// Seed a throwaway `$HOME` with `~/.config/vettd/config.json` (Linux path)
/// and `~/Library/Application Support/vettd/config.json` (macOS path), so
/// tests never touch the developer's real vettd config.
fn seed_home(endpoint: &str, api_key: &str) -> tempfile::TempDir {
    let home = new_temp_home();
    let config = format!(r#"{{"endpoint":"{endpoint}","apiKey":"{api_key}"}}"#);

    let xdg_dir = home.path().join(".config").join("vettd");
    std::fs::create_dir_all(&xdg_dir).unwrap();
    std::fs::write(xdg_dir.join("config.json"), &config).unwrap();

    let mac_dir = home
        .path()
        .join("Library")
        .join("Application Support")
        .join("vettd");
    std::fs::create_dir_all(&mac_dir).unwrap();
    std::fs::write(mac_dir.join("config.json"), &config).unwrap();

    home
}

struct CliOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

fn run_vettd(args: &[&str], home: &std::path::Path, extra_env: &[(&str, &str)]) -> CliOutput {
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    cmd.env("HOME", home);
    // Never inherit an ambient SEARCH_BETA_TESTING from the dev's own shell.
    cmd.env_remove("SEARCH_BETA_TESTING");
    cmd.env_remove("VETTD_DIRECTORY_ENDPOINT");
    cmd.env_remove("VETTD_INVENTORY_ENDPOINT");
    // Force the CLI to resolve config from $HOME/.config (the path
    // `seed_home` writes), not from whatever XDG_CONFIG_HOME the CI or
    // dev shell happens to have set — otherwise the seeded config is
    // missed and the CLI falls through to the real production endpoint.
    cmd.env_remove("XDG_CONFIG_HOME");
    cmd.env_remove("XDG_CONFIG_DIRS");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run vettd binary");
    CliOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip "\x1b[...m"
            for c2 in chars.by_ref() {
                if c2 == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse `s` as JSON and compare it as a whole `Value` against `expected`,
/// so a mismatch anywhere prints the full actual-vs-expected diff instead of
/// a single indexed field.
fn assert_json_eq(actual_str: &str, expected: &Value) {
    let actual: Value = serde_json::from_str(actual_str).expect("stdout is valid json");
    assert_eq!(actual, *expected);
}

const RAW_JSON_MARKER: &str = "--- SEARCH_BETA_TESTING: raw json ---";
const FORMATTED_MARKER: &str = "--- SEARCH_BETA_TESTING: formatted ---";

/// Pull the raw-JSON section out of a `SEARCH_BETA_TESTING` dual dump
/// (raw json, then the formatted table) and parse it — beta mode always
/// prints both, so stdout is never pure JSON even with `--json`.
fn extract_beta_raw_json(stdout: &str) -> Value {
    let out = strip_ansi(stdout);
    let start = out.find(RAW_JSON_MARKER).expect("raw json marker present") + RAW_JSON_MARKER.len();
    let end = out
        .find(FORMATTED_MARKER)
        .expect("formatted marker present");
    serde_json::from_str(out[start..end].trim()).expect("raw dump is valid json")
}

// ---------------------------------------------------------------------------
// Old shape: GET via the saved config endpoint (SEARCH_BETA_TESTING unset).
// ---------------------------------------------------------------------------

#[test]
fn directory_search_json_via_config_endpoint_is_allow_list_filtered() {
    let server = MockServer::start();
    mount_directory_get(&server, &directory_response_basic());
    let home = seed_home(&ingest_endpoint(&server), "");

    let result = run_vettd(&["directory", "search", "pdf", "--json"], home.path(), &[]);

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    assert_json_eq(&result.stdout, &expected_directory_output_basic());
}

#[test]
fn directory_search_human_table_via_config_endpoint() {
    let server = MockServer::start();
    mount_directory_get(&server, &directory_response_basic());
    let home = seed_home(&ingest_endpoint(&server), "");

    let result = run_vettd(&["directory", "search", "pdf"], home.path(), &[]);

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    let out = strip_ansi(&result.stdout);
    assert!(out.contains("pdf-summarizer"));
    assert!(out.contains("[A]"));
    assert!(out.contains("13 scanners")); // scanner_run_count (12) + 1
}

#[test]
fn inventory_search_without_config_is_rejected_before_any_request() {
    let home = new_temp_home();
    // No config seeded at all.
    let result = run_vettd(&["inventory", "search", "notes"], home.path(), &[]);

    assert_eq!(result.status, 3);
    assert!(result.stderr.contains("not authenticated"));
}

#[test]
fn inventory_search_via_config_endpoint_sends_bearer_token() {
    let server = MockServer::start();
    mount_inventory_get(&server, &inventory_response_basic());
    let home = seed_home(&ingest_endpoint(&server), MOCK_API_KEY);

    let result = run_vettd(
        &["inventory", "search", "notes", "--json"],
        home.path(),
        &[],
    );

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    assert_json_eq(&result.stdout, &expected_inventory_output_basic());
}

#[test]
fn inventory_search_wrong_api_key_is_rejected_by_server() {
    let server = MockServer::start();
    mount_inventory_get(&server, &inventory_response_basic());
    let home = seed_home(&ingest_endpoint(&server), "wrong-key");

    let result = run_vettd(&["inventory", "search", "notes"], home.path(), &[]);

    assert_ne!(result.status, 0);
}

// ---------------------------------------------------------------------------
// New shape: POST + JSON via SEARCH_BETA_TESTING, including the
// --language/--agent-compatibility/--rankings filters.
// ---------------------------------------------------------------------------

#[test]
fn env_override_is_ignored_without_search_beta_testing() {
    let server = MockServer::start();
    mount_directory_get(&server, &directory_response_basic());
    // Config points at a decoy endpoint nothing listens on; the env var
    // override must be inert without the flag.
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &["directory", "search", "pdf"],
        home.path(),
        &[("VETTD_DIRECTORY_ENDPOINT", &ingest_endpoint(&server))],
    );

    assert_ne!(result.status, 0);
    assert!(!result.stdout.contains("pdf-summarizer"));
}

#[test]
fn beta_search_sends_post_with_default_body_when_no_filters_given() {
    let server = MockServer::start();
    let mock = mount_directory_post(
        &server,
        &default_search_body("pdf"),
        &directory_response_beta(),
    );
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &["directory", "search", "pdf", "--json"],
        home.path(),
        &[
            ("VETTD_DIRECTORY_ENDPOINT", &ingest_endpoint(&server)),
            ("SEARCH_BETA_TESTING", "1"),
        ],
    );

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    mock.assert(); // fails with a body diff if the POST shape drifts
                   // Beta mode always dumps raw json + formatted table, even with --json.
    assert_eq!(
        extract_beta_raw_json(&result.stdout),
        expected_directory_output_beta()
    );
}

#[test]
fn beta_search_sends_language_agent_and_rankings_filters_in_post_body() {
    let server = MockServer::start();
    let expected_body = json!({
        "search": "pdf",
        "page": 1,
        "sort": "newest",
        "reverse": false,
        "languages": ["python", "typescript"],
        "agentCompatibility": ["claude-code"],
        "rankings": {"stars": 50, "officialClaudeMarketplace": true}
    });
    let mock = mount_directory_post(&server, &expected_body, &directory_response_beta());
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &[
            "directory",
            "search",
            "pdf",
            "--language",
            "python",
            "--language",
            "typescript",
            "--agent-compatibility",
            "claude-code",
            "--rankings",
            r#"{"stars": 50, "officialClaudeMarketplace": true}"#,
            "--json",
        ],
        home.path(),
        &[
            ("VETTD_DIRECTORY_ENDPOINT", &ingest_endpoint(&server)),
            ("SEARCH_BETA_TESTING", "1"),
        ],
    );

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    mock.assert();
    assert_eq!(
        extract_beta_raw_json(&result.stdout),
        expected_directory_output_beta()
    );
}

#[test]
fn beta_search_inventory_sends_bearer_token_and_post_body() {
    let server = MockServer::start();
    let mock = mount_inventory_post(
        &server,
        &default_search_body("notes"),
        &inventory_response_basic(),
    );
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", MOCK_API_KEY);

    let result = run_vettd(
        &["inventory", "search", "notes", "--json"],
        home.path(),
        &[
            ("VETTD_INVENTORY_ENDPOINT", &ingest_endpoint(&server)),
            ("SEARCH_BETA_TESTING", "1"),
        ],
    );

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    mock.assert();
}

#[test]
fn language_filter_without_search_beta_testing_is_rejected() {
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &["directory", "search", "pdf", "--language", "python"],
        home.path(),
        &[],
    );

    assert_eq!(result.status, 1);
    assert!(result
        .stderr
        .contains("--language/--agent-compatibility/--rankings require SEARCH_BETA_TESTING=1"));
}

#[test]
fn invalid_rankings_json_is_rejected_before_any_request() {
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &["directory", "search", "pdf", "--rankings", "not-json"],
        home.path(),
        &[("SEARCH_BETA_TESTING", "1")],
    );

    assert_eq!(result.status, 1);
    assert!(result.stderr.contains("--rankings is not valid JSON"));
}

#[test]
fn search_beta_testing_dumps_raw_json_and_formatted_table_together() {
    let server = MockServer::start();
    mount_directory_post(
        &server,
        &default_search_body("pdf"),
        &directory_response_beta(),
    );
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    // No --json passed — beta mode must still emit the raw JSON dump ahead
    // of the formatted table.
    let result = run_vettd(
        &["directory", "search", "pdf"],
        home.path(),
        &[
            ("VETTD_DIRECTORY_ENDPOINT", &ingest_endpoint(&server)),
            ("SEARCH_BETA_TESTING", "true"),
        ],
    );

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    let out = strip_ansi(&result.stdout);

    let raw_idx = out.find(RAW_JSON_MARKER).expect("raw json marker present");
    let formatted_idx = out
        .find(FORMATTED_MARKER)
        .expect("formatted marker present");
    assert!(raw_idx < formatted_idx, "raw dump must precede the table");

    assert_eq!(
        extract_beta_raw_json(&result.stdout),
        expected_directory_output_beta()
    );

    let formatted_slice = &out[formatted_idx..];
    assert!(formatted_slice.contains("[A]"));
    assert!(formatted_slice.contains("Showing 1 of 1 assets"));
}

#[test]
fn search_beta_testing_still_appends_table_when_json_flag_passed() {
    let server = MockServer::start();
    mount_inventory_post(
        &server,
        &default_search_body("notes"),
        &inventory_response_basic(),
    );
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", MOCK_API_KEY);

    let result = run_vettd(
        &["inventory", "search", "notes", "--json"],
        home.path(),
        &[
            ("VETTD_INVENTORY_ENDPOINT", &ingest_endpoint(&server)),
            ("SEARCH_BETA_TESTING", "1"),
        ],
    );

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    let out = strip_ansi(&result.stdout);
    assert!(out.contains("--- SEARCH_BETA_TESTING: raw json ---"));
    // --json doesn't suppress the formatted table once beta mode is on.
    assert!(out.contains("[B]"));
}

// ---------------------------------------------------------------------------
// Manual testing helper — not run by `cargo test` (see #[ignore]). Stands up
// the same httpmock server the automated tests use and blocks, so you can
// drive the real `vettd` binary against it by hand from another terminal.
// ---------------------------------------------------------------------------

/// Run with:
///
/// ```text
/// cargo test -p vettd-cli --test search_integration \
///     manual_mock_server_for_local_testing -- --ignored --nocapture
/// ```
///
/// It prints the mock's base URL and ready-to-paste `vettd` invocations,
/// then blocks for 10 minutes (Ctrl-C to stop early). Requests are logged to
/// this terminal by httpmock as they arrive, so you can see the real request
/// the CLI sent — including the POST body for `SEARCH_BETA_TESTING` calls —
/// compared against the mocks registered below.
#[test]
#[ignore = "manual-only: starts a live mock server and blocks; run explicitly"]
fn manual_mock_server_for_local_testing() {
    let server = MockServer::start();

    // Same mocks the automated tests use — old GET shape, new POST shape
    // (with and without filters), and the inventory auth check. Extend this
    // list if you need to manually exercise something else.
    mount_directory_get(&server, &directory_response_basic());
    mount_directory_post(
        &server,
        &default_search_body("pdf"),
        &directory_response_beta(),
    );
    mount_directory_post(
        &server,
        &json!({
            "search": "pdf",
            "page": 1,
            "sort": "newest",
            "reverse": false,
            "languages": ["python"],
            "agentCompatibility": ["claude-code"],
            "rankings": {"stars": 50}
        }),
        &directory_response_beta(),
    );
    mount_inventory_get(&server, &inventory_response_basic());
    mount_inventory_post(
        &server,
        &default_search_body("notes"),
        &inventory_response_basic(),
    );

    let ingest = ingest_endpoint(&server);

    eprintln!("\n=== manual mock server is live ===");
    eprintln!("base url: {}", server.base_url());
    eprintln!("ingest endpoint: {ingest}\n");
    eprintln!(
        "Run these from the repo root, using the locally built binary (not `vettd` on PATH)."
    );
    eprintln!(
        "Set HOME to a scratch dir first so `vettd auth` below never touches your real saved config:\n"
    );
    eprintln!("  export HOME=/tmp/vettd-manual-home\n");
    eprintln!("Old shape (SEARCH_BETA_TESTING unset), against a saved config endpoint:");
    eprintln!("  ./target/debug/vettd auth --endpoint {ingest} --key {MOCK_API_KEY}");
    eprintln!("  ./target/debug/vettd directory search pdf --json\n");
    eprintln!("New shape (SEARCH_BETA_TESTING=1), via the env var override — no config needed:");
    eprintln!("  export SEARCH_BETA_TESTING=1");
    eprintln!("  export VETTD_DIRECTORY_ENDPOINT={ingest}");
    eprintln!("  ./target/debug/vettd directory search pdf --json");
    eprintln!("  ./target/debug/vettd directory search pdf --language python --agent-compatibility claude-code --rankings '{{\"stars\": 50}}' --json\n");
    eprintln!(
        "  export VETTD_INVENTORY_ENDPOINT={ingest}   # inventory also needs the `auth` step above for its api key"
    );
    eprintln!("  ./target/debug/vettd inventory search notes --json\n");
    eprintln!("Blocking for 10 minutes — Ctrl-C to stop early.\n");

    std::thread::sleep(std::time::Duration::from_secs(600));
}
