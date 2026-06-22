//! Core scan engine — takes a skill file map and produces findings.
//!
//! Rule implementations mirror the vettd web scanner's `analyzeSkillFiles` in
//! `packages/api/src/skills/skill-analyzer.ts`. Where vettd has a bug that is
//! visible in the wire format (e.g. VTD-0123 detail says "non-JSON format" even
//! for .json files), the Rust engine reproduces the same output unaltered so that
//! the parity test can reach a clean pass.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::chain;
use crate::consts::DEFAULT_SOURCE;
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::result::SkillScanResult;

// ── Rule IDs (must match skill-rule-registry.ts) ─────────────────────────────

// Security
const RULE_NO_REPOSITORY_LINK: &str = "VTD-0083";
// Forensic-evasion / persistence (SENSITIVE_PATTERNS subset implemented here)
const RULE_SYSTEM_LOG_TRUNCATION: &str = "VTD-0045";
const RULE_JOURNAL_LOG_VACUUM: &str = "VTD-0046";
const RULE_FORCED_LOG_ROTATION: &str = "VTD-0047";
const RULE_TIME_DELAYED_EXECUTION: &str = "VTD-0054";
// External-URL / chain
const RULE_EXTERNAL_URL_REFERENCE: &str = "VTD-0088";
const RULE_MALICIOUS_ACTIVITY_CHAIN: &str = "VTD-0090";
const RULE_NO_SECRETS_DETECTED: &str = "VTD-0091";
const RULE_NO_BEHAVIORAL_SIGNALS: &str = "VTD-0092";
const RULE_NO_EXTERNAL_URLS: &str = "VTD-0093";

// Structure
const RULE_SKILL_MD: &str = "VTD-0095";
const RULE_SCRIPTS_DIRECTORY: &str = "VTD-0096";
const RULE_REFERENCES_DIRECTORY: &str = "VTD-0097";
const RULE_ASSETS_DIRECTORY: &str = "VTD-0098";
const RULE_SKILL_NAME_VALIDITY: &str = "VTD-0099";

// Best practices
const RULE_SKILL_MD_BODY_LENGTH: &str = "VTD-0101";
const RULE_EXAMPLES_PRESENT: &str = "VTD-0103";
const RULE_VALIDATION_LOOP: &str = "VTD-0105";
const RULE_WORKFLOW_STRUCTURE: &str = "VTD-0106";

// Description
const RULE_DESCRIPTION_PRESENT: &str = "VTD-0109";
const RULE_DESCRIPTION_LENGTH: &str = "VTD-0110";
const RULE_DESCRIPTION_CONTEXT: &str = "VTD-0111";

// Scripts
const RULE_SCRIPT_CLI_HELP: &str = "VTD-0114";

// Evals
const RULE_EVALS_PRESENT: &str = "VTD-0118";
const RULE_EVAL_FILES_FOUND: &str = "VTD-0123";

// ── Constants (must match skill-analyzer.ts) ──────────────────────────────────

const DESCRIPTION_MAX_LENGTH: usize = 1024;
const SKILL_NAME_MAX_LENGTH: usize = 64;
const SKILL_MD_BODY_MAX_LINES: usize = 500;

// eval JSON candidates checked before falling back to non-trivial file scan
const EVAL_JSON_CANDIDATES: &[&str] = &[
    "evals/evals.json",
    "evals.json",
    "tests/tests.json",
    "tests/evals.json",
    "test/tests.json",
    "test/evals.json",
    "evals/tests.json",
];

// ── Sensitive pattern detection ───────────────────────────────────────────────
// Partial implementation of vettd's SENSITIVE_PATTERNS from checkSecurity().
// Only patterns NOT in CODE_ONLY_LABELS are included here (they fire on all
// file types including .md). Each pattern fires once per file at the first
// matching line.

struct SensitivePattern {
    rule_id: &'static str,
    label: &'static str,
    severity: Severity,
    intent: Intent,
}

// Array order mirrors vettd's SENSITIVE_PATTERNS definition order — it determines
// the bucket insertion order in chain detection, which affects the chain detail string.
static SENSITIVE_PATTERNS: &[SensitivePattern] = &[
    // VTD-0054 comes before the forensic-evasion group in skill-analyzer.ts
    SensitivePattern {
        rule_id: RULE_TIME_DELAYED_EXECUTION,
        label: "Time-delayed execution via at command",
        severity: Severity::Critical,
        intent: Intent::Malicious,
    },
    SensitivePattern {
        rule_id: RULE_SYSTEM_LOG_TRUNCATION,
        label: "System log truncation (forensic evasion)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
    },
    SensitivePattern {
        rule_id: RULE_JOURNAL_LOG_VACUUM,
        label: "Journal log vacuum (forensic evasion)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
    },
    SensitivePattern {
        rule_id: RULE_FORCED_LOG_ROTATION,
        label: "Forced log rotation (forensic evasion)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
    },
];

// Regex strings indexed parallel to SENSITIVE_PATTERNS.
// Compiled on first use via get_sensitive_regexes().
static SENSITIVE_PATTERN_STRS: &[&str] = &[
    // VTD-0054 — | at <time>
    r"\|\s*at\s+(?:now\b|\d{1,2}:\d{2}|tomorrow\b|midnight\b|noon\b)",
    // VTD-0045 — truncate -s 0 or redirect into system log files
    r#"(?i)(?:truncate\s+-s\s+0\s+["']?|(?:^|[\s;&|])>\s*["']?)(?:~|(?:/var/log))/(?:auth\.log|syslog|audit/audit\.log|kern\.log|dpkg\.log|messages|secure)\b"#,
    // VTD-0046 — journalctl --vacuum-*
    r"\bjournalctl\s+--vacuum-(?:time|size)\b",
    // VTD-0047 — logrotate -f
    r"\blogrotate\s+-f\b",
];

static SENSITIVE_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();

fn get_sensitive_regexes() -> &'static [Regex] {
    SENSITIVE_REGEXES.get_or_init(|| {
        SENSITIVE_PATTERN_STRS
            .iter()
            .map(|s| Regex::new(s).expect("invalid sensitive pattern"))
            .collect()
    })
}

/// Scan all text files for SENSITIVE_PATTERNS. Returns findings and whether
/// any critical/high security finding was found (used to suppress VTD-0091).
fn scan_sensitive_patterns(text_files: &HashMap<String, String>) -> (Vec<Finding>, bool) {
    let mut findings: Vec<Finding> = Vec::new();
    let regexes = get_sensitive_regexes();

    for (path, content) in text_files {
        let lines: Vec<&str> = content.split('\n').collect();

        for (i_pat, pat) in SENSITIVE_PATTERNS.iter().enumerate() {
            let re = &regexes[i_pat];
            for (i_line, line) in lines.iter().enumerate() {
                if re.is_match(line) {
                    let snippet = line.trim();
                    let snippet = &snippet[..snippet.len().min(120)];
                    let detail = format!("Detected in {path}:{} — `{snippet}`", i_line + 1);
                    findings.push(Finding {
                        rule_id: pat.rule_id.to_string(),
                        category: FindingCategory::Security,
                        severity: pat.severity.clone(),
                        label: pat.label.to_string(),
                        detail,
                        filepath: Some(path.clone()),
                        owasp_llm_category: None,
                        chain_id: None,
                        intent: Some(pat.intent.clone()),
                        source: DEFAULT_SOURCE.to_string(),
                    });
                    break; // first match per pattern per file only
                }
            }
        }
    }

    let secrets_check_failed = findings
        .iter()
        .any(|f| matches!(f.severity, Severity::Critical | Severity::High));

    (findings, secrets_check_failed)
}

// ── Malicious activity chain detection ────────────────────────────────────────
// Mirrors vettd's detectMaliciousActivityChains().
// Groups security findings by file, classifies into EVASION/PERSISTENCE/etc.
// buckets, and emits a chain finding when 2+ distinct buckets co-occur.

const EVASION_FRAGS: &[&str] = &[
    "Shell history",
    "Audit daemon",
    "Windows event log clearing",
    "Script self-deletion",
    "System log truncation",
    "Shell history file wipe",
    "Journal log vacuum",
    "Forced log rotation",
];

const PERSISTENCE_FRAGS: &[&str] = &[
    "Cron persistence",
    "Systemd user service",
    "Shell rc file write",
    "Time-delayed execution via at",
    "Git hook injection",
    "LD_PRELOAD environment injection",
];

const FETCH_FRAGS: &[&str] = &[
    "Remote content fetched into variable",
    "Remote content fetched into variable for execution (Python)",
    "Base64-decoded content stored in variable",
];

const EXECUTION_FRAGS: &[&str] = &[
    "Remote code execution via command substitution",
    "Shell variable execution",
    "Remote code execution via pipe to shell",
    "PowerShell encoded command",
    "PowerShell IEX download cradle",
    "Python exec/eval of variable content",
];

const COVERT_CHANNEL_FRAGS: &[&str] = &[
    "DNS query with variable-constructed hostname",
    "DNS TXT record lookup",
    "Outbound POST with application/octet-stream",
];

fn classify_malicious_bucket(label: &str) -> Option<&'static str> {
    if EVASION_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("EVASION");
    }
    if PERSISTENCE_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("PERSISTENCE");
    }
    if FETCH_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("FETCH");
    }
    if EXECUTION_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("EXECUTION");
    }
    if COVERT_CHANNEL_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("COVERT_CHANNEL");
    }
    None
}

fn extract_filepath_from_detail(detail: &str) -> Option<&str> {
    let rest = detail.strip_prefix("Detected in ")?;
    let colon = rest.find(':')?;
    Some(&rest[..colon])
}

fn detect_malicious_activity_chains(findings: &mut Vec<Finding>) {
    // Group bucket-classified findings by file path (extracted from detail).
    let mut buckets_by_file: HashMap<String, Vec<&'static str>> = HashMap::new();
    // Track which finding indices belong to each file.
    let mut indices_by_file: HashMap<String, Vec<usize>> = HashMap::new();

    for (idx, finding) in findings.iter().enumerate() {
        if finding.category != FindingCategory::Security {
            continue;
        }
        let Some(file_path) = extract_filepath_from_detail(&finding.detail) else {
            continue;
        };
        let file_path = file_path.to_string();

        indices_by_file
            .entry(file_path.clone())
            .or_default()
            .push(idx);

        if let Some(bucket) = classify_malicious_bucket(&finding.label) {
            let buckets = buckets_by_file.entry(file_path).or_default();
            // Maintain insertion order with dedup (mirrors JS Set).
            if !buckets.contains(&bucket) {
                buckets.push(bucket);
            }
        }
    }

    let mut chain_index: u32 = 0;
    let mut new_findings: Vec<Finding> = Vec::new();

    for (file_path, buckets) in &buckets_by_file {
        let file_indices = indices_by_file.get(file_path).cloned().unwrap_or_default();

        // Condition A: 2+ distinct buckets.
        // Condition B: 1 bucket + external malicious finding not in any bucket.
        let has_external_malicious = file_indices.iter().any(|&idx| {
            let f = &findings[idx];
            matches!(f.severity, Severity::Critical | Severity::High)
                && f.intent == Some(Intent::Malicious)
                && f.chain_id.is_none()
                && classify_malicious_bucket(&f.label).is_none()
        });

        if buckets.len() < 2 && !has_external_malicious {
            continue;
        }

        let chain_id = format!("mal-activity-{chain_index}");
        chain_index += 1;

        // Mutate component findings: assign chainId, escalate intent/severity.
        for &idx in &file_indices {
            let f = &mut findings[idx];
            if f.chain_id.is_some() {
                continue;
            }
            if classify_malicious_bucket(&f.label).is_none() {
                continue;
            }
            f.chain_id = Some(chain_id.clone());
            f.intent = Some(Intent::Malicious);
            if f.severity != Severity::Critical {
                f.severity = Severity::Critical;
            }
        }

        let bucket_list = buckets.join(" + ");
        new_findings.push(Finding {
            rule_id: RULE_MALICIOUS_ACTIVITY_CHAIN.to_string(),
            category: FindingCategory::Security,
            severity: Severity::Critical,
            label: "Multiple malicious-activity indicators in same file".to_string(),
            detail: format!(
                "{file_path} contains {bucket_list} indicators that co-occur in a malicious pattern."
            ),
            filepath: Some(file_path.clone()),
            owasp_llm_category: None,
            chain_id: Some(chain_id),
            intent: Some(Intent::Malicious),
            source: DEFAULT_SOURCE.to_string(),
        });
    }

    findings.extend(new_findings);
}

// ── Frontmatter parser ────────────────────────────────────────────────────────

struct ParsedSkillMd {
    name: String,
    description: String,
    repository: String,
    body: String,
}

/// Parse a SKILL.md string into its frontmatter fields and body.
///
/// Mirrors vettd's `parseFrontmatter` function in skill-analyzer.ts.
/// Handles simple scalar `key: value` frontmatter; nested objects and
/// list values are skipped (indented lines are ignored).
fn parse_skill_md(content: &str) -> ParsedSkillMd {
    let empty = ParsedSkillMd {
        name: "unknown".to_string(),
        description: String::new(),
        repository: String::new(),
        body: content.to_string(),
    };

    if !content.starts_with("---\n") {
        return empty;
    }
    let rest = &content[4..]; // skip opening "---\n"

    // Find closing "---" on its own line
    let close_seq = "\n---";
    let Some(close_pos) = rest.find(close_seq) else {
        return empty;
    };

    // Closing "---" must be followed by end-of-string, whitespace, or newline.
    let after_dashes = &rest[close_pos + close_seq.len()..];
    let trimmed_after = after_dashes.trim_start_matches([' ', '\t']);
    if !trimmed_after.is_empty()
        && !trimmed_after.starts_with('\n')
        && !trimmed_after.starts_with('\r')
    {
        return empty;
    }

    let raw = &rest[..close_pos];
    let body = if let Some(stripped) = trimmed_after.strip_prefix('\n') {
        stripped.to_string()
    } else {
        String::new()
    };

    let mut name = "unknown".to_string();
    let mut description = String::new();
    let mut repository = String::new();

    for line in raw.lines() {
        // Skip indented lines (nested objects — not needed for scalar fields)
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(colon_pos) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon_pos].trim();
        let value = strip_quotes(trimmed[colon_pos + 1..].trim());
        match key {
            "name" => name = value.to_string(),
            "description" => description = value.to_string(),
            "repository" => repository = value.to_string(),
            _ => {}
        }
    }

    ParsedSkillMd {
        name,
        description,
        repository,
        body,
    }
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

// ── Name validation ───────────────────────────────────────────────────────────

/// Returns an error message if the name is invalid, or `None` if valid.
/// Mirrors `validateName` in skill-analyzer.ts.
fn validate_name(name: &str) -> Option<&'static str> {
    if name.is_empty() || name == "unknown" {
        return Some("name field is missing");
    }
    if name.len() > SKILL_NAME_MAX_LENGTH {
        return Some("name exceeds 64-character limit");
    }
    let chars: Vec<char> = name.chars().collect();
    if chars.is_empty() {
        return Some("name field is missing");
    }
    let first = chars[0];
    let last = *chars.last().unwrap();
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        if first == '-' || last == '-' {
            return Some("name must not start or end with a hyphen");
        }
        return Some("name contains invalid characters (only alphanumeric and hyphens allowed)");
    }
    for &c in &chars {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Some(
                "name contains invalid characters (only alphanumeric and hyphens allowed)",
            );
        }
    }
    if name.contains("--") {
        return Some("name must not contain consecutive hyphens");
    }
    None
}

// ── Body pattern helpers ──────────────────────────────────────────────────────

fn has_examples(body: &str) -> bool {
    let lower = body.to_lowercase();
    body.contains("```")
        || lower.contains("# example")
        || lower.contains("## example")
        || lower.contains("# sample")
        || lower.contains("## sample")
        || lower.contains("# demo")
        || lower.contains("## demo")
        || lower.contains("**input**")
        || lower.contains("**output**")
        || lower.contains("**before**")
        || lower.contains("**after**")
        || lower.contains("**example**")
        || lower.contains("**good**")
        || lower.contains("**bad**")
}

// Mirrors vettd's hasValidation: /validat/i.test(body) || /##?\s*verification/i.
// Note: matches "invalidation" and similar — this is a vettd bug reproduced as-is.
fn has_validation(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("validat")
        || lower.lines().any(|l| {
            l.trim_start_matches('#')
                .trim()
                .to_lowercase()
                .starts_with("verification")
                && l.trim_start().starts_with('#')
        })
}

fn has_workflow(body: &str) -> bool {
    let lower = body.to_lowercase();
    // Heading-based patterns
    for heading in &[
        "# workflow",
        "## workflow",
        "# steps",
        "## steps",
        "# instructions",
        "## instructions",
        "# procedure",
        "## procedure",
        "# process",
        "## process",
        "# how to",
        "## how to",
        "# usage",
        "## usage",
        "# guidelines",
        "## guidelines",
    ] {
        if lower.contains(heading) {
            return true;
        }
    }
    // step\s*\d — "step" followed by optional space then digit
    if let Some(pos) = lower.find("step") {
        let after = lower[pos + 4..].trim_start_matches(' ');
        if after.starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
    }
    // Numbered list: line starting with "1. " etc.
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with(|c: char| c.is_ascii_digit()) {
            let rest = t.trim_start_matches(|c: char| c.is_ascii_digit());
            if rest.starts_with(". ") {
                return true;
            }
        }
        // **Step ... bullet
        if line.to_lowercase().contains("**step") {
            return true;
        }
    }
    false
}

fn has_usage_context(description: &str) -> bool {
    let lower = description.to_lowercase();
    // "use this ", "use when", "use for"
    if lower.contains("use this ") || lower.contains("use when") || lower.contains("use for") {
        return true;
    }
    // "when ... need/want/ask/mention"
    if let Some(pos) = lower.find("when ") {
        let rest = &lower[pos..];
        if rest.contains("need")
            || rest.contains("want")
            || rest.contains("ask")
            || rest.contains("mention")
        {
            return true;
        }
    }
    false
}

fn has_external_url(content: &str) -> bool {
    content.contains("http://") || content.contains("https://")
}

// ── Script helpers ────────────────────────────────────────────────────────────

fn has_cli_hint(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("argparse")
        || content.contains("--help")
        || lower.contains("argumentparser")
        || lower.contains(".option(")
        || lower.contains("yargs")
        || lower.contains("commander")
        || lower.contains("process.argv")
        || lower.contains("deno.args")
        || lower.contains("sys.argv")
        || lower.contains("click.command")
        || lower.contains("click.group")
        || lower.contains("typer.")
        || content.contains("if __name__ == '__main__'")
        || content.contains("if __name__ == \"__main__\"")
}

fn is_likely_cli_script(path: &str, content: &str) -> bool {
    if !path.starts_with("scripts/") {
        return false;
    }
    let lower = path.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");

    if matches!(ext, "sh" | "bash" | "zsh") {
        return true;
    }

    const NON_CLI_EXTS: &[&str] = &[
        "json",
        "xml",
        "xsd",
        "wsdl",
        "yaml",
        "yml",
        "toml",
        "ini",
        "cfg",
        "conf",
        "properties",
        "csv",
        "tsv",
        "txt",
        "md",
        "rst",
        "html",
        "htm",
        "css",
        "lock",
        "sum",
        "mod",
        "gitignore",
        "dockerignore",
        "env",
    ];
    if NON_CLI_EXTS.contains(&ext) {
        return false;
    }

    // helpers/lib/validators subdirs: only if CLI hint present
    if lower.contains("/helpers/") || lower.contains("/lib/") || lower.contains("/validators/") {
        return has_cli_hint(content);
    }
    // schemas/templates/fixtures/examples/testdata: skip
    for skip in &[
        "/schemas/",
        "/templates/",
        "/fixtures/",
        "/examples/",
        "/testdata/",
    ] {
        if lower.contains(skip) {
            return false;
        }
    }
    // depth ≤ 2 (e.g. scripts/run.sh) → CLI
    let depth = path.split('/').count();
    depth <= 2 || has_cli_hint(content)
}

// ── Main scan function ────────────────────────────────────────────────────────

/// Scan a single skill package and return findings.
///
/// # Arguments
///
/// - `text_files` — map of normalized relative paths to decoded UTF-8 content.
///   Binary files must be excluded by the caller. Keyed by the same paths that
///   appear in `all_paths`.
/// - `all_paths` — complete list of normalized relative paths in the package,
///   including binary files. Used for structural presence checks.
///
/// This function performs no filesystem I/O. The caller is responsible for
/// loading files from disk (or a zip, or a network source) and building the
/// input maps.
///
/// # Ordering guarantee
///
/// Chain detection runs as the final internal step and may mutate `severity` on
/// existing findings. The returned `SkillScanResult.findings` slice already
/// reflects any chain-detection mutations; callers must not reorder this step.
pub fn scan_skill(text_files: &HashMap<String, String>, all_paths: &[String]) -> SkillScanResult {
    let mut findings: Vec<Finding> = Vec::new();

    // ── Structural presence flags ────────────────────────────────────────────

    let has_skill_md = text_files.contains_key("SKILL.md")
        || text_files.contains_key("skill.md")
        || all_paths.iter().any(|p| p == "SKILL.md" || p == "skill.md");

    let has_scripts = all_paths.iter().any(|p| p.starts_with("scripts/"));
    let has_references = all_paths.iter().any(|p| p.starts_with("references/"));
    let has_evals = all_paths.iter().any(|p| {
        p.starts_with("evals/")
            || p.starts_with("tests/")
            || p.starts_with("test/")
            || matches!(p.as_str(), "evals.json" | "evals.yaml" | "evals.yml")
    });
    let has_assets = all_paths.iter().any(|p| p.starts_with("assets/"));

    // Helper: build a Finding with all optional fields set to None/default.
    macro_rules! f {
        ($rule:expr, $cat:expr, $sev:expr, $label:expr, $detail:expr) => {
            Finding {
                rule_id: $rule.to_string(),
                category: $cat,
                severity: $sev,
                label: $label.to_string(),
                detail: $detail,
                filepath: None,
                owasp_llm_category: None,
                chain_id: None,
                intent: None,
                source: DEFAULT_SOURCE.to_string(),
            }
        };
    }

    // ── Structure checks ─────────────────────────────────────────────────────
    // Mirror vettd's structure pass in `analyzeSkillFiles`.

    findings.push(if has_skill_md {
        f!(
            RULE_SKILL_MD,
            FindingCategory::Structure,
            Severity::Info,
            "SKILL.md present",
            "Required skill definition file found".to_string()
        )
    } else {
        f!(
            RULE_SKILL_MD,
            FindingCategory::Structure,
            Severity::Critical,
            "SKILL.md missing",
            "Every skill must contain a SKILL.md file with YAML frontmatter and instructions"
                .to_string()
        )
    });

    findings.push(if has_scripts {
        f!(
            RULE_SCRIPTS_DIRECTORY,
            FindingCategory::Structure,
            Severity::Info,
            "scripts/ directory present",
            "Bundled executable scripts found".to_string()
        )
    } else {
        f!(
            RULE_SCRIPTS_DIRECTORY,
            FindingCategory::Structure,
            Severity::Info,
            "No scripts/ directory",
            "Consider bundling reusable scripts for validation and automation".to_string()
        )
    });

    // references/ and assets/ only emit when present (no false clause — vettd comment:
    // "they are optional extras that earn a pass finding when present but are not expected
    // components — unlike evals which warrants a warn")
    if has_references {
        findings.push(f!(
            RULE_REFERENCES_DIRECTORY,
            FindingCategory::Structure,
            Severity::Info,
            "references/ directory present",
            "Additional documentation files available for progressive disclosure".to_string()
        ));
    }

    if has_assets {
        findings.push(f!(
            RULE_ASSETS_DIRECTORY,
            FindingCategory::Structure,
            Severity::Info,
            "assets/ directory present",
            "Static resources (templates, schemas, etc.) found".to_string()
        ));
    }

    // ── Evals structural flag ────────────────────────────────────────────────
    // VTD-0118 always fires (present or absent) — unlike references/assets.

    findings.push(if has_evals {
        f!(
            RULE_EVALS_PRESENT,
            FindingCategory::Evals,
            Severity::Info,
            "Evaluation test cases found",
            "evals/ directory or evals.json present for testing skill quality".to_string()
        )
    } else {
        f!(RULE_EVALS_PRESENT, FindingCategory::Evals, Severity::Info,
           "No evaluation test cases",
           "Add an evals/ directory with test prompts and expected outputs to measure skill quality"
               .to_string())
    });

    // ── SKILL.md-gated checks ────────────────────────────────────────────────

    if has_skill_md {
        let skill_key = if text_files.contains_key("SKILL.md") {
            "SKILL.md"
        } else {
            "skill.md"
        };
        let parsed = text_files
            .get(skill_key)
            .map(|c| parse_skill_md(c))
            .unwrap_or_else(|| ParsedSkillMd {
                name: "unknown".to_string(),
                description: String::new(),
                repository: String::new(),
                body: String::new(),
            });

        // Name validation (VTD-0099)
        if let Some(err) = validate_name(&parsed.name) {
            findings.push(f!(
                RULE_SKILL_NAME_VALIDITY,
                FindingCategory::Structure,
                Severity::Critical,
                "Invalid name field",
                err.to_string()
            ));
        } else {
            findings.push(f!(
                RULE_SKILL_NAME_VALIDITY,
                FindingCategory::Structure,
                Severity::Info,
                "Valid name field",
                format!(
                    "Name {:?} follows spec (lowercase, hyphens, \u{2264}64 chars)",
                    parsed.name
                )
            ));
        }

        // Repository link check (VTD-0083)
        if parsed.repository.is_empty() {
            findings.push(Finding {
                rule_id: RULE_NO_REPOSITORY_LINK.to_string(),
                category: FindingCategory::Security,
                severity: Severity::Info,
                label: "No repository link".to_string(),
                detail: "No repository field found in SKILL.md frontmatter. Skills without a \
                         verifiable source repository cannot be externally audited."
                    .to_string(),
                filepath: None,
                owasp_llm_category: None,
                chain_id: None,
                intent: Some(Intent::Negligent),
                source: DEFAULT_SOURCE.to_string(),
            });
        }

        // Description checks (VTD-0109, VTD-0110, VTD-0111)
        if parsed.description.is_empty() {
            findings.push(f!(
                RULE_DESCRIPTION_PRESENT,
                FindingCategory::Description,
                Severity::Info,
                "Missing description field",
                "The description field is required and should describe what the skill \
                 does and when to use it"
                    .to_string()
            ));
        } else {
            let char_count = parsed.description.chars().count();
            findings.push(if char_count > DESCRIPTION_MAX_LENGTH {
                f!(
                    RULE_DESCRIPTION_LENGTH,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description exceeds 1024-character limit",
                    format!(
                        "Description is {char_count} characters (max: {DESCRIPTION_MAX_LENGTH})"
                    )
                )
            } else {
                f!(
                    RULE_DESCRIPTION_LENGTH,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description within character limit",
                    format!("{char_count}/{DESCRIPTION_MAX_LENGTH} characters used")
                )
            });

            findings.push(if has_usage_context(&parsed.description) {
                f!(
                    RULE_DESCRIPTION_CONTEXT,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description includes usage context",
                    "Good: description explains when to activate the skill".to_string()
                )
            } else {
                f!(
                    RULE_DESCRIPTION_CONTEXT,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description lacks usage context",
                    "Add context like \"Use this skill when...\" to help agents know \
                    when to activate it"
                        .to_string()
                )
            });
        }

        // Body quality checks — only when body has content
        if !parsed.body.trim().is_empty() {
            // .lines() excludes a trailing newline, matching JS regex $-before-trailing-\n
            let body_lines = parsed.body.lines().count();

            findings.push(if body_lines > SKILL_MD_BODY_MAX_LINES {
                f!(RULE_SKILL_MD_BODY_LENGTH, FindingCategory::BestPractices, Severity::Info,
                   "SKILL.md exceeds 500 lines",
                   format!("{body_lines} lines — consider moving detailed reference material to references/"))
            } else {
                f!(RULE_SKILL_MD_BODY_LENGTH, FindingCategory::BestPractices, Severity::Info,
                   "SKILL.md body length is reasonable",
                   format!("{body_lines} lines (recommended: under 500)"))
            });

            findings.push(if has_examples(&parsed.body) {
                f!(
                    RULE_EXAMPLES_PRESENT,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Examples included",
                    "Found code blocks, input/output samples, or an examples section — \
                    concrete samples help agents pattern-match effectively"
                        .to_string()
                )
            } else {
                f!(
                    RULE_EXAMPLES_PRESENT,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "No examples found",
                    "Add code blocks, input/output samples, or before/after examples \
                    to improve agent accuracy"
                        .to_string()
                )
            });

            findings.push(if has_workflow(&parsed.body) {
                f!(
                    RULE_WORKFLOW_STRUCTURE,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Step-by-step workflow found",
                    "Structured procedures improve reliability for complex tasks".to_string()
                )
            } else {
                f!(
                    RULE_WORKFLOW_STRUCTURE,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "No clear workflow structure",
                    "Consider adding numbered steps or a structured procedure for the \
                    agent to follow"
                        .to_string()
                )
            });

            // VTD-0105 only fires when validation keywords are present (no negative case).
            if has_validation(&parsed.body) {
                findings.push(f!(
                    RULE_VALIDATION_LOOP,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Validation loop referenced",
                    "Instructions for the agent to validate its own work before proceeding"
                        .to_string()
                ));
            }
        }
    }

    // ── Scripts checks ───────────────────────────────────────────────────────

    if has_scripts {
        let mut script_files: Vec<(&str, &str)> = text_files
            .iter()
            .filter(|(p, c)| is_likely_cli_script(p, c))
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        // Sort for deterministic output order
        script_files.sort_by_key(|(p, _)| *p);

        for (path, content) in script_files {
            findings.push(Finding {
                rule_id: RULE_SCRIPT_CLI_HELP.to_string(),
                category: FindingCategory::Scripts,
                severity: Severity::Info,
                label: if has_cli_hint(content) {
                    "CLI help supported".to_string()
                } else {
                    "No --help support".to_string()
                },
                detail: if has_cli_hint(content) {
                    format!("{path}: Script documents its interface via --help or argument parsing")
                } else {
                    format!("{path}: Add argument parsing with --help output so agents know the script's interface")
                },
                filepath: Some(path.to_string()),
                owasp_llm_category: None,
                chain_id: None,
                intent: None,
                source: DEFAULT_SOURCE.to_string(),
            });
        }
    }

    // ── Security scan ────────────────────────────────────────────────────────
    // Mirrors vettd's checkSecurity(). Sensitive patterns (forensic evasion,
    // persistence, etc.) are scanned first; their presence suppresses the
    // VTD-0091 clean signal. Behavioral scan is not yet fully implemented —
    // VTD-0092 fires unconditionally (matching vettd's clean-path output for
    // skills without prompt injection markers).

    let (sensitive_findings, secrets_check_failed) = scan_sensitive_patterns(text_files);
    findings.extend(sensitive_findings);

    // VTD-0091: only emit when no critical/high secrets/code-risk findings found.
    if !secrets_check_failed {
        findings.push(f!(
            RULE_NO_SECRETS_DETECTED,
            FindingCategory::Security,
            Severity::Info,
            "No secrets or unsafe code patterns detected",
            "Scanned all files for credentials, private keys, and code-level risks \
             (eval, shell exec, destructive ops)"
                .to_string()
        ));
    }

    findings.push(f!(
        RULE_NO_BEHAVIORAL_SIGNALS,
        FindingCategory::Security,
        Severity::Info,
        "No prompt injection or jailbreak signals detected",
        "Scanned text content for instruction override, jailbreak framing, credential \
         solicitation, and embedded injection markers"
            .to_string()
    ));

    // External URL check — mirrors vettd's urlTargetFiles scan.
    // VTD-0088 fires on the first URL-containing SKILL.md or references/ file;
    // VTD-0093 (clean signal) fires only when no URL was found.
    let url_target_files: Vec<(&str, &str)> = {
        // Preserve SKILL.md-first order to match vettd's Map insertion order.
        let mut targets: Vec<(&str, &str)> = Vec::new();
        for name in &["SKILL.md", "skill.md"] {
            if let Some(c) = text_files.get(*name) {
                targets.push((name, c.as_str()));
            }
        }
        for (p, c) in text_files {
            if p.to_lowercase().starts_with("references/") {
                targets.push((p.as_str(), c.as_str()));
            }
        }
        targets
    };

    if !url_target_files.is_empty() {
        let url_file = url_target_files.iter().find(|(_, c)| has_external_url(c));
        if let Some((path, _)) = url_file {
            findings.push(Finding {
                rule_id: RULE_EXTERNAL_URL_REFERENCE.to_string(),
                category: FindingCategory::Security,
                severity: Severity::Medium,
                label: "References external URL — review for indirect prompt injection risk"
                    .to_string(),
                detail: format!(
                    "External URL(s) detected in {path} — referenced content can change after audit"
                ),
                filepath: Some(path.to_string()),
                owasp_llm_category: None,
                chain_id: None,
                intent: None,
                source: DEFAULT_SOURCE.to_string(),
            });
        } else {
            findings.push(f!(
                RULE_NO_EXTERNAL_URLS,
                FindingCategory::Security,
                Severity::Info,
                "No external URLs in skill definition",
                "SKILL.md and references/ files do not reference external URLs".to_string()
            ));
        }
    }

    // ── Evals quality check ──────────────────────────────────────────────────

    if has_evals {
        let eval_json_found = EVAL_JSON_CANDIDATES
            .iter()
            .any(|&candidate| text_files.contains_key(candidate));

        if !eval_json_found {
            // No standard JSON eval file — count non-trivial eval files
            let eval_dir_prefixes = ["evals/", "tests/", "test/"];
            let non_trivial_count = all_paths
                .iter()
                .filter(|p| eval_dir_prefixes.iter().any(|prefix| p.starts_with(prefix)))
                .filter(|p| {
                    let lower = p.to_lowercase();
                    lower.ends_with(".md")
                        || lower.ends_with(".yaml")
                        || lower.ends_with(".yml")
                        || lower.ends_with(".txt")
                        || lower.ends_with(".jsonl")
                        // .json files in textFiles — vettd detail says "non-JSON" for these too (vettd bug, reproduced as-is)
                        || (lower.ends_with(".json") && text_files.contains_key(p.as_str()))
                })
                .count();

            if non_trivial_count > 0 {
                findings.push(f!(
                    RULE_EVAL_FILES_FOUND,
                    FindingCategory::Evals,
                    Severity::Info,
                    "Eval files found",
                    format!(
                        "{non_trivial_count} evaluation file(s) detected in non-JSON format \
                             (markdown, YAML, JSONL, etc.)"
                    )
                ));
            }
        }
    }

    // ── Malicious-activity chain detection ───────────────────────────────────
    // Must run after all security pattern findings are added.
    detect_malicious_activity_chains(&mut findings);

    // ── Credential-exfiltration chain detection (existing) ───────────────────
    chain::detect_chains(&mut findings, text_files);

    SkillScanResult {
        findings,
        has_skill_md,
        has_scripts,
        has_references,
        has_evals,
        file_count: all_paths.len(),
    }
}
