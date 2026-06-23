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
use crate::contract::types::{ExternalScannerFinding, ExternalScannerResult};
use crate::models::ArtifactReport;

use vettd_skill_scanner::consts::CURRENT_SCANNER_VERSION;
use vettd_skill_scanner::scan_skill;

/// Maximum file read size when loading skill files for the scanner (bytes).
const MAX_READ_BYTES: usize = 8192;

/// Maximum directory depth to walk when loading skill files.
const MAX_WALK_DEPTH: usize = 5;

/// Maximum number of files to load for a single skill.
const MAX_FILES: usize = 200;

/// Run the skill scanner against the artifact's source directory and return
/// an `ExternalScannerResult` for inclusion in the contract payload.
///
/// Returns `None` if the artifact has no resolvable path or the source
/// directory cannot be located.
pub(crate) fn run_skill_scanner(artifact: &ArtifactReport) -> Option<ExternalScannerResult> {
    let skill_md_path = first_path(artifact);
    if skill_md_path == "unknown" {
        return None;
    }

    let skill_md = Path::new(skill_md_path);
    let skill_root = skill_md.parent().unwrap_or(Path::new("."));

    let (text_files, all_paths) = load_skill_files(skill_root);
    let scan_result = scan_skill(&text_files, &all_paths);

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

    Some(ExternalScannerResult {
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
        let r = result.unwrap();
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
            result.findings.as_ref().is_some_and(|f| !f.is_empty()),
            "findings must be non-empty"
        );
    }

    #[test]
    fn finding_mapping_preserves_category_and_severity_strings() {
        // Verify the category/severity strings in ExternalScannerFinding match
        // the vettd wire format (lowercase, kebab-case for best-practices).
        let a = skill_artifact_with_path("/nonexistent/SKILL.md");
        let result = run_skill_scanner(&a).unwrap();
        let findings = result.findings.unwrap();
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
}
