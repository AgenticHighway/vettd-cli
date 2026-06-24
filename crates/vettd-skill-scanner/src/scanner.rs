use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::checks::behavioral::scan_behavioral_patterns;
use crate::checks::chains::{detect_exfiltration_chains, detect_malicious_activity_chains};
use crate::checks::description::check_description_behavior_mismatch;
use crate::checks::encoding::{check_base64_payloads, scan_hidden_unicode};
use crate::checks::sensitive::{scan_entropy, scan_env_files, scan_sensitive_patterns};
use crate::checks::typosquat::check_typosquat;
use crate::consts::{
    DEFAULT_SOURCE, DESCRIPTION_MAX_LENGTH, EVALS_MIN_TEST_CASES, EVAL_JSON_CANDIDATES,
    SKILL_MD_BODY_MAX_LINES,
};
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::result::SkillScanResult;
use crate::rules::*;
use crate::skill_md::body::{
    has_checklist, has_cli_hint, has_examples, has_external_url, has_gotchas, has_usage_context,
    has_validation, has_workflow, is_likely_cli_script,
};
use crate::skill_md::validate::validate_name;
use crate::skill_md::{parse_skill_md, ParsedSkillMd};

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

        check_typosquat(&parsed.name, &mut findings);

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

        // Name collision check (VTD-0100)
        const WELL_KNOWN_SKILL_NAMES: &[&str] = &[
            "frontend-design",
            "pdf",
            "web-perf",
            "web-design-guidelines",
            "find-skills",
            "agent-browser",
            "agent-customization",
            "cloudflare",
            "durable-objects",
            "workers-best-practices",
            "wrangler",
            "sandbox-sdk",
            "next-best-practices",
            "vercel-react-best-practices",
            "rust-best-practices",
            "postgresql-optimization",
            "prisma-postgres",
            "aws-skills",
            "powershell-windows",
            "cosmosdb-best-practices",
            "excel",
            "word",
            "powerpoint",
            "git",
            "docker",
            "kubernetes",
            "terraform",
            "ansible",
        ];
        if WELL_KNOWN_SKILL_NAMES.contains(&parsed.name.as_str()) {
            findings.push(f!(
                RULE_SKILL_NAME_COLLISION,
                FindingCategory::BestPractices,
                Severity::Medium,
                "Skill name collides with well-known skill",
                format!(
                    "{:?} matches a well-known skill name — may cause unintended invocation",
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

        // System prompt leakage check (VTD-0085)
        {
            static PROMPT_LEAK_RE: OnceLock<Regex> = OnceLock::new();
            let prompt_leak_re = PROMPT_LEAK_RE.get_or_init(|| {
                Regex::new(r"(?i)\b(?:print|log|echo|output|return|display|show|reveal|dump)\s+(?:the\s+|your\s+|my\s+)?(?:system\s+)?(?:prompt|instructions?|system\s+message|internal\s+(?:prompt|instructions?))\b")
                    .expect("bad prompt leak re")
            });
            let skill_md_raw = text_files.get(skill_key).map(|s| s.as_str()).unwrap_or("");
            if prompt_leak_re.is_match(skill_md_raw) {
                findings.push(Finding {
                    rule_id: RULE_SYSTEM_PROMPT_LEAKAGE.to_string(),
                    category: FindingCategory::Security,
                    severity: Severity::Medium,
                    label: "System prompt leakage risk".to_string(),
                    detail: "Skill instructs agent to output or reveal system prompt/instructions"
                        .to_string(),
                    filepath: None,
                    owasp_llm_category: None,
                    chain_id: None,
                    intent: None,
                    source: DEFAULT_SOURCE.to_string(),
                });
            }
        }

        // Description checks
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

            if parsed.description.split_whitespace().count() < 5 {
                findings.push(f!(
                    RULE_DESCRIPTION_BREVITY,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description too brief",
                    "A few sentences covering scope and trigger conditions improves \
                    activation accuracy"
                        .to_string()
                ));
            }

            {
                static OVERCLAIM_RE: OnceLock<Regex> = OnceLock::new();
                let overclaim_re = OVERCLAIM_RE.get_or_init(|| {
                    Regex::new(r"(?i)\b(?:anything|everything|all\s+(?:files?|data|tasks?|requests?|inputs?|things?)|any\s+(?:file|task|request|input|thing)|whatever)\b")
                        .expect("bad overclaim re")
                });
                if overclaim_re.is_match(&parsed.description) {
                    findings.push(f!(
                        RULE_DESCRIPTION_SCOPE,
                        FindingCategory::Description,
                        Severity::Low,
                        "Description overclaims scope",
                        "Broad trigger words (anything, everything, all files, etc.) widen attack surface — narrow to specific use cases"
                            .to_string()
                    ));
                }
            }
        }

        // Body quality checks
        if !parsed.body.trim().is_empty() {
            let body_lines = parsed.body.split('\n').count();

            findings.push(if body_lines > SKILL_MD_BODY_MAX_LINES {
                f!(RULE_SKILL_MD_BODY_LENGTH, FindingCategory::BestPractices, Severity::Info,
                   "SKILL.md exceeds 500 lines",
                   format!("{body_lines} lines — consider moving detailed reference material to references/"))
            } else {
                f!(RULE_SKILL_MD_BODY_LENGTH, FindingCategory::BestPractices, Severity::Info,
                   "SKILL.md body length is reasonable",
                   format!("{body_lines} lines (recommended: under 500)"))
            });

            if has_gotchas(&parsed.body) {
                findings.push(f!(
                    RULE_GOTCHAS_SECTION,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Gotchas section found",
                    "Documents environment-specific facts and common pitfalls".to_string()
                ));
            }

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

            if has_checklist(&parsed.body) {
                findings.push(f!(
                    RULE_CHECKLIST_PRESENT,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Checklist pattern found",
                    "Explicit checklists help agents track progress in multi-step workflows"
                        .to_string()
                ));
            }

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

            let body_refs_files = parsed.body.contains("references/")
                || parsed.body.contains("scripts/")
                || parsed.body.contains("assets/")
                || {
                    static READ_MD_RE: OnceLock<Regex> = OnceLock::new();
                    let re = READ_MD_RE
                        .get_or_init(|| Regex::new(r"(?i)read.*\.md").expect("bad read md re"));
                    re.is_match(&parsed.body)
                };
            if body_refs_files && (has_references || has_scripts || has_assets) {
                findings.push(f!(
                    RULE_PROGRESSIVE_DISCLOSURE,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Progressive disclosure used",
                    "SKILL.md body references files in references/, scripts/, or assets/ — \
                     agents can load additional context on demand instead of consuming \
                     everything upfront"
                        .to_string()
                ));
            }

            const GENERIC_PHRASES: &[&str] = &[
                "follow best practices",
                "handle errors appropriately",
                "use proper",
                "ensure quality",
            ];
            let body_lower = parsed.body.to_lowercase();
            for &phrase in GENERIC_PHRASES {
                if body_lower.contains(phrase) {
                    findings.push(f!(
                        RULE_GENERIC_INSTRUCTION,
                        FindingCategory::BestPractices,
                        Severity::Info,
                        "Generic instruction detected",
                        format!(
                            "\"{phrase}\" is too vague — provide specific, actionable guidance instead"
                        )
                    ));
                }
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
        script_files.sort_by_key(|(p, _)| *p);

        static INTERACTIVE_RE: OnceLock<Regex> = OnceLock::new();
        static STRUCTURED_RE: OnceLock<Regex> = OnceLock::new();
        let interactive_re = INTERACTIVE_RE.get_or_init(|| {
            Regex::new(r"(?i)input\s*\(|readline|prompt\s*\(|inquirer").expect("bad interactive re")
        });
        let structured_re = STRUCTURED_RE.get_or_init(|| {
            Regex::new(r"(?i)json\.dumps|JSON\.stringify|\.to_json|\.to_csv|csv\.writer")
                .expect("bad structured re")
        });
        static DEP_RE: OnceLock<Regex> = OnceLock::new();
        let dep_re = DEP_RE.get_or_init(|| {
            Regex::new(r"(?i)dependencies\s*=\s*\[|require\(|import\s").expect("bad dep re")
        });

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

            if interactive_re.is_match(content) {
                findings.push(Finding {
                    rule_id: RULE_SCRIPT_INTERACTIVE_PROMPTS.to_string(),
                    category: FindingCategory::Scripts,
                    severity: Severity::High,
                    label: "Interactive prompts detected".to_string(),
                    detail: format!(
                        "{path}: Agents run in non-interactive shells — replace prompts with CLI flags or stdin"
                    ),
                    filepath: Some(path.to_string()),
                    owasp_llm_category: None,
                    chain_id: None,
                    intent: None,
                    source: DEFAULT_SOURCE.to_string(),
                });
            }

            if structured_re.is_match(content) {
                findings.push(Finding {
                    rule_id: RULE_SCRIPT_STRUCTURED_OUTPUT.to_string(),
                    category: FindingCategory::Scripts,
                    severity: Severity::Info,
                    label: "Structured output format".to_string(),
                    detail: format!(
                        "{path}: Uses JSON/CSV output which is easily parseable by agents"
                    ),
                    filepath: Some(path.to_string()),
                    owasp_llm_category: None,
                    chain_id: None,
                    intent: None,
                    source: DEFAULT_SOURCE.to_string(),
                });
            }

            {
                let has_pinned_deps = dep_re.is_match(content);
                if has_pinned_deps && content.contains(">=") && !content.contains('<') {
                    findings.push(Finding {
                        rule_id: RULE_SCRIPT_DEPENDENCY_PINNING.to_string(),
                        category: FindingCategory::Scripts,
                        severity: Severity::Low,
                        label: "Unpinned dependency versions".to_string(),
                        detail: format!(
                            "{path}: Pin dependency versions for reproducibility \
                            (e.g., >=4.12,<5 instead of >=4.12)"
                        ),
                        filepath: Some(path.to_string()),
                        owasp_llm_category: None,
                        chain_id: None,
                        intent: None,
                        source: DEFAULT_SOURCE.to_string(),
                    });
                }
            }
        }
    }

    // ── Security scan ────────────────────────────────────────────────────────

    let (sensitive_findings, secrets_check_failed_pat) = scan_sensitive_patterns(text_files);
    findings.extend(sensitive_findings);

    scan_entropy(text_files, &mut findings);
    scan_env_files(text_files, &mut findings);

    let secrets_check_failed = secrets_check_failed_pat
        || findings.iter().any(|f| {
            f.category == FindingCategory::Security
                && matches!(f.severity, Severity::Critical | Severity::High)
        });

    let (base64_secrets_failed, base64_behavioral_failed) =
        check_base64_payloads(text_files, &mut findings);

    if !secrets_check_failed && !base64_secrets_failed {
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

    let (behavioral_findings, behavioral_check_failed) = scan_behavioral_patterns(text_files);
    findings.extend(behavioral_findings);

    if !behavioral_check_failed && !base64_behavioral_failed {
        findings.push(f!(
            RULE_NO_BEHAVIORAL_SIGNALS,
            FindingCategory::Security,
            Severity::Info,
            "No prompt injection or jailbreak signals detected",
            "Scanned text content for instruction override, jailbreak framing, credential \
             solicitation, and embedded injection markers"
                .to_string()
        ));
    }

    scan_hidden_unicode(text_files, &mut findings);

    // External URL check
    let url_target_files: Vec<(&str, &str)> = {
        let mut targets: Vec<(&str, &str)> = Vec::new();
        for name in &["SKILL.md", "skill.md"] {
            if let Some(c) = text_files.get(*name) {
                targets.push((name, c.as_str()));
            }
        }
        let mut refs: Vec<(&str, &str)> = text_files
            .iter()
            .filter(|(p, _)| p.to_lowercase().starts_with("references/"))
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        refs.sort_by_key(|(p, _)| *p);
        targets.extend(refs);
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
        let eval_json_content = EVAL_JSON_CANDIDATES
            .iter()
            .find_map(|&candidate| text_files.get(candidate));

        let eval_json_found = eval_json_content.is_some();

        if let Some(json_str) = eval_json_content {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                let evals = val
                    .get("evals")
                    .or_else(|| val.get("tests"))
                    .or_else(|| val.get("test_cases"))
                    .or_else(|| val.get("scenarios"))
                    .or_else(|| val.get("cases"))
                    .or_else(|| val.get("examples"))
                    .and_then(|v| v.as_array())
                    .or_else(|| val.as_array());

                if let Some(cases) = evals.filter(|a| !a.is_empty()) {
                    let count = cases.len();
                    findings.push(f!(
                        RULE_EVALS_TEST_CASE_COUNT,
                        FindingCategory::Evals,
                        Severity::Info,
                        "Test cases defined",
                        format!("{count} test case(s) found in evals JSON")
                    ));

                    let has_assertions = cases.iter().any(|e| {
                        (e.get("assertions")
                            .and_then(|v| v.as_array())
                            .map(|a| !a.is_empty())
                            .unwrap_or(false))
                            || (e
                                .get("criteria")
                                .and_then(|v| v.as_array())
                                .map(|a| !a.is_empty())
                                .unwrap_or(false))
                            || (e
                                .get("pass_criteria")
                                .and_then(|v| v.as_array())
                                .map(|a| !a.is_empty())
                                .unwrap_or(false))
                            || e.get("expected").and_then(|v| v.as_str()).is_some()
                            || e.get("expected_output").and_then(|v| v.as_str()).is_some()
                            || e.get("golden_answer").and_then(|v| v.as_str()).is_some()
                            || e.get("rubric").and_then(|v| v.as_str()).is_some()
                    });
                    findings.push(if has_assertions {
                        f!(
                            RULE_EVALS_ASSERTIONS,
                            FindingCategory::Evals,
                            Severity::Info,
                            "Assertions defined for test cases",
                            "Verifiable assertions or expected outputs enable automated grading of skill outputs"
                                .to_string()
                        )
                    } else {
                        f!(
                            RULE_EVALS_ASSERTIONS,
                            FindingCategory::Evals,
                            Severity::Info,
                            "No assertions in test cases",
                            "Add verifiable assertions, expected outputs, or criteria to each test case"
                                .to_string()
                        )
                    });

                    if count < EVALS_MIN_TEST_CASES {
                        findings.push(f!(
                            RULE_EVALS_MIN_COUNT,
                            FindingCategory::Evals,
                            Severity::Info,
                            "Few test cases",
                            format!(
                                "Consider adding at least {EVALS_MIN_TEST_CASES} test cases \
                                 covering varied prompts and edge cases"
                            )
                        ));
                    }
                }
            }
        }

        if !eval_json_found {
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

    // ── Chain detection and mismatch checks ─────────────────────────────────

    detect_exfiltration_chains(&mut findings, text_files);
    detect_malicious_activity_chains(&mut findings);
    let description_for_mismatch = if has_skill_md {
        let key = if text_files.contains_key("SKILL.md") {
            "SKILL.md"
        } else {
            "skill.md"
        };
        text_files
            .get(key)
            .map(|c| parse_skill_md(c).description)
            .unwrap_or_default()
    } else {
        String::new()
    };
    check_description_behavior_mismatch(&description_for_mismatch, &mut findings);

    SkillScanResult {
        findings,
        has_skill_md,
        has_scripts,
        has_references,
        has_evals,
        file_count: all_paths.len(),
    }
}
