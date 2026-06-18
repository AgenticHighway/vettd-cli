//! Prompt building for the scanner data contract.

use crate::capabilities::derive_capabilities;
use crate::models::ArtifactReport;

use super::helpers::{
    capability_level, compute_file_hash, first_path, humanize_capability, make_id, qualified_name,
    read_artifact_head,
};
use super::types::{InjectionSurface, Prompt, PromptCapability, SecretRef};

pub fn build_prompts(artifacts: &[&ArtifactReport]) -> Vec<Prompt> {
    artifacts.iter().map(|a| artifact_to_prompt(a)).collect()
}

fn artifact_to_prompt(a: &ArtifactReport) -> Prompt {
    let source_path = first_path(a).to_string();
    let name = qualified_name(&source_path);
    let id = make_id(&source_path, &a.artifact_hash);

    let classification = match a.artifact_type.as_str() {
        "cursor_rules" | "agents_md" => "System Prompt",
        _ => "User Prompt",
    };

    let tokens = resolve_tokens(a, &source_path);
    let content_hash = resolve_content_hash(a, &source_path);
    let last_changed_date = resolve_last_changed(a, &source_path);

    let capabilities = derive_capabilities(a)
        .into_iter()
        .map(|cap| {
            let level = capability_level(&cap);
            PromptCapability {
                text: humanize_capability(&cap),
                level: level.to_string(),
            }
        })
        .collect();

    let dependencies = a
        .metadata
        .get("dependencies")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Prompt {
        id,
        name,
        source_file_path: source_path,
        classification: classification.to_string(),
        tokens,
        content_hash,
        last_changed_date,
        capabilities,
        secret_refs: build_secret_refs(a),
        injection_surfaces: build_injection_surfaces(a),
        dependencies,
        risk_score: a.risk_score.clamp(0, 100),
    }
}

fn resolve_tokens(a: &ArtifactReport, source_path: &str) -> u64 {
    a.metadata
        .get("file_size_bytes")
        .and_then(|v| v.as_u64())
        .map(|size| size / 4)
        .unwrap_or_else(|| {
            std::fs::metadata(source_path)
                .ok()
                .map(|m| m.len() / 4)
                .unwrap_or(0)
        })
}

fn resolve_content_hash(a: &ArtifactReport, source_path: &str) -> String {
    a.metadata
        .get("content_hash")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| compute_file_hash(source_path))
}

fn resolve_last_changed(a: &ArtifactReport, source_path: &str) -> String {
    a.metadata
        .get("last_modified")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| {
            std::fs::metadata(source_path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.format("%Y-%m-%d").to_string()
                })
                .unwrap_or_else(|| "1970-01-01".to_string())
        })
}

fn build_secret_refs(a: &ArtifactReport) -> Vec<SecretRef> {
    let mut refs = Vec::new();
    for signal in &a.signals {
        if signal == "credential_exposure_signal" {
            refs.push(SecretRef {
                label: "Credential reference detected".to_string(),
                detail: "Redacted — matched known secret pattern".to_string(),
                tone: "danger".to_string(),
            });
        }
    }

    if let Some(content) = read_artifact_head(a) {
        for pattern in &["$", "process.env.", "os.environ"] {
            if content.contains(pattern) {
                let already_dangerous = refs.iter().any(|r| r.tone == "danger");
                if !already_dangerous {
                    refs.push(SecretRef {
                        label: "Env var reference (safe)".to_string(),
                        detail: format!("References environment variable via {pattern}"),
                        tone: "safe".to_string(),
                    });
                    break;
                }
            }
        }
    }
    refs
}

fn build_injection_surfaces(a: &ArtifactReport) -> Vec<InjectionSurface> {
    let mut surfaces = Vec::new();
    for signal in &a.signals {
        if signal.starts_with("dangerous_keyword:") {
            let keyword = signal.strip_prefix("dangerous_keyword:").unwrap_or(signal);
            surfaces.push(InjectionSurface {
                text: format!("Dangerous instruction keyword: {keyword}"),
                severity: "high".to_string(),
            });
        }
        if signal == "dangerous_combo:shell+network+fs" {
            surfaces.push(InjectionSurface {
                text: "Combined shell + network + filesystem access pattern".to_string(),
                severity: "high".to_string(),
            });
        }
    }

    if let Some(content) = read_artifact_head(a) {
        let lowered = content.to_lowercase();
        if lowered.contains("{{") || lowered.contains("{%") || lowered.contains("${") {
            surfaces.push(InjectionSurface {
                text: "Template interpolation detected — potential injection surface".to_string(),
                severity: "medium".to_string(),
            });
        }
        if lowered.contains("user_input") || lowered.contains("user_message") {
            surfaces.push(InjectionSurface {
                text: "Direct user input reference in prompt body".to_string(),
                severity: "medium".to_string(),
            });
        }
    }
    surfaces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ArtifactReport;

    #[test]
    fn secret_refs_credential_signal() {
        let mut a = ArtifactReport::new("prompt_config", 0.8);
        a.signals = vec!["credential_exposure_signal".to_string()];
        let refs = build_secret_refs(&a);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].tone, "danger");
        assert!(refs[0].label.contains("Credential"));
    }

    #[test]
    fn secret_refs_empty_for_no_signals() {
        let a = ArtifactReport::new("prompt_config", 0.8);
        let refs = build_secret_refs(&a);
        assert!(refs.is_empty());
    }

    #[test]
    fn injection_surfaces_dangerous_keyword() {
        let mut a = ArtifactReport::new("prompt_config", 0.8);
        a.signals = vec!["dangerous_keyword:exfiltrate".to_string()];
        let surfaces = build_injection_surfaces(&a);
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].severity, "high");
        assert!(surfaces[0].text.contains("exfiltrate"));
    }

    #[test]
    fn injection_surfaces_dangerous_combo() {
        let mut a = ArtifactReport::new("prompt_config", 0.8);
        a.signals = vec!["dangerous_combo:shell+network+fs".to_string()];
        let surfaces = build_injection_surfaces(&a);
        assert_eq!(surfaces.len(), 1);
        assert!(surfaces[0].text.contains("shell + network + filesystem"));
    }

    #[test]
    fn injection_surfaces_multiple_signals() {
        let mut a = ArtifactReport::new("prompt_config", 0.8);
        a.signals = vec![
            "dangerous_keyword:rm".to_string(),
            "dangerous_keyword:steal".to_string(),
            "dangerous_combo:shell+network+fs".to_string(),
        ];
        let surfaces = build_injection_surfaces(&a);
        assert_eq!(surfaces.len(), 3);
    }

    #[test]
    fn injection_surfaces_empty_for_safe() {
        let mut a = ArtifactReport::new("prompt_config", 0.8);
        a.signals = vec!["keyword:shell".to_string()];
        let surfaces = build_injection_surfaces(&a);
        assert!(surfaces.is_empty());
    }

    #[test]
    fn resolve_tokens_from_metadata() {
        let mut a = ArtifactReport::new("prompt_config", 0.8);
        a.metadata
            .insert("file_size_bytes".to_string(), serde_json::json!(4000));
        assert_eq!(resolve_tokens(&a, "/nonexistent"), 1000);
    }

    #[test]
    fn resolve_tokens_zero_when_no_metadata_and_missing_file() {
        let a = ArtifactReport::new("prompt_config", 0.8);
        assert_eq!(resolve_tokens(&a, "/definitely/not/a/real/file"), 0);
    }

    #[test]
    fn prompt_classification_system_for_cursor_rules() {
        let mut a = ArtifactReport::new("cursor_rules", 0.9);
        a.metadata.insert(
            "paths".to_string(),
            serde_json::json!(["/tmp/.cursorrules"]),
        );
        let prompt = artifact_to_prompt(&a);
        assert_eq!(prompt.classification, "System Prompt");
    }

    #[test]
    fn prompt_classification_user_for_other() {
        let mut a = ArtifactReport::new("prompt_config", 0.8);
        a.metadata
            .insert("paths".to_string(), serde_json::json!(["/tmp/config.md"]));
        let prompt = artifact_to_prompt(&a);
        assert_eq!(prompt.classification, "User Prompt");
    }

    #[test]
    fn prompt_risk_score_clamped() {
        let mut a = ArtifactReport::new("prompt_config", 0.8);
        a.risk_score = 150;
        a.metadata
            .insert("paths".to_string(), serde_json::json!(["/tmp/test.md"]));
        let prompt = artifact_to_prompt(&a);
        assert_eq!(prompt.risk_score, 100);
    }
}
