//! Integration tests for the `scan_skill` public API.
//!
//! These tests drive `scan_skill` through `vettd_skill_scanner`'s public
//! interface using synthetic file maps — no filesystem I/O.
//!
//! **Why these tests matter**: the stub engine must exercise every `Finding`
//! shape (all categories, multiple severity tiers) so that downstream CLI code
//! which maps findings to the wire contract is tested against realistic variety.
//! Tests in this file will fail if the engine stops producing that variety,
//! which is the signal that the mapping layer also needs updating.

use std::collections::HashMap;

use vettd_skill_scanner::{scan_skill, FindingCategory, Severity};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn skill_md() -> (String, String) {
    (
        "SKILL.md".to_string(),
        "---\nname: test-skill\nversion: 1.0.0\n---\n# Test Skill\nDoes a thing.".to_string(),
    )
}

fn with_skill_md() -> (HashMap<String, String>, Vec<String>) {
    let mut text_files = HashMap::new();
    let (path, content) = skill_md();
    text_files.insert(path.clone(), content);
    let all_paths = vec![path];
    (text_files, all_paths)
}

fn with_scripts() -> (HashMap<String, String>, Vec<String>) {
    let (mut text_files, mut all_paths) = with_skill_md();
    text_files.insert(
        "scripts/run.sh".to_string(),
        "#!/bin/bash\necho hello".to_string(),
    );
    all_paths.push("scripts/run.sh".to_string());
    (text_files, all_paths)
}

fn with_evals() -> (HashMap<String, String>, Vec<String>) {
    let (mut text_files, mut all_paths) = with_skill_md();
    all_paths.push("evals/suite.json".to_string());
    text_files.insert("evals/suite.json".to_string(), "{}".to_string());
    (text_files, all_paths)
}

// ---------------------------------------------------------------------------
// Structural flag tests
// ---------------------------------------------------------------------------

#[test]
fn no_skill_md_sets_flag_false() {
    let result = scan_skill(&HashMap::new(), &[]);
    assert!(!result.has_skill_md);
}

#[test]
fn skill_md_in_text_files_sets_flag_true() {
    let (text_files, all_paths) = with_skill_md();
    let result = scan_skill(&text_files, &all_paths);
    assert!(result.has_skill_md);
}

#[test]
fn skill_md_in_all_paths_only_sets_flag_true() {
    // SKILL.md may be detected from all_paths even if content was not read.
    let result = scan_skill(&HashMap::new(), &["SKILL.md".to_string()]);
    assert!(result.has_skill_md);
}

#[test]
fn scripts_dir_detected_from_all_paths() {
    let (text_files, mut all_paths) = with_skill_md();
    all_paths.push("scripts/deploy.sh".to_string());
    let result = scan_skill(&text_files, &all_paths);
    assert!(result.has_scripts);
}

#[test]
fn references_dir_detected() {
    let (text_files, mut all_paths) = with_skill_md();
    all_paths.push("references/guide.md".to_string());
    let result = scan_skill(&text_files, &all_paths);
    assert!(result.has_references);
}

#[test]
fn evals_dir_detected() {
    let (text_files, all_paths) = with_evals();
    let result = scan_skill(&text_files, &all_paths);
    assert!(result.has_evals);
}

#[test]
fn file_count_reflects_all_paths_length() {
    let (text_files, all_paths) = with_skill_md();
    let n = all_paths.len();
    let result = scan_skill(&text_files, &all_paths);
    assert_eq!(result.file_count, n);
}

// ---------------------------------------------------------------------------
// Structural finding tests
// ---------------------------------------------------------------------------

#[test]
fn missing_skill_md_emits_critical_structure_finding() {
    // Without SKILL.md, the skill is malformed — scanner must flag it critical
    // so the grade formula produces F.
    let result = scan_skill(&HashMap::new(), &[]);
    let f = result
        .findings
        .iter()
        .find(|f| f.category == FindingCategory::Structure && f.severity == Severity::Critical)
        .expect("should emit a critical structure finding when SKILL.md is absent");
    assert!(
        f.label.to_lowercase().contains("skill.md"),
        "critical structure finding label should mention SKILL.md"
    );
}

#[test]
fn present_skill_md_emits_info_structure_finding() {
    let (text_files, all_paths) = with_skill_md();
    let result = scan_skill(&text_files, &all_paths);
    let f = result
        .findings
        .iter()
        .find(|f| {
            f.category == FindingCategory::Structure && f.label.to_lowercase().contains("skill.md")
        })
        .expect("should emit a structure finding about SKILL.md");
    assert_eq!(f.severity, Severity::Info);
}

// ---------------------------------------------------------------------------
// Category and severity variety tests
// ---------------------------------------------------------------------------
// These tests guard the stub contract: the engine must produce findings across
// all categories and severity tiers so the CLI mapping layer sees realistic
// input during development. If real rules are added that replace stubs, update
// these tests to reflect the new expected minimum.

#[test]
fn findings_span_multiple_categories() {
    let (text_files, all_paths) = with_skill_md();
    let result = scan_skill(&text_files, &all_paths);

    let categories: std::collections::HashSet<String> = result
        .findings
        .iter()
        .map(|f| f.category.as_str().to_string())
        .collect();

    assert!(
        categories.contains("structure"),
        "missing structure findings"
    );
    assert!(categories.contains("security"), "missing security findings");
    assert!(
        categories.contains("best-practices"),
        "missing best-practices findings"
    );
    assert!(
        categories.contains("description"),
        "missing description findings"
    );
}

#[test]
fn findings_span_multiple_severities() {
    let (text_files, all_paths) = with_skill_md();
    let result = scan_skill(&text_files, &all_paths);

    let severities: std::collections::HashSet<String> = result
        .findings
        .iter()
        .map(|f| f.severity.as_str().to_string())
        .collect();

    assert!(severities.contains("info"), "missing info findings");
    assert!(severities.contains("low"), "missing low findings");
    // At least one of medium/high must be present from stub security findings
    let has_medium_or_higher = severities.contains("medium")
        || severities.contains("high")
        || severities.contains("critical");
    assert!(
        has_medium_or_higher,
        "no medium/high/critical findings present"
    );
}

#[test]
fn scripts_category_present_when_scripts_dir_exists() {
    let (text_files, all_paths) = with_scripts();
    let result = scan_skill(&text_files, &all_paths);
    let has_scripts_finding = result
        .findings
        .iter()
        .any(|f| f.category == FindingCategory::Scripts);
    assert!(
        has_scripts_finding,
        "no scripts category finding when scripts/ is present"
    );
}

#[test]
fn evals_category_present_when_evals_dir_exists() {
    let (text_files, all_paths) = with_evals();
    let result = scan_skill(&text_files, &all_paths);
    let has_evals_finding = result
        .findings
        .iter()
        .any(|f| f.category == FindingCategory::Evals);
    assert!(
        has_evals_finding,
        "no evals category finding when evals/ is present"
    );
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn empty_inputs_returns_well_formed_result() {
    // 0-files edge case: scanner must not panic and must return a valid result.
    let result = scan_skill(&HashMap::new(), &[]);
    assert!(!result.has_skill_md);
    assert!(!result.has_scripts);
    assert!(!result.has_references);
    assert!(!result.has_evals);
    assert_eq!(result.file_count, 0);
    // Must still emit at least the missing-SKILL.md finding.
    assert!(!result.findings.is_empty());
}

#[test]
fn all_findings_have_non_empty_label_and_detail() {
    // Guard that no finding slips through with blank display text.
    let (text_files, all_paths) = with_skill_md();
    let result = scan_skill(&text_files, &all_paths);
    for f in &result.findings {
        assert!(!f.label.is_empty(), "finding has empty label: {:?}", f);
        assert!(!f.detail.is_empty(), "finding has empty detail: {:?}", f);
    }
}

#[test]
fn all_findings_have_valid_source() {
    let (text_files, all_paths) = with_skill_md();
    let result = scan_skill(&text_files, &all_paths);
    for f in &result.findings {
        assert!(!f.source.is_empty(), "finding has empty source: {:?}", f);
    }
}

#[test]
fn scanner_version_const_is_nonzero() {
    // Sanity check that CURRENT_SCANNER_VERSION is set to a real value.
    // Must stay in sync with skill-analyzer.ts's CURRENT_SCANNER_VERSION.
    assert_ne!(vettd_skill_scanner::consts::CURRENT_SCANNER_VERSION, 0);
}
