//! Directory command implementations.
//!
//! All reads go through `crate::read_client` (no `Authorization` header).
//! Deserialization uses narrow allow-list structs — unknown fields are ignored,
//! so any server over-exposure is silently dropped rather than printed.

use serde::{Deserialize, Serialize};

use crate::freshness::{self, PublicFreshness};
use crate::read_client::{self, ReadError};

// ── ANSI helpers (mirrors formatters.rs palette) ──────────────────────────
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";

fn grade_color(grade: &str) -> &'static str {
    match grade {
        "A" => "\x1b[32m",       // green
        "B" => "\x1b[34m",       // blue
        "C" => "\x1b[33m",       // yellow
        "D" | "F" => "\x1b[31m", // red
        _ => "\x1b[2m",          // dim for unknown
    }
}

fn severity_color(sev_lower: &str) -> &'static str {
    match sev_lower {
        "critical" => "\x1b[1;35m", // bold magenta
        "high" => "\x1b[31m",       // red
        "medium" => "\x1b[33m",     // yellow
        "low" => "\x1b[36m",        // cyan
        _ => "\x1b[2m",             // dim (info / unknown)
    }
}

// ---------------------------------------------------------------------------
// Allow-list deserialization structs
//
// Fields here are limited to what we actually render. Any field the server
// returns that isn't listed is silently ignored (serde default = deny on
// unknown_fields is NOT set — that's intentional for forward compatibility).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListResponse {
    pub skills: Vec<DirectoryCard>,
    pub total: u32,
    pub page: u32,
    pub total_pages: u32,
    /// Set on `SEARCH_BETA_TESTING` search responses to flag mock vs. real
    /// data (`SEARCH_BETA_MOCK_DATA` on the server). Skipped on serialize
    /// when absent, matching `language` below.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mock: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryCard {
    pub slug: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub category: Option<String>,
    pub badge_status: Option<String>,
    pub overall_grade: Option<String>,
    pub source_type: Option<String>,
    pub scanner_run_count: Option<u32>,
    /// Compact per-category signal summary (vettd#981). Present on directory
    /// list/search responses that carry signal data; skipped on serialize
    /// when absent so `--json` output stays byte-identical to the pre-signal
    /// shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_categories: Option<Vec<SignalCategorySummary>>,
    /// Total signal count for the asset (vettd#879). Present on directory
    /// list/search responses once the server sends it; `None` against an
    /// older server. Skipped on serialize when absent so `--json` output
    /// stays byte-identical to the pre-signal-count shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_count: Option<u32>,
    /// Present only from `SEARCH_BETA_TESTING` search responses. Skipped on
    /// serialize when absent, so `--json` output is byte-identical to the
    /// pre-beta shape unless the server actually sent this field.
    ///
    /// The server field is `docLanguage` (named that way deliberately to
    /// avoid implying a programming language — see openapi.ts). The alias
    /// keeps accepting the older `language` key too.
    #[serde(skip_serializing_if = "Option::is_none", alias = "docLanguage")]
    pub language: Option<String>,
    /// Present only from `SEARCH_BETA_TESTING` search responses. Skipped on
    /// serialize when absent — see `language` above.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_compatibility: Option<Vec<String>>,
    /// Present only from `SEARCH_BETA_TESTING` search responses. Skipped on
    /// serialize when absent — see `language` above.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rankings: Option<SkillRankings>,
    /// Non-deterministic LLM threat-scan verdict for the matched catalog
    /// entry's SKILL.md. Opaque passthrough — the query service owns this
    /// shape and it churns, so it is *not* modelled as a typed struct.
    /// `object | null` on the wire, snake_case key. Present only on
    /// `SEARCH_BETA_TESTING` (`assetType: "skill"`) search responses;
    /// skipped on serialize when absent so non-beta `--json` stays
    /// byte-identical. See `docs/SEARCH_INTERFACE.md`.
    #[serde(rename = "llm_scan", skip_serializing_if = "Option::is_none")]
    pub llm_scan: Option<serde_json::Value>,
    /// OSV-backed security-history grade for the CLI tools the matched
    /// catalog entry installs. Opaque passthrough — see `llm_scan`.
    #[serde(rename = "cli_security", skip_serializing_if = "Option::is_none")]
    pub cli_security: Option<serde_json::Value>,
    /// Deterministic Vettd scan rollup for the matched catalog entry's repo.
    /// Opaque passthrough — see `llm_scan`.
    #[serde(rename = "vettd_scan", skip_serializing_if = "Option::is_none")]
    pub vettd_scan: Option<serde_json::Value>,
    /// Slice 2 freshness field (public directory only). `None` when no
    /// freshness row exists on the server (asset never verified against
    /// upstream). Omitted on serialize when absent (`skip_serializing_if`) so
    /// the JSON shape stays identical to the pre-slice output — no synthesized
    /// `freshness: null`. Forward-compatible: unknown fields on the wire are
    /// silently dropped. Inventory reuses this struct unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<PublicFreshness>,
}

/// External ranking signals for a skill — used both as the `--rankings`
/// input filter (minimum thresholds) and as the actual values on a
/// `SEARCH_BETA_TESTING` search response. See `docs/SEARCH_INTERFACE.md`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRankings {
    pub stars: Option<u32>,
    pub skills_sh_leaderboard_rank: Option<u32>,
    pub number_of_aggregators: Option<u32>,
    pub official_claude_marketplace: Option<bool>,
}

/// Response envelope for a `SEARCH_BETA_TESTING` search with
/// `assetType: "mcp"`. Discriminated from `DirectoryListResponse` by the
/// `mcpServers` key (vs. `skills`). Thin fail-open proxy of the query
/// service's `mcp_servers` catalog — see `docs/SEARCH_INTERFACE.md`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpListResponse {
    pub mcp_servers: Vec<McpCard>,
    pub total: u32,
    pub page: u32,
    pub total_pages: u32,
    /// Always `false` on this path (nothing to fabricate for an external
    /// catalog) but accepted for forward-compat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mock: Option<bool>,
    /// `false` when the `mcp_servers` collection is empty/absent or the
    /// query service is unreachable — an onboarding/outage state, *not*
    /// "no results for this query". Surfaced distinctly by the CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_ready: Option<bool>,
}

/// Allow-list deserialization for one `mcp_servers` catalog hit. Fields
/// mirror the query service's `McpHit` (`ah-skills .../openapi.json`) —
/// snake_case passthrough, every field `Option` for forward-compat. The
/// `security_*` / `security_direct_deps_*` block is the OSV dependency
/// vulnerability scan. Unknown fields are ignored.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct McpCard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_category_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_installable_package: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_remote: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stars: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly_downloads: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_downloads: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_vuln_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_vuln_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_max_severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_direct_deps_scanned: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_direct_deps_vuln_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_direct_deps_with_vulns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_direct_deps_max_severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_direct_deps_vuln_ids: Option<Vec<String>>,
}

/// The raw filter inputs for a `SEARCH_BETA_TESTING` search, assembled by
/// `cli.rs` from the parsed flags and shared verbatim by the directory and
/// inventory search handlers.
#[derive(Debug, Default, Clone)]
pub struct SearchFilters {
    /// `"skill"` (default) or `"mcp"`. `cli.rs` constrains the values.
    pub asset_type: String,
    pub languages: Vec<String>,
    pub agent_compatibility: Vec<String>,
    pub sources: Vec<String>,
    /// Raw `key=N` strings from `--rank-filter`; parsed by
    /// [`parse_rank_filters`].
    pub rank_filters: Vec<String>,
    pub mcp_category: Vec<String>,
    pub deployment: Vec<String>,
    pub registry_type: Vec<String>,
    /// Raw JSON from `--rankings`.
    pub rankings: Option<String>,
}

/// Parsed / validated filters ready for [`build_search_body`].
#[derive(Debug, Default)]
pub struct ValidatedFilters {
    /// Parsed `--rankings` JSON, or `None` if the flag was absent.
    pub rankings: Option<serde_json::Value>,
    /// Parsed `--rank-filter` map (`{key: N}`); empty if none given.
    pub rank_filters: serde_json::Map<String, serde_json::Value>,
}

/// Parse repeated `--rank-filter key=N` flags into a `{key: N}` map.
///
/// Returns `Err(message)` for a missing `=`, an empty key, or a
/// non-integer value — the caller turns that into an exit-1 CLI error
/// before any request is sent. Pure (no `process::exit`) so it is unit
/// testable.
pub(crate) fn parse_rank_filters(
    raw: &[String],
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut map = serde_json::Map::new();
    for entry in raw {
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| format!("--rank-filter '{entry}' must be in key=N form"))?;
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("--rank-filter '{entry}' has an empty key"));
        }
        let n: i64 = value
            .trim()
            .parse()
            .map_err(|_| format!("--rank-filter '{entry}' value must be an integer"))?;
        map.insert(key.to_string(), serde_json::json!(n));
    }
    Ok(map)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySkillDetail {
    /// The SkillAudit PK — used as the signals endpoint's `subjectId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub slug: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub category: Option<String>,
    pub overall_grade: Option<String>,
    pub license: Option<String>,
    pub source_type: Option<String>,
    pub source_url: Option<String>,
    pub has_skill_md: Option<bool>,
    pub has_scripts: Option<bool>,
    pub has_evals: Option<bool>,
    pub file_count: Option<u32>,
    pub completed_at: Option<String>,
    pub findings: Vec<DirectoryFinding>,
    pub scanner_runs: Vec<ScannerRun>,
    /// Compact per-category signal summary on the detail payload (vettd#981).
    /// Skipped on serialize when absent so `--json` stays byte-identical to
    /// the pre-signal shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_categories: Option<Vec<SignalCategorySummary>>,
    /// Total signal count for the asset (vettd#879). `None` against an older
    /// server. Skipped on serialize when absent so `--json` stays
    /// byte-identical to the pre-signal-count shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_count: Option<u32>,
    /// Slice 2 freshness field (public directory view only). `None` when no
    /// freshness row exists on the server. Omitted on serialize when absent
    /// (`skip_serializing_if`) so JSON shape stays lossless: fields received
    /// are forwarded, synthesized `freshness: null` is not added when the
    /// server omitted the field. Inventory view/compare reuse this unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<PublicFreshness>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryFinding {
    pub severity: String,
    pub rule_id: Option<String>,
    pub category: Option<String>,
    pub label: String,
    pub detail: Option<String>,
    pub source: Option<String>,
    pub filepath: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannerRun {
    pub source: String,
    pub status: String,
    pub verdict: Option<String>,
    pub grade: Option<String>,
    pub finding_count: Option<i32>,
    pub critical_count: Option<i32>,
    pub high_count: Option<i32>,
}

// ---------------------------------------------------------------------------
// Signal category summaries (vettd#981)
//
// The directory API returns a compact `signalCategories` summary on cards,
// detail payloads, and compare entries. These mirror the server's
// `CategorySummary` / `SignalEnvelopeRow` shapes (vettd
// `packages/api/src/signals/verdicts.ts` + `types.ts`) with all fields
// optional and camelCase, so an unknown or absent value degrades to a plain
// display rather than a decode failure. Display-only — never fed into the
// local grade or verdict logic.
// ---------------------------------------------------------------------------

/// One category's verdict. The server union is
/// `null | {form:"graded"; grade:string} | {form:"measured"; magnitudes:[…]} |
/// {form:"unjudged"}`; modelled as a flat all-optional struct so any future
/// form still decodes (`form` is the discriminator, the other fields are the
/// payload of whichever form is present).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalCategoryVerdict {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magnitudes: Option<Vec<SignalCategoryMagnitude>>,
}

/// One magnitude behind a `measured` verdict.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalCategoryMagnitude {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

/// One normalized envelope row (findings / signals / coverage projected into
/// one shape). Mirrors `SignalEnvelopeRow` with all fields optional.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalEnvelopeRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_num: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_party: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// Compact per-category summary over a skill's signal envelope.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalCategorySummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<SignalCategoryVerdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<SignalEnvelopeRow>>,
}

/// Response envelope for the public signals read
/// (`GET /api/assets/skill_audit/{id}/signals`).
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSignalsResponse {
    pub subject_type: Option<String>,
    pub subject_id: Option<String>,
    pub signals: Vec<SignalEnvelopeRow>,
    pub categories: Vec<SignalCategorySummary>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the effective ingest endpoint for directory reads.
///
/// Priority: `VETTD_DIRECTORY_ENDPOINT` (honored only when
/// `SEARCH_BETA_TESTING` is enabled, see
/// [`crate::network::search_beta_testing_enabled`]) → auth-config endpoint →
/// [`crate::submit::DEFAULT_PRODUCTION_ENDPOINT`].
///
/// Shared by every directory surface (list, detail, signals, compare) so the
/// same base is used across the whole command, including the signals fetch.
fn directory_endpoint() -> String {
    let override_endpoint = crate::network::search_beta_testing_enabled()
        .then(|| std::env::var("VETTD_DIRECTORY_ENDPOINT").ok())
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    override_endpoint
        .or_else(|| crate::submit::load_auth_config().map(|c| c.endpoint))
        .unwrap_or_else(|| crate::submit::DEFAULT_PRODUCTION_ENDPOINT.to_string())
}

/// Derive the directory API base URL from the effective ingest endpoint.
pub(crate) fn directory_base_url() -> String {
    crate::network::derive_api_url(&directory_endpoint(), "directory")
}

/// Percent-encode a query parameter value (UTF-8, RFC 3986 unreserved chars
/// pass through; everything else is `%XX` encoded).
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Normalize source type identifiers to display-friendly labels.
fn display_source_type(s: &str) -> &str {
    match s {
        "scan" => "cli",
        "zip" => "upload",
        other => other,
    }
}

/// Numeric value for a severity string (higher = more severe).
pub(crate) fn severity_value(s: &str) -> u8 {
    match s.to_ascii_lowercase().as_str() {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0, // "info" and anything unrecognised
    }
}

/// Count findings by severity level, returning (critical, high, medium, low, info).
fn count_by_severity(findings: &[DirectoryFinding]) -> (usize, usize, usize, usize, usize) {
    let mut critical = 0usize;
    let mut high = 0usize;
    let mut medium = 0usize;
    let mut low = 0usize;
    let mut info = 0usize;
    for f in findings {
        match f.severity.to_ascii_lowercase().as_str() {
            "critical" => critical += 1,
            "high" => high += 1,
            "medium" => medium += 1,
            "low" => low += 1,
            _ => info += 1,
        }
    }
    (critical, high, medium, low, info)
}

/// Number of distinct successful external scanners (source != "vettd", status == "success").
fn external_scanner_run_count(runs: &[ScannerRun]) -> usize {
    use std::collections::HashSet;
    runs.iter()
        .filter(|r| r.source != "vettd" && r.status == "success")
        .map(|r| r.source.as_str())
        .collect::<HashSet<_>>()
        .len()
}

/// Format a severity breakdown as a plain string (for truncation in compare).
fn fmt_severity_breakdown(c: usize, h: usize, m: usize, l: usize, i: usize) -> String {
    let mut parts = Vec::new();
    if c > 0 {
        parts.push(format!("{c} critical"));
    }
    if h > 0 {
        parts.push(format!("{h} high"));
    }
    if m > 0 {
        parts.push(format!("{m} medium"));
    }
    if l > 0 {
        parts.push(format!("{l} low"));
    }
    if i > 0 {
        parts.push(format!("{i} info"));
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

/// Format a severity breakdown with ANSI color per level.
fn fmt_severity_breakdown_colored(c: usize, h: usize, m: usize, l: usize, i: usize) -> String {
    let mut parts = Vec::new();
    if c > 0 {
        parts.push(format!("\x1b[1;35m{c} critical{RESET}"));
    }
    if h > 0 {
        parts.push(format!("\x1b[31m{h} high{RESET}"));
    }
    if m > 0 {
        parts.push(format!("\x1b[33m{m} medium{RESET}"));
    }
    if l > 0 {
        parts.push(format!("\x1b[36m{l} low{RESET}"));
    }
    if i > 0 {
        parts.push(format!("{DIM}{i} info{RESET}"));
    }
    if parts.is_empty() {
        format!("{DIM}none{RESET}")
    } else {
        parts.join(", ")
    }
}

/// Fetch a single skill detail, mapping errors to clear exit messages.
fn fetch_skill(slug: &str) -> DirectorySkillDetail {
    match fetch_skill_result(slug) {
        Ok(detail) => detail,
        Err(ReadError::NotFound) => exit_detail_not_found(slug),
        Err(ReadError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd directory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error fetching skill '{slug}': {e}");
            std::process::exit(1);
        }
    }
}

/// The anonymous directory detail GET — `Result`-returning so the transport
/// contract (no `Authorization` header, distinct 404) is unit-testable against
/// a mock server without exiting the process.
fn fetch_skill_result(slug: &str) -> Result<DirectorySkillDetail, ReadError> {
    fetch_skill_url(&skill_detail_url(slug))
}

/// The anonymous directory detail GET at an explicit URL.
fn fetch_skill_url(url: &str) -> Result<DirectorySkillDetail, ReadError> {
    read_client::fetch_json::<DirectorySkillDetail>(url)
}

/// Build the directory detail URL for `slug`.
fn skill_detail_url(slug: &str) -> String {
    format!("{}/{}", directory_base_url(), percent_encode(slug))
}

/// Distinct 404 exit for the detail route — `directory signals` first fetches
/// the detail, so this must not be conflated with a missing signal record.
fn exit_detail_not_found(slug: &str) -> ! {
    eprintln!("{}", skill_not_found_message(slug));
    std::process::exit(1);
}

/// The detail-route 404 message, as a value so tests can assert it is distinct
/// from the signals-route 404 message.
fn skill_not_found_message(slug: &str) -> String {
    format!("Error: skill '{slug}' not found (not public or does not exist).")
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

pub(crate) fn api_sort_params(sort: &str, reverse: bool) -> String {
    let s = match sort {
        "rating" => "verdict",
        other => other,
    };
    let default_asc = sort == "alpha";
    let dir = if default_asc ^ reverse { "asc" } else { "desc" };
    format!("sort={s}&dir={dir}")
}

pub fn handle_list(page: u32, sort: &str, reverse: bool, json: bool) {
    let url = format!(
        "{}?{}&page={page}",
        directory_base_url(),
        api_sort_params(sort, reverse)
    );
    match read_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                print_cards(&resp.skills, true);
                let shown = resp.skills.len();
                if resp.page < resp.total_pages {
                    println!(
                        "\n{DIM}Showing {} of {} assets — use --page {} to see more.{RESET}",
                        shown,
                        resp.total,
                        resp.page + 1,
                    );
                } else {
                    println!("\n{DIM}Showing {} of {} assets.{RESET}", shown, resp.total);
                }
            }
        }
        Err(ReadError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd directory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

/// Whether any beta-gated search filter is in use — anything beyond a plain
/// query/page/sort. `assetType: "mcp"` counts (the MCP catalog is a
/// beta-only surface).
fn any_filter_set(f: &SearchFilters) -> bool {
    f.asset_type == "mcp"
        || !f.languages.is_empty()
        || !f.agent_compatibility.is_empty()
        || !f.sources.is_empty()
        || !f.rank_filters.is_empty()
        || !f.mcp_category.is_empty()
        || !f.deployment.is_empty()
        || !f.registry_type.is_empty()
        || f.rankings.is_some()
}

/// Validate the beta-gated search filters and parse `--rankings` /
/// `--rank-filter`.
///
/// Exits the process with a clear error if any filter (or
/// `--asset-type mcp`) is supplied without `SEARCH_BETA_TESTING` enabled,
/// if `--rankings` isn't valid JSON, or if a `--rank-filter` is malformed.
/// See `docs/SEARCH_INTERFACE.md`.
pub(crate) fn validate_search_filters(f: &SearchFilters, beta: bool) -> ValidatedFilters {
    if any_filter_set(f) && !beta {
        eprintln!(
            "Error: search filters (--language/--agent-compatibility/--rankings/--source/\
--rank-filter/--asset-type mcp/--mcp-category/--deployment/--registry-type) require \
SEARCH_BETA_TESTING=1."
        );
        std::process::exit(1);
    }
    let rankings = f
        .rankings
        .as_deref()
        .map(|raw| match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Error: --rankings is not valid JSON: {e}");
                std::process::exit(1);
            }
        });
    let rank_filters = match parse_rank_filters(&f.rank_filters) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("Error: {msg}");
            std::process::exit(1);
        }
    };
    ValidatedFilters {
        rankings,
        rank_filters,
    }
}

/// Build the POST body for a `SEARCH_BETA_TESTING` search request. See
/// `docs/SEARCH_INTERFACE.md` for the shape.
///
/// `assetType`, `languages`, `agentCompatibility`, `sources` and
/// `rankFilters` are always present (arrays empty / object empty when
/// unset); `rankings` is `null` when `--rankings` was absent. The
/// mcp-only `mcpCategory` / `deployment` / `registryType` arrays are
/// included only when `assetType == "mcp"`.
pub(crate) fn build_search_body(
    query: &str,
    page: u32,
    sort: &str,
    reverse: bool,
    filters: &SearchFilters,
    validated: &ValidatedFilters,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "search": query,
        "page": page,
        "sort": sort,
        "reverse": reverse,
        "assetType": filters.asset_type,
        "languages": filters.languages,
        "agentCompatibility": filters.agent_compatibility,
        "sources": filters.sources,
        "rankFilters": validated.rank_filters,
        "rankings": validated.rankings,
    });
    if filters.asset_type == "mcp" {
        let obj = body.as_object_mut().expect("json object");
        obj.insert(
            "mcpCategory".into(),
            serde_json::json!(filters.mcp_category),
        );
        obj.insert("deployment".into(), serde_json::json!(filters.deployment));
        obj.insert(
            "registryType".into(),
            serde_json::json!(filters.registry_type),
        );
    }
    body
}

pub fn handle_search(
    query: &str,
    page: u32,
    sort: &str,
    reverse: bool,
    json: bool,
    filters: &SearchFilters,
) {
    let beta = crate::network::search_beta_testing_enabled();
    let validated = validate_search_filters(filters, beta);

    // The MCP catalog is a beta-only surface with a different response
    // envelope — `validate_search_filters` already exited if it was
    // requested without the beta flag, so here `beta` is implied.
    if filters.asset_type == "mcp" {
        return handle_mcp_search(query, page, sort, reverse, json, filters, &validated);
    }

    let result = if beta {
        let body = build_search_body(query, page, sort, reverse, filters, &validated);
        read_client::post_json::<DirectoryListResponse>(&directory_base_url(), &body)
    } else {
        let url = format!(
            "{}?search={}&{}&page={page}",
            directory_base_url(),
            percent_encode(query),
            api_sort_params(sort, reverse),
        );
        read_client::fetch_json::<DirectoryListResponse>(&url)
    };

    match result {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else if resp.skills.is_empty() {
                println!("No results for \"{}\".", query);
            } else {
                print_cards(&resp.skills, true);
                let shown = resp.skills.len();
                if resp.page < resp.total_pages {
                    println!(
                        "\n{DIM}Showing {} of {} assets for \"{}\" — use --page {} to see more.{RESET}",
                        shown,
                        resp.total,
                        query,
                        resp.page + 1,
                    );
                } else {
                    println!(
                        "\n{DIM}Showing {} of {} assets for \"{}\".{RESET}",
                        shown, resp.total, query,
                    );
                }
            }
        }
        Err(ReadError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd directory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

/// `directory search --asset-type mcp` — always beta, always `POST`, and a
/// different response envelope (`mcpServers`, not `skills`). `--json` prints
/// the raw response; otherwise a compact table. `indexReady: false` is
/// reported distinctly from "no results".
fn handle_mcp_search(
    query: &str,
    page: u32,
    sort: &str,
    reverse: bool,
    json: bool,
    filters: &SearchFilters,
    validated: &ValidatedFilters,
) {
    let body = build_search_body(query, page, sort, reverse, filters, validated);
    let result = read_client::post_json::<McpListResponse>(&directory_base_url(), &body);

    match result {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else if resp.mcp_servers.is_empty() {
                if resp.index_ready == Some(false) {
                    println!(
                        "The MCP catalog is not ready yet (indexReady=false) — this is an \
onboarding/outage state, not an empty result. Try again shortly."
                    );
                } else {
                    println!("No MCP servers for \"{}\".", query);
                }
            } else {
                print_mcp_cards(&resp.mcp_servers);
                let shown = resp.mcp_servers.len();
                if resp.page < resp.total_pages {
                    println!(
                        "\n{DIM}Showing {} of {} MCP servers for \"{}\" — use --page {} to see more.{RESET}",
                        shown,
                        resp.total,
                        query,
                        resp.page + 1,
                    );
                } else {
                    println!(
                        "\n{DIM}Showing {} of {} MCP servers for \"{}\".{RESET}",
                        shown, resp.total, query,
                    );
                }
            }
        }
        Err(ReadError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd directory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

/// Compact one-line-per-server table for MCP search results. Deliberately
/// minimal — the raw JSON dump carries the full `McpHit` shape (OSV block
/// included). Columns: id, category, registry, stars, dep-vuln count.
fn print_mcp_cards(cards: &[McpCard]) {
    let id_w = cards
        .iter()
        .map(|c| {
            c.mcp_id
                .as_deref()
                .or(c.name.as_deref())
                .unwrap_or("—")
                .len()
        })
        .max()
        .unwrap_or(0)
        .max(2);
    let term_w = terminal_width();

    println!(
        "{BOLD}{:<w$}  {:<10}  {:<9}  {:>7}  {:>9}  description{RESET}",
        "mcp",
        "category",
        "registry",
        "stars",
        "dep vulns",
        w = id_w,
    );
    println!("{DIM}{}{RESET}", "─".repeat(term_w.saturating_sub(5)));

    for card in cards {
        let id = card
            .mcp_id
            .as_deref()
            .or(card.name.as_deref())
            .unwrap_or("—");
        let category = card.mcp_category.as_deref().unwrap_or("—");
        let registry = card.registry_type.as_deref().unwrap_or("—");
        let stars = card
            .stars
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string());
        let dep_vulns = card
            .security_direct_deps_vuln_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string());
        let desc = card.description.as_deref().unwrap_or("");

        let visual_prefix_w = id_w + 2 + 10 + 2 + 9 + 2 + 7 + 2 + 9 + 2;
        let desc_budget = term_w.saturating_sub(visual_prefix_w).saturating_sub(5);
        let desc_display = truncate_to_display(desc, desc_budget);
        println!(
            "{id:<id_w$}  {category:<10}  {registry:<9}  {stars:>7}  {dep_vulns:>9}  {DIM}{desc_display}{RESET}"
        );
    }
}

pub fn handle_view(slug: &str, json: bool) {
    let detail = fetch_skill(slug);
    if json {
        let mut val = serde_json::to_value(&detail).unwrap_or_default();
        if let Some(obj) = val.as_object_mut() {
            obj.remove("findings");
        }
        println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
        return;
    }
    // Full 7-category signal envelope, fetched from the public signals
    // endpoint using the audit `id`; any failure (incl. 404 — no published
    // signal record) degrades to no rows — view must not error on it.
    let signals = signals_for_detail(detail.id.as_deref());
    // One label width for every row — the signals block carries the longest
    // labels printed (17 today — `Unresolvable refs`; the longest fixed row
    // label, `Last scanned:`, is 13), so it drives the column.
    let label_w = signals_table_label_w();
    let (c, h, m, l, i) = count_by_severity(&detail.findings);

    let mut scanned_by: Vec<&str> = detail
        .scanner_runs
        .iter()
        .filter(|r| r.status == "success")
        .map(|r| r.source.as_str())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    scanned_by.sort_unstable();
    let scanned_by_str = if scanned_by.is_empty() {
        "—".to_string()
    } else {
        scanned_by.join(", ")
    };

    let last_scanned = detail
        .completed_at
        .as_deref()
        .and_then(|s| s.get(..10))
        .unwrap_or("—");

    let source = detail
        .source_type
        .as_deref()
        .map(display_source_type)
        .unwrap_or("—");

    let mut contains: Vec<&str> = Vec::new();
    if detail.has_skill_md.unwrap_or(false) {
        contains.push("skill.md");
    }
    if detail.has_scripts.unwrap_or(false) {
        contains.push("scripts");
    }
    if detail.has_evals.unwrap_or(false) {
        contains.push("evals");
    }
    let contains_str = if contains.is_empty() {
        "—".to_string()
    } else {
        contains.join(", ")
    };

    let display_slug = detail.slug.as_deref().unwrap_or(slug);
    let grade_str = detail.overall_grade.as_deref().unwrap_or("—");
    let gc = grade_color(grade_str);

    println!("{BOLD}{}{RESET}", detail.name);
    if let Some(desc) = &detail.description {
        println!("  {desc}");
    }
    println!();
    println!(
        "  {DIM}{:<label_w$}{RESET}  {gc}{}{RESET}",
        "Grade:",
        grade_str,
        label_w = label_w
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Version:",
        detail.version.as_deref().unwrap_or("—"),
        label_w = label_w
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "License:",
        detail.license.as_deref().unwrap_or("—"),
        label_w = label_w
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Author:",
        detail.author.as_deref().unwrap_or("—"),
        label_w = label_w
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Category:",
        detail.category.as_deref().unwrap_or("—"),
        label_w = label_w
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Source:",
        source,
        label_w = label_w
    );
    if let Some(url) = &detail.source_url {
        if !url.is_empty() {
            println!(
                "  {DIM}{:<label_w$}{RESET}  {}",
                "Source URL:",
                url,
                label_w = label_w
            );
        }
    }
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Contains:",
        contains_str,
        label_w = label_w
    );
    println!();
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Findings:",
        fmt_severity_breakdown_colored(c, h, m, l, i),
        label_w = label_w
    );
    // Full signals block — same selection and value precedence as compare,
    // rendered as a single column (see `render_signals_block`).
    print!(
        "{}",
        render_signals_block(&signals.signals, None, detail.signal_count, None, label_w)
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Scanned by:",
        scanned_by_str,
        label_w = label_w
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Last scanned:",
        last_scanned,
        label_w = label_w
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}",
        "Files:",
        detail
            .file_count
            .map(|n| n.to_string())
            .as_deref()
            .unwrap_or("—"),
        label_w = label_w
    );
    // Slice 2 freshness detail.
    for line in freshness::fmt_freshness_detail(&detail.freshness.as_ref()) {
        println!("  {line}");
    }
    println!();
    println!("  {DIM}Run `vettd directory findings {display_slug}` to see finding details.{RESET}");
}

pub fn handle_findings(slug: &str, min_severity: &str, json: bool) {
    let detail = fetch_skill(slug);
    let min_val = severity_value(min_severity);

    let filtered: Vec<&DirectoryFinding> = detail
        .findings
        .iter()
        .filter(|f| severity_value(&f.severity) >= min_val)
        .collect();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&filtered).unwrap_or_default()
        );
        return;
    }

    let total = detail.findings.len();
    let shown = filtered.len();

    println!(
        "{BOLD}Findings for {}{RESET}  {DIM}(--min-severity {min_severity}){RESET}",
        detail.name
    );
    println!();

    if filtered.is_empty() {
        println!("  {DIM}No findings at or above the '{min_severity}' severity threshold.{RESET}");
    } else {
        for f in &filtered {
            let rule = f.rule_id.as_deref().unwrap_or("—");
            let src = f.source.as_deref().unwrap_or("—");
            let sc = severity_color(&f.severity.to_ascii_lowercase());
            println!(
                "  {sc}[{}]{RESET}  {BOLD}{}{RESET}  {DIM}({rule}){RESET}",
                f.severity.to_uppercase(),
                f.label
            );
            if let Some(cat) = &f.category {
                println!("       {DIM}Category:{RESET} {cat}  {DIM}|  Source:{RESET} {src}");
            } else {
                println!("       {DIM}Source:{RESET} {src}");
            }
            if let Some(detail_text) = &f.detail {
                println!("       {detail_text}");
            }
            println!();
        }
        println!("  {DIM}Showing {shown}/{total} findings (filter: >= {min_severity}).{RESET}");
    }
}

/// Fetch and render the public signal record for a skill
/// (`GET /api/assets/skill_audit/{id}/signals`). The audit `id` is read from
/// the directory detail payload, and the request itself is anonymous via
/// [`read_client`] — never sets `Authorization`.
pub fn handle_signals(slug: &str, json: bool) {
    let detail = fetch_skill(slug);
    let id = match detail.id.as_deref() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            eprintln!("Error: skill '{slug}' has no audit id on the directory record.");
            std::process::exit(1);
        }
    };

    match fetch_signals(&id) {
        Ok(raw) => {
            if json {
                // Print the raw endpoint payload. Re-serializing the typed
                // allow-list view would drop neutral `null`s and unknown
                // fields (`skip_serializing_if`), so `--json` must forward the
                // fetched Value verbatim.
                println!("{}", render_signals_json(&raw));
                return;
            }
            let resp: SkillSignalsResponse = match serde_json::from_value(raw) {
                Ok(resp) => resp,
                Err(e) => {
                    eprintln!("Error decoding signals for skill '{slug}': {e}");
                    std::process::exit(1);
                }
            };
            print_signal_categories(&resp, &detail.name, slug);
        }
        Err(ReadError::NotFound) => exit_signals_not_found(slug),
        Err(ReadError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd directory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error fetching signals for skill '{slug}': {e}");
            std::process::exit(1);
        }
    }
}

/// Fetch the raw public signals payload for an audit id. Returns the JSON
/// Value untouched so `--json` can print it losslessly (neutral `null`s and
/// unknown fields survive); the human path deserializes a copy instead.
fn fetch_signals(detail_id: &str) -> Result<serde_json::Value, ReadError> {
    let url = crate::network::derive_api_url(
        &directory_endpoint(),
        &format!("assets/skill_audit/{detail_id}/signals"),
    );
    fetch_signals_url(&url)
}

/// Fetch and decode the signals envelope for one audit id.
///
/// `Result`-returning so callers can map any [`ReadError`] (including a 404 —
/// no published signal record) to "no signal rows" without exiting, unlike
/// [`handle_signals`] which exits on the same errors.
fn fetch_signals_envelope(detail_id: &str) -> Result<SkillSignalsResponse, ReadError> {
    let raw = fetch_signals(detail_id)?;
    serde_json::from_value(raw).map_err(|e| ReadError::Decode(e.to_string()))
}

/// Full signal envelope for a directory detail record, or no rows when the
/// record carries no audit id or the signals read fails (including 404 — no
/// published signal record). Non-exiting: `directory view` and
/// `directory compare` must not error out because signals are unavailable.
fn signals_for_detail(detail_id: Option<&str>) -> SkillSignalsResponse {
    match detail_id {
        Some(id) if !id.is_empty() => fetch_signals_envelope(id).unwrap_or_default(),
        _ => SkillSignalsResponse::default(),
    }
}

/// The anonymous signals GET at an explicit URL — `Result`-returning so the
/// transport contract (no `Authorization` header, distinct 404) is
/// unit-testable against a mock server without exiting the process.
fn fetch_signals_url(url: &str) -> Result<serde_json::Value, ReadError> {
    read_client::fetch_json::<serde_json::Value>(url)
}

/// Pretty-printed JSON passthrough for `directory signals --json`.
fn render_signals_json(raw: &serde_json::Value) -> String {
    serde_json::to_string_pretty(raw).unwrap_or_default()
}

/// Distinct 404 exit for the signals route — a missing published signal
/// record must not be conflated with a missing skill.
fn exit_signals_not_found(slug: &str) -> ! {
    eprintln!("{}", signals_not_found_message(slug));
    std::process::exit(1);
}

/// The signals-route 404 message, as a value so tests can assert it is
/// distinct from the detail-route 404 message.
fn signals_not_found_message(slug: &str) -> String {
    format!("Error: no published signal record for skill '{slug}'.")
}

/// Render a [`SkillSignalsResponse`] in human form: the categories in server
/// order (all seven catalog categories, including empty ones) with their
/// verdict form and row count, then each category's rows.
fn print_signal_categories(resp: &SkillSignalsResponse, name: &str, slug: &str) {
    println!("{BOLD}Signals for {name}{RESET}  {DIM}({slug}){RESET}");
    println!();

    if resp.categories.is_empty() {
        println!("  {DIM}No published signal categories for this skill.{RESET}");
        return;
    }

    let subject = match (&resp.subject_type, &resp.subject_id) {
        (Some(t), Some(id)) => format!("{t} {id}"),
        (Some(t), None) => t.clone(),
        (None, Some(id)) => id.clone(),
        (None, None) => "—".to_string(),
    };
    println!("  {DIM}Subject:{RESET} {subject}");
    println!();

    for cat in &resp.categories {
        let label = cat.label.as_deref().unwrap_or("—");
        let form = cat.form.as_deref().unwrap_or("?");
        let rows = cat.rows.as_ref().map(|r| r.len()).unwrap_or(0);
        let verdict_s = fmt_signal_verdict(&cat.verdict);
        println!(
            "  {BOLD}{label}{RESET}  {DIM}{form}{RESET}  {rows} row{}  {verdict_s}",
            if rows == 1 { "" } else { "s" }
        );
        if let Some(cat_rows) = &cat.rows {
            for row in cat_rows {
                println!("    {}", fmt_signal_row(row));
            }
        }
        println!();
    }

    println!("  {DIM}Run `vettd directory signals {slug} --json` for the raw payload.{RESET}");
}

/// Compact verdict token for a category: `verdict: <grade>` for graded,
/// `verdict: measured (N magnitude(s))` for measured, `unjudged` for
/// unjudged, `no verdict` when null (absence is not a verdict).
fn fmt_signal_verdict(verdict: &Option<SignalCategoryVerdict>) -> String {
    match verdict {
        None => format!("{DIM}no verdict{RESET}"),
        Some(v) => match v.form.as_deref() {
            Some("graded") => match &v.grade {
                Some(g) => format!("verdict: {g}"),
                None => format!("{DIM}graded (no grade){RESET}"),
            },
            Some("measured") => {
                let n = v.magnitudes.as_ref().map(|m| m.len()).unwrap_or(0);
                format!(
                    "verdict: measured ({} magnitude{})",
                    n,
                    if n == 1 { "" } else { "s" }
                )
            }
            Some("unjudged") => format!("{DIM}unjudged{RESET}"),
            Some(other) => format!("verdict: {other}"),
            None => format!("{DIM}no verdict form{RESET}"),
        },
    }
}

/// One compact line for a single envelope row.
fn fmt_signal_row(row: &SignalEnvelopeRow) -> String {
    let origin = row.origin.as_deref().unwrap_or("row");
    let rule = row.rule_id.as_deref().unwrap_or("—");
    let mut parts: Vec<String> = Vec::new();
    if let Some(sev) = row.severity.as_deref() {
        let sc = severity_color(&sev.to_ascii_lowercase());
        parts.push(format!("{sc}[{}]{RESET}", sev.to_uppercase()));
    }
    if let Some(label) = row.label.as_deref() {
        if !label.is_empty() {
            parts.push(label.to_string());
        }
    }
    if let Some(vt) = row.value_text.as_deref().filter(|vt| !vt.is_empty()) {
        parts.push(format!("= {vt}"));
    } else if let Some(vn) = row.value_num {
        let unit = row.unit.as_deref().unwrap_or("");
        parts.push(format!("= {vn}{unit}"));
    }
    let head = if parts.is_empty() {
        rule.to_string()
    } else {
        format!("{}  {DIM}({rule}){RESET}", parts.join(" "))
    };
    format!("{DIM}[{origin}]{RESET} {head}")
}

/// Compact one-line signal summary for directory cards (list/search output).
/// One short token per non-empty category, joined with `·`; `None` when no
/// category has rows (keeps the table from flooding).
///
/// NOTE (vettd#879): no longer called by the CLI — `directory view` now
/// renders the full vertical signals block like `directory compare` instead
/// of this compact line. Kept (not deleted speculatively) because list/search
/// responses still carry `signalCategories` and a future surface may want it.
#[allow(dead_code)]
pub(crate) fn fmt_signal_categories_compact(cats: &[SignalCategorySummary]) -> Option<String> {
    let tokens: Vec<String> = cats
        .iter()
        .filter(|c| c.rows.as_ref().is_some_and(|r| !r.is_empty()))
        .map(|c| {
            let label = c.label.as_deref().unwrap_or("—");
            match &c.verdict {
                Some(v) if v.form.as_deref() == Some("graded") => match &v.grade {
                    Some(g) => format!("{label}: {g}"),
                    None => format!("{label}: graded"),
                },
                Some(v) if v.form.as_deref() == Some("measured") => {
                    if let Some(m) = v
                        .magnitudes
                        .as_ref()
                        .and_then(|mags| mags.first())
                        .and_then(|m| m.value)
                    {
                        let unit = v
                            .magnitudes
                            .as_ref()
                            .and_then(|mags| mags.first())
                            .and_then(|m| m.unit.as_deref())
                            .unwrap_or("");
                        format!("{label}: {m}{unit}")
                    } else {
                        format!("{label}: measured")
                    }
                }
                _ => {
                    let n = c.rows.as_ref().map(|r| r.len()).unwrap_or(0);
                    format!("{label}: {n}")
                }
            }
        })
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" · "))
    }
}

// ---------------------------------------------------------------------------
// Signals block (vettd#879)
//
// `directory compare` and `directory view` show a fixed selection of signals
// vertically, one per line, sourced from the full 7-category envelope (`GET
// /api/assets/skill_audit/{id}/signals`) rather than the deliberately narrow
// 3-category `signalCategories` summary on the detail payload.
// ---------------------------------------------------------------------------

/// Selected signals shown in the vertical signals block, in display order:
/// `(ruleId, display label)`. The license row already shows the license, so
/// the `characteristics/declared-license` signal (which carries an empty
/// `valueText`) is excluded; `sentiment/watchers` is shown in its place,
/// keeping the sentiment family together.
const SIGNAL_SELECTION: &[(&str, &str)] = &[
    ("performance/static-context-tokens", "Context tokens"),
    ("reliability/eval-test-case-count", "Eval test cases"),
    (
        "reliability/unresolvable-internal-references",
        "Unresolvable refs",
    ),
    ("sentiment/stars", "Stars"),
    ("sentiment/forks", "Forks"),
    ("sentiment/watchers", "Watchers"),
    ("sentiment/open-issues", "Open issues"),
    ("sentiment/last-commit-age", "Last commit age"),
    ("characteristics/archived", "Archived"),
    ("characteristics/primary-language", "Primary language"),
];

/// Width of each side's value column. Every row of the compare table shares
/// this one width — fixed rows and the signals block — so value A and value B
/// sit at identical byte offsets on every line.
const COMPARE_VALUE_W: usize = 30;

/// Longest selected signal label, in display chars. The signals block's
/// label column is sized from this so every signal row's values line up.
/// Computed from the selection so the column can never drift out of sync
/// with the labels.
fn signals_label_w() -> usize {
    SIGNAL_SELECTION
        .iter()
        .map(|(_, label)| label.chars().count())
        .max()
        .unwrap_or(0)
}

/// The single label-column width for a directory command's rows — the fixed
/// rows and the signals block share it, so value A starts at the same column
/// on every line. The signals selection carries the longest labels printed
/// (17 today — `Unresolvable refs`; the longest fixed row label,
/// `Last scanned:`, is 13), so it drives the width; the 13 floor keeps the
/// fixed rows aligned if the selection is ever shortened below them.
fn signals_table_label_w() -> usize {
    signals_label_w().max(13)
}

/// Pick the display value for one signal rule on one side.
///
/// Precedence: `valueNum` (unit appended after a single space; no trailing
/// `.0`) → non-empty `valueText` → `severity` → count of the rule's list rows
/// on this side (only when there is more than one — a lone scalar row with no
/// usable value is absent, not a list) → `—`.
fn signal_value(rows: &[SignalEnvelopeRow], rule_id: &str) -> String {
    let matching: Vec<&SignalEnvelopeRow> = rows
        .iter()
        .filter(|r| r.rule_id.as_deref() == Some(rule_id))
        .collect();
    let first = matching.first();
    if let Some(v) = first.and_then(|r| r.value_num) {
        let unit = first.and_then(|r| r.unit.as_deref()).unwrap_or("");
        if !unit.is_empty() {
            return format!("{v} {unit}");
        }
        return format!("{v}");
    }
    // Empty or whitespace-only `valueText` carries no usable value — treat it
    // as absent so it falls through to the severity check instead of
    // rendering a blank cell or degrading into a bare row count.
    if let Some(t) = first
        .and_then(|r| r.value_text.as_deref())
        .filter(|t| !t.trim().is_empty())
    {
        return t.to_string();
    }
    if let Some(sev) = first
        .and_then(|r| r.severity.as_deref())
        .filter(|s| !s.is_empty())
    {
        return sev.to_string();
    }
    // Row-count fallback — only for genuine list rules (multiple rows on this
    // side). A single scalar row with no usable value is NOT a list and must
    // not be reported as a bare count.
    if matching.len() > 1 {
        return matching.len().to_string();
    }
    "—".to_string()
}

/// Append one signals-block row to `out`. `b` carries the second value column
/// when a second side is present (`directory compare`); `None` renders a
/// single value column (`directory view`). Two-column values are truncated to
/// `COMPARE_VALUE_W` so the right column stays put; single-column values are
/// never clipped.
fn push_signal_row(out: &mut String, label: &str, a: String, b: Option<String>, label_w: usize) {
    match b {
        Some(b) => out.push_str(&format!(
            "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}\n",
            label,
            truncate_to_display(&a, COMPARE_VALUE_W),
            truncate_to_display(&b, COMPARE_VALUE_W),
            label_w = label_w,
            val_w = COMPARE_VALUE_W,
        )),
        None => out.push_str(&format!(
            "  {DIM}{:<label_w$}{RESET}  {}\n",
            label,
            a,
            label_w = label_w,
        )),
    }
}

/// Render the vertical signals block: a dim heading, a `Total signals` line
/// (per-side `signal_count`, `—` when absent), then one line per selected
/// signal. `b_rows`/`b_signal_count` as `None` renders a single value column
/// (`directory view`); with both sides the columns line up against the
/// caller's unified label width (`directory compare`).
///
/// Pure — no stdout, so the layout is unit-testable without capturing output
/// (mirrors how `render_cards_table` was split out of `print_cards`).
fn render_signals_block(
    a_rows: &[SignalEnvelopeRow],
    b_rows: Option<&[SignalEnvelopeRow]>,
    a_signal_count: Option<u32>,
    b_signal_count: Option<u32>,
    label_w: usize,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {DIM}Signals{RESET}\n"));
    let count_a = a_signal_count.map_or_else(|| "—".to_string(), |n| n.to_string());
    let count_b = b_signal_count.map_or_else(|| "—".to_string(), |n| n.to_string());
    push_signal_row(
        &mut out,
        "Total signals",
        count_a,
        b_rows.map(|_| count_b),
        label_w,
    );
    for (rule_id, label) in SIGNAL_SELECTION {
        let a = signal_value(a_rows, rule_id);
        let b = b_rows.map(|rows| signal_value(rows, rule_id));
        push_signal_row(&mut out, label, a, b, label_w);
    }
    out
}

pub fn handle_compare(slug_a: &str, slug_b: &str, json: bool) {
    let detail_a = fetch_skill(slug_a);
    let detail_b = fetch_skill(slug_b);

    if json {
        #[derive(Serialize)]
        struct CompareOutput<'a> {
            a: &'a DirectorySkillDetail,
            b: &'a DirectorySkillDetail,
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&CompareOutput {
                a: &detail_a,
                b: &detail_b,
            })
            .unwrap_or_default()
        );
        return;
    }

    // Column geometry: 2 indent + 17 label + 2 sep + 30 value + 2 sep + right
    // value. ONE label width and ONE value width for every row — fixed rows
    // and the signals block — so value A starts at the same column on every
    // line (the signals selection carries the longest labels; see
    // `signals_table_label_w`).
    let label_w = signals_table_label_w();
    let val_w: usize = COMPARE_VALUE_W;
    let prefix_w = 2 + label_w + 2; // chars before the left value column

    let (ca, ha, ma, la, ia) = count_by_severity(&detail_a.findings);
    let (cb, hb, mb, lb, ib) = count_by_severity(&detail_b.findings);
    let scanners_a = external_scanner_run_count(&detail_a.scanner_runs) + 1;
    let scanners_b = external_scanner_run_count(&detail_b.scanner_runs) + 1;

    let source_a = detail_a
        .source_type
        .as_deref()
        .map(display_source_type)
        .unwrap_or("—");
    let source_b = detail_b
        .source_type
        .as_deref()
        .map(display_source_type)
        .unwrap_or("—");

    let contains_a = {
        let mut v: Vec<&str> = Vec::new();
        if detail_a.has_skill_md.unwrap_or(false) {
            v.push("skill.md");
        }
        if detail_a.has_scripts.unwrap_or(false) {
            v.push("scripts");
        }
        if detail_a.has_evals.unwrap_or(false) {
            v.push("evals");
        }
        if v.is_empty() {
            "—".to_string()
        } else {
            v.join(", ")
        }
    };
    let contains_b = {
        let mut v: Vec<&str> = Vec::new();
        if detail_b.has_skill_md.unwrap_or(false) {
            v.push("skill.md");
        }
        if detail_b.has_scripts.unwrap_or(false) {
            v.push("scripts");
        }
        if detail_b.has_evals.unwrap_or(false) {
            v.push("evals");
        }
        if v.is_empty() {
            "—".to_string()
        } else {
            v.join(", ")
        }
    };

    let last_scanned_a = detail_a
        .completed_at
        .as_deref()
        .and_then(|s| s.get(..10))
        .unwrap_or("—");
    let last_scanned_b = detail_b
        .completed_at
        .as_deref()
        .and_then(|s| s.get(..10))
        .unwrap_or("—");

    let files_a = detail_a
        .file_count
        .map_or_else(|| "—".to_string(), |n| n.to_string());
    let files_b = detail_b
        .file_count
        .map_or_else(|| "—".to_string(), |n| n.to_string());

    let findings_a = fmt_severity_breakdown(ca, ha, ma, la, ia);
    let findings_b = fmt_severity_breakdown(cb, hb, mb, lb, ib);
    let signals_a = signals_for_detail(detail_a.id.as_deref());
    let signals_b = signals_for_detail(detail_b.id.as_deref());
    let scanners_a_s = format!(
        "{scanners_a} scanner{}",
        if scanners_a == 1 { "" } else { "s" }
    );
    let scanners_b_s = format!(
        "{scanners_b} scanner{}",
        if scanners_b == 1 { "" } else { "s" }
    );

    let slug_display_a = detail_a.slug.as_deref().unwrap_or(slug_a);
    let slug_display_b = detail_b.slug.as_deref().unwrap_or(slug_b);

    // Truncate a value to fit within a column cell (plain text — no ANSI before truncation).
    let col = |s: &str| truncate_to_display(s, val_w);

    let grade_a = detail_a.overall_grade.as_deref().unwrap_or("—");
    let grade_b = detail_b.overall_grade.as_deref().unwrap_or("—");
    let gca = grade_color(grade_a);
    let gcb = grade_color(grade_b);

    // Header: indent to the value column so slugs align with their data
    let gap = " ".repeat(prefix_w);
    println!("{gap}{BOLD}{slug_display_a:<val_w$}{RESET}  {BOLD}{slug_display_b}{RESET}");
    println!("{DIM}{}{RESET}", "─".repeat(prefix_w + val_w + 2 + val_w));

    // Top section — dim labels, colored grade, plain other values
    println!(
        "  {DIM}{:<label_w$}{RESET}  {gca}{:<val_w$}{RESET}  {gcb}{}{RESET}",
        "Grade:", grade_a, grade_b
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
        "License:",
        col(detail_a.license.as_deref().unwrap_or("—")),
        col(detail_b.license.as_deref().unwrap_or("—"))
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
        "Author:",
        col(detail_a.author.as_deref().unwrap_or("—")),
        col(detail_b.author.as_deref().unwrap_or("—"))
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
        "Source:",
        col(source_a),
        col(source_b)
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
        "Contains:",
        col(&contains_a),
        col(&contains_b)
    );
    println!();

    // Bottom section
    println!(
        "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
        "Findings:",
        col(&findings_a),
        col(&findings_b)
    );
    // Vertical signals block — replaces the old horizontal `Signals:` row,
    // which truncated the narrow 3-category summary. Shows a fixed selection
    // of signals one per line for both sides, sharing the table's label and
    // value columns (see `render_signals_block`).
    print!(
        "{}",
        render_signals_block(
            &signals_a.signals,
            Some(&signals_b.signals),
            detail_a.signal_count,
            detail_b.signal_count,
            label_w,
        )
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
        "Scanners:",
        col(&scanners_a_s),
        col(&scanners_b_s)
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
        "Last scanned:",
        col(last_scanned_a),
        col(last_scanned_b)
    );
    println!(
        "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
        "Files:",
        col(&files_a),
        col(&files_b)
    );

    // Slice 2 freshness rows. The status line always uses colored compact
    // labels; subsequent timestamp/hash rows are symmetric — each row is
    // emitted when EITHER side has a value, with `—` for the missing side
    // (see `freshness::compare_row`). Hashes are abbreviated only here, for
    // column fit; the JSON output retains full hashes.
    //
    // The status cell is colored, so it must be padded to `val_w` by VISIBLE
    // width (ANSI bytes are invisible) to keep the right-hand column aligned
    // with the rest of compare, which pads plain text via `{:<val_w$}`.
    let fa = detail_a.freshness.as_ref();
    let fb = detail_b.freshness.as_ref();
    let status_a = pad_to_visible(&freshness::fmt_freshness_colored(&fa), val_w);
    let status_b = pad_to_visible(&freshness::fmt_freshness_colored(&fb), val_w);
    println!(
        "  {DIM}{:<label_w$}{RESET}  {}  {}",
        "Freshness:", status_a, status_b
    );

    let mut freshness_rows: Vec<(&str, Option<String>, Option<String>)> = Vec::new();
    freshness_rows.push((
        "Checked:",
        fa.and_then(|f| f.last_checked_at.clone()),
        fb.and_then(|f| f.last_checked_at.clone()),
    ));
    freshness_rows.push((
        "Verified:",
        fa.and_then(|f| f.last_verified_at.clone()),
        fb.and_then(|f| f.last_verified_at.clone()),
    ));
    freshness_rows.push((
        "Last change:",
        fa.and_then(|f| f.last_change_detected_at.clone()),
        fb.and_then(|f| f.last_change_detected_at.clone()),
    ));
    freshness_rows.push((
        "Scanned hash:",
        fa.and_then(|f| f.scanned_hash.as_deref())
            .map(freshness::abbrev_hash),
        fb.and_then(|f| f.scanned_hash.as_deref())
            .map(freshness::abbrev_hash),
    ));
    freshness_rows.push((
        "Upstream:",
        fa.and_then(|f| f.latest_upstream_hash.as_deref())
            .map(freshness::abbrev_hash),
        fb.and_then(|f| f.latest_upstream_hash.as_deref())
            .map(freshness::abbrev_hash),
    ));

    for (label, left, right) in freshness_rows {
        if let Some((l, r)) = freshness::compare_row(left, right) {
            println!(
                "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
                label,
                truncate_to_display(&l, val_w),
                truncate_to_display(&r, val_w)
            );
        }
    }
}

pub fn handle_trending() {
    let url = format!("{}?sort=downloads", directory_base_url());
    match read_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            println!(
                "Trending by downloads ({} skills, page {}/{}):",
                resp.total, resp.page, resp.total_pages
            );
            print_cards(&resp.skills, true);
        }
        Err(ReadError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd directory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RandomSkillResponse {
    pub skill: Option<DirectoryCard>,
}

pub fn handle_random(json: bool) {
    let endpoint = crate::submit::load_auth_config()
        .map(|c| c.endpoint)
        .unwrap_or_else(|| crate::submit::DEFAULT_PRODUCTION_ENDPOINT.to_string());
    let url = crate::network::derive_api_url(&endpoint, "directory/random");
    match read_client::fetch_json::<RandomSkillResponse>(&url) {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                match resp.skill {
                    Some(card) => print_cards(std::slice::from_ref(&card), true),
                    None => println!("No public skills available."),
                }
            }
        }
        Err(ReadError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd directory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Card display helpers
// ---------------------------------------------------------------------------

/// Fixed visible width of the safety (grade badge) column in the directory table.
const SAFETY_COL_W: usize = 6;
/// Fixed visible width of the slice-2 freshness column. Anchored to the widest
/// compact label (`[offline]` = 9 chars) so the name column never shifts.
const FRESH_COL_W: usize = 9;
/// Fixed visible width of the source column.
const SOURCE_COL_W: usize = 10;
/// Fixed visible width of the "signals" (total signal count) column. Wide
/// enough for a left-aligned count with room to grow, keeping one-line rows.
const SIGNALS_COL_W: usize = 8;
/// Separator width between table columns (two spaces).
const COL_GAP: usize = 2;

/// Print a slice of cards as a padded, single-line-per-card table with a header.
///
/// `show_freshness` controls whether the slice-2 freshness column is rendered.
/// Directory calls pass `true`; inventory reuses this renderer with `false` so
/// authenticated inventory output is byte-identical to its pre-freshness shape.
pub(crate) fn print_cards(cards: &[DirectoryCard], show_freshness: bool) {
    print!(
        "{}",
        render_cards_table(cards, show_freshness, terminal_width())
    );
}

/// Build the padded, single-line-per-card table (header + separator + rows) as
/// a string. Split out of `print_cards` so the layout is unit-testable without
/// capturing stdout.
fn render_cards_table(cards: &[DirectoryCard], show_freshness: bool, term_w: usize) -> String {
    let slug_w = cards
        .iter()
        .map(|c| c.slug.as_deref().unwrap_or(&c.name).len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();

    if show_freshness {
        out.push_str(&format!(
            "{BOLD}{:<safety$}  {:<fresh$}  {:<w$}  {:<src$}  {:<sig$}  description{RESET}\n",
            "safety",
            "fresh.",
            "name",
            "source",
            "signals",
            safety = SAFETY_COL_W,
            fresh = FRESH_COL_W,
            w = slug_w,
            src = SOURCE_COL_W,
            sig = SIGNALS_COL_W,
        ));
    } else {
        out.push_str(&format!(
            "{BOLD}{:<safety$}  {:<w$}  {:<src$}  {:<sig$}  description{RESET}\n",
            "safety",
            "name",
            "source",
            "signals",
            safety = SAFETY_COL_W,
            w = slug_w,
            src = SOURCE_COL_W,
            sig = SIGNALS_COL_W,
        ));
    }
    out.push_str(&format!(
        "{DIM}{}{RESET}\n",
        "─".repeat(term_w.saturating_sub(5))
    ));

    for card in cards {
        out.push_str(&render_card_row(card, slug_w, term_w, show_freshness));
    }
    out
}

fn render_card_row(
    card: &DirectoryCard,
    slug_w: usize,
    term_w: usize,
    show_freshness: bool,
) -> String {
    let grade = card.overall_grade.as_deref().unwrap_or("?");
    let gc = grade_color(grade);
    // Grade badge visual text (no ANSI) — always 3 chars like "[A]"
    let grade_visible = format!("[{grade}]");
    let grade_pad = " ".repeat(SAFETY_COL_W.saturating_sub(grade_visible.len()));
    let grade_display = format!("{gc}{grade_visible}{RESET}{grade_pad}");

    let slug = card.slug.as_deref().unwrap_or(&card.name);
    let slug_padded = format!("{slug:<w$}", w = slug_w);
    let asset_type = card
        .source_type
        .as_deref()
        .map(display_source_type)
        .unwrap_or("—");
    let signals_display = card
        .signal_count
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".to_string());
    let desc = card.description.as_deref().unwrap_or("");

    // Compute desc budget from visual widths (ANSI codes are invisible).
    // Width depends on whether the freshness column is present.
    let freshness_col_w = if show_freshness {
        FRESH_COL_W + COL_GAP
    } else {
        0
    };
    let visual_prefix_w = SAFETY_COL_W
        + COL_GAP
        + freshness_col_w
        + slug_w
        + COL_GAP
        + SOURCE_COL_W
        + COL_GAP
        + SIGNALS_COL_W
        + COL_GAP;
    let desc_budget = term_w.saturating_sub(visual_prefix_w).saturating_sub(5);
    let desc_display = truncate_to_display(desc, desc_budget);

    if show_freshness {
        let freshness_display = pad_to_visible(
            &freshness::fmt_freshness_colored(&card.freshness.as_ref()),
            FRESH_COL_W,
        );
        format!(
            "{grade_display}  {freshness_display}  {slug_padded}  {asset_type:<src$}  {signals_display:<sig$}  {DIM}{desc_display}{RESET}\n",
            src = SOURCE_COL_W,
            sig = SIGNALS_COL_W,
        )
    } else {
        format!(
            "{grade_display}  {slug_padded}  {asset_type:<src$}  {signals_display:<sig$}  {DIM}{desc_display}{RESET}\n",
            src = SOURCE_COL_W,
            sig = SIGNALS_COL_W,
        )
    }
}

/// Read terminal width from `$COLUMNS`, falling back to 120.
fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120)
}

/// Truncate a string to at most `max` display characters, appending `…` if cut.
///
/// This counts raw characters and so MUST only be given visible text (no ANSI
/// escape sequences) — pass the plain portion of a colored cell, never the
/// colored string itself (see `pad_to_visible` for ANSI-aware padding).
pub(crate) fn truncate_to_display(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max.saturating_sub(1)).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        // String fit within max — return without the ellipsis slot we reserved.
        s.to_string()
    }
}

/// Strip ANSI escape sequences (CSI `ESC [ … ]` blocks) from `s`, returning
/// only the visible text. Used to compute display widths of colored cells.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI sequence: consume `[ … ]` through the final byte in @–~.
            // Any other lone escape is dropped wholesale.
            if chars.next() == Some('[') {
                for n in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&n) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Number of visible characters in `s` once ANSI escape codes are removed.
fn visible_width(s: &str) -> usize {
    strip_ansi(s).chars().count()
}

/// Pad `s` on the right with plain spaces to `width` visible columns, ignoring
/// ANSI escape sequences in the width calculation.
///
/// Keeps colored cells (which carry invisible ANSI bytes) aligned with the
/// plain-text cells that other table/compare columns pad via `{:<width$}`.
/// Returns `s` unchanged if it is already at or wider than `width`.
fn pad_to_visible(s: &str, width: usize) -> String {
    let cur = visible_width(s);
    if cur >= width {
        return s.to_string();
    }
    format!("{s}{}", " ".repeat(width - cur))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;
    use serde_json::json;

    #[test]
    fn severity_ordering_is_correct() {
        assert!(severity_value("critical") > severity_value("high"));
        assert!(severity_value("high") > severity_value("medium"));
        assert!(severity_value("medium") > severity_value("low"));
        assert!(severity_value("low") > severity_value("info"));
        // Unknown maps to info-level.
        assert_eq!(severity_value("unknown"), severity_value("info"));
    }

    #[test]
    fn severity_case_insensitive() {
        assert_eq!(severity_value("CRITICAL"), severity_value("critical"));
        assert_eq!(severity_value("High"), severity_value("high"));
    }

    #[test]
    fn count_by_severity_basic() {
        let findings = vec![
            DirectoryFinding {
                severity: "critical".to_string(),
                rule_id: None,
                category: None,
                label: "a".to_string(),
                detail: None,
                source: None,
                filepath: None,
            },
            DirectoryFinding {
                severity: "high".to_string(),
                rule_id: None,
                category: None,
                label: "b".to_string(),
                detail: None,
                source: None,
                filepath: None,
            },
            DirectoryFinding {
                severity: "info".to_string(),
                rule_id: None,
                category: None,
                label: "c".to_string(),
                detail: None,
                source: None,
                filepath: None,
            },
        ];
        let (c, h, m, l, i) = count_by_severity(&findings);
        assert_eq!((c, h, m, l, i), (1, 1, 0, 0, 1));
    }

    #[test]
    fn external_scanner_count_excludes_vettd_and_non_success() {
        let runs = vec![
            ScannerRun {
                source: "vettd".to_string(),
                status: "success".to_string(),
                verdict: None,
                grade: None,
                finding_count: None,
                critical_count: None,
                high_count: None,
            },
            ScannerRun {
                source: "openai".to_string(),
                status: "success".to_string(),
                verdict: None,
                grade: None,
                finding_count: None,
                critical_count: None,
                high_count: None,
            },
            ScannerRun {
                source: "openai".to_string(), // duplicate — deduped
                status: "success".to_string(),
                verdict: None,
                grade: None,
                finding_count: None,
                critical_count: None,
                high_count: None,
            },
            ScannerRun {
                source: "anthropic".to_string(),
                status: "failed".to_string(), // non-success — excluded
                verdict: None,
                grade: None,
                finding_count: None,
                critical_count: None,
                high_count: None,
            },
        ];
        assert_eq!(external_scanner_run_count(&runs), 1); // only "openai" (deduped)
    }

    #[test]
    fn percent_encode_basic() {
        assert_eq!(percent_encode("hello"), "hello");
        assert_eq!(percent_encode("hello world"), "hello%20world");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn fmt_severity_breakdown_empty() {
        assert_eq!(fmt_severity_breakdown(0, 0, 0, 0, 0), "none");
    }

    // ── build_search_body / filter parsing ────────────────────────────────

    fn skill_filters() -> SearchFilters {
        SearchFilters {
            asset_type: "skill".to_string(),
            ..SearchFilters::default()
        }
    }

    #[test]
    fn build_search_body_skill_default_shape() {
        let body = build_search_body(
            "pdf",
            1,
            "newest",
            false,
            &skill_filters(),
            &ValidatedFilters::default(),
        );
        assert_eq!(
            body,
            serde_json::json!({
                "search": "pdf",
                "page": 1,
                "sort": "newest",
                "reverse": false,
                "assetType": "skill",
                "languages": [],
                "agentCompatibility": [],
                "sources": [],
                "rankFilters": {},
                "rankings": null,
            })
        );
        // mcp-only keys are absent on the skill path.
        let obj = body.as_object().unwrap();
        assert!(!obj.contains_key("mcpCategory"));
        assert!(!obj.contains_key("deployment"));
        assert!(!obj.contains_key("registryType"));
    }

    #[test]
    fn build_search_body_threads_all_skill_filters() {
        let filters = SearchFilters {
            asset_type: "skill".to_string(),
            languages: vec!["python".to_string(), "typescript".to_string()],
            agent_compatibility: vec!["claude-code".to_string()],
            sources: vec!["marketplace".to_string()],
            rank_filters: vec!["search_rank_skills_sh_rank=100".to_string()],
            ..SearchFilters::default()
        };
        let validated = validate_search_filters(&filters, true);
        let body = build_search_body("pdf", 2, "rating", true, &filters, &validated);
        assert_eq!(
            body,
            serde_json::json!({
                "search": "pdf",
                "page": 2,
                "sort": "rating",
                "reverse": true,
                "assetType": "skill",
                "languages": ["python", "typescript"],
                "agentCompatibility": ["claude-code"],
                "sources": ["marketplace"],
                "rankFilters": {"search_rank_skills_sh_rank": 100},
                "rankings": null,
            })
        );
    }

    #[test]
    fn build_search_body_mcp_adds_mcp_only_arrays() {
        let filters = SearchFilters {
            asset_type: "mcp".to_string(),
            sources: vec!["glama".to_string()],
            mcp_category: vec!["server".to_string()],
            deployment: vec!["hybrid".to_string()],
            registry_type: vec!["npm".to_string()],
            ..SearchFilters::default()
        };
        let body = build_search_body(
            "context7",
            1,
            "newest",
            false,
            &filters,
            &ValidatedFilters::default(),
        );
        assert_eq!(
            body,
            serde_json::json!({
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
                "registryType": ["npm"],
            })
        );
    }

    #[test]
    fn build_search_body_mcp_empty_filters_still_present() {
        let filters = SearchFilters {
            asset_type: "mcp".to_string(),
            ..SearchFilters::default()
        };
        let body = build_search_body(
            "x",
            1,
            "newest",
            false,
            &filters,
            &ValidatedFilters::default(),
        );
        let obj = body.as_object().unwrap();
        assert_eq!(obj["mcpCategory"], serde_json::json!([]));
        assert_eq!(obj["deployment"], serde_json::json!([]));
        assert_eq!(obj["registryType"], serde_json::json!([]));
    }

    #[test]
    fn build_search_body_passes_rankings_object_through() {
        let filters = SearchFilters {
            asset_type: "skill".to_string(),
            rankings: Some(r#"{"stars": 50, "officialClaudeMarketplace": true}"#.to_string()),
            ..SearchFilters::default()
        };
        let validated = validate_search_filters(&filters, true);
        let body = build_search_body("q", 1, "newest", false, &filters, &validated);
        assert_eq!(
            body["rankings"],
            serde_json::json!({"stars": 50, "officialClaudeMarketplace": true})
        );
    }

    #[test]
    fn parse_rank_filters_valid() {
        let got = parse_rank_filters(&[
            "search_rank_skills_sh_rank=100".to_string(),
            "search_rank_seed_rank=5".to_string(),
        ])
        .unwrap();
        assert_eq!(got["search_rank_skills_sh_rank"], serde_json::json!(100));
        assert_eq!(got["search_rank_seed_rank"], serde_json::json!(5));
    }

    #[test]
    fn parse_rank_filters_empty_is_empty_map() {
        assert!(parse_rank_filters(&[]).unwrap().is_empty());
    }

    #[test]
    fn parse_rank_filters_rejects_missing_equals() {
        let err = parse_rank_filters(&["search_rank_seed_rank".to_string()]).unwrap_err();
        assert!(err.contains("key=N"));
    }

    #[test]
    fn parse_rank_filters_rejects_non_integer_value() {
        let err = parse_rank_filters(&["k=abc".to_string()]).unwrap_err();
        assert!(err.contains("must be an integer"));
    }

    #[test]
    fn parse_rank_filters_rejects_empty_key() {
        let err = parse_rank_filters(&["=10".to_string()]).unwrap_err();
        assert!(err.contains("empty key"));
    }

    #[test]
    fn any_filter_set_detects_each_filter() {
        assert!(!any_filter_set(&skill_filters()));
        assert!(any_filter_set(&SearchFilters {
            asset_type: "mcp".to_string(),
            ..SearchFilters::default()
        }));
        assert!(any_filter_set(&SearchFilters {
            asset_type: "skill".to_string(),
            sources: vec!["seed".to_string()],
            ..SearchFilters::default()
        }));
        assert!(any_filter_set(&SearchFilters {
            asset_type: "skill".to_string(),
            rank_filters: vec!["k=1".to_string()],
            ..SearchFilters::default()
        }));
    }

    #[test]
    fn directory_card_surfaces_scan_verdicts_in_json() {
        let raw = serde_json::json!({
            "name": "e2e-testing",
            "llm_scan": {"max_severity": "LOW", "finding_count": 1},
            "cli_security": {"grade": "C"},
            "vettd_scan": {"overall_grade": "B", "trust_level": "cautious"}
        });
        let card: DirectoryCard = serde_json::from_value(raw).unwrap();
        let out = serde_json::to_value(&card).unwrap();
        assert_eq!(out["llm_scan"]["max_severity"], "LOW");
        assert_eq!(out["cli_security"]["grade"], "C");
        assert_eq!(out["vettd_scan"]["overall_grade"], "B");
    }

    #[test]
    fn directory_card_without_verdicts_omits_them() {
        let raw = serde_json::json!({"name": "plain"});
        let card: DirectoryCard = serde_json::from_value(raw).unwrap();
        let out = serde_json::to_value(&card).unwrap();
        let obj = out.as_object().unwrap();
        assert!(!obj.contains_key("llm_scan"));
        assert!(!obj.contains_key("cli_security"));
        assert!(!obj.contains_key("vettd_scan"));
    }

    #[test]
    fn mcp_card_is_snake_case_passthrough() {
        let raw = serde_json::json!({
            "mcp_id": "github:upstash/context7",
            "name": "context7",
            "registry_type": "npm",
            "stars": 61421,
            "security_direct_deps_vuln_count": 44,
            "security_direct_deps_with_vulns": ["zod", "jose"]
        });
        let card: McpCard = serde_json::from_value(raw).unwrap();
        let out = serde_json::to_value(&card).unwrap();
        assert_eq!(out["mcp_id"], "github:upstash/context7");
        assert_eq!(out["security_direct_deps_vuln_count"], 44);
        assert_eq!(out["security_direct_deps_with_vulns"][0], "zod");
        // absent fields are not serialized as null
        assert!(!out.as_object().unwrap().contains_key("readme"));
    }

    #[test]
    fn fmt_severity_breakdown_mixed() {
        let s = fmt_severity_breakdown(1, 2, 0, 0, 3);
        assert!(s.contains("1 critical"));
        assert!(s.contains("2 high"));
        assert!(s.contains("3 info"));
        assert!(!s.contains("medium"));
    }

    /// A minimal `DirectoryCard` with a name and a freshness field.
    fn card_with_freshness(freshness: Option<PublicFreshness>) -> DirectoryCard {
        DirectoryCard {
            slug: Some("pdf-summarizer".into()),
            name: "PDF Summarizer".into(),
            description: None,
            version: None,
            author: None,
            category: None,
            badge_status: None,
            overall_grade: None,
            source_type: None,
            scanner_run_count: None,
            signal_categories: None,
            signal_count: None,
            language: None,
            agent_compatibility: None,
            rankings: None,
            llm_scan: None,
            cli_security: None,
            vettd_scan: None,
            freshness,
        }
    }

    #[test]
    fn directory_card_json_omits_freshness_when_absent() {
        // Inventory reuses this struct. When the server sends no freshness
        // row, `--json` output must NOT synthesize `freshness: null` — it must
        // be byte-identical to the pre-slice shape.
        let card = card_with_freshness(None);
        let val: serde_json::Value = serde_json::to_value(&card).unwrap();
        assert!(
            val.get("freshness").is_none(),
            "absent freshness must be omitted, not null: {}",
            val
        );
    }

    #[test]
    fn directory_card_json_forwards_freshness_when_present() {
        // Directory (public) responses that DO carry freshness must forward it
        // losslessly.
        let card = card_with_freshness(Some(PublicFreshness {
            status: "changed".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        }));
        let val: serde_json::Value = serde_json::to_value(&card).unwrap();
        assert_eq!(val["freshness"]["status"], "changed");
    }

    #[test]
    fn directory_detail_json_omits_freshness_when_absent() {
        let detail = DirectorySkillDetail {
            id: None,
            slug: None,
            name: "PDF Summarizer".into(),
            description: None,
            version: None,
            author: None,
            category: None,
            overall_grade: None,
            license: None,
            source_type: None,
            source_url: None,
            has_skill_md: None,
            has_scripts: None,
            has_evals: None,
            file_count: None,
            completed_at: None,
            findings: vec![],
            scanner_runs: vec![],
            signal_categories: None,
            signal_count: None,
            freshness: None,
        };
        let val: serde_json::Value = serde_json::to_value(&detail).unwrap();
        // Inventory view/compare reuse this struct — absent freshness must be
        // omitted, preserving the pre-slice JSON shape.
        assert!(
            val.get("freshness").is_none(),
            "absent freshness must be omitted from detail JSON: {}",
            val
        );
    }

    // ── signal category summary tests ─────────────────────────────────

    fn sample_category_summary_json() -> serde_json::Value {
        serde_json::json!({
            "category": "safety",
            "label": "Safety",
            "form": "graded",
            "verdict": {"form": "graded", "grade": "C"},
            "rows": [
                {
                    "origin": "signal",
                    "id": "sig-1",
                    "subjectType": "skill_audit",
                    "subjectId": "audit-1",
                    "relatedType": "",
                    "relatedId": "",
                    "dataCategory": "safety",
                    "sourceClass": "scan",
                    "source": "vettd",
                    "ruleId": "VTD-0001",
                    "severity": "medium",
                    "label": "Prompt injection",
                    "detail": null,
                    "valueNum": null,
                    "valueText": null,
                    "unit": null,
                    "method": null,
                    "derivation": null,
                    "confidence": null,
                    "sampleSize": null,
                    "synthetic": false,
                    "firstParty": true,
                    "observedAt": "2026-08-24T00:00:00.000Z",
                    "payload": null
                }
            ]
        })
    }

    #[test]
    fn signal_categories_decode_from_server_shape() {
        // The allow-list struct must decode the live `CategorySummary` shape
        // (verdict union + envelope rows) with all fields present.
        let cat: SignalCategorySummary =
            serde_json::from_value(sample_category_summary_json()).unwrap();
        assert_eq!(cat.category.as_deref(), Some("safety"));
        assert_eq!(cat.form.as_deref(), Some("graded"));
        let verdict = cat.verdict.unwrap();
        assert_eq!(verdict.form.as_deref(), Some("graded"));
        assert_eq!(verdict.grade.as_deref(), Some("C"));
        let rows = cat.rows.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rule_id.as_deref(), Some("VTD-0001"));
        assert_eq!(rows[0].origin.as_deref(), Some("signal"));
        assert_eq!(rows[0].severity.as_deref(), Some("medium"));
        assert_eq!(
            rows[0].observed_at.as_deref(),
            Some("2026-08-24T00:00:00.000Z")
        );
    }

    #[test]
    fn signal_categories_decode_unjudged_and_measured_verdicts() {
        // unjudged (no grade, no magnitudes)
        let unjudged: SignalCategorySummary = serde_json::from_value(serde_json::json!({
            "category": "characteristics",
            "label": "Characteristics",
            "form": "unjudged",
            "verdict": {"form": "unjudged"},
            "rows": []
        }))
        .unwrap();
        assert_eq!(unjudged.verdict.unwrap().form.as_deref(), Some("unjudged"));

        // measured with magnitudes
        let measured: SignalCategorySummary = serde_json::from_value(serde_json::json!({
            "category": "performance",
            "label": "Performance",
            "form": "measured",
            "verdict": {
                "form": "measured",
                "magnitudes": [
                    {"ruleId": "perf/context", "label": "Context", "value": 42.5, "unit": "KB", "method": "count"}
                ]
            },
            "rows": []
        }))
        .unwrap();
        let m = measured.verdict.unwrap();
        assert_eq!(m.form.as_deref(), Some("measured"));
        let mags = m.magnitudes.unwrap();
        assert_eq!(mags[0].value, Some(42.5));
        assert_eq!(mags[0].unit.as_deref(), Some("KB"));
    }

    #[test]
    fn signal_categories_absent_from_json_is_omitted() {
        // A card without signal data must not synthesize `signalCategories`
        // in `--json` output — byte-identical to the pre-signal shape.
        let card = card_with_freshness(None);
        let val: serde_json::Value = serde_json::to_value(&card).unwrap();
        assert!(
            val.get("signalCategories").is_none(),
            "absent signalCategories must be omitted: {}",
            val
        );
    }

    #[test]
    fn signal_categories_json_forwards_when_present() {
        // Cards that DO carry signal data must forward it losslessly.
        let card = DirectoryCard {
            signal_categories: Some(vec![
                serde_json::from_value(sample_category_summary_json()).unwrap()
            ]),
            ..card_with_freshness(None)
        };
        let val: serde_json::Value = serde_json::to_value(&card).unwrap();
        assert_eq!(val["signalCategories"][0]["category"], "safety");
        assert_eq!(val["signalCategories"][0]["verdict"]["grade"], "C");
    }

    #[test]
    fn fmt_signal_categories_compact_lists_nonempty_categories_only() {
        let cats: Vec<SignalCategorySummary> = vec![
            serde_json::from_value(sample_category_summary_json()).unwrap(),
            serde_json::from_value(serde_json::json!({
                "category": "reliability",
                "label": "Reliability",
                "form": "graded",
                "verdict": null,
                "rows": []
            }))
            .unwrap(),
        ];
        let line = fmt_signal_categories_compact(&cats).unwrap();
        assert!(line.contains("Safety: C"));
        assert!(
            !line.contains("Reliability"),
            "empty categories must be skipped: {line}"
        );
    }

    #[test]
    fn fmt_signal_categories_compact_none_when_no_rows() {
        let cats: Vec<SignalCategorySummary> = vec![serde_json::from_value(serde_json::json!({
            "category": "safety",
            "label": "Safety",
            "form": "graded",
            "verdict": null,
            "rows": []
        }))
        .unwrap()];
        assert!(fmt_signal_categories_compact(&cats).is_none());
    }

    #[test]
    fn skill_signals_response_decodes_envelope() {
        // The public signals endpoint returns {subjectType, subjectId,
        // signals, categories}; the response struct must decode it.
        let resp: SkillSignalsResponse = serde_json::from_value(serde_json::json!({
            "subjectType": "skill_audit",
            "subjectId": "audit-1",
            "signals": [
                {
                    "origin": "coverage",
                    "id": "cov-1",
                    "subjectType": "skill_audit",
                    "subjectId": "audit-1",
                    "relatedType": "",
                    "relatedId": "",
                    "dataCategory": "safety",
                    "sourceClass": "scan",
                    "source": "vettd",
                    "ruleId": "VTD-0092",
                    "severity": null,
                    "label": "No behavioral signals",
                    "detail": null,
                    "valueNum": null,
                    "valueText": null,
                    "unit": null,
                    "method": null,
                    "derivation": null,
                    "confidence": null,
                    "sampleSize": null,
                    "synthetic": false,
                    "firstParty": true,
                    "observedAt": "2026-08-24T00:00:00.000Z",
                    "payload": null
                }
            ],
            "categories": [serde_json::from_value::<SignalCategorySummary>(sample_category_summary_json()).unwrap()]
        }))
        .unwrap();
        assert_eq!(resp.subject_type.as_deref(), Some("skill_audit"));
        assert_eq!(resp.subject_id.as_deref(), Some("audit-1"));
        assert_eq!(resp.signals.len(), 1);
        assert_eq!(resp.categories.len(), 1);
    }

    #[test]
    fn signals_json_passthrough_preserves_neutral_nulls_and_unknown_fields() {
        // The typed allow-list view drops neutral `null`s and unknown fields on
        // reserialize (`skip_serializing_if`), so `directory signals --json`
        // must print the raw endpoint Value verbatim rather than re-encoding
        // the typed view. This test guards the raw passthrough specifically.
        let raw: serde_json::Value = serde_json::json!({
            "subjectType": "skill_audit",
            "subjectId": "audit-1",
            "signals": [
                {
                    "origin": "signal",
                    "id": "sig-1",
                    "dataCategory": "safety",
                    "sourceClass": "scan",
                    "ruleId": "VTD-0001",
                    "severity": null,
                    "label": "Prompt injection",
                    "detail": null,
                    "valueNum": null,
                    "valueText": null,
                    "unit": null,
                    "method": null,
                    "derivation": null,
                    "confidence": null,
                    "sampleSize": null,
                    "synthetic": false,
                    "payload": null,
                    "futureField": {"anything": [1, 2, 3]}
                }
            ],
            "categories": []
        });
        let printed = render_signals_json(&raw);
        assert!(
            printed.contains("\"valueNum\": null"),
            "neutral null must survive --json: {printed}"
        );
        assert!(
            printed.contains("\"severity\": null"),
            "neutral null must survive --json: {printed}"
        );
        assert!(
            printed.contains("\"payload\": null"),
            "neutral null must survive --json: {printed}"
        );
        assert!(
            printed.contains("\"futureField\""),
            "unknown fields must survive --json: {printed}"
        );

        // And the typed view really is lossy here — proving why --json cannot
        // re-encode it (this is the regression this finding guards against).
        let typed: SkillSignalsResponse = serde_json::from_value(raw).unwrap();
        let reencoded = serde_json::to_string(&typed).unwrap();
        assert!(
            !reencoded.contains("\"valueNum\""),
            "typed reserialize must drop neutral nulls: {reencoded}"
        );
        assert!(
            !reencoded.contains("\"futureField\""),
            "typed reserialize must drop unknown fields: {reencoded}"
        );
    }

    // ── anonymous read transport contract (epic #879) ─────────────────

    /// A directory detail body as the API would serve it (carries the `id`
    /// that the signals drill-down reads as its `subjectId`).
    fn sample_detail_json(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "slug": "pdf-summarizer",
            "name": "PDF Summarizer",
            "description": null,
            "version": "1.0",
            "author": "test",
            "category": null,
            "overallGrade": "A",
            "license": "MIT",
            "sourceType": "github",
            "sourceUrl": null,
            "hasSkillMd": true,
            "hasScripts": false,
            "hasEvals": false,
            "fileCount": 3,
            "completedAt": "2026-08-24T00:00:00.000Z",
            "findings": [],
            "scannerRuns": [],
            "freshness": null
        })
    }

    #[test]
    fn directory_detail_and_signals_requests_send_no_authorization() {
        // The detail GET and the signals GET are public reads — the transport
        // (`read_client`) must never attach an `Authorization` header. Each
        // route gets a mock that 401s IF an authorization header is present,
        // plus the real 200 mock; a passing 200 therefore proves the header
        // was left off (same pattern as directory_download's resolve_download
        // test).
        let server = MockServer::start();
        let base = server.base_url();

        // Detail route: GET /api/directory/pdf-summarizer
        let detail_auth_401 = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/directory/pdf-summarizer")
                .header_exists("authorization");
            then.status(401).json_body(json!({"error": "unauthorized"}));
        });
        let detail_ok = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/directory/pdf-summarizer");
            then.status(200).json_body(sample_detail_json("audit-1"));
        });

        let detail = fetch_skill_url(&format!("{base}/api/directory/pdf-summarizer"))
            .unwrap_or_else(|e| panic!("detail GET must succeed without Authorization: {e}"));
        assert_eq!(detail.name, "PDF Summarizer");

        // Signals route: GET /api/assets/skill_audit/audit-1/signals
        let signals_auth_401 = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/assets/skill_audit/audit-1/signals")
                .header_exists("authorization");
            then.status(401).json_body(json!({"error": "unauthorized"}));
        });
        let signals_ok = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/assets/skill_audit/audit-1/signals");
            then.status(200).json_body(json!({
                "subjectType": "skill_audit",
                "subjectId": "audit-1",
                "signals": [],
                "categories": []
            }));
        });

        let raw = fetch_signals_url(&format!("{base}/api/assets/skill_audit/audit-1/signals"))
            .unwrap_or_else(|e| panic!("signals GET must succeed without Authorization: {e}"));
        assert_eq!(raw["subjectId"], "audit-1");

        assert_eq!(
            detail_auth_401.calls(),
            0,
            "detail request must not send Authorization"
        );
        assert_eq!(
            signals_auth_401.calls(),
            0,
            "signals request must not send Authorization"
        );
        assert_eq!(detail_ok.calls(), 1);
        assert_eq!(signals_ok.calls(), 1);
    }

    #[test]
    fn directory_detail_and_signals_404s_are_distinct_not_generic_errors() {
        let server = MockServer::start();
        let base = server.base_url();

        // 404 on the detail route → NotFound (not a generic ServerError).
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/directory/missing");
            then.status(404).json_body(json!({"error": "not found"}));
        });
        let detail_err = fetch_skill_url(&format!("{base}/api/directory/missing")).unwrap_err();
        assert!(
            matches!(detail_err, ReadError::NotFound),
            "detail 404 must be NotFound: {detail_err}"
        );

        // 404 on the signals route → NotFound.
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/assets/skill_audit/absent/signals");
            then.status(404)
                .json_body(json!({"error": "no signal record"}));
        });
        let signals_err =
            fetch_signals_url(&format!("{base}/api/assets/skill_audit/absent/signals"))
                .unwrap_err();
        assert!(
            matches!(signals_err, ReadError::NotFound),
            "signals 404 must be NotFound: {signals_err}"
        );

        // 500 on the signals route stays a ServerError — 404 is not folded in.
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/assets/skill_audit/boom/signals");
            then.status(500).json_body(json!({"error": "boom"}));
        });
        let boom =
            fetch_signals_url(&format!("{base}/api/assets/skill_audit/boom/signals")).unwrap_err();
        assert!(
            matches!(boom, ReadError::ServerError(500)),
            "signals 500 must stay ServerError: {boom}"
        );

        // The two 404 messages the handlers print are distinct per route.
        assert_ne!(
            skill_not_found_message("x"),
            signals_not_found_message("x"),
            "detail 404 and signals 404 must render different messages"
        );
        assert!(skill_not_found_message("pdf-summarizer").contains("pdf-summarizer"));
        assert!(signals_not_found_message("pdf-summarizer").contains("pdf-summarizer"));
    }

    // ── ANSI-aware width helpers ──────────────────────────────────────

    #[test]
    fn visible_width_ignores_ansi_escape_sequences() {
        // A colored cell's visible width must exclude the ESC[..m codes, or
        // the table columns would misalign.
        let colored = "\x1b[32m[ok]\x1b[0m";
        assert_eq!(visible_width(colored), 4);
        assert_eq!(colored.len(), 13, "raw length is polluted by ANSI bytes");
    }

    #[test]
    fn pad_to_visible_pads_to_visible_width_not_raw_len() {
        // `[offline]` is 9 visible chars (== FRESH_COL_W), filling the column
        // exactly; colored it carries ~9 extra ANSI bytes. Padding must add
        // ZERO spaces despite the raw byte length being far larger than the
        // target — the count is by VISIBLE width, not raw bytes.
        let colored = "\x1b[31m[offline]\x1b[0m";
        assert_eq!(visible_width(colored), FRESH_COL_W);
        assert_eq!(pad_to_visible(colored, FRESH_COL_W), colored);

        // `[ok]` is 4 visible chars — pad to 9 visible (5 trailing spaces).
        let colored_ok = "\x1b[32m[ok]\x1b[0m";
        let padded_ok = pad_to_visible(colored_ok, FRESH_COL_W);
        assert_eq!(visible_width(&padded_ok), FRESH_COL_W);
        assert!(
            padded_ok.ends_with("     "),
            "expected 5 trailing spaces, got: {:?}",
            padded_ok
        );
    }

    #[test]
    fn pad_to_visible_does_not_shrink_wide_content() {
        // When content already exceeds the target, it's returned unchanged —
        // the caller truncates separately (`truncate_to_display` on visible text).
        let long = "abcdefghijkl"; // 12 visible chars
        assert_eq!(pad_to_visible(long, FRESH_COL_W), long);
    }

    #[test]
    fn freshness_rows_align_name_column_across_statuses() {
        // The whole point of the fixed freshness column: a colored `[ok]` and
        // colored `[offline]` row must place the slug at the SAME column so the
        // name/description column is stable regardless of freshness label.
        // We reproduce the layout prefix used by `render_card_row`.
        let grade = "[A]";
        let grade_pad = " ".repeat(SAFETY_COL_W - grade.len());
        let cases = [
            "\x1b[32m[ok]\x1b[0m",
            "\x1b[31m[offline]\x1b[0m",
            "\x1b[2m[? ]\x1b[0m",
        ];
        let mut slug_cols = Vec::new();
        for c in cases {
            let fresh = pad_to_visible(c, FRESH_COL_W);
            let prefix = format!("{grade}{grade_pad}  {fresh}  ");
            slug_cols.push(visible_width(&prefix));
        }
        assert_eq!(
            slug_cols[0], slug_cols[1],
            "[ok] and [offline] rows must put slug at same column"
        );
        assert_eq!(
            slug_cols[1], slug_cols[2],
            "[offline] and [? ] rows must put slug at same column"
        );
    }

    #[test]
    fn describe_distinct_freshness_labels() {
        // Sanity: the compact status labels are all distinct and ≤ the fixed
        // column width, so no freshness cell overflows into the name column.
        let fresh = |status: &str| {
            crate::freshness::fmt_freshness_compact(&Some(&crate::freshness::PublicFreshness {
                status: status.into(),
                reason: None,
                retryable: false,
                renamed_to: None,
                last_checked_at: None,
                last_verified_at: None,
                last_change_detected_at: None,
                scanned_hash: None,
                latest_upstream_hash: None,
            }))
        };
        let labels = [
            fresh("verified_unchanged"),
            fresh("changed"),
            fresh("unreachable"),
            fresh("check_failed"),
        ];
        for l in &labels {
            assert!(l.len() <= FRESH_COL_W, "{l} exceeds fresh column width");
        }
        assert!(labels.windows(2).all(|w| w[0] != w[1]));
    }

    // ── card table renderer (vettd#879) ────────────────────────────────

    #[test]
    fn render_cards_table_uses_safety_header_and_signals_column() {
        // The table header must say `safety` (was `rating`), drop `scanned by`
        // entirely, and expose a `signals` column carrying the total signal
        // count. Each skill renders as exactly one line.
        let cards = vec![
            DirectoryCard {
                slug: Some("pr".into()),
                name: "pr".into(),
                overall_grade: Some("B".into()),
                source_type: Some("github".into()),
                signal_count: Some(24),
                ..card_with_freshness(None)
            },
            DirectoryCard {
                slug: Some("pdf-summarizer".into()),
                name: "pdf-summarizer".into(),
                overall_grade: Some("A".into()),
                source_type: Some("scan".into()),
                signal_count: Some(127),
                ..card_with_freshness(None)
            },
        ];
        let table = render_cards_table(&cards, true, 120);
        let lines: Vec<&str> = table.lines().collect();
        let header = lines[0];
        assert!(
            header.contains("safety"),
            "header must label the grade column `safety`: {header}"
        );
        assert!(
            !header.contains("scanned by"),
            "header must not contain `scanned by`: {header}"
        );
        assert!(
            header.contains("signals"),
            "header must include the `signals` column: {header}"
        );

        let row = strip_ansi(lines[2]);
        assert!(
            row.contains("[B]"),
            "row must render the grade badge: {row}"
        );
        assert!(
            row.contains("24"),
            "row must render the signal count: {row}"
        );
        assert!(
            !row.contains("signals:"),
            "no dangling `signals:` continuation line: {row}"
        );
        assert_eq!(
            lines.len(),
            4,
            "exactly header + separator + 2 rows — one line per skill"
        );
    }

    #[test]
    fn render_cards_table_drops_dangling_signals_continuation_line() {
        // A card carrying signal categories used to print a second
        // `signals: …` line under the row (vettd#981). That continuation line
        // is removed — every skill must still occupy exactly one table line.
        let card_with_signals = DirectoryCard {
            slug: Some("pr".into()),
            name: "pr".into(),
            signal_categories: Some(vec![
                serde_json::from_value(sample_category_summary_json()).unwrap()
            ]),
            ..card_with_freshness(None)
        };
        let plain_card = DirectoryCard {
            slug: Some("other".into()),
            name: "other".into(),
            ..card_with_freshness(None)
        };
        let table = render_cards_table(&[card_with_signals, plain_card], true, 120);
        let lines: Vec<&str> = table.lines().collect();
        for line in &lines {
            assert!(
                !line.contains("signals:"),
                "no row may carry a `signals:` continuation line: {line}"
            );
        }
        assert_eq!(
            lines.len(),
            4,
            "header + separator + 2 skills must stay 4 lines (one per skill)"
        );
    }

    #[test]
    fn render_cards_table_signals_absent_renders_dash() {
        // `signalCount` is additive from the server: against an older server
        // it is absent, and the signals column must render `—` (matching how
        // the removed `scanned by` column handled absence).
        let card = DirectoryCard {
            slug: Some("pr".into()),
            name: "pr".into(),
            overall_grade: Some("B".into()),
            source_type: Some("github".into()),
            signal_count: None,
            ..card_with_freshness(None)
        };
        let table = render_cards_table(&[card], true, 120);
        let row = strip_ansi(table.lines().nth(2).unwrap());
        let dash_at = row
            .find("—")
            .unwrap_or_else(|| panic!("signals column must render —: {row}"));
        assert!(
            row[..dash_at].contains("github"),
            "the — must sit in the signals column after source: {row}"
        );
        assert!(
            !row.contains("scanner"),
            "scanned-by wording must be gone: {row}"
        );
    }

    #[test]
    fn render_cards_table_inventory_variant_omits_freshness_column() {
        // Inventory reuses the shared renderer with `show_freshness = false`:
        // the header and rows must omit the freshness column but still carry
        // the safety header and the signals column.
        let card = DirectoryCard {
            slug: Some("pr".into()),
            name: "pr".into(),
            overall_grade: Some("B".into()),
            source_type: Some("github".into()),
            signal_count: Some(24),
            ..card_with_freshness(None)
        };
        let table = render_cards_table(&[card], false, 120);
        let lines: Vec<&str> = table.lines().collect();
        assert!(
            !lines[0].contains("fresh."),
            "inventory header must omit the freshness column: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("safety"),
            "inventory header keeps safety: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("signals"),
            "inventory header keeps signals: {}",
            lines[0]
        );
        let row = strip_ansi(lines[2]);
        assert!(
            row.contains("24"),
            "inventory row renders the signal count: {row}"
        );
        assert_eq!(
            lines.len(),
            3,
            "header + separator + 1 row — one line per asset"
        );
    }

    #[test]
    fn signal_count_serde_parses_and_stays_none_when_absent() {
        // `signalCount` is an additive camelCase field (vettd#879): it must
        // parse from the directory JSON and stay `None` when the server omits
        // it (pre-deploy), without synthesizing a null in `--json` output.
        let with_count: DirectoryCard = serde_json::from_value(serde_json::json!({
            "name": "pr",
            "slug": "pr",
            "signalCount": 24,
        }))
        .unwrap();
        assert_eq!(with_count.signal_count, Some(24));

        let without: DirectoryCard = serde_json::from_value(serde_json::json!({
            "name": "pr",
            "slug": "pr",
        }))
        .unwrap();
        assert_eq!(without.signal_count, None);
        let val = serde_json::to_value(&without).unwrap();
        assert!(
            val.get("signalCount").is_none(),
            "absent signalCount must be omitted from JSON: {}",
            val
        );

        // The detail payload carries the same additive field.
        let detail: DirectorySkillDetail = serde_json::from_value(serde_json::json!({
            "name": "pr",
            "slug": "pr",
            "findings": [],
            "scannerRuns": [],
            "signalCount": 7,
        }))
        .unwrap();
        assert_eq!(detail.signal_count, Some(7));
        let detail_val = serde_json::to_value(&detail).unwrap();
        assert_eq!(detail_val["signalCount"], 7);
    }

    // ── signals block (vettd#879) ─────────────────────────────────────

    fn env_row(rule_id: &str) -> SignalEnvelopeRow {
        SignalEnvelopeRow {
            rule_id: Some(rule_id.to_string()),
            ..SignalEnvelopeRow::default()
        }
    }

    fn env_row_num(rule_id: &str, value: f64, unit: Option<&str>) -> SignalEnvelopeRow {
        let mut r = env_row(rule_id);
        r.value_num = Some(value);
        r.unit = unit.map(|u| u.to_string());
        r
    }

    fn env_row_text(rule_id: &str, text: &str) -> SignalEnvelopeRow {
        let mut r = env_row(rule_id);
        r.value_text = Some(text.to_string());
        r
    }

    fn env_row_sev(rule_id: &str, sev: &str) -> SignalEnvelopeRow {
        let mut r = env_row(rule_id);
        r.severity = Some(sev.to_string());
        r
    }

    /// Split one ANSI-stripped two-column signals-block line into
    /// (label, a-value, b-value) using the unified compare geometry:
    /// 2-space indent, the shared table label column (`signals_table_label_w`),
    /// 2-space gap, the shared 30-wide value column (`COMPARE_VALUE_W`),
    /// 2-space gap, then value B. Derives the offsets from the same widths
    /// the renderer uses, so the parse always matches layout.
    fn split_signal_line(line: &str) -> (&str, &str, &str) {
        let label_w = signals_table_label_w();
        let label_end = 2 + label_w;
        let a_start = label_end + 2;
        let a_end = a_start + COMPARE_VALUE_W;
        let label = line[2..label_end].trim_end();
        let a = line[a_start..a_end].trim();
        let b = line[a_end + 2..].trim();
        (label, a, b)
    }

    #[test]
    fn render_signals_block_heading_total_and_one_line_per_signal() {
        // The block is: a `Signals` heading, a `Total signals` line, then
        // EXACTLY one line per selected signal — no wrapping or continuation.
        let rendered = render_signals_block(
            &[],
            Some(&empty_rows()),
            None,
            None,
            signals_table_label_w(),
        );
        let plain = strip_ansi(&rendered);
        let lines = plain.lines().collect::<Vec<_>>();
        assert_eq!(
            lines.len(),
            2 + SIGNAL_SELECTION.len(),
            "heading + total + one line per selected signal"
        );
        assert_eq!(lines[0].trim(), "Signals");

        let (label, a, b) = split_signal_line(lines[1]);
        assert_eq!(label, "Total signals");
        assert_eq!(a, "—", "absent signal_count must render —");
        assert_eq!(b, "—");

        for (_, label) in SIGNAL_SELECTION {
            let hits = lines.iter().filter(|l| l.contains(label)).count();
            assert_eq!(hits, 1, "label '{label}' must appear exactly once");
        }
        assert!(
            !plain.contains("Declared license"),
            "declared-license is not in the selection (the license row above already shows it)"
        );
    }

    #[test]
    fn render_signals_block_both_sides_and_dash_for_missing_side() {
        // Side A carries a numeric + a text signal; side B has no rows — every
        // B column must be `—`. The total-signals line uses each side's count.
        let a_rows = vec![
            env_row_num("performance/static-context-tokens", 12.5, Some("KB")),
            env_row_text("characteristics/primary-language", "Markdown"),
        ];
        let rendered = render_signals_block(
            &a_rows,
            Some(&empty_rows()),
            Some(24),
            None,
            signals_table_label_w(),
        );
        let plain = strip_ansi(&rendered);
        let lines = plain.lines().collect::<Vec<_>>();

        let (_, a, b) = split_signal_line(lines[1]);
        assert_eq!(a, "24");
        assert_eq!(b, "—");

        let (_, a, b) = split_signal_line(lines[2]); // Context tokens
        assert_eq!(
            a, "12.5 KB",
            "unit must follow valueNum with a single space"
        );
        assert_eq!(b, "—");

        let language_line = lines
            .iter()
            .find(|l| l.contains("Primary language"))
            .unwrap();
        let (_, a, b) = split_signal_line(language_line);
        assert_eq!(a, "Markdown");
        assert_eq!(b, "—");

        // No line may exceed the table's full width, so the block never wraps
        // at normal terminal widths (worst case: 2 + 17 label + 2 + 30 value
        // + 2 + 30 = 83).
        for line in &lines {
            assert!(
                line.chars().count()
                    <= 2 + signals_table_label_w() + 2 + COMPARE_VALUE_W + 2 + COMPARE_VALUE_W,
                "signals line too wide ({}) : {line}",
                line.chars().count()
            );
        }
    }

    #[test]
    fn render_signals_block_value_precedence() {
        // valueNum (unit appended after a single space, no trailing .0) beats
        // valueText; valueText beats severity; severity beats the matching-row
        // count; nothing → —.
        let rows = vec![
            env_row_num("sentiment/stars", 1024.0, None),
            env_row_text("reliability/eval-test-case-count", "12 tests"),
            env_row_sev("characteristics/archived", "low"),
            env_row("reliability/unresolvable-internal-references"),
            env_row("reliability/unresolvable-internal-references"),
            env_row_num("sentiment/last-commit-age", 0.0, Some("days")),
        ];
        let rendered = render_signals_block(
            &rows,
            Some(&empty_rows()),
            None,
            None,
            signals_table_label_w(),
        );
        let plain = strip_ansi(&rendered);
        let lines = plain.lines().collect::<Vec<_>>();

        // Selection order: Signals, Total, Context tokens, Eval test cases,
        // Unresolvable refs, Stars, Forks, Watchers, Open issues,
        // Last commit age, Archived, Primary language.
        let (_, a, _) = split_signal_line(lines[2]); // Context tokens — absent
        assert_eq!(a, "—");
        let (_, a, _) = split_signal_line(lines[3]); // Eval test cases — valueText
        assert_eq!(a, "12 tests");
        let (_, a, _) = split_signal_line(lines[4]); // Unresolvable refs — row count
        assert_eq!(a, "2");
        let (_, a, _) = split_signal_line(lines[5]); // Stars — valueNum
        assert_eq!(a, "1024", "valueNum must not print a trailing .0");
        let (_, a, _) = split_signal_line(lines[9]); // Last commit age — unit spacing
        assert_eq!(a, "0 days", "valueNum and unit joined by a single space");
        let (_, a, _) = split_signal_line(lines[10]); // Archived — severity
        assert_eq!(a, "low");
    }

    #[test]
    fn render_signals_block_empty_value_text_and_watchers() {
        // Empty or whitespace-only `valueText` carries no usable value: it must
        // fall through to the severity check and then to `—` — never render a
        // blank cell and never degrade into a bare row count (`1`) on a scalar
        // rule. A genuine multi-row list still gets its count, and the
        // `watchers` replacement signal renders its valueNum like the other
        // sentiment rows (the license is already shown by the License row).
        let mut sev_row = env_row_text("characteristics/archived", "  ");
        sev_row.severity = Some("low".to_string());
        let rows = vec![
            env_row_text("reliability/eval-test-case-count", ""),
            env_row_text("sentiment/forks", "   "),
            sev_row,
            env_row_num("sentiment/watchers", 1460.0, None),
            env_row("reliability/unresolvable-internal-references"),
            env_row("reliability/unresolvable-internal-references"),
        ];
        let rendered = render_signals_block(
            &rows,
            Some(&empty_rows()),
            None,
            None,
            signals_table_label_w(),
        );
        let plain = strip_ansi(&rendered);
        let lines = plain.lines().collect::<Vec<_>>();

        let (_, a, _) = split_signal_line(lines[3]); // Eval test cases — empty valueText
        assert_eq!(
            a, "—",
            "empty valueText must not render as a bare row count"
        );
        let (_, a, _) = split_signal_line(lines[6]); // Forks — whitespace-only valueText
        assert_eq!(
            a, "—",
            "whitespace-only valueText must be treated as absent"
        );
        let (_, a, _) = split_signal_line(lines[7]); // Watchers — valueNum
        assert_eq!(a, "1460");
        let (_, a, _) = split_signal_line(lines[4]); // Unresolvable refs — real list
        assert_eq!(a, "2", "a genuine multi-row list still gets the row count");
        let (_, a, _) = split_signal_line(lines[10]); // Archived — severity wins over blank valueText
        assert_eq!(a, "low", "blank valueText must fall through to severity");

        assert!(
            !plain.contains("Declared license"),
            "declared-license must be replaced by watchers"
        );
    }

    #[test]
    fn render_signals_block_longest_label_keeps_value_alignment() {
        // `Unresolvable refs` (17 chars) is the longest selected label, so it
        // defines the shared label column; its value columns sit at exactly
        // the same offsets as every other row's — the label must never push
        // its values right of the shared geometry.
        let rows = vec![
            env_row("reliability/unresolvable-internal-references"),
            env_row("reliability/unresolvable-internal-references"),
            env_row_num("sentiment/stars", 7.0, None),
        ];
        let rendered = render_signals_block(
            &rows,
            Some(&empty_rows()),
            None,
            None,
            signals_table_label_w(),
        );
        let plain = strip_ansi(&rendered);
        let lines = plain.lines().collect::<Vec<_>>();

        // The longest label parses cleanly at the shared offsets and its
        // values land in the value columns, not shifted by the label length.
        let (label, a, b) = split_signal_line(lines[4]); // Unresolvable refs
        assert_eq!(label, "Unresolvable refs");
        assert_eq!(a, "2");
        assert_eq!(b, "—");

        // Every line's label slice (heading + total + one per signal) is
        // exactly its expected label — nothing overflows into the gap or the
        // value columns, which all start at the same byte offsets.
        let label_w = signals_table_label_w();
        let label_end = 2 + label_w;
        let mut expect = vec!["Total signals"];
        for (_, label) in SIGNAL_SELECTION {
            expect.push(label);
        }
        for i in 0..expect.len() {
            // Line 0 is the `Signals` heading; `expect[i]` is line i + 1.
            assert_eq!(
                lines[i + 1][2..label_end].trim_end(),
                expect[i],
                "row {i}: label must fit its column without overflow"
            );
        }
    }

    #[test]
    fn compare_fixed_and_signal_rows_share_value_column_offsets() {
        // The unified compare geometry (one label width, one value width)
        // must put value A and value B at identical byte offsets on fixed
        // rows and signals-block rows alike. This is the alignment contract
        // behind the whole change: the signals block may not use its own
        // column widths.
        let label_w = signals_table_label_w();
        let val_w = COMPARE_VALUE_W;
        let a_start = 2 + label_w + 2;
        let b_start = a_start + val_w + 2;

        // A fixed row, formatted exactly as `handle_compare` prints it.
        let fixed = format!(
            "  {DIM}{:<label_w$}{RESET}  {:<val_w$}  {}",
            "Findings:",
            truncate_to_display("1 medium, 1 info", val_w),
            truncate_to_display("1 info", val_w),
            label_w = label_w,
            val_w = val_w,
        );
        let fixed = strip_ansi(&fixed);

        // A signals-block row (Stars).
        let block = render_signals_block(
            &[env_row_num("sentiment/stars", 1024.0, None)],
            Some(&empty_rows()),
            None,
            None,
            label_w,
        );
        let block_plain = strip_ansi(&block);
        let signal = block_plain.lines().nth(5).unwrap();

        // The columns START at the same offsets in both rows (value B is not
        // right-padded, exactly like the fixed rows' `{}` cell, so total row
        // widths may differ) — value A at `a_start`, value B at `b_start`.
        let cell = |line: &str, start: usize| -> String {
            line.chars().skip(start).take(COMPARE_VALUE_W).collect()
        };
        assert_eq!(cell(&fixed, a_start).trim(), "1 medium, 1 info");
        assert_eq!(cell(signal, a_start).trim(), "1024");
        assert_eq!(cell(&fixed, b_start).trim(), "1 info");
        assert_eq!(cell(signal, b_start).trim(), "—");
    }

    #[test]
    fn render_signals_block_single_column_view() {
        // `directory view` renders the same block with a single value column:
        // heading, Total signals, then one line per selected signal, each
        // value starting at the same column as view's own fixed rows (which
        // share the same label width) and never clipped.
        let rows = vec![
            env_row_num("performance/static-context-tokens", 899.0, None),
            env_row_text("characteristics/primary-language", "Markdown"),
        ];
        let label_w = signals_table_label_w();
        let rendered = render_signals_block(&rows, None, Some(24), None, label_w);
        let plain = strip_ansi(&rendered);
        let lines = plain.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2 + SIGNAL_SELECTION.len());

        let label_end = 2 + label_w;
        let a_start = label_end + 2;
        let cell = |line: &str, start: usize| -> String {
            line.chars().skip(start).take(COMPARE_VALUE_W).collect()
        };

        assert_eq!(lines[0].trim(), "Signals");
        assert_eq!(lines[1][2..label_end].trim_end(), "Total signals");
        assert_eq!(cell(lines[1], a_start).trim(), "24");
        assert_eq!(lines[2][2..label_end].trim_end(), "Context tokens");
        assert_eq!(cell(lines[2], a_start).trim(), "899");
        assert_eq!(
            cell(lines[3], a_start).trim(),
            "—",
            "missing row falls back to —"
        );
        let lang = lines
            .iter()
            .find(|l| l.contains("Primary language"))
            .unwrap();
        assert_eq!(
            cell(lang, a_start).trim(),
            "Markdown",
            "single-column values are never clipped"
        );

        // The block's value column starts where view's fixed rows start:
        // the same label width drives both.
        let fixed = format!(
            "  {DIM}{:<label_w$}{RESET}  {}",
            "Scanned by:",
            "scanner-suite",
            label_w = label_w,
        );
        assert_eq!(cell(&strip_ansi(&fixed), a_start).trim(), "scanner-suite");
    }

    #[test]
    fn signal_value_precedence_shared_formatter() {
        // The value formatter is the ONE shared by view and compare; its
        // precedence: valueNum (+unit) → non-empty valueText → severity →
        // multi-row list count → —.
        let rows = vec![
            env_row_num("sentiment/stars", 1024.0, Some("k")),
            env_row_text("reliability/eval-test-case-count", "12 tests"),
            env_row_sev("characteristics/archived", "low"),
            env_row("reliability/unresolvable-internal-references"),
            env_row("reliability/unresolvable-internal-references"),
            env_row("performance/static-context-tokens"),
        ];
        assert_eq!(signal_value(&rows, "sentiment/stars"), "1024 k");
        assert_eq!(
            signal_value(&rows, "reliability/eval-test-case-count"),
            "12 tests"
        );
        assert_eq!(signal_value(&rows, "characteristics/archived"), "low");
        assert_eq!(
            signal_value(&rows, "reliability/unresolvable-internal-references"),
            "2"
        );
        assert_eq!(
            signal_value(&rows, "performance/static-context-tokens"),
            "—",
            "a lone scalar row with no usable value is absent, not a bare 1"
        );
        assert_eq!(
            signal_value(&[], "sentiment/stars"),
            "—",
            "no rows at all renders —"
        );
        assert_eq!(
            signal_value(&[env_row_text("sentiment/forks", "   ")], "sentiment/forks"),
            "—",
            "whitespace-only valueText falls through to —"
        );
    }

    /// The empty side B for two-column renderer tests, so a missing side is
    /// exercised explicitly.
    fn empty_rows() -> Vec<SignalEnvelopeRow> {
        Vec::new()
    }
}
