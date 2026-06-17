use crate::content_patterns::scan_secret_signals;
use crate::discovery::Candidate;
use crate::models::{ArtifactReport, CONTENT_READ_ALLOWLIST, CONTENT_READ_GLOB_PATTERNS};
use crate::source_patterns::{
    cognitive_file_names, cognitive_target_function_pattern, internal_hostname_context_pattern,
    json_secret_patterns, json_url_patterns, link_local_ip_pattern, network_call_pattern,
    private_ip_pattern, sensitive_path_patterns, should_skip_json_config, source_context_patterns,
    write_function_pattern, MAX_JSON_CONFIG_BYTES, MAX_SOURCE_ANALYSIS_BYTES,
};
use glob::Pattern;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const MAX_SOURCE_SURFACE_FILES: usize = 512;

const SOURCE_EXTENSIONS: &[&str] = &[
    "js", "jsx", "ts", "tsx", "mjs", "cjs", "py", "go", "rs", "java", "rb", "sh",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceFinding {
    pub(crate) family: &'static str,
    pub(crate) signal: String,
    pub(crate) path: PathBuf,
    pub(crate) line: Option<usize>,
    pub(crate) summary: String,
}

pub(crate) fn is_supported_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            SOURCE_EXTENSIONS
                .iter()
                .any(|candidate| ext.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or(false)
}

pub(crate) fn is_supported_json_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

pub(crate) fn is_scannable_json_file(path: &Path) -> bool {
    is_supported_json_file(path) && !should_skip_json_config(path)
}

pub(crate) fn has_ai_adjacent_candidate(candidates: &[Candidate]) -> bool {
    candidates
        .iter()
        .any(|candidate| is_ai_adjacent_path(&candidate.path))
}

pub(crate) fn build_source_risk_surface(
    root: &Path,
    scanned_source_count: usize,
    scanned_json_count: usize,
    findings: &[SourceFinding],
    ai_adjacent: bool,
    truncated: bool,
) -> ArtifactReport {
    let mut artifact = ArtifactReport::new(
        "source_risk_surface",
        if findings.is_empty() { 0.35 } else { 0.65 },
    );

    let mut families: BTreeSet<&str> = BTreeSet::new();
    let mut finding_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut file_counts: BTreeMap<String, usize> = BTreeMap::new();

    for finding in findings {
        families.insert(finding.family);
        *finding_counts.entry(finding.family).or_default() += 1;
        *file_counts
            .entry(finding.path.to_string_lossy().to_string())
            .or_default() += 1;
    }

    let mut signals: Vec<String> = findings
        .iter()
        .map(|finding| finding.signal.clone())
        .collect();
    signals.sort();
    signals.dedup();
    artifact.signals = signals;

    let mut ranked_files: Vec<_> = file_counts.into_iter().collect();
    ranked_files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let top_risky_files: Vec<String> = ranked_files
        .into_iter()
        .take(5)
        .map(|(path, _)| path)
        .collect();

    artifact
        .metadata
        .insert("paths".into(), json!([root.to_string_lossy().to_string()]));
    artifact.metadata.insert(
        "matched_families".into(),
        json!(families.into_iter().collect::<Vec<_>>()),
    );
    artifact
        .metadata
        .insert("finding_counts".into(), json!(finding_counts));
    artifact
        .metadata
        .insert("top_risky_files".into(), json!(top_risky_files));
    artifact.metadata.insert(
        "scanned_source_file_count".into(),
        json!(scanned_source_count),
    );
    artifact
        .metadata
        .insert("scanned_json_file_count".into(), json!(scanned_json_count));
    artifact
        .metadata
        .insert("ai_adjacent_context".into(), json!(ai_adjacent));
    artifact
        .metadata
        .insert("bounded_scan_limit".into(), json!(MAX_SOURCE_SURFACE_FILES));
    artifact
        .metadata
        .insert("truncated".into(), json!(truncated));
    artifact.compute_hash();
    artifact
}

pub(crate) fn common_root(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut components: Vec<_> = paths.first()?.components().collect();

    for path in paths.iter().skip(1) {
        let current: Vec<_> = path.components().collect();
        let shared_len = components
            .iter()
            .zip(current.iter())
            .take_while(|(left, right)| left == right)
            .count();
        components.truncate(shared_len);
        if components.is_empty() {
            break;
        }
    }

    if components.is_empty() {
        return None;
    }

    let mut root = PathBuf::new();
    for component in components {
        root.push(component.as_os_str());
    }
    Some(root)
}

pub(crate) fn is_ai_adjacent_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    if CONTENT_READ_ALLOWLIST.contains(&name) {
        return true;
    }

    CONTENT_READ_GLOB_PATTERNS.iter().any(|pattern| {
        Pattern::new(pattern)
            .map(|compiled| compiled.matches(name))
            .unwrap_or(false)
    })
}

pub(crate) fn scan_json_config_file(path: &Path) -> Vec<SourceFinding> {
    if !is_scannable_json_file(path) {
        return Vec::new();
    }

    if fs::metadata(path)
        .map(|metadata| metadata.len() > MAX_JSON_CONFIG_BYTES as u64)
        .unwrap_or(false)
    {
        return Vec::new();
    }

    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    scan_json_config_content(path, &content)
}

pub(crate) fn scan_source_file(path: &Path) -> Vec<SourceFinding> {
    if !is_supported_source_file(path) {
        return Vec::new();
    }

    if fs::metadata(path)
        .map(|metadata| metadata.len() > MAX_SOURCE_ANALYSIS_BYTES as u64)
        .unwrap_or(false)
    {
        return Vec::new();
    }

    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    scan_source_content(path, &content)
}

fn scan_json_config_content(path: &Path, content: &str) -> Vec<SourceFinding> {
    let mut findings = Vec::new();
    let mut seen_signals: BTreeSet<String> = BTreeSet::new();

    for signal in scan_secret_signals(content) {
        if seen_signals.insert(signal.clone()) {
            findings.push(SourceFinding {
                family: "json_secret",
                signal,
                path: path.to_path_buf(),
                line: None,
                summary: "JSON config contains embedded secret material".to_string(),
            });
        }
    }

    for pattern in json_secret_patterns() {
        if let Some(matched) = pattern.regex.find(content) {
            let signal = pattern.signal.to_string();
            if seen_signals.insert(signal.clone()) {
                findings.push(SourceFinding {
                    family: "json_secret",
                    signal,
                    path: path.to_path_buf(),
                    line: Some(line_number_for_offset(content, matched.start())),
                    summary: pattern.summary.to_string(),
                });
            }
        }
    }

    for pattern in json_url_patterns() {
        if let Some(matched) = pattern.regex.find(content) {
            let signal = pattern.signal.to_string();
            if seen_signals.insert(signal.clone()) {
                findings.push(SourceFinding {
                    family: "json_destination",
                    signal,
                    path: path.to_path_buf(),
                    line: Some(line_number_for_offset(content, matched.start())),
                    summary: pattern.summary.to_string(),
                });
            }
        }
    }

    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.signal.cmp(&right.signal))
            .then_with(|| left.line.cmp(&right.line))
    });
    findings
}

fn scan_source_content(path: &Path, content: &str) -> Vec<SourceFinding> {
    let mut findings = Vec::new();
    let mut seen_signals: BTreeSet<String> = BTreeSet::new();
    let lines: Vec<&str> = content.lines().collect();

    for (line_index, line) in lines.iter().enumerate() {
        for pattern in source_context_patterns() {
            if let Some(matched) = pattern.regex.find(line) {
                if !has_nonliteral_call_argument(pattern.signal, line, matched.end()) {
                    continue;
                }
                push_finding(
                    &mut findings,
                    &mut seen_signals,
                    SourceFinding {
                        family: "dynamic_execution",
                        signal: pattern.signal.to_string(),
                        path: path.to_path_buf(),
                        line: Some(line_index + 1),
                        summary: pattern.summary.to_string(),
                    },
                );
            }
        }

        if network_call_pattern().is_match(line)
            && (private_ip_pattern().is_match(line) || link_local_ip_pattern().is_match(line))
        {
            push_finding(
                &mut findings,
                &mut seen_signals,
                SourceFinding {
                    family: "network_context",
                    signal: "source:ssrf_private_ip".to_string(),
                    path: path.to_path_buf(),
                    line: Some(line_index + 1),
                    summary: "Network call targets a private or link-local address".to_string(),
                },
            );
        }

        if internal_hostname_context_pattern().is_match(line) {
            push_finding(
                &mut findings,
                &mut seen_signals,
                SourceFinding {
                    family: "network_context",
                    signal: "source:ssrf_internal_host".to_string(),
                    path: path.to_path_buf(),
                    line: Some(line_index + 1),
                    summary: "Network call targets an internal-only hostname".to_string(),
                },
            );
        }

        for pattern in sensitive_path_patterns() {
            if pattern.regex.is_match(line) {
                push_finding(
                    &mut findings,
                    &mut seen_signals,
                    SourceFinding {
                        family: "sensitive_path",
                        signal: pattern.signal.to_string(),
                        path: path.to_path_buf(),
                        line: Some(line_index + 1),
                        summary: pattern.summary.to_string(),
                    },
                );
            }
        }
    }

    scan_cognitive_file_findings(path, content, &lines, &mut findings, &mut seen_signals);

    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.signal.cmp(&right.signal))
            .then_with(|| left.line.cmp(&right.line))
    });
    findings
}

fn scan_cognitive_file_findings(
    path: &Path,
    content: &str,
    lines: &[&str],
    findings: &mut Vec<SourceFinding>,
    seen_signals: &mut BTreeSet<String>,
) {
    let lowered_content = content.to_ascii_lowercase();
    let matched_file = cognitive_file_names().iter().find_map(|name| {
        lowered_content
            .contains(name)
            .then(|| (*name, find_line_for_substring(lines, name)))
    });

    if let Some((name, line)) = matched_file {
        if write_function_pattern().is_match(content) {
            push_finding(
                findings,
                seen_signals,
                SourceFinding {
                    family: "cognitive_file",
                    signal: "cognitive_tampering:file_write".to_string(),
                    path: path.to_path_buf(),
                    line,
                    summary: format!("Source references and may write agent identity file {name}"),
                },
            );
        }
    }

    for (index, line) in lines.iter().enumerate() {
        let lowered_line = line.to_ascii_lowercase();
        if !cognitive_target_function_pattern().is_match(line) {
            continue;
        }

        if let Some(name) = cognitive_file_names()
            .iter()
            .find(|candidate| lowered_line.contains(**candidate))
        {
            push_finding(
                findings,
                seen_signals,
                SourceFinding {
                    family: "cognitive_file",
                    signal: "cognitive_tampering:file_target".to_string(),
                    path: path.to_path_buf(),
                    line: Some(index + 1),
                    summary: format!("Source targets agent identity file {name}"),
                },
            );
        }
    }
}

fn find_line_for_substring(lines: &[&str], needle: &str) -> Option<usize> {
    lines.iter().enumerate().find_map(|(index, line)| {
        line.to_ascii_lowercase()
            .contains(needle)
            .then_some(index + 1)
    })
}

fn push_finding(
    findings: &mut Vec<SourceFinding>,
    seen_signals: &mut BTreeSet<String>,
    finding: SourceFinding,
) {
    if seen_signals.insert(finding.signal.clone()) {
        findings.push(finding);
    }
}

fn has_nonliteral_call_argument(signal: &str, line: &str, call_end: usize) -> bool {
    let trimmed = line[call_end..].trim_start();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };

    match signal {
        "source:nonliteral_spawn" => !matches!(first, '\'' | '"' | '['),
        _ => !matches!(first, '\'' | '"'),
    }
}

fn line_number_for_offset(content: &str, offset: usize) -> usize {
    content[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn candidate(path: &str) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            origin: "workdir".to_string(),
        }
    }

    #[test]
    fn supported_source_file_matches_expected_extensions() {
        assert!(is_supported_source_file(Path::new("src/main.ts")));
        assert!(is_supported_source_file(Path::new("src/lib.rs")));
        assert!(!is_supported_source_file(Path::new("README.md")));
    }

    #[test]
    fn supported_json_file_matches_json_only() {
        assert!(is_supported_json_file(Path::new("config/app.json")));
        assert!(!is_supported_json_file(Path::new("config/app.yaml")));
    }

    #[test]
    fn scannable_json_file_skips_noisy_metadata() {
        assert!(!is_scannable_json_file(Path::new("package.json")));
        assert!(!is_scannable_json_file(Path::new("package-lock.json")));
        assert!(is_scannable_json_file(Path::new("config/app.json")));
    }

    #[test]
    fn ai_adjacent_candidate_detected_from_known_prompt_files() {
        let candidates = vec![
            candidate("/project/AGENTS.md"),
            candidate("/project/src/main.ts"),
        ];
        assert!(has_ai_adjacent_candidate(&candidates));
    }

    #[test]
    fn common_root_returns_shared_parent() {
        let paths = vec![
            PathBuf::from("/project/src/main.ts"),
            PathBuf::from("/project/src/lib/util.ts"),
        ];
        assert_eq!(common_root(&paths), Some(PathBuf::from("/project/src")));
    }

    #[test]
    fn build_source_risk_surface_aggregates_findings() {
        let findings = vec![
            SourceFinding {
                family: "dynamic_execution",
                signal: "source:nonliteral_spawn".to_string(),
                path: PathBuf::from("/project/src/main.ts"),
                line: Some(12),
                summary: "spawn with non-literal command".to_string(),
            },
            SourceFinding {
                family: "dynamic_execution",
                signal: "source:nonliteral_spawn".to_string(),
                path: PathBuf::from("/project/src/main.ts"),
                line: Some(19),
                summary: "spawn with non-literal command".to_string(),
            },
            SourceFinding {
                family: "network_context",
                signal: "source:ssrf_internal_host".to_string(),
                path: PathBuf::from("/project/src/http.ts"),
                line: Some(7),
                summary: "internal hostname in request context".to_string(),
            },
        ];

        let artifact =
            build_source_risk_surface(Path::new("/project"), 2, 1, &findings, true, false);

        assert_eq!(artifact.artifact_type, "source_risk_surface");
        assert_eq!(artifact.metadata["scanned_source_file_count"], 2);
        assert_eq!(artifact.metadata["scanned_json_file_count"], 1);
        assert_eq!(artifact.metadata["ai_adjacent_context"], true);
        assert_eq!(artifact.metadata["truncated"], false);
        assert_eq!(
            artifact.metadata["matched_families"],
            json!(["dynamic_execution", "network_context"])
        );
        assert_eq!(artifact.signals.len(), 2);
        assert_eq!(
            artifact.metadata["top_risky_files"][0],
            "/project/src/main.ts"
        );
    }

    #[test]
    fn build_source_risk_surface_sorts_top_risky_files_by_count_then_path() {
        let findings = vec![
            SourceFinding {
                family: "dynamic_execution",
                signal: "source:nonliteral_spawn".to_string(),
                path: PathBuf::from("/project/src/b.ts"),
                line: Some(3),
                summary: "spawn with non-literal command".to_string(),
            },
            SourceFinding {
                family: "dynamic_execution",
                signal: "source:nonliteral_spawn".to_string(),
                path: PathBuf::from("/project/src/a.ts"),
                line: Some(7),
                summary: "spawn with non-literal command".to_string(),
            },
            SourceFinding {
                family: "dynamic_execution",
                signal: "source:nonliteral_spawn".to_string(),
                path: PathBuf::from("/project/src/a.ts"),
                line: Some(9),
                summary: "spawn with non-literal command".to_string(),
            },
            SourceFinding {
                family: "network_context",
                signal: "source:ssrf_internal_host".to_string(),
                path: PathBuf::from("/project/src/b.ts"),
                line: Some(11),
                summary: "internal hostname in request context".to_string(),
            },
        ];

        let artifact =
            build_source_risk_surface(Path::new("/project"), 2, 0, &findings, false, false);

        assert_eq!(
            artifact.metadata["top_risky_files"],
            json!(["/project/src/a.ts", "/project/src/b.ts"])
        );
    }

    #[test]
    fn scan_json_config_file_reuses_secret_engine_and_json_specific_secret_patterns() {
        let temp = tempdir().unwrap();
        let config_path = temp.path().join("agent-config.json");
        fs::write(
            &config_path,
            r#"{
  "github_token": "ghp_123456789012345678901234567890123456",
  "password": "supersecret12345",
  "database_url": "postgres://alice:swordfish@example.com/app"
}"#,
        )
        .unwrap();

        let findings = scan_json_config_file(&config_path);

        assert!(findings
            .iter()
            .any(|finding| finding.signal == "secret:github:pat"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "json_config:credential_value"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "json_config:credential_connection_string"));
    }

    #[test]
    fn scan_json_config_file_detects_suspicious_urls() {
        let temp = tempdir().unwrap();
        let config_path = temp.path().join("destinations.json");
        fs::write(
            &config_path,
            r#"{
  "metadata": "http://169.254.169.254/latest/meta-data/",
  "relay": "https://service.internal.example/collect",
  "collector": "https://webhook.site/abc123"
}"#,
        )
        .unwrap();

        let findings = scan_json_config_file(&config_path);

        assert!(findings
            .iter()
            .any(|finding| finding.signal == "json_config:metadata_url"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "json_config:internal_url"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "json_config:c2_url"));
    }

    #[test]
    fn scan_json_config_file_skips_package_json() {
        let temp = tempdir().unwrap();
        let config_path = temp.path().join("package.json");
        fs::write(
            &config_path,
            r#"{
  "collector": "https://webhook.site/abc123"
}"#,
        )
        .unwrap();

        assert!(scan_json_config_file(&config_path).is_empty());
    }

    #[test]
    fn scan_source_file_detects_contextual_dynamic_execution_and_ssrf() {
        let temp = tempdir().unwrap();
        let source_path = temp.path().join("main.ts");
        fs::write(
            &source_path,
            r#"
const loader = pluginName;
await import(loader);
require(runtimeModule);
spawn(commandName, args);
fetch("http://10.0.0.7/token");
request("http://service.internal.example/collect");
"#,
        )
        .unwrap();

        let findings = scan_source_file(&source_path);

        assert!(findings
            .iter()
            .any(|finding| finding.signal == "source:dynamic_import"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "source:nonliteral_require"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "source:nonliteral_spawn"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "source:ssrf_private_ip"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "source:ssrf_internal_host"));
    }

    #[test]
    fn scan_source_file_ignores_literal_imports_and_non_network_internal_strings() {
        let temp = tempdir().unwrap();
        let source_path = temp.path().join("safe.ts");
        fs::write(
            &source_path,
            r#"
await import("./safe-module");
require("./config");
spawn("ls", ["-la"]);
const label = "internal process notes";
const ip = "10.0.0.7";
"#,
        )
        .unwrap();

        let findings = scan_source_file(&source_path);
        assert!(findings.is_empty());
    }

    #[test]
    fn scan_source_file_detects_sensitive_path_access() {
        let temp = tempdir().unwrap();
        let source_path = temp.path().join("steal.py");
        fs::write(
            &source_path,
            r#"
open("/proc/self/environ").read()
readFileSync("~/.aws/credentials")
"#,
        )
        .unwrap();

        let findings = scan_source_file(&source_path);
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "source:sensitive_path_access"));
    }

    #[test]
    fn scan_source_file_detects_cognitive_file_target_and_write() {
        let temp = tempdir().unwrap();
        let source_path = temp.path().join("agent.ts");
        fs::write(
            &source_path,
            r#"
const promptFile = "AGENTS.md";
readFileSync(".cursorrules", "utf8");
await writeFile(promptFile, "new behavior");
"#,
        )
        .unwrap();

        let findings = scan_source_file(&source_path);
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "cognitive_tampering:file_target"));
        assert!(findings
            .iter()
            .any(|finding| finding.signal == "cognitive_tampering:file_write"));
    }
}
