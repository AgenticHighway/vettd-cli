//! Human-readable output formatters for scan reports.
//!
//! Provides overview, full detail, and summary views of scan results.
//! All output uses ANSI escape codes for terminal coloring.

use std::collections::HashMap;

use crate::capabilities::derive_capabilities;
use crate::models::{ArtifactReport, ScanReport};
use crate::scoring::{
    SEVERITY_CRITICAL_SCORE, SEVERITY_HIGH_SCORE, SEVERITY_LOW_SCORE, SEVERITY_MEDIUM_SCORE,
};
use crate::verifier::{
    SEVERITY_CRITICAL, SEVERITY_HIGH, SEVERITY_INFO, SEVERITY_LOW, SEVERITY_MEDIUM,
};

// ── ANSI helpers ────────────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";

pub fn severity(score: i32) -> (&'static str, &'static str) {
    if score >= SEVERITY_CRITICAL_SCORE {
        ("CRITICAL", "\x1b[1;35m")
    } else if score >= SEVERITY_HIGH_SCORE {
        ("HIGH    ", "\x1b[31m")
    } else if score >= SEVERITY_MEDIUM_SCORE {
        ("MEDIUM  ", "\x1b[33m")
    } else if score >= SEVERITY_LOW_SCORE {
        ("LOW     ", "\x1b[36m")
    } else {
        ("INFO    ", "\x1b[2m")
    }
}

// ── Counting helpers (pure logic) ───────────────────────────────────────

fn count_by<T, F>(artifacts: &[T], key_fn: F) -> Vec<(String, usize)>
where
    T: std::borrow::Borrow<ArtifactReport>,
    F: Fn(&ArtifactReport) -> &str,
{
    let mut counts: HashMap<String, usize> = HashMap::new();
    for a in artifacts {
        *counts.entry(key_fn(a.borrow()).to_string()).or_default() += 1;
    }
    let mut pairs: Vec<_> = counts.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    pairs
}

fn artifact_location(a: &ArtifactReport) -> &str {
    a.metadata
        .get("paths")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
}

fn top_risk_reasons(a: &ArtifactReport) -> &[String] {
    let len = a.risk_reasons.len().min(2);
    &a.risk_reasons[..len]
}

// ── Shared helpers ──────────────────────────────────────────────────────

fn shorten_path(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

fn pretty_type(raw: &str) -> &str {
    match raw {
        "agents_md" => "AGENTS.md",
        "cursor_rules" => "Cursor rules",
        "prompt_config" => "prompt config",
        "skill" => "skill",
        "source_risk_surface" => "source risk surface",
        "mcp_config" => "MCP server config",
        "container_config" => "Docker config",
        "container_candidate" => "Docker candidate",
        "browser_footprint" => "browser footprint",
        other => other,
    }
}

fn status_icon(status: &str) -> (&'static str, &'static str) {
    match status {
        SEVERITY_CRITICAL | SEVERITY_HIGH => ("\x1b[31m✗\x1b[0m", "\x1b[31m"),
        SEVERITY_MEDIUM => ("\x1b[33m⚠\x1b[0m", "\x1b[33m"),
        _ => ("\x1b[32m✓\x1b[0m", "\x1b[32m"),
    }
}

fn status_rank(s: &str) -> u8 {
    match s {
        SEVERITY_CRITICAL => 5,
        SEVERITY_HIGH => 4,
        SEVERITY_MEDIUM => 3,
        SEVERITY_LOW => 2,
        SEVERITY_INFO => 1,
        _ => 0,
    }
}

// ── print_overview ──────────────────────────────────────────────────────

pub fn print_overview(report: &ScanReport, cmd_name: &str) {
    let w = 58;
    let line = format!("{DIM}{}{RESET}", "─".repeat(w));

    println!();
    println!("{line}");
    println!("  {BOLD}vettd{RESET} · AI Execution Inventory");
    println!("  Scanned: {CYAN}{}{RESET}", report.scanned_path);
    println!("{line}");

    if report.artifacts.is_empty() {
        println!();
        println!("  {DIM}No AI execution artifacts detected.{RESET}");
        println!();
        return;
    }

    // ── Posture headline ────────────────────────────────────────────
    let flagged_count = report
        .artifacts
        .iter()
        .filter(|a| {
            matches!(
                a.verification_status.as_str(),
                SEVERITY_CRITICAL | SEVERITY_HIGH
            )
        })
        .count();
    let review_count = report
        .artifacts
        .iter()
        .filter(|a| a.verification_status == SEVERITY_MEDIUM)
        .count();
    let clear_count = report
        .artifacts
        .iter()
        .filter(|a| matches!(a.verification_status.as_str(), SEVERITY_LOW | SEVERITY_INFO))
        .count();

    println!();
    let mut status_parts: Vec<String> = Vec::new();
    if flagged_count > 0 {
        status_parts.push(format!("\x1b[31m{flagged_count} flagged\x1b[0m"));
    }
    if review_count > 0 {
        status_parts.push(format!("\x1b[33m{review_count} review\x1b[0m"));
    }
    if clear_count > 0 {
        status_parts.push(format!("\x1b[32m{clear_count} clear\x1b[0m"));
    }
    println!(
        "  {BOLD}RISK{RESET}  {}  {DIM}({} artifact(s)){RESET}",
        status_parts.join(&format!(" {DIM}·{RESET} ")),
        report.artifacts.len()
    );
    println!("  {DIM}{}{RESET}", "─".repeat(w - 2));

    // ── Top 3 riskiest findings ─────────────────────────────────────
    let mut sorted: Vec<&ArtifactReport> = report.artifacts.iter().collect();
    sorted.sort_by(|a, b| {
        status_rank(&b.verification_status)
            .cmp(&status_rank(&a.verification_status))
            .then(b.risk_score.cmp(&a.risk_score))
    });

    const TOP_N: usize = 3;

    for a in sorted.iter().take(TOP_N) {
        print_risk_card(a);
    }

    // ── Remaining artifacts grouped by directory ────────────────────
    let remaining = &sorted[TOP_N.min(sorted.len())..];
    if !remaining.is_empty() {
        println!();
        println!("  {DIM}… and {} more:{RESET}", remaining.len());
        print_directory_summary(remaining);
    }

    // ── Save & share ────────────────────────────────────────────────
    println!();
    println!("  {BOLD}SAVE & SHARE{RESET}");
    println!("  {DIM}{}{RESET}", "─".repeat(w - 2));
    println!("  {DIM}vettd {cmd_name} --json{RESET}          {DIM}→{RESET} JSON to stdout");
    println!(
        "  {DIM}vettd {cmd_name} --out{RESET}           {DIM}→{RESET} write vettd-report.json"
    );
    println!("  {DIM}vettd {cmd_name} --submit{RESET}        {DIM}→{RESET} send to Vettd");
    println!();
}

fn print_risk_card(a: &ArtifactReport) {
    let loc = shorten_path(artifact_location(a));
    let (icon, color) = status_icon(&a.verification_status);
    let label = match a.verification_status.as_str() {
        SEVERITY_CRITICAL => "CRITICAL",
        SEVERITY_HIGH => "FLAGGED",
        SEVERITY_MEDIUM => "REVIEW",
        _ => "CLEAR",
    };
    let kind = pretty_type(&a.artifact_type);

    println!();
    println!(
        "  {icon} {color}{BOLD}{label}{RESET}  {BOLD}{kind}{RESET}{DIM}{:>width$}{RESET}",
        format!("risk {}", a.risk_score),
        width = 50 - label.len() - kind.len()
    );
    println!("    {DIM}{loc}{RESET}");

    let reasons = top_risk_reasons(a);
    if !reasons.is_empty() {
        println!("    {}", reasons.join(", "));
    }

    let caps = derive_capabilities(a);
    if !caps.is_empty() {
        println!("    {CYAN}{}{RESET}", caps.join(", "));
    }
}

/// Group remaining artifacts by parent directory and print a compact summary.
fn print_directory_summary(artifacts: &[&ArtifactReport]) {
    let mut by_dir: Vec<(String, Vec<&ArtifactReport>)> = Vec::new();
    let mut dir_index: HashMap<String, usize> = HashMap::new();

    for a in artifacts {
        let loc = artifact_location(a);
        let dir = std::path::Path::new(loc)
            .parent()
            .map(|p| shorten_path(&p.to_string_lossy()))
            .unwrap_or_else(|| shorten_path(loc));

        if let Some(&idx) = dir_index.get(&dir) {
            by_dir[idx].1.push(a);
        } else {
            dir_index.insert(dir.clone(), by_dir.len());
            by_dir.push((dir, vec![a]));
        }
    }

    // Sort by count descending
    by_dir.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (dir, members) in &by_dir {
        let type_counts = count_by(members, |a| &a.artifact_type);
        let parts: Vec<String> = type_counts
            .iter()
            .map(|(t, c)| {
                if *c == 1 {
                    pretty_type(t).to_string()
                } else {
                    format!("{c} {}", pretty_type(t))
                }
            })
            .collect();
        println!("     {DIM}{dir}/{RESET}  {parts}", parts = parts.join(", "));
    }
}

// ── print_human ─────────────────────────────────────────────────────────

pub fn print_human(report: &ScanReport, _cmd_name: &str) {
    let w = 58;
    let line = format!("{DIM}{}{RESET}", "─".repeat(w));

    println!();
    println!("{line}");
    println!("  {BOLD}vettd{RESET} · AI Execution Inventory  {DIM}(full detail){RESET}");
    println!("  Run ID:  {DIM}{}{RESET}", report.run_id);
    println!("  Scanned: {CYAN}{}{RESET}", report.scanned_path);
    println!("  Time:    {DIM}{}{RESET}", report.timestamp);
    println!("{line}");

    if report.artifacts.is_empty() {
        println!();
        println!("  {DIM}No AI execution artifacts detected.{RESET}");
        println!();
        return;
    }

    print_type_counts(report);
    print_artifact_details(report);
}

fn print_type_counts(report: &ScanReport) {
    let type_counts = count_by(&report.artifacts, |a| &a.artifact_type);

    println!();
    println!(
        "  {BOLD}INVENTORY{RESET}{DIM}{:>width$} artifact(s){RESET}",
        report.artifacts.len(),
        width = 36
    );
    println!("  {DIM}{}{RESET}", "─".repeat(56));
    for (atype, count) in &type_counts {
        let label = pretty_type(atype);
        println!("  {BOLD}{count:>4}{RESET}  {label}");
    }
    println!();
}

fn print_artifact_details(report: &ScanReport) {
    let mut sorted: Vec<&ArtifactReport> = report.artifacts.iter().collect();
    sorted.sort_by(|a, b| {
        status_rank(&b.verification_status)
            .cmp(&status_rank(&a.verification_status))
            .then(b.risk_score.cmp(&a.risk_score))
    });

    for (i, a) in sorted.iter().enumerate() {
        let loc = shorten_path(artifact_location(a));
        let (icon, color) = status_icon(&a.verification_status);
        let kind = pretty_type(&a.artifact_type);
        let hash_short = if a.artifact_hash.len() >= 12 {
            &a.artifact_hash[..12]
        } else if a.artifact_hash.is_empty() {
            "n/a"
        } else {
            &a.artifact_hash
        };
        let status_label = match a.verification_status.as_str() {
            SEVERITY_CRITICAL => "CRITICAL",
            SEVERITY_HIGH => "FLAGGED",
            SEVERITY_MEDIUM => "REVIEW",
            _ => "CLEAR",
        };

        println!(
            "  {icon} {color}{BOLD}{}{RESET}. {BOLD}{kind}{RESET}  {color}{status_label}{RESET}  risk {}{RESET}",
            i + 1,
            a.risk_score
        );
        println!("    {DIM}{loc}{RESET}");
        println!(
            "    {DIM}hash:{RESET} {hash_short}  \
{DIM}scope:{RESET} {}  \
{DIM}confidence:{RESET} {:.0}%",
            a.artifact_scope,
            a.confidence * 100.0
        );

        let reasons = top_risk_reasons(a);
        if !reasons.is_empty() {
            println!("    {DIM}Reason:{RESET} {}", reasons.join(", "));
        }

        let caps = derive_capabilities(a);
        if !caps.is_empty() {
            println!(
                "    {DIM}Capabilities:{RESET} {CYAN}{}{RESET}",
                caps.join(", ")
            );
        }

        if !a.signals.is_empty() {
            println!("    {DIM}Signals:{RESET} {}", a.signals.join(", "));
        }
        println!();
    }
}

// ── Human-readable signal/reason labels ─────────────────────────────────

fn humanize_reason(raw: &str) -> &str {
    // Strip the "(+N)" weight suffix to match the signal key
    let key = raw.split(" (+").next().unwrap_or(raw);
    match key {
        "keyword:api" => "Makes external API calls",
        "keyword:shell" => "Runs shell commands",
        "keyword:browser" => "Controls a browser",
        "keyword:execute" => "Executes code at runtime",
        "keyword:network" => "Accesses the network",
        "keyword:filesystem" => "Reads/writes the filesystem",
        "keyword:docker" => "Uses container runtimes",
        "keyword:system" => "Sets or overrides the system prompt",
        "keyword:permissions" => "Requests elevated permissions",
        "keyword:tools" => "Declares callable tools",
        "keyword:dependencies" => "Installs or runs dependencies",
        "keyword:secrets" => "References secrets or credentials",
        "keyword:instructions" => "Contains instruction directives",
        "credential_exposure_signal" => "Credential / secret exposure",
        _ if key.starts_with("secret:") => "Credential / secret exposure",
        _ if key.starts_with("ssrf:") => "Potential SSRF target or evasion pattern",
        "json_config:credential_connection_string" => {
            "JSON config embeds credentials in a connection string"
        }
        "json_config:credential_value" => "JSON config contains an embedded credential value",
        "json_config:metadata_url" => "JSON config references a metadata or localhost URL",
        "json_config:internal_url" => "JSON config references an internal-only URL",
        "json_config:c2_url" => "JSON config references a known collector or C2 URL",
        "source:dynamic_import" => "Source code uses import() with a non-literal argument",
        "source:nonliteral_require" => "Source code uses require() with a non-literal argument",
        "source:nonliteral_spawn" => "Source code spawns a process with a non-literal command",
        "source:ssrf_private_ip" => {
            "Source code makes a network call to a private or link-local IP"
        }
        "source:ssrf_internal_host" => "Source code makes a network call to an internal hostname",
        "source:sensitive_path_access" => "Source code accesses sensitive local credential paths",
        "cognitive_tampering:file_target" => {
            "Source code targets agent identity or instruction files"
        }
        "cognitive_tampering:file_write" => {
            "Source code may modify agent identity or instruction files"
        }
        "cognitive_tampering:role_override" => "Role-override prompt injection",
        "cognitive_tampering:instruction_injection" => "Instruction override / prompt injection",
        "cognitive_tampering:delimiter_framing" => "Prompt framing / delimiter injection",
        "cognitive_tampering:unicode_steganography" => "Hidden unicode instruction markers",
        "cognitive_tampering:base64_encoded" => "Base64-encoded instruction content",
        "mcp_server_declared" => "Declares an MCP server",
        "dangerous_combo:shell+network+fs" => "Shell + network + filesystem combined",
        "dangerous_keyword:exfiltrate" => "References data exfiltration",
        "dangerous_keyword:wipe" => "References destructive wiping",
        "dangerous_keyword:rm" => "References file deletion",
        "dangerous_keyword:steal" => "References data theft",
        "dangerous_keyword:upload" => "References data upload",
        "dangerous_keyword:reverse" => "References reverse connection",
        "dangerous_keyword:disable" => "References disabling protections",
        "dangerous_keyword:bypass" => "References bypassing controls",
        "extensions_directory_present" => "Browser extensions directory found",
        _ if key.starts_with("extension_count:") => "Browser extensions installed",
        _ if key.starts_with("mcp_server_count:") => "Multiple MCP servers declared",
        _ => raw,
    }
}

// ── print_summary ───────────────────────────────────────────────────────

pub fn print_summary(report: &ScanReport, _cmd_name: &str) {
    let w = 58;
    let line = format!("{DIM}{}{RESET}", "─".repeat(w));

    println!();
    println!("{line}");
    println!("  {BOLD}vettd{RESET} · Summary");
    println!("{line}");

    if report.artifacts.is_empty() {
        println!();
        println!("  {DIM}No AI execution artifacts detected.{RESET}");
        println!();
        return;
    }

    let eligible: Vec<&ArtifactReport> = report
        .artifacts
        .iter()
        .filter(|a| a.registry_eligible)
        .collect();
    let counts = count_by_status(&eligible);
    let flagged_n = counts.get(SEVERITY_CRITICAL).copied().unwrap_or(0)
        + counts.get(SEVERITY_HIGH).copied().unwrap_or(0);
    let review_n = counts.get(SEVERITY_MEDIUM).copied().unwrap_or(0);
    let clear_n = counts.get(SEVERITY_LOW).copied().unwrap_or(0)
        + counts.get(SEVERITY_INFO).copied().unwrap_or(0);

    // ── Posture headline ────────────────────────────────────────────
    println!();
    let mut parts: Vec<String> = Vec::new();
    if flagged_n > 0 {
        parts.push(format!("\x1b[31m{flagged_n} flagged\x1b[0m"));
    }
    if review_n > 0 {
        parts.push(format!("\x1b[33m{review_n} need review\x1b[0m"));
    }
    if clear_n > 0 {
        parts.push(format!("\x1b[32m{clear_n} clear\x1b[0m"));
    }
    println!("  {}", parts.join(&format!("  {DIM}·{RESET}  ")));

    // ── Flagged items (listed individually — usually few) ───────────
    let mut sorted: Vec<&ArtifactReport> = eligible.to_vec();
    sorted.sort_by(|a, b| {
        status_rank(&b.verification_status)
            .cmp(&status_rank(&a.verification_status))
            .then(b.risk_score.cmp(&a.risk_score))
    });

    let flagged: Vec<&&ArtifactReport> = sorted
        .iter()
        .filter(|a| {
            matches!(
                a.verification_status.as_str(),
                SEVERITY_CRITICAL | SEVERITY_HIGH
            )
        })
        .collect();

    if !flagged.is_empty() {
        println!();
        println!("  \x1b[31m{BOLD}FLAGGED{RESET} {DIM}── investigate before use{RESET}");
        println!("  {DIM}{}{RESET}", "─".repeat(w - 2));
        for a in &flagged {
            print_summary_card(a);
        }
    }

    // ── Review items (grouped by top risk reason) ───────────────────
    let review: Vec<&&ArtifactReport> = sorted
        .iter()
        .filter(|a| a.verification_status == SEVERITY_MEDIUM)
        .collect();

    if !review.is_empty() {
        println!();
        println!("  \x1b[33m{BOLD}NEEDS REVIEW{RESET} {DIM}── restrict until reviewed{RESET}");
        println!("  {DIM}{}{RESET}", "─".repeat(w - 2));
        print_review_groups(&review, w);
    }

    // ── Clear items (compact one-liner) ─────────────────────────────
    if clear_n > 0 {
        println!();
        println!(
            "  \x1b[32m{BOLD}CLEAR{RESET} {DIM}── {clear_n} artifact(s) passed all checks{RESET}"
        );
    }

    // ── Compact inventory ───────────────────────────────────────────
    let type_counts = count_by(&report.artifacts, |a| &a.artifact_type);
    let inventory_parts: Vec<String> = type_counts
        .iter()
        .map(|(t, c)| format!("{c} {}", pretty_type(t)))
        .collect();
    println!();
    println!(
        "  {DIM}Scanned {total} artifact(s): {list}{RESET}",
        total = report.artifacts.len(),
        list = inventory_parts.join(", ")
    );
    println!();
}

fn print_summary_card(a: &ArtifactReport) {
    let loc = shorten_path(artifact_location(a));
    let (icon, _) = status_icon(&a.verification_status);
    let kind = pretty_type(&a.artifact_type);

    println!(
        "  {icon}  {BOLD}{kind}{RESET}  {DIM}risk {}{RESET}",
        a.risk_score
    );
    println!("     {DIM}{loc}{RESET}");

    let reasons = top_risk_reasons(a);
    if !reasons.is_empty() {
        let human: Vec<&str> = reasons.iter().map(|r| humanize_reason(r)).collect();
        println!("     {CYAN}{}{RESET}", human.join(", "));
    }
}

/// Group review artifacts by their top risk reason and show counts.
fn print_review_groups(items: &[&&ArtifactReport], _w: usize) {
    // Build groups keyed by the first (highest-weight) risk reason.
    let mut groups: Vec<(String, Vec<&ArtifactReport>)> = Vec::new();
    let mut group_map: HashMap<String, usize> = HashMap::new();

    for a in items {
        let key = a
            .risk_reasons
            .first()
            .map(|r| humanize_reason(r).to_string())
            .unwrap_or_else(|| "Other".to_string());

        if let Some(&idx) = group_map.get(&key) {
            groups[idx].1.push(a);
        } else {
            group_map.insert(key.clone(), groups.len());
            groups.push((key, vec![a]));
        }
    }

    // Sort groups by count descending
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    const MAX_EXAMPLES: usize = 2;

    for (reason, members) in &groups {
        let n = members.len();
        let plural = if n == 1 { "" } else { "s" };
        println!("  \x1b[33m⚠{RESET}  {BOLD}{reason}{RESET}  {DIM}({n} artifact{plural}){RESET}");

        // Show a couple of example paths
        for a in members.iter().take(MAX_EXAMPLES) {
            let loc = shorten_path(artifact_location(a));
            let kind = pretty_type(&a.artifact_type);
            println!("     {DIM}{kind} · {loc}{RESET}");
        }
        if n > MAX_EXAMPLES {
            println!("     {DIM}… and {} more{RESET}", n - MAX_EXAMPLES);
        }
    }
}

fn count_by_status(artifacts: &[&ArtifactReport]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for a in artifacts {
        *counts.entry(a.verification_status.clone()).or_default() += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ArtifactReport;
    use crate::verifier::{
        SEVERITY_CRITICAL, SEVERITY_HIGH, SEVERITY_INFO, SEVERITY_LOW, SEVERITY_MEDIUM,
    };
    use serde_json::json;

    fn make_artifact(atype: &str, risk: i32, status: &str) -> ArtifactReport {
        let mut a = ArtifactReport::new(atype, 0.8);
        a.risk_score = risk;
        a.verification_status = status.to_string();
        a.metadata.insert("paths".to_string(), json!(["/tmp/test"]));
        a
    }

    #[test]
    fn severity_critical() {
        assert_eq!(severity(90).0, "CRITICAL");
        assert_eq!(severity(100).0, "CRITICAL");
    }

    #[test]
    fn severity_high() {
        assert_eq!(severity(70).0, "HIGH    ");
        assert_eq!(severity(89).0, "HIGH    ");
    }

    #[test]
    fn severity_medium() {
        assert_eq!(severity(40).0, "MEDIUM  ");
        assert_eq!(severity(69).0, "MEDIUM  ");
    }

    #[test]
    fn severity_low() {
        assert_eq!(severity(10).0, "LOW     ");
        assert_eq!(severity(39).0, "LOW     ");
    }

    #[test]
    fn severity_info() {
        assert_eq!(severity(0).0, "INFO    ");
        assert_eq!(severity(9).0, "INFO    ");
    }

    #[test]
    fn pretty_type_known() {
        assert_eq!(pretty_type("agents_md"), "AGENTS.md");
        assert_eq!(pretty_type("cursor_rules"), "Cursor rules");
        assert_eq!(pretty_type("prompt_config"), "prompt config");
        assert_eq!(pretty_type("skill"), "skill");
        assert_eq!(pretty_type("source_risk_surface"), "source risk surface");
        assert_eq!(pretty_type("mcp_config"), "MCP server config");
        assert_eq!(pretty_type("container_config"), "Docker config");
        assert_eq!(pretty_type("container_candidate"), "Docker candidate");
        assert_eq!(pretty_type("browser_footprint"), "browser footprint");
    }

    #[test]
    fn pretty_type_unknown_passthrough() {
        assert_eq!(pretty_type("something_else"), "something_else");
    }

    #[test]
    fn status_icon_critical_and_high_are_red() {
        let (icon_c, _) = status_icon(SEVERITY_CRITICAL);
        let (icon_h, _) = status_icon(SEVERITY_HIGH);
        assert!(icon_c.contains("✗"));
        assert!(icon_h.contains("✗"));
    }

    #[test]
    fn status_icon_medium_is_warning() {
        let (icon, _) = status_icon(SEVERITY_MEDIUM);
        assert!(icon.contains("⚠"));
    }

    #[test]
    fn status_icon_low_and_info_are_clear() {
        let (icon_l, _) = status_icon(SEVERITY_LOW);
        let (icon_i, _) = status_icon(SEVERITY_INFO);
        assert!(icon_l.contains("✓"));
        assert!(icon_i.contains("✓"));
    }

    #[test]
    fn status_rank_ordering() {
        assert!(status_rank(SEVERITY_CRITICAL) > status_rank(SEVERITY_HIGH));
        assert!(status_rank(SEVERITY_HIGH) > status_rank(SEVERITY_MEDIUM));
        assert!(status_rank(SEVERITY_MEDIUM) > status_rank(SEVERITY_LOW));
        assert!(status_rank(SEVERITY_LOW) > status_rank(SEVERITY_INFO));
    }

    #[test]
    fn count_by_groups_by_type() {
        let artifacts = vec![
            make_artifact("prompt_config", 10, SEVERITY_INFO),
            make_artifact("prompt_config", 20, SEVERITY_LOW),
            make_artifact("agents_md", 50, SEVERITY_HIGH),
        ];
        let counts = count_by(&artifacts, |a| &a.artifact_type);
        let prompt_count = counts.iter().find(|(k, _)| k == "prompt_config").unwrap().1;
        let agents_count = counts.iter().find(|(k, _)| k == "agents_md").unwrap().1;
        assert_eq!(prompt_count, 2);
        assert_eq!(agents_count, 1);
    }

    #[test]
    fn count_by_sorted_descending() {
        let artifacts = vec![
            make_artifact("prompt_config", 10, SEVERITY_INFO),
            make_artifact("agents_md", 50, SEVERITY_HIGH),
            make_artifact("agents_md", 60, SEVERITY_HIGH),
            make_artifact("agents_md", 70, SEVERITY_CRITICAL),
        ];
        let counts = count_by(&artifacts, |a| &a.artifact_type);
        assert_eq!(counts[0].0, "agents_md");
        assert_eq!(counts[0].1, 3);
    }

    #[test]
    fn count_by_status_counts_correctly() {
        let a1 = make_artifact("prompt_config", 10, SEVERITY_INFO);
        let a2 = make_artifact("prompt_config", 50, SEVERITY_HIGH);
        let a3 = make_artifact("prompt_config", 30, SEVERITY_MEDIUM);
        let a4 = make_artifact("prompt_config", 15, SEVERITY_LOW);
        let refs: Vec<&ArtifactReport> = vec![&a1, &a2, &a3, &a4];
        let counts = count_by_status(&refs);
        assert_eq!(counts.get(SEVERITY_INFO).copied().unwrap_or(0), 1);
        assert_eq!(counts.get(SEVERITY_HIGH).copied().unwrap_or(0), 1);
        assert_eq!(counts.get(SEVERITY_MEDIUM).copied().unwrap_or(0), 1);
        assert_eq!(counts.get(SEVERITY_LOW).copied().unwrap_or(0), 1);
    }

    #[test]
    fn artifact_location_from_paths() {
        let a = make_artifact("prompt_config", 10, SEVERITY_INFO);
        assert_eq!(artifact_location(&a), "/tmp/test");
    }

    #[test]
    fn artifact_location_unknown_when_missing() {
        let a = ArtifactReport::new("prompt_config", 0.8);
        assert_eq!(artifact_location(&a), "unknown");
    }

    #[test]
    fn top_risk_reasons_max_two() {
        let mut a = make_artifact("prompt_config", 50, SEVERITY_HIGH);
        a.risk_reasons = vec![
            "reason1".to_string(),
            "reason2".to_string(),
            "reason3".to_string(),
        ];
        assert_eq!(top_risk_reasons(&a).len(), 2);
    }

    #[test]
    fn top_risk_reasons_empty() {
        let a = make_artifact("prompt_config", 10, SEVERITY_INFO);
        assert!(top_risk_reasons(&a).is_empty());
    }

    #[test]
    fn humanize_reason_known_keys() {
        assert_eq!(humanize_reason("keyword:api"), "Makes external API calls");
        assert_eq!(humanize_reason("keyword:shell"), "Runs shell commands");
        assert_eq!(
            humanize_reason("credential_exposure_signal"),
            "Credential / secret exposure"
        );
        assert_eq!(
            humanize_reason("secret:github:pat"),
            "Credential / secret exposure"
        );
        assert_eq!(
            humanize_reason("ssrf:metadata:aws"),
            "Potential SSRF target or evasion pattern"
        );
        assert_eq!(
            humanize_reason("cognitive_tampering:role_override"),
            "Role-override prompt injection"
        );
        assert_eq!(
            humanize_reason("dangerous_combo:shell+network+fs"),
            "Shell + network + filesystem combined"
        );
        assert_eq!(
            humanize_reason("json_config:c2_url"),
            "JSON config references a known collector or C2 URL"
        );
        assert_eq!(
            humanize_reason("source:nonliteral_spawn"),
            "Source code spawns a process with a non-literal command"
        );
        assert_eq!(
            humanize_reason("cognitive_tampering:file_write"),
            "Source code may modify agent identity or instruction files"
        );
    }

    #[test]
    fn humanize_reason_strips_weight_suffix() {
        assert_eq!(
            humanize_reason("keyword:api (+10)"),
            "Makes external API calls"
        );
    }

    #[test]
    fn humanize_reason_passthrough_unknown() {
        assert_eq!(humanize_reason("something_custom"), "something_custom");
    }
}
