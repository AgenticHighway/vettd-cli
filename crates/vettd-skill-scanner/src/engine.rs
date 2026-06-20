//! Core scan engine — takes a skill file map and produces findings.
//!
//! This module is **partially stubbed**. Structural checks (SKILL.md presence,
//! scripts/, references/, evals/) are real and mirror the vettd web scanner.
//! Security, best-practices, description, scripts, and evals checks are stub
//! findings that span all categories and severity tiers so downstream consumers
//! can exercise the full `Finding` shape before real rules are implemented.

use std::collections::HashMap;

use crate::chain;
use crate::consts::DEFAULT_SOURCE;
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::result::SkillScanResult;

/// Rule IDs for structural checks (mirrors vettd's `RULE_IDS` enum).
const RULE_SKILL_MD: &str = "VTD-0095";
const RULE_SCRIPTS_DIRECTORY: &str = "VTD-0096";
const RULE_REFERENCES_DIRECTORY: &str = "VTD-0097";
const RULE_EVALS_DIRECTORY: &str = "VTD-0098";

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

    // ── Structure checks (real) ──────────────────────────────────────────────
    // These match the vettd web scanner's structure pass in `analyzeSkillFiles`.

    if has_skill_md {
        findings.push(Finding {
            rule_id: RULE_SKILL_MD.to_string(),
            category: FindingCategory::Structure,
            severity: Severity::Info,
            label: "SKILL.md present".to_string(),
            detail: "Required skill definition file found".to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: DEFAULT_SOURCE.to_string(),
        });
    } else {
        findings.push(Finding {
            rule_id: RULE_SKILL_MD.to_string(),
            category: FindingCategory::Structure,
            severity: Severity::Critical,
            label: "SKILL.md missing".to_string(),
            detail:
                "Every skill must contain a SKILL.md file with YAML frontmatter and instructions"
                    .to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: DEFAULT_SOURCE.to_string(),
        });
    }

    if has_scripts {
        findings.push(Finding {
            rule_id: RULE_SCRIPTS_DIRECTORY.to_string(),
            category: FindingCategory::Structure,
            severity: Severity::Info,
            label: "scripts/ directory present".to_string(),
            detail: "Bundled executable scripts found".to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: DEFAULT_SOURCE.to_string(),
        });
    } else {
        findings.push(Finding {
            rule_id: RULE_SCRIPTS_DIRECTORY.to_string(),
            category: FindingCategory::Structure,
            severity: Severity::Info,
            label: "No scripts/ directory".to_string(),
            detail: "Consider bundling reusable scripts for validation and automation".to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: DEFAULT_SOURCE.to_string(),
        });
    }

    if has_references {
        findings.push(Finding {
            rule_id: RULE_REFERENCES_DIRECTORY.to_string(),
            category: FindingCategory::Structure,
            severity: Severity::Info,
            label: "references/ directory present".to_string(),
            detail: "Additional documentation files available for progressive disclosure"
                .to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: DEFAULT_SOURCE.to_string(),
        });
    }

    if has_evals {
        findings.push(Finding {
            rule_id: RULE_EVALS_DIRECTORY.to_string(),
            category: FindingCategory::Structure,
            severity: Severity::Info,
            label: "evals/ directory present".to_string(),
            detail: "Evaluation suite found".to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: DEFAULT_SOURCE.to_string(),
        });
    }

    // ── Stub findings — span all categories and severity tiers ───────────────
    // These are placeholder findings. Each one is marked [STUB] in its detail
    // and will be replaced by real rule implementations in subsequent issues.

    findings.push(Finding {
        rule_id: String::new(),
        category: FindingCategory::Security,
        severity: Severity::High,
        label: "[STUB] Potential unsafe instruction pattern".to_string(),
        detail: "[STUB] Security scan not yet implemented. This is a placeholder finding."
            .to_string(),
        filepath: None,
        owasp_llm_category: Some("LLM01".to_string()),
        chain_id: None,
        intent: None,
        source: DEFAULT_SOURCE.to_string(),
    });

    findings.push(Finding {
        rule_id: String::new(),
        category: FindingCategory::Security,
        severity: Severity::Medium,
        label: "[STUB] External URL reference".to_string(),
        detail: "[STUB] External URL check not yet implemented. This is a placeholder finding."
            .to_string(),
        filepath: None,
        owasp_llm_category: Some("LLM02".to_string()),
        chain_id: None,
        intent: None,
        source: DEFAULT_SOURCE.to_string(),
    });

    findings.push(Finding {
        rule_id: String::new(),
        category: FindingCategory::BestPractices,
        severity: Severity::Low,
        label: "[STUB] Missing input validation guidance".to_string(),
        detail: "[STUB] Best-practices scan not yet implemented. This is a placeholder finding."
            .to_string(),
        filepath: None,
        owasp_llm_category: None,
        chain_id: None,
        intent: None,
        source: DEFAULT_SOURCE.to_string(),
    });

    findings.push(Finding {
        rule_id: String::new(),
        category: FindingCategory::Description,
        severity: Severity::Info,
        label: "[STUB] Description quality check".to_string(),
        detail: "[STUB] Description analysis not yet implemented. This is a placeholder finding."
            .to_string(),
        filepath: None,
        owasp_llm_category: None,
        chain_id: None,
        intent: None,
        source: DEFAULT_SOURCE.to_string(),
    });

    if has_scripts {
        findings.push(Finding {
            rule_id: String::new(),
            category: FindingCategory::Scripts,
            severity: Severity::Low,
            label: "[STUB] Script execution pattern".to_string(),
            detail: "[STUB] Script analysis not yet implemented. This is a placeholder finding."
                .to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: Some(Intent::Negligent),
            source: DEFAULT_SOURCE.to_string(),
        });
    }

    if has_evals {
        findings.push(Finding {
            rule_id: String::new(),
            category: FindingCategory::Evals,
            severity: Severity::Info,
            label: "[STUB] Evaluation coverage".to_string(),
            detail: "[STUB] Eval analysis not yet implemented. This is a placeholder finding."
                .to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: DEFAULT_SOURCE.to_string(),
        });
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
