//! Scan orchestration — wire discovery → detectors → risk scoring → verification.
//!
//! Runs built-in detectors and custom rule-based detectors,
//! merging their results into a single report.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::detectors::get_all_detectors;
use crate::discovery::{
    default_user_space_roots, discover_direct_home_files, discover_file_surface,
    discover_filesystem_surfaces, discover_home_surfaces, discover_root_surfaces,
    discover_workdir_surfaces, host_roots, walk_bounded, Candidate,
};
use crate::models::{ArtifactReport, ScanReport};
use crate::risk_engine::score_artifact;
use crate::scan_cache::{
    build_profile, cache_enabled_for_mode, cacheable_detector, detector_fingerprint,
    snapshot_candidates, CachedCandidate, ScanCache, ScanCacheProfile,
};
use crate::scan_refresh::{plan_root_refresh, DiscoveryRoot, RootCursorUpdate, RootRefreshAction};
use crate::verifier::verify;

struct ScanTimings {
    enabled: bool,
}

struct PreparedDiscovery {
    live_candidates: Vec<Candidate>,
    reused_cached_candidates: Vec<CachedCandidate>,
    refreshed_roots: Vec<RefreshedRoot>,
    cursor_updates: Vec<RootCursorUpdate>,
}

struct RefreshedRoot {
    path: PathBuf,
    keep_paths: HashSet<String>,
}

impl ScanTimings {
    fn from_env() -> Self {
        let enabled = std::env::var("VETTD_TIMINGS")
            .ok()
            .is_some_and(|value| timing_value_enabled(&value));
        Self { enabled }
    }

    fn emit(&self, stage: &str, detail: &str, started_at: Instant) {
        if self.enabled {
            eprintln!(
                "[timing] stage={stage} elapsed_ms={} {detail}",
                started_at.elapsed().as_millis()
            );
        }
    }
}

fn timing_value_enabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Execute a full scan and return a populated [`ScanReport`].
///
/// `mode` selects the discovery strategy:
/// - `"host"` — bounded host config roots (quick scan / agentic areas)
/// - `"scan"` — tiered default scan across critical host roots and bounded user-space roots
/// - `"home"` — recursive home directory scan (legacy internal mode)
/// - `"filesystem"` — full home + system app paths (legacy)
/// - `"root"` — entire filesystem from / (full scan)
/// - `"workdir"` — explicit project directory
/// - `"file"` — single file
pub fn run_scan(
    mode: &str,
    workdir: Option<&Path>,
    file: Option<&Path>,
    deep: bool,
    on_tick: Option<&dyn Fn(&str)>,
) -> ScanReport {
    let noop = |_: &str| {};
    let tick: &dyn Fn(&str) = on_tick.unwrap_or(&noop);
    let timings = ScanTimings::from_env();
    let scan_started_at = Instant::now();
    let detectors = get_all_detectors(mode);
    let scanned_path = resolve_scanned_path(mode, workdir, file);
    let mut scan_cache = if cache_enabled_for_mode(mode) {
        match ScanCache::open_default() {
            Ok(cache) => Some(cache),
            Err(e) => {
                eprintln!("Warning: scan-cache disabled: {e}");
                None
            }
        }
    } else {
        None
    };
    let cache_profile = scan_cache.as_ref().map(|_| {
        build_profile(
            mode,
            deep,
            &scanned_path,
            &detectors
                .iter()
                .map(|detector| detector.name().to_string())
                .collect::<Vec<_>>(),
            &crate::rule_engine::rules_fingerprint(),
        )
    });

    // 1. Discover candidates
    let discovery_started_at = Instant::now();
    let prepared = discover_candidates(
        mode,
        workdir,
        file,
        deep,
        tick,
        scan_cache.as_ref(),
        cache_profile.as_ref(),
    );
    let mut cached_candidates = snapshot_candidates(&prepared.live_candidates);
    let mut candidates = prepared.live_candidates;
    candidates.extend(
        prepared
            .reused_cached_candidates
            .iter()
            .map(|cached| cached.candidate.clone()),
    );
    cached_candidates.extend(prepared.reused_cached_candidates);
    timings.emit(
        "discovery",
        &format!("mode={mode} files={}", candidates.len()),
        discovery_started_at,
    );

    // 2. Run built-in (native) detectors
    tick(&format!("Scanning {} files…", candidates.len()));
    if let (Some(cache), Some(profile)) = (scan_cache.as_mut(), cache_profile.as_ref()) {
        if let Err(e) = cache.upsert_profile(profile) {
            eprintln!("Warning: failed to initialize scan-cache profile: {e}");
        }
        if let Err(e) = cache.upsert_file_states(&profile.profile_key, &cached_candidates) {
            eprintln!("Warning: failed to record scan-cache file states: {e}");
        }
    }

    let mut artifacts: Vec<ArtifactReport> = Vec::new();
    for (i, detector) in detectors.iter().enumerate() {
        tick(&format!(
            "detector {}/{}: {}",
            i + 1,
            detectors.len(),
            detector.name()
        ));
        let detector_started_at = Instant::now();
        let before_len = artifacts.len();
        let detector_name = detector.name();
        if let (Some(cache), Some(profile)) = (scan_cache.as_mut(), cache_profile.as_ref()) {
            if cacheable_detector(mode, detector_name) {
                match reuse_detector_results(
                    cache,
                    profile,
                    detector.as_ref(),
                    &cached_candidates,
                    deep,
                    &mut artifacts,
                ) {
                    Ok(cache_stats) => {
                        timings.emit(
                            "detector",
                            &format!(
                                "index={}/{} name={} new_artifacts={} cache_hits={} cache_misses={}",
                                i + 1,
                                detectors.len(),
                                detector_name,
                                artifacts.len() - before_len,
                                cache_stats.hit_files,
                                cache_stats.miss_files,
                            ),
                            detector_started_at,
                        );
                        continue;
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: detector cache disabled for {} during this run: {e}",
                            detector_name
                        );
                    }
                }
            }
        }
        artifacts.extend(detector.detect(&candidates, deep));
        timings.emit(
            "detector",
            &format!(
                "index={}/{} name={} new_artifacts={}",
                i + 1,
                detectors.len(),
                detector.name(),
                artifacts.len() - before_len
            ),
            detector_started_at,
        );
    }

    // 3. Score, verify, classify each artifact
    tick(&format!("Analyzing {} artifact(s)…", artifacts.len()));
    let analysis_started_at = Instant::now();
    for artifact in &mut artifacts {
        score_artifact(artifact);
        verify(artifact);
        classify_artifact(artifact, mode);
        if artifact.artifact_type == "skill" {
            artifact.cached_scan_result = crate::contract::run_skill_scanner(artifact);
        }
    }
    timings.emit(
        "analysis",
        &format!("mode={mode} artifacts={}", artifacts.len()),
        analysis_started_at,
    );

    tick(&format!(
        "Found {} artifact(s) across {} files",
        artifacts.len(),
        candidates.len()
    ));
    timings.emit(
        "scan_total",
        &format!(
            "mode={mode} files={} artifacts={}",
            candidates.len(),
            artifacts.len()
        ),
        scan_started_at,
    );

    if let (Some(cache), Some(profile)) = (scan_cache.as_mut(), cache_profile.as_ref()) {
        for refreshed_root in &prepared.refreshed_roots {
            if let Err(e) = cache.prune_profile_root_artifacts(
                &profile.profile_key,
                &refreshed_root.path,
                &refreshed_root.keep_paths,
            ) {
                eprintln!("Warning: failed to prune stale scan-cache rows: {e}");
            }
        }
        for cursor_update in &prepared.cursor_updates {
            if let Err(e) = cache.upsert_root_cursor(
                &cursor_update.root_path,
                &cursor_update.backend_type,
                &cursor_update.cursor_token,
            ) {
                eprintln!("Warning: failed to update scan-cache root cursor: {e}");
            }
        }
    }

    let mut report = ScanReport::new(&scanned_path);
    report.artifacts = artifacts;
    report
}

fn resolve_scanned_path(mode: &str, workdir: Option<&Path>, file: Option<&Path>) -> String {
    match mode {
        "file" => file
            .expect("file path is required for file mode")
            .canonicalize()
            .unwrap_or_else(|_| {
                file.expect("file path is required for file mode")
                    .to_path_buf()
            })
            .display()
            .to_string(),
        "workdir" => workdir
            .expect("workdir path is required for workdir mode")
            .canonicalize()
            .unwrap_or_else(|_| {
                workdir
                    .expect("workdir path is required for workdir mode")
                    .to_path_buf()
            })
            .display()
            .to_string(),
        "scan" => "~ (default critical + user-space surfaces)".to_string(),
        "filesystem" => "/ (full filesystem)".to_string(),
        "home" => "~ (home directory)".to_string(),
        "root" => "/ (full filesystem)".to_string(),
        _ => "~".to_string(),
    }
}

fn discover_candidates(
    mode: &str,
    workdir: Option<&Path>,
    file: Option<&Path>,
    deep: bool,
    tick: &dyn Fn(&str),
    scan_cache: Option<&ScanCache>,
    cache_profile: Option<&ScanCacheProfile>,
) -> PreparedDiscovery {
    match mode {
        "file" => PreparedDiscovery {
            live_candidates: discover_file_surface(
                file.expect("file path is required for file mode"),
            ),
            reused_cached_candidates: Vec::new(),
            refreshed_roots: Vec::new(),
            cursor_updates: Vec::new(),
        },
        "workdir" => PreparedDiscovery {
            live_candidates: discover_workdir_surfaces(
                workdir.expect("workdir path is required for workdir mode"),
                deep,
                Some(tick),
            ),
            reused_cached_candidates: Vec::new(),
            refreshed_roots: Vec::new(),
            cursor_updates: Vec::new(),
        },
        "scan" => discover_scan_candidates(tick, scan_cache, cache_profile),
        "filesystem" => PreparedDiscovery {
            live_candidates: discover_filesystem_surfaces(Some(tick)),
            reused_cached_candidates: Vec::new(),
            refreshed_roots: Vec::new(),
            cursor_updates: Vec::new(),
        },
        "home" => PreparedDiscovery {
            live_candidates: discover_home_surfaces(Some(tick)),
            reused_cached_candidates: Vec::new(),
            refreshed_roots: Vec::new(),
            cursor_updates: Vec::new(),
        },
        "root" => PreparedDiscovery {
            live_candidates: discover_root_surfaces(Some(tick)),
            reused_cached_candidates: Vec::new(),
            refreshed_roots: Vec::new(),
            cursor_updates: Vec::new(),
        },
        _ => discover_host_candidates(tick, scan_cache, cache_profile),
    }
}

fn discover_host_candidates(
    tick: &dyn Fn(&str),
    scan_cache: Option<&ScanCache>,
    cache_profile: Option<&ScanCacheProfile>,
) -> PreparedDiscovery {
    let roots = host_roots()
        .into_iter()
        .map(|path| DiscoveryRoot {
            path,
            origin: "host".to_string(),
        })
        .collect::<Vec<_>>();
    discover_refreshable_roots(roots, tick, scan_cache, cache_profile)
}

fn discover_scan_candidates(
    tick: &dyn Fn(&str),
    scan_cache: Option<&ScanCache>,
    cache_profile: Option<&ScanCacheProfile>,
) -> PreparedDiscovery {
    let mut prepared = PreparedDiscovery {
        live_candidates: discover_direct_home_files(),
        reused_cached_candidates: Vec::new(),
        refreshed_roots: Vec::new(),
        cursor_updates: Vec::new(),
    };
    let mut roots = host_roots()
        .into_iter()
        .map(|path| DiscoveryRoot {
            path,
            origin: "host".to_string(),
        })
        .collect::<Vec<_>>();
    roots.extend(
        default_user_space_roots()
            .into_iter()
            .map(|path| DiscoveryRoot {
                path,
                origin: "home".to_string(),
            }),
    );

    let refreshed = discover_refreshable_roots(roots, tick, scan_cache, cache_profile);
    prepared.live_candidates.extend(refreshed.live_candidates);
    prepared
        .reused_cached_candidates
        .extend(refreshed.reused_cached_candidates);
    prepared.refreshed_roots.extend(refreshed.refreshed_roots);
    prepared.cursor_updates.extend(refreshed.cursor_updates);
    prepared
}

fn discover_refreshable_roots(
    roots: Vec<DiscoveryRoot>,
    tick: &dyn Fn(&str),
    scan_cache: Option<&ScanCache>,
    cache_profile: Option<&ScanCacheProfile>,
) -> PreparedDiscovery {
    let plans = plan_root_refresh(scan_cache, &roots);
    let mut live_candidates = Vec::new();
    let mut reused_cached_candidates = Vec::new();
    let mut refreshed_roots = Vec::new();
    let mut cursor_updates = Vec::new();

    for plan in plans {
        if let Some(cursor_update) = plan.cursor_update.clone() {
            cursor_updates.push(cursor_update);
        }

        if matches!(plan.action, RootRefreshAction::ReuseCached) {
            if let (Some(cache), Some(profile)) = (scan_cache, cache_profile) {
                match cache.load_profile_candidates_for_root(&profile.profile_key, &plan.root.path)
                {
                    Ok(cached) if !cached.is_empty() => {
                        reused_cached_candidates.extend(cached);
                        continue;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!(
                            "Warning: failed to load cached root membership for {}: {e}",
                            plan.root.path.display()
                        );
                    }
                }
            }
        }

        let root_candidates = walk_bounded(&plan.root.path, &plan.root.origin, Some(tick));
        let keep_paths = root_candidates
            .iter()
            .map(|candidate| candidate.path.to_string_lossy().to_string())
            .collect();
        refreshed_roots.push(RefreshedRoot {
            path: plan.root.path,
            keep_paths,
        });
        live_candidates.extend(root_candidates);
    }

    PreparedDiscovery {
        live_candidates,
        reused_cached_candidates,
        refreshed_roots,
        cursor_updates,
    }
}

struct DetectorCacheStats {
    hit_files: usize,
    miss_files: usize,
}

fn reuse_detector_results(
    cache: &mut ScanCache,
    profile: &crate::scan_cache::ScanCacheProfile,
    detector: &dyn crate::detectors::base::Detector,
    candidates: &[CachedCandidate],
    deep: bool,
    all_artifacts: &mut Vec<ArtifactReport>,
) -> Result<DetectorCacheStats, String> {
    let detector_name = detector.name();
    let detector_fingerprint = detector_fingerprint(detector_name);
    let cached_bundles = cache.load_detector_bundles(&profile.profile_key, detector_name)?;
    let mut misses = Vec::new();
    let mut miss_candidates = Vec::new();
    let mut hit_files = 0;

    for candidate in candidates {
        let Some(file_state) = &candidate.file_state else {
            misses.push(candidate.clone());
            miss_candidates.push(candidate.candidate.clone());
            continue;
        };

        match cached_bundles.get(&file_state.canonical_path) {
            Some(bundle)
                if bundle.file_state_key == file_state.state_key
                    && bundle.detector_fingerprint == detector_fingerprint =>
            {
                all_artifacts.extend(bundle.artifacts.clone());
                hit_files += 1;
            }
            _ => {
                misses.push(candidate.clone());
                miss_candidates.push(candidate.candidate.clone());
            }
        }
    }

    let fresh_artifacts = detector.detect(&miss_candidates, deep);
    let (artifacts_by_path, unattributed) = group_artifacts_by_path(&fresh_artifacts);
    if unattributed > 0 {
        eprintln!(
            "Warning: detector '{detector_name}' produced {unattributed} artifact(s) without a \
             resolvable metadata[\"paths\"][0]; skipping scan-cache persistence for this \
             detector this run to avoid silently caching an empty result for the candidate(s) \
             those artifacts belong to. Results will be re-detected on every scan until this \
             detector is fixed."
        );
    }
    if unattributed == 0 && cache_attribution_is_safe(detector_name, &artifacts_by_path, &misses) {
        cache.persist_detector_results(
            &profile.profile_key,
            detector_name,
            &detector_fingerprint,
            &misses,
            &artifacts_by_path,
        )?;
    }
    all_artifacts.extend(fresh_artifacts);

    Ok(DetectorCacheStats {
        hit_files,
        miss_files: misses.len(),
    })
}

fn group_artifacts_by_path(
    artifacts: &[ArtifactReport],
) -> (HashMap<String, Vec<ArtifactReport>>, usize) {
    let mut grouped = HashMap::new();
    let mut unattributed = 0;
    for artifact in artifacts {
        let Some(path) = artifact
            .metadata
            .get("paths")
            .and_then(|value| value.as_array())
            .and_then(|paths| paths.first())
            .and_then(|value| value.as_str())
        else {
            unattributed += 1;
            continue;
        };
        grouped
            .entry(path.to_string())
            .or_insert_with(Vec::new)
            .push(artifact.clone());
    }
    (grouped, unattributed)
}

/// Guards the detector-registration contract's path-key invariant: a
/// cacheable detector's artifacts must be keyed (via `metadata["paths"][0]`)
/// to a canonical path that's actually among this run's candidates.
/// `persist_detector_results` looks up `artifacts_by_path` by each
/// candidate's canonical path and silently persists an empty result on a
/// miss — so an artifact keyed to a path outside `misses` would otherwise
/// vanish from the cache immediately, then look like a legitimate "zero
/// artifacts" hit on every subsequent warm scan. If that happens, skip
/// caching for this detector this run (falling back to always re-detecting,
/// which is slow but never silently wrong) and warn loudly instead.
fn cache_attribution_is_safe(
    detector_name: &str,
    artifacts_by_path: &HashMap<String, Vec<ArtifactReport>>,
    misses: &[CachedCandidate],
) -> bool {
    let known_paths: HashSet<&str> = misses
        .iter()
        .filter_map(|candidate| candidate.file_state.as_ref())
        .map(|file_state| file_state.canonical_path.as_str())
        .collect();
    let orphaned: Vec<&str> = artifacts_by_path
        .keys()
        .map(String::as_str)
        .filter(|path| !known_paths.contains(path))
        .collect();
    if orphaned.is_empty() {
        return true;
    }
    eprintln!(
        "Warning: detector '{detector_name}' reported artifacts for {} path(s) not present in \
         this run's candidate set; skipping scan-cache persistence for this detector this run \
         to avoid silently caching an empty result. This usually means \
         metadata[\"paths\"][0] does not exactly equal the candidate's canonicalized path.",
        orphaned.len()
    );
    false
}

// ---------------------------------------------------------------------------
// Post-detection classification
// ---------------------------------------------------------------------------

const DOCS_PATH_SEGMENTS: &[&str] = &[
    "docs",
    "doc",
    "documentation",
    "reference",
    "concepts",
    "examples",
];

fn classify_artifact(artifact: &mut ArtifactReport, mode: &str) {
    let atype = artifact.artifact_type.as_str();
    let discovery_origin = artifact.artifact_scope.clone();
    let first_path = artifact
        .metadata
        .get("paths")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    let path_parts: HashSet<&str> = Path::new(&first_path)
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    // --- artifact_scope ---
    artifact.artifact_scope = if atype == "browser_footprint" {
        "host".to_string()
    } else if atype == "container_config" || atype == "container_candidate" {
        "container".to_string()
    } else if DOCS_PATH_SEGMENTS
        .iter()
        .any(|seg| path_parts.contains(seg))
    {
        "docs".to_string()
    } else if mode == "host" || discovery_origin == "host" {
        "host".to_string()
    } else {
        "project".to_string()
    };

    // --- registry_eligible ---
    artifact.registry_eligible = match atype {
        "cursor_rules" | "agents_md" => true,
        "source_risk_surface" => false,
        "container_candidate" => false,
        "prompt_config" if artifact.artifact_scope == "docs" => false,
        "prompt_config" => {
            let has_keywords = artifact.signals.iter().any(|s| s.starts_with("keyword:"));
            has_keywords || artifact.confidence >= 0.85
        }
        "container_config" => true,
        _ => artifact.confidence >= 0.6,
    };

    tag_analysis_origin(artifact);
}

// ---------------------------------------------------------------------------
// Analysis-origin tagging
// ---------------------------------------------------------------------------

const LOCAL_ANALYSIS_SIGNALS: &[&str] = &[
    "credential_exposure_signal",
    "dangerous_combo:shell+network+fs",
    "dangerous_keyword:exfiltrate",
    "dangerous_keyword:reverse",
    "dangerous_keyword:steal",
    "dangerous_keyword:wipe",
    "dangerous_keyword:bypass",
    "keyword:shell",
    "keyword:browser",
    "keyword:api",
    "keyword:execute",
    "keyword:network",
    "keyword:filesystem",
];

fn tag_analysis_origin(artifact: &mut ArtifactReport) {
    let has_local_signal = artifact.artifact_type == "source_risk_surface"
        || artifact.signals.iter().any(|s| {
            LOCAL_ANALYSIS_SIGNALS.contains(&s.as_str())
                || s.starts_with("secret:")
                || s.starts_with("ssrf:")
                || s.starts_with("cognitive_tampering:")
        });

    let origin = if has_local_signal
        || matches!(artifact.verification_status.as_str(), "critical" | "high")
    {
        "local"
    } else {
        "server_candidate"
    };

    artifact.metadata.insert(
        "analysis_origin".to_string(),
        serde_json::Value::String(origin.to_string()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::build_contract_payload;
    use crate::scan_cache::FileStateSnapshot;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn artifact_at(atype: &str, path: &str) -> ArtifactReport {
        let mut a = ArtifactReport::new(atype, 0.8);
        a.metadata.insert("paths".into(), json!([path]));
        a
    }

    fn cached_candidate_at(path: &str) -> CachedCandidate {
        CachedCandidate {
            candidate: Candidate {
                path: PathBuf::from(path),
                origin: "workdir".to_string(),
            },
            file_state: Some(FileStateSnapshot {
                canonical_path: path.to_string(),
                origin: "workdir".to_string(),
                stable_file_id: None,
                size_bytes: 0,
                modified_ns: None,
                state_key: "state".to_string(),
            }),
        }
    }

    // --- group_artifacts_by_path ---

    #[test]
    fn group_artifacts_by_path_groups_by_metadata_paths_first_entry() {
        let artifacts = vec![artifact_at("mcp_config", "/project/mcp.json")];
        let (grouped, unattributed) = group_artifacts_by_path(&artifacts);
        assert_eq!(unattributed, 0);
        assert_eq!(grouped.get("/project/mcp.json").unwrap().len(), 1);
    }

    #[test]
    fn group_artifacts_by_path_counts_artifacts_missing_paths_metadata() {
        let artifact = ArtifactReport::new("mcp_config", 0.8); // no metadata["paths"]
        let (grouped, unattributed) = group_artifacts_by_path(&[artifact]);
        assert!(grouped.is_empty());
        assert_eq!(unattributed, 1);
    }

    // --- cache_attribution_is_safe: the paths[0]-must-match-a-candidate invariant ---

    #[test]
    fn cache_attribution_is_safe_when_artifact_paths_match_candidates() {
        let artifacts = vec![artifact_at("mcp_config", "/project/mcp.json")];
        let (grouped, _) = group_artifacts_by_path(&artifacts);
        let misses = vec![cached_candidate_at("/project/mcp.json")];

        assert!(cache_attribution_is_safe("mcp_configs", &grouped, &misses));
    }

    #[test]
    fn cache_attribution_is_unsafe_when_artifact_path_has_no_matching_candidate() {
        // Simulates a detector whose metadata["paths"][0] doesn't exactly
        // equal the candidate's canonicalized path — e.g. a relative path,
        // or a symlink target instead of the symlink itself. Without this
        // guard, persist_detector_results would cache an empty result under
        // the real candidate's path, silently dropping the artifact from
        // every subsequent warm scan.
        let artifacts = vec![artifact_at("mcp_config", "/elsewhere/mcp.json")];
        let (grouped, _) = group_artifacts_by_path(&artifacts);
        let misses = vec![cached_candidate_at("/project/mcp.json")];

        assert!(!cache_attribution_is_safe("mcp_configs", &grouped, &misses));
    }

    #[test]
    fn cache_attribution_is_safe_for_empty_artifacts() {
        let (grouped, _) = group_artifacts_by_path(&[]);
        let misses = vec![cached_candidate_at("/project/mcp.json")];

        assert!(cache_attribution_is_safe("mcp_configs", &grouped, &misses));
    }

    // --- reuse_detector_results: end-to-end warm-scan regression coverage ---

    struct StubDetector {
        name: &'static str,
        artifacts: Vec<ArtifactReport>,
    }

    impl crate::detectors::base::Detector for StubDetector {
        fn name(&self) -> &str {
            self.name
        }

        fn detect(&self, _candidates: &[Candidate], _deep: bool) -> Vec<ArtifactReport> {
            self.artifacts.clone()
        }
    }

    #[test]
    fn reuse_detector_results_never_silently_drops_an_unattributed_artifact_on_warm_scans() {
        // Regression test for the exact failure mode issue #200 calls out:
        // a cacheable detector that returns an artifact without a
        // metadata["paths"][0] matching its candidate's canonical path
        // must not have an empty result cached for that candidate — that
        // would make the artifact vanish on every subsequent unchanged
        // (warm) scan, without any error or indication anything is wrong.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("scan-v1.sqlite3");
        let file = dir.path().join("mcp.json");
        std::fs::write(&file, "{}").unwrap();

        let mut cache = ScanCache::open_at(&db_path).unwrap();
        let profile = build_profile(
            "workdir",
            false,
            "test",
            &["stub_detector".to_string()],
            "rules",
        );
        cache.upsert_profile(&profile).unwrap();

        let candidates = snapshot_candidates(&[Candidate {
            path: file.clone(),
            origin: "workdir".to_string(),
        }]);
        cache
            .upsert_file_states(&profile.profile_key, &candidates)
            .unwrap();

        // Deliberately missing metadata["paths"] — simulates a detector
        // that violates the cache-key invariant.
        let mut broken_artifact = ArtifactReport::new("mcp_config", 0.9);
        broken_artifact.compute_hash();
        let detector = StubDetector {
            name: "stub_detector",
            artifacts: vec![broken_artifact],
        };

        let mut cold_scan = Vec::new();
        reuse_detector_results(
            &mut cache,
            &profile,
            &detector,
            &candidates,
            false,
            &mut cold_scan,
        )
        .unwrap();
        assert_eq!(cold_scan.len(), 1, "artifact should surface on a cold scan");

        // Second scan against the same unchanged file: file_state hasn't
        // changed, so a naive cache would report this as a hit backed by
        // whatever was persisted after the first scan.
        let mut warm_scan = Vec::new();
        reuse_detector_results(
            &mut cache,
            &profile,
            &detector,
            &candidates,
            false,
            &mut warm_scan,
        )
        .unwrap();
        assert_eq!(
            warm_scan.len(),
            1,
            "artifact must still surface on a warm scan — it should be treated as a \
             permanent cache miss (always re-detected) rather than have an empty result \
             cached for it"
        );
    }

    // --- classify_artifact: scope ---

    #[test]
    fn browser_footprint_scope_is_host() {
        let mut a = artifact_at("browser_footprint", "/home/user/.config/chrome");
        classify_artifact(&mut a, "home");
        assert_eq!(a.artifact_scope, "host");
    }

    #[test]
    fn container_scope_is_container() {
        let mut a = artifact_at("container_config", "/project/Dockerfile");
        classify_artifact(&mut a, "workdir");
        assert_eq!(a.artifact_scope, "container");
    }

    #[test]
    fn container_candidate_scope_is_container() {
        let mut a = artifact_at("container_candidate", "/project/Dockerfile");
        classify_artifact(&mut a, "workdir");
        assert_eq!(a.artifact_scope, "container");
    }

    #[test]
    fn docs_path_produces_docs_scope() {
        let mut a = artifact_at("cursor_rules", "/project/docs/.cursorrules");
        classify_artifact(&mut a, "workdir");
        assert_eq!(a.artifact_scope, "docs");
    }

    #[test]
    fn host_mode_produces_host_scope() {
        let mut a = artifact_at("cursor_rules", "/home/user/.cursorrules");
        classify_artifact(&mut a, "host");
        assert_eq!(a.artifact_scope, "host");
    }

    #[test]
    fn scan_mode_preserves_host_origin_scope() {
        let mut a = artifact_at("cursor_rules", "/home/user/.cursor/rules.md");
        a.artifact_scope = "host".to_string();
        classify_artifact(&mut a, "scan");
        assert_eq!(a.artifact_scope, "host");
    }

    #[test]
    fn workdir_mode_produces_project_scope() {
        let mut a = artifact_at("cursor_rules", "/project/.cursorrules");
        classify_artifact(&mut a, "workdir");
        assert_eq!(a.artifact_scope, "project");
    }

    // --- classify_artifact: registry_eligible ---

    #[test]
    fn cursor_rules_always_eligible() {
        let mut a = artifact_at("cursor_rules", "/project/.cursorrules");
        classify_artifact(&mut a, "workdir");
        assert!(a.registry_eligible);
    }

    #[test]
    fn container_candidate_never_eligible() {
        let mut a = artifact_at("container_candidate", "/project/Dockerfile");
        classify_artifact(&mut a, "workdir");
        assert!(!a.registry_eligible);
    }

    #[test]
    fn prompt_config_in_docs_not_eligible() {
        let mut a = artifact_at("prompt_config", "/project/docs/prompt.md");
        classify_artifact(&mut a, "workdir");
        assert!(!a.registry_eligible);
    }

    #[test]
    fn prompt_config_with_keywords_eligible() {
        let mut a = artifact_at("prompt_config", "/project/prompt.md");
        a.signals.push("keyword:shell".into());
        classify_artifact(&mut a, "workdir");
        assert!(a.registry_eligible);
    }

    #[test]
    fn prompt_config_high_confidence_eligible() {
        let mut a = ArtifactReport::new("prompt_config", 0.90);
        a.metadata
            .insert("paths".into(), json!(["/project/prompt.md"]));
        classify_artifact(&mut a, "workdir");
        assert!(a.registry_eligible);
    }

    #[test]
    fn timing_value_enabled_accepts_true_like_values() {
        assert!(timing_value_enabled("1"));
        assert!(timing_value_enabled("true"));
        assert!(timing_value_enabled("YES"));
        assert!(timing_value_enabled("on"));
    }

    #[test]
    fn timing_value_enabled_rejects_other_values() {
        assert!(!timing_value_enabled("0"));
        assert!(!timing_value_enabled("false"));
        assert!(!timing_value_enabled(""));
        assert!(!timing_value_enabled("maybe"));
    }

    #[test]
    fn source_risk_surface_never_registry_eligible() {
        let mut a = artifact_at("source_risk_surface", "/project");
        classify_artifact(&mut a, "workdir");
        assert!(!a.registry_eligible);
    }

    // --- tag_analysis_origin ---

    #[test]
    fn local_signal_tags_local_origin() {
        let mut a = artifact_at("cursor_rules", "/project/.cursorrules");
        a.signals.push("keyword:shell".into());
        tag_analysis_origin(&mut a);
        assert_eq!(a.metadata["analysis_origin"], "local");
    }

    #[test]
    fn structured_ssrf_signal_tags_local_origin() {
        let mut a = artifact_at("prompt_config", "/project/prompt.md");
        a.signals.push("ssrf:metadata:aws".into());
        tag_analysis_origin(&mut a);
        assert_eq!(a.metadata["analysis_origin"], "local");
    }

    #[test]
    fn source_risk_surface_tags_local_origin() {
        let mut a = artifact_at("source_risk_surface", "/project");
        tag_analysis_origin(&mut a);
        assert_eq!(a.metadata["analysis_origin"], "local");
    }

    #[test]
    fn no_local_signal_tags_server_candidate() {
        let mut a = artifact_at("cursor_rules", "/project/.cursorrules");
        a.signals.push("filename_match:.cursorrules".into());
        tag_analysis_origin(&mut a);
        assert_eq!(a.metadata["analysis_origin"], "server_candidate");
    }

    #[test]
    fn failed_verification_tags_local_origin() {
        let mut a = artifact_at("cursor_rules", "/project/.cursorrules");
        a.verification_status = "critical".into();
        tag_analysis_origin(&mut a);
        assert_eq!(a.metadata["analysis_origin"], "local");
    }

    fn fixture_dir(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("docker")
            .join(name)
    }

    fn container_artifact<'a>(report: &'a ScanReport, suffix: &str) -> &'a ArtifactReport {
        report
            .artifacts
            .iter()
            .find(|artifact| {
                artifact
                    .metadata
                    .get("paths")
                    .and_then(|v| v.as_array())
                    .and_then(|paths| paths.first())
                    .and_then(|path| path.as_str())
                    .map(|path| path.ends_with(suffix))
                    .unwrap_or(false)
            })
            .expect("expected fixture artifact")
    }

    #[test]
    fn fixture_plain_docker_stays_candidate_and_not_agentic_app() {
        let fixture = fixture_dir("plain-image-definition");
        let report = run_scan("workdir", Some(&fixture), None, true, None);
        let docker = container_artifact(&report, "plain-image-definition/Dockerfile");

        assert_eq!(docker.artifact_type, "container_candidate");
        assert_eq!(docker.metadata["container_kind"], "image_definition");
        assert_eq!(docker.metadata["direct_ai_evidence"], false);
        assert_eq!(docker.metadata["direct_agentic_evidence"], false);

        let payload = build_contract_payload(&report, 0);
        assert!(payload.agentic_apps.is_empty());
    }

    #[test]
    fn fixture_direct_agentic_compose_builds_agentic_app() {
        let fixture = fixture_dir("direct-agentic-compose");
        let report = run_scan("workdir", Some(&fixture), None, true, None);
        let compose = container_artifact(&report, "direct-agentic-compose/docker-compose.yml");

        assert_eq!(compose.artifact_type, "container_config");
        assert_eq!(compose.metadata["container_kind"], "service_orchestration");
        assert_eq!(compose.metadata["direct_ai_evidence"], true);
        assert_eq!(compose.metadata["direct_agentic_evidence"], true);
        assert_eq!(compose.metadata["services"], json!(["orchestrator"]));

        let payload = build_contract_payload(&report, 0);
        assert_eq!(payload.agentic_apps.len(), 1);
        assert_eq!(payload.agentic_apps[0].agent_count, 0);
        assert!(payload.agentic_apps[0]
            .description
            .contains("Service orchestration configuration"));
    }

    #[test]
    fn fixture_colocated_agent_project_promotes_docker_candidate() {
        let fixture = fixture_dir("colocated-agent-project");
        let report = run_scan("workdir", Some(&fixture), None, true, None);
        let docker = container_artifact(&report, "colocated-agent-project/Dockerfile");

        assert_eq!(docker.artifact_type, "container_candidate");
        assert_eq!(docker.metadata["container_kind"], "image_definition");
        assert_eq!(docker.metadata["ai_artifact_proximity"], true);

        let payload = build_contract_payload(&report, 0);
        assert_eq!(payload.agents.len(), 1);
        assert_eq!(payload.agentic_apps.len(), 1);
        assert_eq!(payload.agentic_apps[0].agent_count, 1);
        assert!(payload.agentic_apps[0]
            .description
            .contains("Container image definition"));
    }

    #[test]
    fn workdir_scan_detects_nested_skill_files_as_skills_not_prompts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skills").join("release-notes");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "# Release notes\n\nUse shell tools and API calls to draft release notes.",
        )
        .unwrap();

        let report = run_scan("workdir", Some(tmp.path()), None, false, None);
        let skill_artifact = report
            .artifacts
            .iter()
            .find(|artifact| artifact.artifact_type == "skill")
            .expect("expected nested SKILL.md to be detected as a skill artifact");

        assert!(skill_artifact
            .metadata
            .get("paths")
            .and_then(|value| value.as_array())
            .and_then(|paths| paths.first())
            .and_then(|path| path.as_str())
            .is_some_and(|path| path.ends_with("skills/release-notes/SKILL.md")));

        let payload = build_contract_payload(&report, 0);
        assert_eq!(payload.skills.len(), 1);
        assert_eq!(payload.skills[0].name, "release-notes/SKILL");
        assert!(payload.prompts.is_empty());
    }

    #[test]
    fn workdir_scan_detects_vscode_mcp_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vscode_dir = tmp.path().join(".vscode");
        std::fs::create_dir_all(&vscode_dir).unwrap();
        std::fs::write(
            vscode_dir.join("mcp.json"),
            r#"{"mcpServers": {"filesystem": {"command": "npx"}}}"#,
        )
        .unwrap();
        std::fs::write(vscode_dir.join("settings.json"), "{}").unwrap();

        let report = run_scan("workdir", Some(tmp.path()), None, false, None);
        let mcp_artifact = report
            .artifacts
            .iter()
            .find(|artifact| artifact.artifact_type == "mcp_config")
            .expect("expected .vscode/mcp.json to be detected as an mcp_config artifact");

        assert!(mcp_artifact
            .metadata
            .get("paths")
            .and_then(|value| value.as_array())
            .and_then(|paths| paths.first())
            .and_then(|path| path.as_str())
            .is_some_and(|path| path.ends_with(".vscode/mcp.json")));
    }
}
