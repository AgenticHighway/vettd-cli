//! Directory command implementations.
//!
//! All reads go through `crate::read_client` (no `Authorization` header).
//! Deserialization uses narrow allow-list structs — unknown fields are ignored,
//! so any server over-exposure is silently dropped rather than printed.

use serde::Deserialize;

use crate::read_client::{self, ReadError};

// ---------------------------------------------------------------------------
// Allow-list deserialization structs
//
// Fields here are limited to what we actually render. Any field the server
// returns that isn't listed is silently ignored (serde default = deny on
// unknown_fields is NOT set — that's intentional for forward compatibility).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListResponse {
    pub skills: Vec<DirectoryCard>,
    pub total: u32,
    pub page: u32,
    pub total_pages: u32,
}

#[derive(Debug, Deserialize)]
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
    pub download_count: Option<i64>,
    pub scanner_run_count: Option<u32>,
    pub security_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySkillDetail {
    pub slug: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub category: Option<String>,
    pub badge_status: Option<String>,
    pub overall_grade: Option<String>,
    pub download_count: Option<i64>,
    pub verdict_rationale: Option<String>,
    pub findings: Vec<DirectoryFinding>,
    pub scanner_runs: Vec<ScannerRun>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryFinding {
    pub severity: String,
    pub rule_id: Option<String>,
    pub category: Option<String>,
    pub label: String,
    pub detail: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
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
// Helpers
// ---------------------------------------------------------------------------

/// Derive the directory API base URL from the configured ingest endpoint.
fn directory_base_url() -> String {
    let endpoint = crate::submit::load_auth_config()
        .map(|c| c.endpoint)
        .unwrap_or_else(|| crate::submit::DEFAULT_PRODUCTION_ENDPOINT.to_string());
    crate::network::derive_api_url(&endpoint, "directory")
}

/// Percent-encode a query parameter value (UTF-8, RFC 3986 unreserved chars
/// pass through; everything else is `%XX` encoded).
fn percent_encode(s: &str) -> String {
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

/// Numeric value for a severity string (higher = more severe).
fn severity_value(s: &str) -> u8 {
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

/// Format a severity breakdown as a compact string, omitting zero counts.
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

/// Fetch a single skill detail, mapping errors to clear exit messages.
fn fetch_skill(slug: &str) -> DirectorySkillDetail {
    let base = directory_base_url();
    let url = format!("{base}/{}", percent_encode(slug));
    match read_client::fetch_json::<DirectorySkillDetail>(&url) {
        Ok(detail) => detail,
        Err(ReadError::NotFound) => {
            eprintln!("Error: skill '{slug}' not found (not public or does not exist).");
            std::process::exit(1);
        }
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

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

pub fn handle_list() {
    let url = format!("{}?sort=newest", directory_base_url());
    match read_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            println!(
                "{} skills (page {}/{}):\n",
                resp.total, resp.page, resp.total_pages
            );
            for card in &resp.skills {
                print_card(card);
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

pub fn handle_search(query: &str) {
    let url = format!(
        "{}?search={}&sort=newest",
        directory_base_url(),
        percent_encode(query)
    );
    match read_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            if resp.skills.is_empty() {
                println!("No results for \"{}\".", query);
            } else {
                println!(
                    "{} results for \"{}\" (page {}/{}):\n",
                    resp.total, query, resp.page, resp.total_pages
                );
                for card in &resp.skills {
                    print_card(card);
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

pub fn handle_view(slug: &str) {
    let detail = fetch_skill(slug);
    let (c, h, m, l, i) = count_by_severity(&detail.findings);
    let total = c + h + m + l + i;
    let ext_scanners = external_scanner_run_count(&detail.scanner_runs);

    println!("{}", detail.name);
    if let Some(desc) = &detail.description {
        println!("  {desc}");
    }
    println!();
    println!(
        "  Grade:    {}",
        detail.overall_grade.as_deref().unwrap_or("—")
    );
    println!(
        "  Status:   {}",
        detail.badge_status.as_deref().unwrap_or("—")
    );
    println!("  Version:  {}", detail.version.as_deref().unwrap_or("—"));
    println!("  Author:   {}", detail.author.as_deref().unwrap_or("—"));
    println!("  Category: {}", detail.category.as_deref().unwrap_or("—"));
    println!();
    println!(
        "  Findings:         {} total — {}",
        total,
        fmt_severity_breakdown(c, h, m, l, i)
    );
    println!("  External scanners: {ext_scanners}");
    if let Some(rationale) = &detail.verdict_rationale {
        println!();
        println!("  Verdict: {rationale}");
    }
}

pub fn handle_findings(slug: &str, min_severity: &str) {
    let detail = fetch_skill(slug);
    let min_val = severity_value(min_severity);

    let filtered: Vec<&DirectoryFinding> = detail
        .findings
        .iter()
        .filter(|f| severity_value(&f.severity) >= min_val)
        .collect();

    let total = detail.findings.len();
    let shown = filtered.len();

    println!(
        "Findings for {} (--min-severity {min_severity}):",
        detail.name
    );
    println!();

    if filtered.is_empty() {
        println!("  No findings at or above the '{min_severity}' severity threshold.");
    } else {
        for f in &filtered {
            let rule = f.rule_id.as_deref().unwrap_or("—");
            let src = f.source.as_deref().unwrap_or("—");
            println!("  [{}] {} ({})", f.severity.to_uppercase(), f.label, rule);
            if let Some(cat) = &f.category {
                println!("       Category: {cat}  |  Source: {src}");
            } else {
                println!("       Source: {src}");
            }
            if let Some(detail_text) = &f.detail {
                println!("       {detail_text}");
            }
            println!();
        }
        println!("  Showing {shown}/{total} findings (filter: >= {min_severity}).");
    }
}

pub fn handle_compare(slug_a: &str, slug_b: &str) {
    let same = slug_a == slug_b;
    let detail_a = fetch_skill(slug_a);
    // Avoid a redundant HTTP call when both slugs are identical.
    let detail_b = if same {
        // Safety: DirectorySkillDetail doesn't implement Clone, but we can
        // re-fetch — however the same slug means the same data, so we just
        // re-use the one we have by re-fetching. Given this is an edge case,
        // keep it simple.
        fetch_skill(slug_b)
    } else {
        fetch_skill(slug_b)
    };

    let (ca, ha, ma, la, ia) = count_by_severity(&detail_a.findings);
    let (cb, hb, mb, lb, ib) = count_by_severity(&detail_b.findings);
    let total_a = ca + ha + ma + la + ia;
    let total_b = cb + hb + mb + lb + ib;
    let scanners_a = external_scanner_run_count(&detail_a.scanner_runs);
    let scanners_b = external_scanner_run_count(&detail_b.scanner_runs);

    let slug_display_a = detail_a.slug.as_deref().unwrap_or(slug_a);
    let slug_display_b = detail_b.slug.as_deref().unwrap_or(slug_b);

    println!("{:<40}  {}", slug_display_a, slug_display_b);
    println!("{}", "─".repeat(80));
    println!("  Name:     {:<36}  {}", detail_a.name, detail_b.name);
    println!(
        "  Grade:    {:<36}  {}",
        detail_a.overall_grade.as_deref().unwrap_or("—"),
        detail_b.overall_grade.as_deref().unwrap_or("—")
    );
    println!(
        "  Status:   {:<36}  {}",
        detail_a.badge_status.as_deref().unwrap_or("—"),
        detail_b.badge_status.as_deref().unwrap_or("—")
    );
    println!();
    let findings_a = format!("{total_a} ({})", fmt_severity_breakdown(ca, ha, ma, la, ia));
    let findings_b = format!("{total_b} ({})", fmt_severity_breakdown(cb, hb, mb, lb, ib));
    println!("  Findings: {findings_a:<36}  {findings_b}");
    let scanners_a_s = format!("{scanners_a} external");
    let scanners_b_s = format!("{scanners_b} external");
    println!("  Scanners: {scanners_a_s:<36}  {scanners_b_s}");
}

// ---------------------------------------------------------------------------
// Card display helper
// ---------------------------------------------------------------------------

fn print_card(card: &DirectoryCard) {
    let grade = card.overall_grade.as_deref().unwrap_or("?");
    let slug = card.slug.as_deref().unwrap_or(&card.name);
    let category = card.category.as_deref().unwrap_or("—");
    let downloads = card
        .download_count
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".to_string());
    let scanners = card
        .scanner_run_count
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".to_string());

    println!("  [{grade}] {slug}  |  {category}  |  {scanners} scanners  |  {downloads} downloads");
    if let Some(desc) = &card.description {
        // Truncate long descriptions at 80 chars to keep list output compact.
        let truncated = if desc.len() > 78 {
            format!("{}…", &desc[..77])
        } else {
            desc.clone()
        };
        println!("       {truncated}");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            DirectoryFinding {
                severity: "high".to_string(),
                rule_id: None,
                category: None,
                label: "b".to_string(),
                detail: None,
                source: None,
            },
            DirectoryFinding {
                severity: "info".to_string(),
                rule_id: None,
                category: None,
                label: "c".to_string(),
                detail: None,
                source: None,
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

    #[test]
    fn fmt_severity_breakdown_mixed() {
        let s = fmt_severity_breakdown(1, 2, 0, 0, 3);
        assert!(s.contains("1 critical"));
        assert!(s.contains("2 high"));
        assert!(s.contains("3 info"));
        assert!(!s.contains("medium"));
    }
}
