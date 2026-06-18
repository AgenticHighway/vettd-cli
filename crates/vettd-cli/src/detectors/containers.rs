use std::collections::HashSet;
use std::path::PathBuf;

use crate::discovery::Candidate;
use crate::models::{
    check_for_dangerous_patterns, check_for_secrets, gather_file_primitives,
    is_content_read_allowed, ArtifactReport,
};

use super::base::Detector;
use super::read_utf8_head;
use serde_json::json;

const MAX_READ_BYTES: usize = 8192;

const CONTAINER_FILENAMES: &[&str] = &[
    "Dockerfile",
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

const AI_RELEVANCE_TOKENS: &[&str] = &[
    "langchain",
    "langgraph",
    "autogen",
    "crewai",
    "autogpt",
    "opendevin",
    "swe-agent",
    "aider",
    "cursor",
    "copilot",
    "openai",
    "anthropic",
    "ollama",
    "huggingface",
    "replicate",
    "together.ai",
    "groq",
    "mistral",
    "llm",
    "model",
    "embedding",
    "vector",
    "rag",
    "agent",
    "ai-tool",
    "mcp",
];

const DIRECT_AGENTIC_TOKENS: &[&str] = &[
    "langchain",
    "langgraph",
    "autogen",
    "crewai",
    "autogpt",
    "opendevin",
    "swe-agent",
    "aider",
];

struct ContentScan {
    signals: Vec<String>,
    has_ai_content: bool,
    has_agentic_content: bool,
}

pub struct ContainerDetector;

impl Detector for ContainerDetector {
    fn name(&self) -> &str {
        "containers"
    }

    fn detect(&self, candidates: &[Candidate], _deep: bool) -> Vec<ArtifactReport> {
        let (ai_dirs, container_candidates) = build_ai_dir_set_and_container_candidates(candidates);
        let mut results = Vec::new();

        for candidate in container_candidates {
            if let Some(report) = classify_candidate(candidate, &ai_dirs) {
                results.push(report);
            }
        }
        results
    }
}

fn build_ai_dir_set_and_container_candidates(
    candidates: &[Candidate],
) -> (HashSet<PathBuf>, Vec<&Candidate>) {
    let mut dirs = HashSet::new();
    let mut containers = Vec::new();
    for c in candidates {
        let name = match c.path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let is_ai_file = name == ".cursorrules"
            || name == "agents.md"
            || name == "AGENTS.md"
            || name == "mcp.json"
            || name == "mcp_config.json";
        if is_ai_file {
            if let Some(parent) = c.path.parent() {
                dirs.insert(parent.to_path_buf());
            }
        }

        if CONTAINER_FILENAMES.contains(&name) {
            containers.push(c);
        }
    }
    (dirs, containers)
}

fn classify_candidate(candidate: &Candidate, ai_dirs: &HashSet<PathBuf>) -> Option<ArtifactReport> {
    let name = candidate.path.file_name()?.to_str()?;
    if !CONTAINER_FILENAMES.contains(&name) {
        return None;
    }

    let mut signals = Vec::new();
    let mut has_ai_proximity = false;
    let mut has_ai_content = false;
    let mut has_agentic_content = false;
    let mut metadata = serde_json::Map::new();

    // File primitives — gather once
    let file_prims = gather_file_primitives(&candidate.path);
    metadata.extend(file_prims);
    metadata.insert("container_kind".into(), json!(container_kind(name)));

    // Proximity check: container file lives alongside AI artifacts
    if let Some(parent) = candidate.path.parent() {
        if ai_dirs.contains(parent) {
            signals.push("ai_artifact_proximity".to_string());
            has_ai_proximity = true;
        }
    }

    // Content scan for AI relevance tokens + container-specific primitives
    if is_content_read_allowed(&candidate.path) {
        if let Some(content) = read_utf8_head(&candidate.path, MAX_READ_BYTES) {
            let content_scan = scan_content(&content);
            signals.extend(content_scan.signals);
            has_ai_content = content_scan.has_ai_content;
            has_agentic_content = content_scan.has_agentic_content;

            extract_container_metadata(name, &content, &mut metadata);
        }
    }

    let (artifact_type, confidence) = if has_ai_content {
        ("container_config", 0.8)
    } else if has_ai_proximity {
        ("container_candidate", 0.55)
    } else {
        ("container_candidate", 0.4)
    };

    metadata.insert("direct_ai_evidence".into(), json!(has_ai_content));
    metadata.insert("direct_agentic_evidence".into(), json!(has_agentic_content));
    metadata.insert("ai_artifact_proximity".into(), json!(has_ai_proximity));
    metadata.insert("paths".into(), json!([candidate.path.to_string_lossy()]));

    let mut report = ArtifactReport::new(artifact_type, confidence);
    report.signals = signals;
    report.metadata = metadata;
    report.artifact_scope = candidate.origin.clone();
    report.compute_hash();
    Some(report)
}

fn scan_content(content: &str) -> ContentScan {
    let mut signals = Vec::new();
    let lowered = content.to_lowercase();

    let found: Vec<String> = AI_RELEVANCE_TOKENS
        .iter()
        .filter(|t| lowered.contains(**t))
        .map(|s| format!("ai_token:{s}"))
        .collect();
    let has_ai_content = !found.is_empty();
    signals.extend(found);
    let has_agentic_content = DIRECT_AGENTIC_TOKENS.iter().any(|t| lowered.contains(t));
    signals.extend(check_for_secrets(content));
    signals.extend(check_for_dangerous_patterns(content));

    ContentScan {
        signals,
        has_ai_content,
        has_agentic_content,
    }
}

fn container_kind(name: &str) -> &'static str {
    if name.contains("compose") {
        "service_orchestration"
    } else {
        "image_definition"
    }
}

fn extract_container_metadata(
    name: &str,
    content: &str,
    metadata: &mut serde_json::Map<String, serde_json::Value>,
) {
    let is_compose = name.contains("compose");
    if is_compose {
        let services = extract_compose_services(content);
        if !services.is_empty() {
            metadata.insert("services".into(), json!(services));
        }
    } else {
        if let Some(base) = extract_base_image(content) {
            metadata.insert("base_image".into(), json!(base));
        }
        let ports = extract_exposed_ports(content);
        if !ports.is_empty() {
            metadata.insert("exposed_ports".into(), json!(ports));
        }
    }
}

/// Extract the base image from the first FROM instruction in a Dockerfile.
fn extract_base_image(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.to_uppercase().starts_with("FROM ") {
            // FROM image:tag AS stage  →  "image:tag"
            let rest = trimmed[5..].trim();
            let image = rest.split_whitespace().next()?;
            return Some(image.to_string());
        }
    }
    None
}

/// Extract EXPOSE port numbers from a Dockerfile.
fn extract_exposed_ports(content: &str) -> Vec<String> {
    let mut ports = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.to_uppercase().starts_with("EXPOSE ") {
            for token in trimmed[7..].split_whitespace() {
                // Strip protocol suffix like 8080/tcp
                let port = token.split('/').next().unwrap_or(token);
                if port.chars().all(|c| c.is_ascii_digit()) {
                    ports.push(port.to_string());
                }
            }
        }
    }
    ports
}

/// Extract top-level service names from a compose file.
///
/// Looks for the `services:` key and collects immediate children
/// using simple indentation-based parsing (avoids a YAML dependency).
fn extract_compose_services(content: &str) -> Vec<String> {
    let mut services = Vec::new();
    let mut in_services = false;
    let mut service_indent: Option<usize> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Top-level key detection (no leading whitespace)
        if !line.starts_with(' ') && !line.starts_with('\t') {
            in_services = trimmed.starts_with("services:");
            if !in_services {
                service_indent = None;
            }
            continue;
        }

        if in_services {
            let leading = line.len() - line.trim_start().len();
            if trimmed.ends_with(':') {
                let expected_indent = service_indent.get_or_insert(leading);
                if leading == *expected_indent {
                    if let Some(name) = trimmed.strip_suffix(':') {
                        let name = name.trim();
                        if !name.is_empty() && !name.contains(' ') {
                            services.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    services
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::Candidate;
    use tempfile::TempDir;

    fn candidate(path: &std::path::Path) -> Candidate {
        Candidate {
            path: path.to_path_buf(),
            origin: "workdir".to_string(),
        }
    }

    fn temp_candidate_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn extract_base_image_simple() {
        let content = "FROM python:3.11-slim\nRUN pip install -r requirements.txt";
        assert_eq!(extract_base_image(content).unwrap(), "python:3.11-slim");
    }

    #[test]
    fn extract_base_image_with_as_stage() {
        let content = "FROM node:20-alpine AS builder\nWORKDIR /app";
        assert_eq!(extract_base_image(content).unwrap(), "node:20-alpine");
    }

    #[test]
    fn extract_base_image_none_without_from() {
        assert!(extract_base_image("RUN echo hello").is_none());
    }

    #[test]
    fn extract_exposed_ports_single() {
        let content = "EXPOSE 8080";
        assert_eq!(extract_exposed_ports(content), vec!["8080"]);
    }

    #[test]
    fn extract_exposed_ports_multiple() {
        let content = "EXPOSE 8080 5432/tcp 3000";
        assert_eq!(extract_exposed_ports(content), vec!["8080", "5432", "3000"]);
    }

    #[test]
    fn extract_exposed_ports_empty() {
        assert!(extract_exposed_ports("RUN echo hello").is_empty());
    }

    #[test]
    fn extract_compose_services_basic() {
        let content = "services:\n  web:\n    image: nginx\n  redis:\n    image: redis:7";
        let services = extract_compose_services(content);
        assert_eq!(services, vec!["web", "redis"]);
    }

    #[test]
    fn extract_compose_services_with_other_keys() {
        let content = "version: '3'\nservices:\n  app:\n    build: .\nnetworks:\n  default:";
        let services = extract_compose_services(content);
        assert_eq!(services, vec!["app"]);
    }

    #[test]
    fn extract_compose_services_empty() {
        assert!(extract_compose_services("version: '3'").is_empty());
    }

    #[test]
    fn scan_content_distinguishes_ai_and_agentic_signals() {
        let scan = scan_content("FROM python:3.11\nRUN pip install openai crewai");
        assert!(scan.has_ai_content);
        assert!(scan.has_agentic_content);
        assert!(scan.signals.iter().any(|s| s == "ai_token:openai"));
        assert!(scan.signals.iter().any(|s| s == "ai_token:crewai"));
    }

    #[test]
    fn classify_candidate_proximity_only_stays_candidate() {
        let dir = temp_candidate_dir();
        let dockerfile = dir.path().join("Dockerfile");
        let agents = dir.path().join("agents.md");
        std::fs::write(&dockerfile, "FROM python:3.11-slim\nRUN echo hello").unwrap();
        std::fs::write(&agents, "# agents").unwrap();

        let candidates = vec![candidate(&dockerfile), candidate(&agents)];
        let (ai_dirs, _) = build_ai_dir_set_and_container_candidates(&candidates);
        let report = classify_candidate(&candidates[0], &ai_dirs).unwrap();

        assert_eq!(report.artifact_type, "container_candidate");
        assert_eq!(report.confidence, 0.55);
        assert_eq!(report.metadata["container_kind"], "image_definition");
        assert_eq!(report.metadata["direct_ai_evidence"], false);
        assert_eq!(report.metadata["direct_agentic_evidence"], false);
        assert_eq!(report.metadata["ai_artifact_proximity"], true);
        assert!(report
            .signals
            .iter()
            .any(|signal| signal == "ai_artifact_proximity"));
    }

    #[test]
    fn classify_candidate_content_evidence_upgrades_to_config() {
        let dir = temp_candidate_dir();
        let dockerfile = dir.path().join("Dockerfile");
        std::fs::write(
            &dockerfile,
            "FROM python:3.11-slim\nRUN pip install openai crewai\nEXPOSE 8080",
        )
        .unwrap();

        let candidates = vec![candidate(&dockerfile)];
        let (ai_dirs, _) = build_ai_dir_set_and_container_candidates(&candidates);
        let report = classify_candidate(&candidates[0], &ai_dirs).unwrap();

        assert_eq!(report.artifact_type, "container_config");
        assert_eq!(report.metadata["container_kind"], "image_definition");
        assert_eq!(report.metadata["direct_ai_evidence"], true);
        assert_eq!(report.metadata["direct_agentic_evidence"], true);
        assert_eq!(report.metadata["base_image"], "python:3.11-slim");
        assert_eq!(report.metadata["exposed_ports"], json!(["8080"]));
    }

    #[test]
    fn classify_candidate_compose_sets_service_orchestration_kind() {
        let dir = temp_candidate_dir();
        let compose = dir.path().join("docker-compose.yml");
        std::fs::write(
            &compose,
            "services:\n  web:\n    image: app\n    environment:\n      OPENAI_API_KEY: test",
        )
        .unwrap();

        let candidates = vec![candidate(&compose)];
        let (ai_dirs, _) = build_ai_dir_set_and_container_candidates(&candidates);
        let report = classify_candidate(&candidates[0], &ai_dirs).unwrap();

        assert_eq!(report.metadata["container_kind"], "service_orchestration");
        assert_eq!(report.metadata["services"], json!(["web"]));
        assert_eq!(report.metadata["direct_ai_evidence"], true);
    }

    #[test]
    fn build_ai_dir_set_and_container_candidates_separates_roles() {
        let candidates = vec![
            Candidate {
                path: PathBuf::from("/project/AGENTS.md"),
                origin: "workdir".to_string(),
            },
            Candidate {
                path: PathBuf::from("/project/Dockerfile"),
                origin: "workdir".to_string(),
            },
            Candidate {
                path: PathBuf::from("/project/src/main.rs"),
                origin: "workdir".to_string(),
            },
        ];

        let (ai_dirs, containers) = build_ai_dir_set_and_container_candidates(&candidates);

        assert!(ai_dirs.contains(&PathBuf::from("/project")));
        assert_eq!(containers.len(), 1);
        assert!(containers[0].path.ends_with("Dockerfile"));
    }
}
