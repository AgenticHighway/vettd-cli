//! Output type for a single skill scan.

use crate::finding::Finding;

/// The result of scanning one skill package.
///
/// Mirrors `SkillAnalysisResult` from the vettd web app's `skill-analyzer.ts`,
/// minus `overallGrade` and `overallScore` — grade computation is the caller's
/// responsibility and lives outside this crate.
#[derive(Debug, Clone)]
pub struct SkillScanResult {
    /// All findings produced by the scanner, including structural checks,
    /// security detections, and chain synthesis findings.
    ///
    /// **Important**: chain detection may mutate `severity` on existing entries.
    /// If the caller computes a grade, it must do so *after* receiving this result
    /// (chain detection runs as the final step inside `scan_skill`).
    pub findings: Vec<Finding>,

    /// Whether a `SKILL.md` or `skill.md` file exists at the package root.
    pub has_skill_md: bool,

    /// Whether a `scripts/` directory exists in the package.
    pub has_scripts: bool,

    /// Whether a `references/` directory exists in the package.
    pub has_references: bool,

    /// Whether an `evals/` directory or `evals.json` exists in the package.
    pub has_evals: bool,

    /// Total number of paths in the package (text + binary).
    pub file_count: usize,
}
