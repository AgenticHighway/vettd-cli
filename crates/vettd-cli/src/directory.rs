//! Directory command implementations.
//!
//! All reads go through `crate::read_client` (no `Authorization` header).
//! Deserialization uses narrow allow-list structs — unknown fields are ignored,
//! so any server over-exposure is silently dropped rather than printed.

use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySkillDetail {
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

/// Normalize source type identifiers to display-friendly labels.
fn display_source_type(s: &str) -> &str {
    match s {
        "scan" => "cli",
        "zip" => "upload",
        other => other,
    }
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

fn api_sort_params(sort: &str, reverse: bool) -> String {
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
                print_cards(&resp.skills);
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

pub fn handle_search(query: &str, page: u32, sort: &str, reverse: bool, json: bool) {
    let url = format!(
        "{}?search={}&{}&page={page}",
        directory_base_url(),
        percent_encode(query),
        api_sort_params(sort, reverse),
    );
    match read_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else if resp.skills.is_empty() {
                println!("No results for \"{}\".", query);
            } else {
                print_cards(&resp.skills);
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

pub fn handle_view(slug: &str, json: bool) {
    let detail = fetch_skill(slug);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&detail).unwrap_or_default()
        );
        return;
    }
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
    println!("  {DIM}{:<13}{RESET}  {gc}{}{RESET}", "Grade:", grade_str);
    println!(
        "  {DIM}{:<13}{RESET}  {}",
        "Version:",
        detail.version.as_deref().unwrap_or("—")
    );
    println!(
        "  {DIM}{:<13}{RESET}  {}",
        "License:",
        detail.license.as_deref().unwrap_or("—")
    );
    println!(
        "  {DIM}{:<13}{RESET}  {}",
        "Author:",
        detail.author.as_deref().unwrap_or("—")
    );
    println!(
        "  {DIM}{:<13}{RESET}  {}",
        "Category:",
        detail.category.as_deref().unwrap_or("—")
    );
    println!("  {DIM}{:<13}{RESET}  {}", "Source:", source);
    if let Some(url) = &detail.source_url {
        if !url.is_empty() {
            println!("  {DIM}{:<13}{RESET}  {}", "Source URL:", url);
        }
    }
    println!("  {DIM}{:<13}{RESET}  {}", "Contains:", contains_str);
    println!();
    println!(
        "  {DIM}{:<13}{RESET}  {}",
        "Findings:",
        fmt_severity_breakdown_colored(c, h, m, l, i)
    );
    println!("  {DIM}{:<13}{RESET}  {}", "Scanned by:", scanned_by_str);
    println!("  {DIM}{:<13}{RESET}  {}", "Last scanned:", last_scanned);
    println!(
        "  {DIM}{:<13}{RESET}  {}",
        "Files:",
        detail
            .file_count
            .map(|n| n.to_string())
            .as_deref()
            .unwrap_or("—")
    );
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

    // Column geometry: 2 indent + 13 label + 2 sep + 30 value + 2 sep + right value
    let label_w: usize = 13;
    let val_w: usize = 30;
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
}

pub fn handle_trending() {
    let url = format!("{}?sort=downloads", directory_base_url());
    match read_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            println!(
                "Trending by downloads ({} skills, page {}/{}):",
                resp.total, resp.page, resp.total_pages
            );
            print_cards(&resp.skills);
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
                    Some(card) => print_cards(std::slice::from_ref(&card)),
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

/// Print a slice of cards as a padded, single-line-per-card table with a header.
///
/// Slug column width is computed from the batch so all rows align. Description
/// is truncated to fit the remaining terminal width.
fn print_cards(cards: &[DirectoryCard]) {
    let slug_w = cards
        .iter()
        .map(|c| c.slug.as_deref().unwrap_or(&c.name).len())
        .max()
        .unwrap_or(0);
    let term_w = terminal_width();

    println!(
        "{BOLD}{:<6}  {:<w$}  {:<10}  {:<12}  description{RESET}",
        "rating",
        "name",
        "source",
        "scanned by",
        w = slug_w,
    );
    println!("{DIM}{}{RESET}", "─".repeat(term_w.saturating_sub(5)));

    for card in cards {
        print_card_row(card, slug_w, term_w);
    }
}

fn print_card_row(card: &DirectoryCard, slug_w: usize, term_w: usize) {
    let grade = card.overall_grade.as_deref().unwrap_or("?");
    let gc = grade_color(grade);
    // Grade badge visual text (no ANSI) — always 3 chars like "[A]"
    let grade_visible = format!("[{grade}]");
    let grade_pad = " ".repeat(6usize.saturating_sub(grade_visible.len()));
    let grade_display = format!("{gc}{grade_visible}{RESET}{grade_pad}");

    let slug = card.slug.as_deref().unwrap_or(&card.name);
    let slug_padded = format!("{slug:<w$}", w = slug_w);
    let asset_type = card
        .source_type
        .as_deref()
        .map(display_source_type)
        .unwrap_or("—");
    let scanners = match card.scanner_run_count.map(|n| n + 1) {
        Some(1) => "1 scanner".to_string(),
        Some(n) => format!("{n} scanners"),
        None => "—".to_string(),
    };
    let desc = card.description.as_deref().unwrap_or("");

    // Compute desc budget from visual widths (ANSI codes are invisible).
    let visual_prefix_w = 6 + 2 + slug_w + 2 + 10 + 2 + 12 + 2;
    let desc_budget = term_w.saturating_sub(visual_prefix_w).saturating_sub(5);
    let desc_display = truncate_to_display(desc, desc_budget);
    println!("{grade_display}  {slug_padded}  {asset_type:<10}  {scanners:<12}  {DIM}{desc_display}{RESET}");
}

/// Read terminal width from `$COLUMNS`, falling back to 120.
fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120)
}

/// Truncate a string to at most `max` display characters, appending `…` if cut.
fn truncate_to_display(s: &str, max: usize) -> String {
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

    #[test]
    fn fmt_severity_breakdown_mixed() {
        let s = fmt_severity_breakdown(1, 2, 0, 0, 3);
        assert!(s.contains("1 critical"));
        assert!(s.contains("2 high"));
        assert!(s.contains("3 info"));
        assert!(!s.contains("medium"));
    }
}
