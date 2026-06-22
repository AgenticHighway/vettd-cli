//! Core scan engine — takes a skill file map and produces findings.
//!
//! Rule implementations mirror the vettd web scanner's `analyzeSkillFiles` in
//! `packages/api/src/skills/skill-analyzer.ts`. Where vettd has a bug that is
//! visible in the wire format (e.g. VTD-0123 detail says "non-JSON format" even
//! for .json files), the Rust engine reproduces the same output unaltered so that
//! the parity test can reach a clean pass.

use std::collections::HashMap;

use crate::chain;
use crate::consts::DEFAULT_SOURCE;
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::result::SkillScanResult;

// ── Rule IDs (must match skill-rule-registry.ts) ─────────────────────────────

// Security
const RULE_NO_REPOSITORY_LINK: &str = "VTD-0083";
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

    // ── Security clean signals ───────────────────────────────────────────────
    // These "no bad things found" findings mirror vettd's checkSecurity()
    // clean-path signals. A full behavioral + secrets scan is not yet
    // implemented in the Rust engine; these fire unconditionally for inputs
    // that have no dangerous patterns. Tracked in a follow-on issue.

    findings.push(f!(
        RULE_NO_SECRETS_DETECTED,
        FindingCategory::Security,
        Severity::Info,
        "No secrets or unsafe code patterns detected",
        "Scanned all files for credentials, private keys, and code-level risks \
         (eval, shell exec, destructive ops)"
            .to_string()
    ));

    findings.push(f!(
        RULE_NO_BEHAVIORAL_SIGNALS,
        FindingCategory::Security,
        Severity::Info,
        "No prompt injection or jailbreak signals detected",
        "Scanned text content for instruction override, jailbreak framing, credential \
         solicitation, and embedded injection markers"
            .to_string()
    ));

    // VTD-0093 only fires when at least one SKILL.md or references/ file is present
    let has_url_target = text_files
        .keys()
        .any(|p| p.eq_ignore_ascii_case("skill.md") || p.to_lowercase().starts_with("references/"));
    if has_url_target {
        let external_url_found = text_files.iter().any(|(p, c)| {
            (p.eq_ignore_ascii_case("skill.md") || p.to_lowercase().starts_with("references/"))
                && has_external_url(c)
        });
        if !external_url_found {
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

    // ── Chain detection (must run last) ──────────────────────────────────────
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
