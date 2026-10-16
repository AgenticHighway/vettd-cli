//! Scan orchestration — wire discovery → detectors → risk scoring → verification.
//!
//! Runs built-in detectors and custom rule-based detectors,
//! merging their results into a single report.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::detectors::get_all_detectors;
use crate::discovery::{
    default_user_space_roots, discover_direct_home_files, discover_file_surface,
    discover_filesystem_surfaces, discover_home_surfaces, discover_workdir_surfaces,
    host_roots_for, walk_bounded, walk_harness_root, Candidate,
};
use crate::models::{ArtifactReport, ScanReport};
use crate::risk_engine::score_artifact;
use crate::verifier::verify;

struct ScanTimings {
    enabled: bool,
}

/// A directory to walk, with the origin tag its candidates carry.
struct DiscoveryRoot {
    path: PathBuf,
    origin: String,
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
    run_scan_with_options(mode, workdir, file, deep, false, on_tick)
}

/// Like [`run_scan`], and with `all_users` set `host` mode (`scan quick`)
/// also examines other local users' harness roots.
pub fn run_scan_with_options(
    mode: &str,
    workdir: Option<&Path>,
    file: Option<&Path>,
    deep: bool,
    all_users: bool,
    on_tick: Option<&dyn Fn(&str)>,
) -> ScanReport {
    let noop = |_: &str| {};
    let tick: &dyn Fn(&str) = on_tick.unwrap_or(&noop);
    let timings = ScanTimings::from_env();
    let scan_started_at = Instant::now();
    let detectors = get_all_detectors(mode);
    let scanned_path = resolve_scanned_path(mode, workdir, file);

    // 1. Discover candidates
    let discovery_started_at = Instant::now();
    let host_roots = if mode == "host" {
        host_roots_for(all_users)
    } else {
        Vec::new()
    };
    let target = DiscoveryTarget {
        workdir,
        file,
        deep,
        host_roots,
    };
    let candidates = discover_candidates(mode, &target, tick);
    timings.emit(
        "discovery",
        &format!("mode={mode} files={}", candidates.len()),
        discovery_started_at,
    );

    // 2. Run built-in (native) detectors
    tick(&format!("checking {} files…", candidates.len()));

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
            if let Some(output) = crate::contract::run_skill_scanner(artifact) {
                artifact.cached_scan_result = Some(output.external.clone());
                artifact.cached_skill_scan = Some(output);
            }
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

    let mut report = ScanReport::new(&scanned_path);
    report.artifacts = artifacts;
    report.examined_roots = target
        .host_roots
        .iter()
        .map(|root| root.display().to_string())
        .collect();
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

/// What a scan targets, resolved once before discovery.
struct DiscoveryTarget<'a> {
    workdir: Option<&'a Path>,
    file: Option<&'a Path>,
    deep: bool,
    /// Harness roots for `host` mode (`scan quick`); empty for other modes.
    host_roots: Vec<PathBuf>,
}

fn discover_candidates(
    mode: &str,
    target: &DiscoveryTarget,
    tick: &dyn Fn(&str),
) -> Vec<Candidate> {
    let DiscoveryTarget {
        workdir,
        file,
        deep,
        host_roots,
    } = target;
    let (workdir, file, deep) = (*workdir, *file, *deep);
    match mode {
        "file" => discover_file_surface(file.expect("file path is required for file mode")),
        "workdir" => discover_workdir_surfaces(
            workdir.expect("workdir path is required for workdir mode"),
            deep,
            Some(tick),
        ),
        "scan" => discover_scan_candidates(tick),
        "filesystem" => discover_filesystem_surfaces(Some(tick)),
        "home" => discover_home_surfaces(Some(tick)),
        "root" => crate::root_walk::discover_root_surfaces(Some(tick)),
        _ => discover_host_candidates(host_roots, tick),
    }
}

fn discover_host_candidates(host_roots: &[PathBuf], tick: &dyn Fn(&str)) -> Vec<Candidate> {
    let roots = host_roots
        .iter()
        .cloned()
        .map(|path| DiscoveryRoot {
            path,
            origin: "host".to_string(),
        })
        .collect::<Vec<_>>();
    discover_refreshable_roots(roots, tick)
}

fn discover_scan_candidates(tick: &dyn Fn(&str)) -> Vec<Candidate> {
    let mut candidates = discover_direct_home_files();
    let mut roots = host_roots_for(false)
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

    candidates.extend(discover_refreshable_roots(roots, tick));
    candidates
}

/// A root path for progress display, with the home directory shortened to `~`.
fn display_root(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rel) => format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display()),
        None => path.display().to_string(),
    }
}

fn discover_refreshable_roots(roots: Vec<DiscoveryRoot>, tick: &dyn Fn(&str)) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    for root in roots {
        // Name the directory being walked, and keep the name beside the file
        // count the walkers emit, so a long walk shows where it is.
        let root_label = display_root(&root.path);
        tick(&format!("walking {root_label}"));
        let root_tick = |detail: &str| tick(&format!("{root_label}: {detail}"));
        // Harness roots (origin "host") are walked completely; user-space
        // roots stay depth-bounded.
        let root_candidates = if root.origin == "host" {
            walk_harness_root(&root.path, &root.origin, Some(&root_tick))
        } else {
            walk_bounded(&root.path, &root.origin, Some(&root_tick))
        };
        candidates.extend(root_candidates);
    }

    candidates
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
    #[test]
    fn display_root_shortens_the_home_directory() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            super::display_root(&home.join(".claude")),
            format!("~{}.claude", std::path::MAIN_SEPARATOR)
        );
        assert_eq!(
            super::display_root(std::path::Path::new("/opt/x")),
            "/opt/x"
        );
    }

    use super::*;
    use crate::contract::build_contract_payload;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn artifact_at(atype: &str, path: &str) -> ArtifactReport {
        let mut a = ArtifactReport::new(atype, 0.8);
        a.metadata.insert("paths".into(), json!([path]));
        a
    }

    // --- cross-run freshness: nothing is persisted between scans ---

    /// Content digest reported for the first artifact whose `metadata["paths"][0]`
    /// ends with `suffix`.
    fn content_hash_for(report: &ScanReport, suffix: &str) -> String {
        report
            .artifacts
            .iter()
            .find(|artifact| {
                artifact
                    .metadata
                    .get("paths")
                    .and_then(|value| value.as_array())
                    .and_then(|paths| paths.first())
                    .and_then(|path| path.as_str())
                    .is_some_and(|path| Path::new(path).ends_with(suffix))
            })
            .and_then(|artifact| artifact.metadata.get("content_hash"))
            .and_then(|value| value.as_str())
            .expect("expected an artifact with a content_hash for the given path suffix")
            .to_string()
    }

    #[test]
    fn repeated_scans_reflect_current_content_on_a_same_size_same_mtime_tamper() {
        // Issue #293 deleted the per-file scan cache, which treated a file as
        // unchanged when its size AND mtime matched the previous run, and then
        // reused that run's detector results. With nothing persisted between
        // runs, a file edited to different content of the SAME size and the
        // SAME mtime must still be seen as changed by the next scan — this is
        // precisely the case the removed cache got wrong, so the property must
        // not become silently reintroducible.
        let dir = tempfile::tempdir().unwrap();
        let vscode_dir = dir.path().join(".vscode");
        std::fs::create_dir_all(&vscode_dir).unwrap();
        let mcp = vscode_dir.join("mcp.json");

        // Two valid MCP configs of identical byte length, differing only in the
        // server name.
        let content_a = r#"{"mcpServers":{"alpha":{"command":"node"}}}"#;
        let content_b = r#"{"mcpServers":{"bravo":{"command":"node"}}}"#;
        assert_eq!(
            content_a.len(),
            content_b.len(),
            "the two contents must be the same size for this test to be meaningful"
        );

        std::fs::write(&mcp, content_a).unwrap();
        let first = content_hash_for(
            &run_scan("workdir", Some(dir.path()), None, false, None),
            ".vscode/mcp.json",
        );

        // Re-scanning the untouched file must be stable, so the `assert_ne!`
        // below can only pass because the content actually changed.
        let rescan = content_hash_for(
            &run_scan("workdir", Some(dir.path()), None, false, None),
            ".vscode/mcp.json",
        );
        assert_eq!(
            rescan, first,
            "re-scanning unchanged content must report the same content hash"
        );

        // Tamper: different content of the same size, with the original mtime
        // restored so the stat tuple is identical to the pre-edit one.
        let original_mtime = std::fs::metadata(&mcp).unwrap().modified().unwrap();
        std::fs::write(&mcp, content_b).unwrap();
        {
            use std::fs::FileTimes;
            use std::fs::OpenOptions;
            OpenOptions::new()
                .write(true)
                .open(&mcp)
                .unwrap()
                .set_times(FileTimes::new().set_modified(original_mtime))
                .unwrap();
        }
        let meta = std::fs::metadata(&mcp).unwrap();
        assert_eq!(meta.len(), content_a.len() as u64, "size must be unchanged");
        assert_eq!(
            meta.modified().unwrap(),
            original_mtime,
            "mtime must be unchanged for this test to be meaningful"
        );

        let after = content_hash_for(
            &run_scan("workdir", Some(dir.path()), None, false, None),
            ".vscode/mcp.json",
        );
        assert_ne!(
            after, first,
            "a same-size same-mtime content change must be reflected by the next scan; \
             nothing may be reused across runs"
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
                    .map(|path| Path::new(path).ends_with(suffix))
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
    fn editing_the_dockerfile_keeps_ai_artifact_proximity_from_sibling_ai_files() {
        // The `containers` detector derives a container's `ai_artifact_proximity`
        // from *sibling* AI files, so the verdict must be re-derived from the
        // whole tree on every scan. Editing ONLY the Dockerfile (AGENTS.md
        // unchanged) must therefore not change it.
        let tmp = tempfile::TempDir::new().unwrap();
        let dockerfile = tmp.path().join("Dockerfile");
        let agents = tmp.path().join("AGENTS.md");
        std::fs::write(
            &dockerfile,
            "FROM python:3.11-slim\nRUN pip install -r requirements.txt\n",
        )
        .unwrap();
        std::fs::write(&agents, "# Agents\n\nUse shell tools to inspect files.\n").unwrap();

        let before = run_scan("workdir", Some(tmp.path()), None, true, None);
        assert_eq!(
            container_artifact(&before, "Dockerfile").metadata["ai_artifact_proximity"],
            true
        );

        // Change ONLY the Dockerfile; the sibling AGENTS.md is untouched.
        std::fs::write(
            &dockerfile,
            "FROM node:20-alpine\nWORKDIR /app\nCOPY . .\nCMD [\"npm\", \"start\"]\n",
        )
        .unwrap();

        let after = run_scan("workdir", Some(tmp.path()), None, true, None);
        assert_eq!(
            container_artifact(&after, "Dockerfile").metadata["ai_artifact_proximity"],
            true,
            "the second scan must still see the colocated AGENTS.md sibling"
        );
    }

    #[test]
    fn deleting_the_sibling_ai_file_drops_ai_artifact_proximity() {
        // The inverse case: deleting the sibling AI file must flip
        // `ai_artifact_proximity` off on the next scan even though the
        // Dockerfile itself is untouched, because the container verdict is
        // re-derived from the whole tree on every scan.
        let tmp = tempfile::TempDir::new().unwrap();
        let dockerfile = tmp.path().join("Dockerfile");
        let agents = tmp.path().join("AGENTS.md");
        std::fs::write(&dockerfile, "FROM python:3.11-slim\n").unwrap();
        std::fs::write(&agents, "# Agents\n").unwrap();

        let before = run_scan("workdir", Some(tmp.path()), None, true, None);
        assert_eq!(
            container_artifact(&before, "Dockerfile").metadata["ai_artifact_proximity"],
            true
        );

        std::fs::remove_file(&agents).unwrap();

        let after = run_scan("workdir", Some(tmp.path()), None, true, None);
        assert_eq!(
            container_artifact(&after, "Dockerfile").metadata["ai_artifact_proximity"],
            false,
            "deleting the only colocated AI file must drop proximity on the next scan"
        );
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
            .is_some_and(|path| Path::new(path).ends_with("skills/release-notes/SKILL.md")));

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
            .is_some_and(|path| Path::new(path).ends_with(".vscode/mcp.json")));
    }
}
