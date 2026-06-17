use crate::discovery::{browser_profile_roots, Candidate};
use crate::models::ArtifactReport;

use super::base::Detector;
use serde_json::json;
use std::fs;

pub struct BrowserFootprintDetector;

impl Detector for BrowserFootprintDetector {
    fn name(&self) -> &str {
        "browser_footprints"
    }

    fn detect(&self, _candidates: &[Candidate], _deep: bool) -> Vec<ArtifactReport> {
        let mut results = Vec::new();
        for root in browser_profile_roots() {
            if let Some(report) = scan_profile_root(&root) {
                results.push(report);
            }
        }
        results
    }
}

fn scan_profile_root(root: &std::path::Path) -> Option<ArtifactReport> {
    let extensions_dir = find_extensions_dir(root)?;
    let (count, ids) = enumerate_extensions(&extensions_dir);
    if count == 0 {
        return None;
    }

    let mut metadata = serde_json::Map::new();
    metadata.insert("paths".into(), json!([extensions_dir.to_string_lossy()]));
    metadata.insert("extension_count".into(), json!(count));
    metadata.insert("extension_ids".into(), json!(ids));
    metadata.insert("profile_root".into(), json!(root.to_string_lossy()));

    let mut report = ArtifactReport::new("browser_footprint", 0.6);
    report.metadata = metadata;
    report.artifact_scope = "host".to_string();
    report.compute_hash();
    Some(report)
}

fn find_extensions_dir(root: &std::path::Path) -> Option<std::path::PathBuf> {
    // Check Default/Extensions first
    let default_ext = root.join("Default").join("Extensions");
    if default_ext.is_dir() {
        return Some(default_ext);
    }
    // Check direct children for an Extensions subdir
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let ext_dir = entry.path().join("Extensions");
        if ext_dir.is_dir() {
            return Some(ext_dir);
        }
    }
    None
}

/// Return (count, ids) for extension subdirectories.
fn enumerate_extensions(dir: &std::path::Path) -> (usize, Vec<String>) {
    let entries: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flat_map(|rd| rd.flatten())
        .filter(|e| e.path().is_dir())
        .collect();
    let ids: Vec<String> = entries
        .iter()
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect();
    let count = ids.len();
    (count, ids)
}
