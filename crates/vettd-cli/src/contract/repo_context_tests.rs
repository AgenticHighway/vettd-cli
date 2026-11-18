//! End to end: a local skill inside a git work tree is assessed with its repository context
//! (vettd#1271). Real-data fixture text: nowork-studio/notfair-plugin at
//! be3a43233c8836525fafcfdb85f78fb08e56a43f, `skills/google-ads-assets/SKILL.md`, and
//! tinybirdco/tinybird-agent-skills at 19a3ec18b2270fb82cc3489eabf122da0706e6a0.

use std::fs;
use std::path::Path;

use crate::contract::build_skills;
use crate::contract::skill_scan::{assess, output_for};
use crate::models::ArtifactReport;

const NOTFAIR_WRAPPER: &str = "---\nname: google-ads-assets\ndescription: Plan, validate, and safely publish Google Ads assets, including sitelinks, callouts, structured snippets, image assets, and Performance Max asset briefs. Use when asked for Google Ads assets, ad extensions, sitelinks, callouts, snippets, image assets, Performance Max assets, PMax creative, or an asset audit.\n---\n\n# Canonical NotFair workflow\n\nRead [`../../google-ads/assets/WORKFLOW.md`](../../google-ads/assets/WORKFLOW.md) completely, then follow it as the active workflow.\n";

const TINYBIRD: &str = "---\nname: tinybird\ndescription: Tinybird file formats, SQL rules, optimization patterns, and best practices for datasources, pipes, endpoints, and materialized views.\n---\n\n# Tinybird Best Practices\n\nGuidance for Tinybird file formats, SQL rules, optimization patterns, and data modeling. Use this skill when creating or editing Tinybird datafiles.\n";

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn artifact(skill_md: &Path) -> ArtifactReport {
    let mut a = ArtifactReport::new("skill", 0.9);
    a.metadata.insert(
        "paths".to_string(),
        serde_json::json!([skill_md.to_string_lossy()]),
    );
    a.compute_hash();
    a
}

fn repo_with_git() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    dir
}

#[test]
fn wrapper_pointing_at_shared_workflow_is_partial_pending_conditional() {
    let repo = repo_with_git();
    write(
        repo.path(),
        "skills/google-ads-assets/SKILL.md",
        NOTFAIR_WRAPPER,
    );
    write(repo.path(), "google-ads/assets/WORKFLOW.md", "Workflow.\n");
    let a = artifact(&repo.path().join("skills/google-ads-assets/SKILL.md"));
    let out = output_for(&a).unwrap();
    let assessment = assess(Some(&out));
    assert_eq!(assessment.state, "partial");
    assert!(assessment.reasons.iter().any(|r| r == "unanalyzed-content"));
    let skills = build_skills(&[a], &[]);
    assert_eq!(skills[0].overall_grade, "pending");
    assert_eq!(skills[0].trust_level, "Conditional");
}

#[test]
fn repository_content_changes_move_the_fingerprint() {
    // The path walk is cached per root for the life of one process, so two repositories stand in
    // for "the repository changed between runs".
    let fingerprint = |extra: bool| {
        let repo = repo_with_git();
        write(
            repo.path(),
            "skills/google-ads-assets/SKILL.md",
            NOTFAIR_WRAPPER,
        );
        write(repo.path(), "google-ads/assets/WORKFLOW.md", "Workflow.\n");
        if extra {
            write(repo.path(), "google-ads/assets/EXTRA.md", "Another file.\n");
        }
        let a = artifact(&repo.path().join("skills/google-ads-assets/SKILL.md"));
        let out = output_for(&a).unwrap();
        assert_eq!(assess(Some(&out)).state, "partial");
        out.identity.assessment_fingerprint
    };
    assert_ne!(fingerprint(false), fingerprint(true));
}

#[test]
fn skill_without_outside_references_stays_complete_and_trusted() {
    let repo = repo_with_git();
    write(repo.path(), "skills/tinybird/SKILL.md", TINYBIRD);
    let a = artifact(&repo.path().join("skills/tinybird/SKILL.md"));
    let out = output_for(&a).unwrap();
    assert!(assess(Some(&out)).is_complete());
    let skills = build_skills(&[a], &[]);
    assert_eq!(skills[0].trust_level, "Trusted");
}

#[test]
fn folder_mismatch_caps_trust_without_changing_the_grade() {
    let repo = repo_with_git();
    write(
        repo.path(),
        "skills/tinybird-best-practices/SKILL.md",
        TINYBIRD,
    );
    let a = artifact(&repo.path().join("skills/tinybird-best-practices/SKILL.md"));
    let skills = build_skills(&[a], &[]);
    assert_eq!(skills[0].overall_grade, "A");
    assert_eq!(skills[0].trust_level, "Conditional");
}

#[test]
fn no_frontmatter_caps_trust() {
    let repo = repo_with_git();
    write(
        repo.path(),
        "prioritization/SKILL.md",
        "# PM Prioritization Framework\n\nA comprehensive prioritization skill for product managers to evaluate features, initiatives, and tasks using proven frameworks.\n",
    );
    let a = artifact(&repo.path().join("prioritization/SKILL.md"));
    let skills = build_skills(&[a], &[]);
    assert_eq!(skills[0].trust_level, "Conditional");
}

#[test]
fn outside_a_git_work_tree_an_escape_is_unverifiable_partial() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "skills/google-ads-assets/SKILL.md",
        NOTFAIR_WRAPPER,
    );
    let a = artifact(&dir.path().join("skills/google-ads-assets/SKILL.md"));
    let out = output_for(&a).unwrap();
    assert_eq!(assess(Some(&out)).state, "partial");
}
