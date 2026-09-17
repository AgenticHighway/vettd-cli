//! Adapter between `vettd-skill-scanner` and the v2 contract types.
//!
//! Loads the skill's files from disk, calls `scan_skill`, and maps the result
//! onto `ExternalScannerResult` for inclusion in the contract payload.
//!
//! ## Stub note
//!
//! File loading here does a local directory re-walk. The real implementation
//! should thread the file map already assembled during discovery through to
//! this call instead of re-reading from disk.

use std::collections::HashMap;
use std::path::Path;

use crate::contract::helpers::first_path;
use crate::contract::types::{
    ExternalScannerFinding, ExternalScannerResult, ScannerCoverage, ScannerSignal,
};
use crate::models::ArtifactReport;

use vettd_skill_scanner::consts::{CURRENT_SCANNER_VERSION, DEFAULT_SOURCE};
use vettd_skill_scanner::scan_skill;

/// Maximum file read size when loading skill files for the scanner (bytes).
const MAX_READ_BYTES: usize = 8192;

/// Maximum directory depth to walk when loading skill files.
const MAX_WALK_DEPTH: usize = 5;

/// Maximum number of files to load for a single skill.
const MAX_FILES: usize = 200;

/// Structural facts computed by the skill scanner, surfaced at the skill level
/// (`skills[].hasSkillMd`, `skills[].fileCount`, ...) rather than inside
/// `externalScannerResults[]` — see scanner-field-gate.json.
#[derive(Debug, Clone)]
pub(crate) struct SkillStructuralFacts {
    pub file_count: usize,
    pub has_skill_md: bool,
    pub has_scripts: bool,
    pub has_references: bool,
    pub has_evals: bool,
    pub has_assets: bool,
}

/// Full output of one skill scan: the contract-shaped `ExternalScannerResult`
/// plus the raw structural facts the skill builder surfaces at the skill level.
#[derive(Debug, Clone)]
pub(crate) struct SkillScanOutput {
    pub external: ExternalScannerResult,
    pub structural: SkillStructuralFacts,
}

/// Run the skill scanner against the artifact's source directory and return
/// the full scan output (contract result + structural facts) for inclusion in
/// the contract payload.
///
/// Returns `None` if the artifact has no resolvable path or the source
/// directory cannot be located.
pub(crate) fn run_skill_scanner(artifact: &ArtifactReport) -> Option<SkillScanOutput> {
    let skill_md_path = first_path(artifact);
    if skill_md_path == "unknown" {
        return None;
    }

    let skill_md = Path::new(skill_md_path);
    let skill_root = skill_md.parent().unwrap_or(Path::new("."));

    let (text_files, all_paths) = load_skill_files(skill_root);
    // The pinned scanner (v0.2.0) requires a caller-supplied RFC 3339
    // observation time; signals carry it unmodified. The pure scanner never
    // reads a clock, so the CLI stamps "now" here.
    let observed_at = chrono::Utc::now().to_rfc3339();
    let scan_result = match scan_skill(&text_files, &all_paths, &observed_at) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Warning: skill scanner error for {skill_md_path}: {e}");
            return None;
        }
    };

    let findings: Vec<ExternalScannerFinding> = scan_result
        .findings
        .iter()
        .map(|f| ExternalScannerFinding {
            rule_id: f.rule_id.clone(),
            category: f.category.as_str().to_string(),
            severity: f.severity.as_str().to_string(),
            label: f.label.clone(),
            detail: if f.detail.is_empty() {
                None
            } else {
                Some(f.detail.clone())
            },
        })
        .collect();

    // Signals are display-only — they travel separately from findings and are
    // never mapped into `ExternalScannerFinding` nor into the local grade.
    let signals: Vec<ScannerSignal> = scan_result
        .signals
        .iter()
        .map(|s| ScannerSignal {
            data_category: s.data_category.clone(),
            source_class: s.source_class.clone(),
            rule_id: s.rule_id.clone(),
            observed_at: s.observed_at.clone(),
            source: (s.source != DEFAULT_SOURCE).then(|| s.source.clone()),
            subject_type: s.subject_type.clone(),
            subject_id: s.subject_id.clone(),
            related_type: s.related_type.clone(),
            related_id: s.related_id.clone(),
            severity: s.severity.clone(),
            label: s.label.clone(),
            detail: s.detail.clone(),
            value_num: s.value_num,
            value_text: s.value_text.clone(),
            unit: s.unit.clone(),
            method: s.method.clone(),
            derivation: s.derivation.clone(),
            confidence: s.confidence,
            sample_size: s.sample_size,
            synthetic: s.synthetic,
            payload: s.payload.clone(),
        })
        .collect();

    let coverage: Vec<ScannerCoverage> = scan_result
        .coverage
        .iter()
        .map(|c| ScannerCoverage {
            kind: c.kind.clone(),
            rule_id: c.rule_id.clone(),
            label: c.label.clone(),
            detail: c.detail.clone(),
            category: c.category.clone(),
        })
        .collect();

    Some(SkillScanOutput {
        external: ExternalScannerResult {
            source: "vettd".to_string(),
            version: Some(CURRENT_SCANNER_VERSION.to_string()),
            status: "success".to_string(),
            verdict: None,
            raw_report: None,
            findings: if findings.is_empty() {
                None
            } else {
                Some(findings)
            },
            signals: if signals.is_empty() {
                None
            } else {
                Some(signals)
            },
            coverage: if coverage.is_empty() {
                None
            } else {
                Some(coverage)
            },
        },
        structural: SkillStructuralFacts {
            file_count: scan_result.file_count,
            has_skill_md: scan_result.has_skill_md,
            has_scripts: scan_result.has_scripts,
            has_references: scan_result.has_references,
            has_evals: scan_result.has_evals,
            has_assets: scan_result.has_assets,
        },
    })
}

/// Load text files and collect all paths from a skill root directory.
///
/// Files that appear to be binary (by extension) are included in `all_paths`
/// but not in `text_files`. Content is capped at `MAX_READ_BYTES` per file,
/// matching the existing detector read semantics in this crate.
fn load_skill_files(root: &Path) -> (HashMap<String, String>, Vec<String>) {
    let mut text_files = HashMap::new();
    let mut all_paths = Vec::new();
    walk_dir(root, root, &mut text_files, &mut all_paths, 0);
    (text_files, all_paths)
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    text_files: &mut HashMap<String, String>,
    all_paths: &mut Vec<String>,
    depth: usize,
) {
    if depth > MAX_WALK_DEPTH || all_paths.len() >= MAX_FILES {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if all_paths.len() >= MAX_FILES {
            break;
        }

        let path = entry.path();
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        if path.is_dir() {
            walk_dir(root, &path, text_files, all_paths, depth + 1);
        } else {
            all_paths.push(rel.clone());
            if is_likely_text(&path) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let head: String = content.chars().take(MAX_READ_BYTES).collect();
                    text_files.insert(rel, head);
                }
            }
        }
    }
}

/// Heuristic: treat a file as text if its extension is in a known set or it
/// has no extension at all (e.g. `Makefile`).
fn is_likely_text(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext.is_empty() {
        return true;
    }

    matches!(
        ext.as_str(),
        "md" | "txt"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "sh"
            | "bash"
            | "zsh"
            | "py"
            | "js"
            | "ts"
            | "mjs"
            | "cjs"
            | "rs"
            | "go"
            | "rb"
            | "php"
            | "java"
            | "kt"
            | "swift"
            | "c"
            | "cpp"
            | "h"
            | "cs"
            | "html"
            | "xml"
            | "css"
            | "sql"
            | "env"
            | "ini"
            | "cfg"
            | "conf"
            | "lock"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ArtifactReport;

    fn skill_artifact_with_path(path: &str) -> ArtifactReport {
        let mut a = ArtifactReport::new("skill", 0.9);
        a.metadata
            .insert("paths".to_string(), serde_json::json!([path]));
        a
    }

    #[test]
    fn unknown_path_returns_none() {
        // An artifact with no resolvable path must not produce a scanner result.
        let a = ArtifactReport::new("skill", 0.9);
        assert!(run_skill_scanner(&a).is_none());
    }

    #[test]
    fn nonexistent_path_returns_some_result() {
        // A path that doesn't exist on disk should still succeed — the file map
        // will just be empty and the scanner returns stub findings for a missing
        // SKILL.md.
        let a = skill_artifact_with_path("/nonexistent/path/SKILL.md");
        let result = run_skill_scanner(&a);
        assert!(result.is_some());
        let r = result.unwrap().external;
        assert_eq!(r.source, "vettd");
        assert_eq!(r.status, "success");
        assert_eq!(r.version, Some(CURRENT_SCANNER_VERSION.to_string()));
    }

    #[test]
    fn result_has_nonempty_findings_for_any_skill() {
        // Even with a nonexistent path, the scanner emits at least the
        // missing-SKILL.md finding so the contract field is populated.
        let a = skill_artifact_with_path("/nonexistent/SKILL.md");
        let result = run_skill_scanner(&a).unwrap();
        assert!(
            result
                .external
                .findings
                .as_ref()
                .is_some_and(|f| !f.is_empty()),
            "findings must be non-empty"
        );
    }

    #[test]
    fn finding_mapping_preserves_category_and_severity_strings() {
        // Verify the category/severity strings in ExternalScannerFinding match
        // the vettd wire format (lowercase, kebab-case for best-practices).
        let a = skill_artifact_with_path("/nonexistent/SKILL.md");
        let result = run_skill_scanner(&a).unwrap();
        let findings = result.external.findings.unwrap();
        for f in &findings {
            // Category must be one of the known wire values
            assert!(
                matches!(
                    f.category.as_str(),
                    "security"
                        | "structure"
                        | "best-practices"
                        | "description"
                        | "scripts"
                        | "evals"
                ),
                "unexpected category string: {}",
                f.category
            );
            // Severity must be one of the known wire values
            assert!(
                matches!(
                    f.severity.as_str(),
                    "info" | "low" | "medium" | "high" | "critical"
                ),
                "unexpected severity string: {}",
                f.severity
            );
        }
    }

    #[test]
    fn output_carries_structural_facts_at_skill_level() {
        // The six structural facts must travel with the scan output so the
        // skill builder can surface them at `skills[].<field>` (v2.6.0). A
        // nonexistent path yields the missing-SKILL.md stub, so the facts are
        // all "absent" — but they must still be present, not swallowed.
        let a = skill_artifact_with_path("/nonexistent/SKILL.md");
        let output = run_skill_scanner(&a).unwrap();
        assert_eq!(output.structural.file_count, 0);
        assert!(!output.structural.has_skill_md);
        assert!(!output.structural.has_scripts);
        assert!(!output.structural.has_references);
        assert!(!output.structural.has_evals);
        assert!(!output.structural.has_assets);
    }

    // ── signals / coverage wire-shape tests ─────────────────────────────

    fn minimal_signal() -> ScannerSignal {
        ScannerSignal {
            data_category: "characteristics".to_string(),
            source_class: "scan".to_string(),
            rule_id: "characteristics/declared-license".to_string(),
            observed_at: "2026-08-24T00:00:00Z".to_string(),
            source: None,
            subject_type: None,
            subject_id: None,
            related_type: None,
            related_id: None,
            severity: None,
            label: None,
            detail: None,
            value_num: None,
            value_text: None,
            unit: None,
            method: None,
            derivation: None,
            confidence: None,
            sample_size: None,
            synthetic: false,
            payload: None,
        }
    }

    #[test]
    fn minimal_signal_serialises_to_four_camel_case_keys() {
        // A signal with only the required fields must serialize to exactly the
        // four required camelCase keys — optional fields are omitted, never
        // emitted as null (matches the scanner's own wire contract).
        let v = serde_json::to_value(minimal_signal()).unwrap();
        let obj = v.as_object().unwrap();
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["dataCategory", "observedAt", "ruleId", "sourceClass"]
        );
    }

    #[test]
    fn signal_serialises_full_shape_in_camel_case() {
        let s = ScannerSignal {
            source: Some("third-party".to_string()),
            subject_type: Some("skill_audit".to_string()),
            subject_id: Some("audit-1".to_string()),
            related_type: Some("skill".to_string()),
            related_id: Some("other".to_string()),
            severity: Some("low".to_string()),
            label: Some("Declared license".to_string()),
            detail: Some("MIT".to_string()),
            value_num: Some(3.5),
            value_text: Some("text".to_string()),
            unit: Some("MB".to_string()),
            method: Some("parse".to_string()),
            derivation: Some("derived".to_string()),
            confidence: Some(0.9),
            sample_size: Some(12),
            synthetic: true,
            payload: Some(serde_json::Map::new()),
            ..minimal_signal()
        };
        let v = serde_json::to_value(&s).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj["sourceClass"], "scan");
        assert_eq!(obj["valueNum"], 3.5);
        assert_eq!(obj["sampleSize"], 12);
        assert_eq!(obj["synthetic"], true);
        assert_eq!(obj["relatedType"], "skill");
        assert_eq!(obj["relatedId"], "other");
        assert!(
            obj.keys().all(|k| !k.contains('_')),
            "snake_case keys must not appear: {obj:?}"
        );
    }

    #[test]
    fn coverage_serialises_camel_case_with_optional_category() {
        let c = ScannerCoverage {
            kind: "applicable".to_string(),
            rule_id: "VTD-0001".to_string(),
            label: "Checked".to_string(),
            detail: "Rule ran".to_string(),
            category: Some("structure".to_string()),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["kind"], "applicable");
        assert_eq!(v["category"], "structure");
        assert!(v.get("ruleId").is_some());
    }

    #[test]
    fn signals_and_coverage_do_not_become_findings() {
        // The write path must keep signals/coverage in their own arrays —
        // never folded into `findings`, which is the only array the local
        // grade reads.
        let mut result = ExternalScannerResult {
            source: "vettd".to_string(),
            version: None,
            status: "success".to_string(),
            verdict: None,
            raw_report: None,
            findings: None,
            signals: Some(vec![minimal_signal()]),
            coverage: Some(vec![ScannerCoverage {
                kind: "applicable".to_string(),
                rule_id: "VTD-0001".to_string(),
                label: "l".to_string(),
                detail: "d".to_string(),
                category: None,
            }]),
        };
        assert!(result.findings.is_none());
        assert_eq!(result.signals.as_ref().unwrap().len(), 1);
        assert_eq!(result.coverage.as_ref().unwrap().len(), 1);

        let v = serde_json::to_value(&result).unwrap();
        assert!(v.get("signals").is_some());
        assert!(v.get("coverage").is_some());
        assert!(v.get("findings").is_none());

        // Round-trip: deserialize back and confirm separation holds.
        result = serde_json::from_value(v).unwrap();
        assert!(result.findings.is_none());
        assert_eq!(
            result.signals.as_ref().unwrap()[0].rule_id,
            "characteristics/declared-license"
        );
    }
}
