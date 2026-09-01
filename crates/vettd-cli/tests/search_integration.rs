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
//!   `--language`/`--agent-compatibility`/`--rankings` (SEARCH_BETA_TESTING=1).
//!   Beta mode no longer dumps raw JSON alongside the human table — output is
//!   driven by `--json`, identical to the non-beta path.
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

/// What the exact same CLI call prints to stdout for `directory_response_basic()`.
/// `freshness` is omitted entirely (not even `null`) since the server didn't
/// send it — `--json` output must be byte-identical to the pre-slice shape.
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
            "docLanguage": "python",
            "agentCompatibility": ["claude-code", "cursor"],
            "rankings": {
                "stars": 812,
                "skillsShLeaderboardRank": 14,
                "numberOfAggregators": 3,
                "officialClaudeMarketplace": true
            },
            "llm_scan": {
                "max_severity": "LOW",
                "finding_count": 1,
                "primary_threats": ["unpinned-dependency-install"]
            },
            "cli_security": {
                "grade": "C",
                "packages": [{"package": "playwright", "ecosystem": "npm"}]
            },
            "vettd_scan": {
                "overall_grade": "B",
                "trust_level": "cautious",
                "has_malicious_findings": false
            }
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1,
        "mock": false
    })
}

/// What the exact same CLI call prints to stdout for `directory_response_beta()`:
/// allow-listed fields only (`internalRiskScore` dropped), the server's
/// `docLanguage` surfaced under the CLI's own `language` key, plus
/// `agentCompatibility`/`rankings`/`llm_scan`/`cli_security`/`vettd_scan`
/// forwarded as-is. `freshness` is omitted since the server didn't send it.
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
            },
            "llm_scan": {
                "max_severity": "LOW",
                "finding_count": 1,
                "primary_threats": ["unpinned-dependency-install"]
            },
            "cli_security": {
                "grade": "C",
                "packages": [{"package": "playwright", "ecosystem": "npm"}]
            },
            "vettd_scan": {
                "overall_grade": "B",
                "trust_level": "cautious",
                "has_malicious_findings": false
            }
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1,
        "mock": false
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

/// The default POST body the CLI sends when no filter flags are given
/// (`assetType` defaults to `"skill"`, so the mcp-only arrays are absent).
fn default_search_body(query: &str) -> Value {
    json!({
        "search": query,
        "page": 1,
        "sort": "newest",
        "reverse": false,
        "assetType": "skill",
        "languages": [],
        "agentCompatibility": [],
        "sources": [],
        "rankFilters": {},
        "rankings": null
    })
}

/// The default POST body for an `--asset-type mcp` search with no filters:
/// same base keys plus the always-present mcp-only arrays.
fn default_mcp_search_body(query: &str) -> Value {
    json!({
        "search": query,
        "page": 1,
        "sort": "newest",
        "reverse": false,
        "assetType": "mcp",
        "languages": [],
        "agentCompatibility": [],
        "sources": [],
        "rankFilters": {},
        "rankings": null,
        "mcpCategory": [],
        "deployment": [],
        "registryType": []
    })
}

/// An `mcpServers`-envelope response (assetType: "mcp"), carrying one
/// `github:upstash/context7` hit with the OSV dependency-security block.
fn mcp_response() -> Value {
    json!({
        "mcpServers": [{
            "score": 0.75,
            "rank": 1,
            "mcp_id": "github:upstash/context7",
            "name": "io.github.upstash/context7",
            "description": "Fetches up-to-date, version-specific docs and code examples.",
            "repo_url": "https://github.com/upstash/context7",
            "status": "active",
            "mcp_category": "server",
            "sources": ["repo_scan", "official_registry", "glama"],
            "registry_type": "npm",
            "package_identifier": "@upstash/context7-mcp",
            "deployment": "hybrid",
            "transport": "stdio",
            "has_installable_package": true,
            "has_remote": true,
            "license": "MIT License",
            "stars": 61421,
            "language": "TypeScript",
            "weekly_downloads": 867314,
            "security_source": "osv",
            "security_vuln_count": 0,
            "security_max_severity": null,
            "security_direct_deps_scanned": 8,
            "security_direct_deps_vuln_count": 44,
            "security_direct_deps_with_vulns": ["zod", "jose", "undici", "express"],
            "security_direct_deps_max_severity": "HIGH",
            "internalRiskScore": 0.02
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1,
        "mock": false,
        "indexReady": true
    })
}

/// What the CLI's raw-json dump renders for `mcp_response()`: allow-listed
/// snake_case fields only (`internalRiskScore` dropped).
fn expected_mcp_output() -> Value {
    json!({
        "mcpServers": [{
            "score": 0.75,
            "rank": 1,
            "mcp_id": "github:upstash/context7",
            "name": "io.github.upstash/context7",
            "description": "Fetches up-to-date, version-specific docs and code examples.",
            "repo_url": "https://github.com/upstash/context7",
            "status": "active",
            "mcp_category": "server",
            "sources": ["repo_scan", "official_registry", "glama"],
            "registry_type": "npm",
            "package_identifier": "@upstash/context7-mcp",
            "deployment": "hybrid",
            "transport": "stdio",
            "has_installable_package": true,
            "has_remote": true,
            "license": "MIT License",
            "stars": 61421,
            "language": "TypeScript",
            "weekly_downloads": 867314,
            "security_source": "osv",
            "security_vuln_count": 0,
            "security_direct_deps_scanned": 8,
            "security_direct_deps_vuln_count": 44,
            "security_direct_deps_with_vulns": ["zod", "jose", "undici", "express"],
            "security_direct_deps_max_severity": "HIGH"
        }],
        "total": 1,
        "page": 1,
        "totalPages": 1,
        "mock": false,
        "indexReady": true
    })
}

/// An `mcpServers` response for an empty catalog: no hits, `indexReady:false`
/// (onboarding/outage — distinct from "no results for this query").
fn mcp_response_index_not_ready() -> Value {
    json!({
        "mcpServers": [],
        "total": 0,
        "page": 1,
        "totalPages": 0,
        "mock": false,
        "indexReady": false
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

// (no RAW_JSON_MARKER / FORMATTED_MARKER / extract_beta_raw_json — the
// mixed dump was removed; see issues #231-#233)

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

// Regression tests for #231: invalid beta-gated filters must be rejected
// before the auth check runs. An unauthenticated invocation with a bad filter
// should exit 1 with the filter-validation error, NOT exit 3 with the auth
// error.
#[test]
fn inventory_search_with_beta_gated_filter_without_flag_exits_before_auth_check() {
    let home = new_temp_home();
    // No config seeded — no auth.
    let result = run_vettd(
        &["inventory", "search", "notes", "--language", "python"],
        home.path(),
        &[],
    );

    assert_eq!(
        result.status, 1,
        "expected filter-validation exit (1), not auth exit (3); stderr: {}",
        result.stderr
    );
    assert!(
        result.stderr.contains("--language")
            && result.stderr.contains("require SEARCH_BETA_TESTING=1"),
        "stderr should mention the beta-gate message: {}",
        result.stderr
    );
}

#[test]
fn inventory_search_with_invalid_rankings_and_beta_flag_exits_before_auth_check() {
    let home = new_temp_home();
    // SEARCH_BETA_TESTING is set so the filter-validation step runs, but the
    // rankings value is invalid JSON → must fail at validation, before auth.
    let result = run_vettd(
        &["inventory", "search", "notes", "--rankings", "not-json"],
        home.path(),
        &[("SEARCH_BETA_TESTING", "1")],
    );

    assert_eq!(
        result.status, 1,
        "expected filter-validation exit (1), not auth exit (3); stderr: {}",
        result.stderr
    );
    assert!(
        result.stderr.contains("--rankings is not valid JSON"),
        "stderr should mention the invalid-JSON message: {}",
        result.stderr
    );
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
    mock.assert();
    assert_json_eq(&result.stdout, &expected_directory_output_beta());
}

#[test]
fn beta_search_sends_language_agent_and_rankings_filters_in_post_body() {
    let server = MockServer::start();
    let expected_body = json!({
        "search": "pdf",
        "page": 1,
        "sort": "newest",
        "reverse": false,
        "assetType": "skill",
        "languages": ["python", "typescript"],
        "agentCompatibility": ["claude-code"],
        "sources": [],
        "rankFilters": {},
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
    assert_json_eq(&result.stdout, &expected_directory_output_beta());
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
    assert!(result.stderr.contains("require SEARCH_BETA_TESTING=1"));
}

/// `--source` / `--rank-filter` / `--asset-type mcp` / mcp-only filters are
/// all beta-gated the same way `--language` is: a hard exit-1 error, and no
/// HTTP request is made.
#[test]
fn new_filter_flags_without_search_beta_testing_are_rejected_before_any_request() {
    let cases: &[&[&str]] = &[
        &["directory", "search", "pdf", "--source", "marketplace"],
        &[
            "directory",
            "search",
            "pdf",
            "--rank-filter",
            "search_rank_seed_rank=5",
        ],
        &["directory", "search", "pdf", "--asset-type", "mcp"],
        &["directory", "search", "pdf", "--mcp-category", "server"],
        &["directory", "search", "pdf", "--deployment", "hybrid"],
        &["directory", "search", "pdf", "--registry-type", "npm"],
        &["inventory", "search", "notes", "--source", "seed"],
        &[
            "inventory",
            "search",
            "notes",
            "--rank-filter",
            "search_rank_seed_rank=5",
        ],
    ];
    for args in cases {
        let server = MockServer::start();
        let get_dir = server.mock(|when, then| {
            when.method(GET).path("/api/directory");
            then.status(200).json_body(directory_response_basic());
        });
        let post_dir = server.mock(|when, then| {
            when.method(POST).path("/api/directory");
            then.status(200).json_body(directory_response_beta());
        });
        let post_inv = server.mock(|when, then| {
            when.method(POST).path("/api/inventory");
            then.status(200).json_body(inventory_response_basic());
        });
        let home = seed_home(&ingest_endpoint(&server), MOCK_API_KEY);
        let is_inventory = args[0] == "inventory";
        let env_key = if is_inventory {
            "VETTD_INVENTORY_ENDPOINT"
        } else {
            "VETTD_DIRECTORY_ENDPOINT"
        };
        let result = run_vettd(args, home.path(), &[(env_key, &ingest_endpoint(&server))]);

        assert_ne!(result.status, 0, "{args:?} should exit non-zero");
        assert!(
            result.stderr.contains("require SEARCH_BETA_TESTING=1"),
            "{args:?} stderr: {}",
            result.stderr
        );
        get_dir.assert_calls(0);
        post_dir.assert_calls(0);
        post_inv.assert_calls(0);
    }
}

#[test]
fn invalid_rank_filter_is_rejected_before_any_request() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST).path("/api/directory");
        then.status(200).json_body(directory_response_beta());
    });
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    for bad in ["no-equals", "k=abc", "=10"] {
        let result = run_vettd(
            &["directory", "search", "pdf", "--rank-filter", bad],
            home.path(),
            &[
                ("VETTD_DIRECTORY_ENDPOINT", &ingest_endpoint(&server)),
                ("SEARCH_BETA_TESTING", "1"),
            ],
        );
        assert_eq!(result.status, 1, "{bad:?} stderr: {}", result.stderr);
        assert!(result.stderr.contains("--rank-filter"));
    }
    mock.assert_calls(0);
}

#[test]
fn beta_search_threads_source_and_rank_filter_into_post_body() {
    let server = MockServer::start();
    let expected_body = json!({
        "search": "pdf",
        "page": 1,
        "sort": "newest",
        "reverse": false,
        "assetType": "skill",
        "languages": [],
        "agentCompatibility": [],
        "sources": ["marketplace", "seed"],
        "rankFilters": {"search_rank_skills_sh_rank": 100},
        "rankings": null
    });
    let mock = mount_directory_post(&server, &expected_body, &directory_response_beta());
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &[
            "directory",
            "search",
            "pdf",
            "--source",
            "marketplace",
            "--source",
            "seed",
            "--rank-filter",
            "search_rank_skills_sh_rank=100",
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
    assert_json_eq(&result.stdout, &expected_directory_output_beta());
}

#[test]
fn beta_search_renders_scan_verdicts_in_json_dump() {
    let server = MockServer::start();
    mount_directory_post(
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
    let raw: Value = serde_json::from_str(&result.stdout).expect("stdout is valid json");
    assert_eq!(raw["skills"][0]["llm_scan"]["max_severity"], "LOW");
    assert_eq!(raw["skills"][0]["cli_security"]["grade"], "C");
    assert_eq!(raw["skills"][0]["vettd_scan"]["overall_grade"], "B");
}

// ---------------------------------------------------------------------------
// assetType: "mcp" — different request body key + different response envelope.
// ---------------------------------------------------------------------------

/// Mount `POST /api/directory` asserting an exact `{"assetType":"mcp",...}`
/// body and returning an `mcpServers` envelope.
fn mount_mcp_post<'a>(
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

#[test]
fn mcp_search_sends_asset_type_mcp_body_and_renders_mcp_servers() {
    let server = MockServer::start();
    let mock = mount_mcp_post(
        &server,
        &default_mcp_search_body("context7"),
        &mcp_response(),
    );
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &[
            "directory",
            "search",
            "context7",
            "--asset-type",
            "mcp",
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
    assert_json_eq(&result.stdout, &expected_mcp_output());
    assert!(result.stdout.contains("github:upstash/context7"));
}

#[test]
fn mcp_search_threads_mcp_filters_into_body() {
    let server = MockServer::start();
    let expected_body = json!({
        "search": "context7",
        "page": 1,
        "sort": "newest",
        "reverse": false,
        "assetType": "mcp",
        "languages": [],
        "agentCompatibility": [],
        "sources": ["glama"],
        "rankFilters": {},
        "rankings": null,
        "mcpCategory": ["server"],
        "deployment": ["hybrid"],
        "registryType": ["npm"]
    });
    let mock = mount_mcp_post(&server, &expected_body, &mcp_response());
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &[
            "directory",
            "search",
            "context7",
            "--asset-type",
            "mcp",
            "--source",
            "glama",
            "--mcp-category",
            "server",
            "--deployment",
            "hybrid",
            "--registry-type",
            "npm",
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
}

#[test]
fn mcp_search_index_not_ready_is_distinct_from_no_results() {
    let server = MockServer::start();
    mount_mcp_post(
        &server,
        &default_mcp_search_body("context7"),
        &mcp_response_index_not_ready(),
    );
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &["directory", "search", "context7", "--asset-type", "mcp"],
        home.path(),
        &[
            ("VETTD_DIRECTORY_ENDPOINT", &ingest_endpoint(&server)),
            ("SEARCH_BETA_TESTING", "1"),
        ],
    );

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    let out = strip_ansi(&result.stdout);
    assert!(out.contains("indexReady=false"), "stdout: {out}");
    assert!(!out.contains("No MCP servers for"));
}

#[test]
fn inventory_mcp_search_is_rejected_client_side() {
    let server = MockServer::start();
    let post_inv = server.mock(|when, then| {
        when.method(POST).path("/api/inventory");
        then.status(200).json_body(inventory_response_basic());
    });
    let home = seed_home(&ingest_endpoint(&server), MOCK_API_KEY);

    let result = run_vettd(
        &["inventory", "search", "notes", "--asset-type", "mcp"],
        home.path(),
        &[
            ("VETTD_INVENTORY_ENDPOINT", &ingest_endpoint(&server)),
            ("SEARCH_BETA_TESTING", "1"),
        ],
    );

    assert_eq!(result.status, 1);
    assert!(result
        .stderr
        .contains("not supported for `inventory search`"));
    post_inv.assert_calls(0);
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
fn beta_search_without_json_prints_only_table() {
    let server = MockServer::start();
    mount_directory_post(
        &server,
        &default_search_body("pdf"),
        &directory_response_beta(),
    );
    let home = seed_home("http://127.0.0.1:1/api/scans/ingest", "");

    let result = run_vettd(
        &["directory", "search", "pdf"],
        home.path(),
        &[
            ("VETTD_DIRECTORY_ENDPOINT", &ingest_endpoint(&server)),
            ("SEARCH_BETA_TESTING", "1"),
        ],
    );

    assert_eq!(result.status, 0, "stderr: {}", result.stderr);
    let out = strip_ansi(&result.stdout);

    // No SEARCH_BETA_TESTING markers may leak into stdout — output is
    // driven by --json only, identical to the non-beta path.
    assert!(!out.contains("SEARCH_BETA_TESTING"));
    assert!(!out.contains("raw json"));
    assert!(!out.contains("formatted"));

    // Human table still renders.
    assert!(out.contains("[A]"));
    assert!(out.contains("Showing 1 of 1 assets"));
}

#[test]
fn beta_inventory_search_json_emits_only_json() {
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
    // No SEARCH_BETA_TESTING markers may leak into stdout — output is
    // driven by --json only, identical to the non-beta path.
    assert!(!out.contains("SEARCH_BETA_TESTING"));
    assert!(!out.contains("raw json"));
    assert!(!out.contains("formatted"));
    assert!(!out.contains("[B]"));
    // The whole stdout must be a single valid JSON document.
    let parsed: Value = serde_json::from_str(&out).expect("stdout must parse as JSON");
    assert_eq!(parsed, expected_inventory_output_basic());
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
            "assetType": "skill",
            "languages": ["python"],
            "agentCompatibility": ["claude-code"],
            "sources": [],
            "rankFilters": {},
            "rankings": {"stars": 50}
        }),
        &directory_response_beta(),
    );
    mount_mcp_post(
        &server,
        &default_mcp_search_body("context7"),
        &mcp_response(),
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
    eprintln!("  ./target/debug/vettd directory search pdf --language python --agent-compatibility claude-code --rankings '{{\"stars\": 50}}' --json");
    eprintln!("  ./target/debug/vettd directory search context7 --asset-type mcp --json\n");
    eprintln!(
        "  export VETTD_INVENTORY_ENDPOINT={ingest}   # inventory also needs the `auth` step above for its api key"
    );
    eprintln!("  ./target/debug/vettd inventory search notes --json\n");
    eprintln!("Blocking for 10 minutes — Ctrl-C to stop early.\n");

    std::thread::sleep(std::time::Duration::from_secs(600));
}
