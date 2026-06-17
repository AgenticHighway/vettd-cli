use crate::models::ArtifactReport;
use crate::scoring::{
    common_cognitive_signal_weight, secret_signal_weight, shared_signal_weight, ssrf_signal_weight,
};
use std::collections::HashMap;

const DECLARED_DISCOUNT: f64 = 0.5;

fn capability_base_risk() -> HashMap<&'static str, i32> {
    HashMap::from([
        ("keyword:shell", 15),
        ("keyword:browser", 10),
        ("keyword:api", 10),
        ("keyword:permissions", 8),
        ("keyword:system", 5),
        ("keyword:tools", 5),
        ("keyword:instructions", 3),
        ("keyword:execute", 12),
        ("keyword:network", 10),
        ("keyword:filesystem", 5),
        ("keyword:docker", 5),
        ("keyword:secrets", 8),
        ("keyword:dependencies", 3),
    ])
}

fn keyword_to_declared() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("keyword:shell", "shell"),
        ("keyword:browser", "browser"),
        ("keyword:api", "api"),
        ("keyword:filesystem", "filesystem"),
        ("keyword:docker", "docker"),
        ("keyword:execute", "python"),
    ])
}

fn risk_engine_signal_weight(signal: &str) -> Option<i32> {
    if let Some(weight) = shared_signal_weight(signal) {
        return Some(weight);
    }

    match signal {
        "json_config:credential_connection_string" => Some(20),
        "json_config:credential_value" => Some(15),
        "json_config:metadata_url" => Some(25),
        "json_config:internal_url" => Some(15),
        "json_config:c2_url" => Some(35),
        "source:dynamic_import" | "source:nonliteral_require" => Some(20),
        "source:nonliteral_spawn" => Some(30),
        "source:ssrf_private_ip" => Some(25),
        "source:ssrf_internal_host" => Some(20),
        "source:sensitive_path_access" => Some(25),
        "mcp_server_declared" => Some(20),
        "extensions_directory_present" => Some(5),
        "dangerous_keyword:rm" | "dangerous_keyword:disable" => Some(25),
        "dangerous_keyword:upload" => Some(20),
        _ => None,
    }
}

fn cognitive_signal_weight(signal: &str) -> Option<i32> {
    common_cognitive_signal_weight(signal).or(match signal {
        "cognitive_tampering:file_write" => Some(35),
        "cognitive_tampering:file_target" => Some(25),
        _ => None,
    })
}

fn type_base_score() -> HashMap<&'static str, i32> {
    HashMap::from([
        ("cursor_rules", 10),
        ("agents_md", 8),
        ("skill", 8),
        ("source_risk_surface", 4),
        ("container_config", 12),
        ("container_candidate", 3),
        ("browser_footprint", 5),
        ("mcp_config", 20),
    ])
}

fn declared_tools_from_metadata(artifact: &ArtifactReport) -> Vec<String> {
    artifact
        .metadata
        .get("declared_tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn is_declared(signal: &str, declared_tools: &[String], ktd: &HashMap<&str, &str>) -> bool {
    ktd.get(signal)
        .map(|tool_name| declared_tools.iter().any(|t| t == tool_name))
        .unwrap_or(false)
}

fn parse_count_signal(signal: &str, prefix: &str) -> Option<i32> {
    signal
        .strip_prefix(prefix)
        .and_then(|n| n.parse::<i32>().ok())
}

pub fn score_artifact(artifact: &mut ArtifactReport) -> i32 {
    let cap_risk = capability_base_risk();
    let ktd = keyword_to_declared();
    let type_base = type_base_score();

    let base = type_base
        .get(artifact.artifact_type.as_str())
        .copied()
        .unwrap_or(5);

    let declared_tools = declared_tools_from_metadata(artifact);
    let has_structured_secret = artifact.signals.iter().any(|s| s.starts_with("secret:"));
    let mut score = base;
    let mut contributions: Vec<(i32, String)> = Vec::new();

    for signal in &artifact.signals {
        let sig = signal.as_str();

        if sig == "credential_exposure_signal" && has_structured_secret {
            continue;
        }

        if let Some(&weight) = cap_risk.get(sig) {
            let effective = if is_declared(sig, &declared_tools, &ktd) {
                (weight as f64 * DECLARED_DISCOUNT) as i32
            } else {
                weight
            };
            score += effective;
            contributions.push((effective, signal.clone()));
        } else if let Some(weight) = secret_signal_weight(sig) {
            score += weight;
            contributions.push((weight, signal.clone()));
        } else if let Some(weight) = ssrf_signal_weight(sig) {
            score += weight;
            contributions.push((weight, signal.clone()));
        } else if let Some(weight) = cognitive_signal_weight(sig) {
            score += weight;
            contributions.push((weight, signal.clone()));
        } else if let Some(weight) = risk_engine_signal_weight(sig) {
            score += weight;
            contributions.push((weight, signal.clone()));
        } else if let Some(n) = parse_count_signal(sig, "extension_count:") {
            let w = n.min(10);
            score += w;
            contributions.push((w, signal.clone()));
        } else if let Some(n) = parse_count_signal(sig, "mcp_server_count:") {
            let w = (n * 5).min(20);
            score += w;
            contributions.push((w, signal.clone()));
        }
    }

    score = score.min(100);

    contributions.sort_by(|a, b| b.0.cmp(&a.0));
    artifact.risk_reasons = contributions
        .iter()
        .take(2)
        .map(|(w, s)| format!("{s} (+{w})"))
        .collect();

    artifact.risk_score = score;
    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_artifact(artifact_type: &str, signals: Vec<&str>) -> ArtifactReport {
        let mut a = ArtifactReport::new(artifact_type, 1.0);
        a.signals = signals.into_iter().map(String::from).collect();
        a
    }

    #[test]
    fn test_base_score_only() {
        let mut a = make_artifact("mcp_config", vec![]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 20);
    }

    #[test]
    fn test_capability_risk_no_discount() {
        let mut a = make_artifact("cursor_rules", vec!["keyword:shell"]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 25); // 10 + 15
    }

    #[test]
    fn test_capability_risk_with_discount() {
        let mut a = make_artifact("cursor_rules", vec!["keyword:shell"]);
        a.metadata
            .insert("declared_tools".to_string(), json!(["shell"]));
        let score = score_artifact(&mut a);
        assert_eq!(score, 17); // 10 + 7 (15*0.5 truncated)
    }

    #[test]
    fn test_signal_weight() {
        let mut a = make_artifact("agents_md", vec!["dangerous_keyword:steal"]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 43); // 8 + 35
    }

    #[test]
    fn test_json_config_c2_signal_weight() {
        let mut a = make_artifact("source_risk_surface", vec!["json_config:c2_url"]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 39); // 4 + 35
    }

    #[test]
    fn test_source_dynamic_spawn_weight() {
        let mut a = make_artifact("source_risk_surface", vec!["source:nonliteral_spawn"]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 34); // 4 + 30
    }

    #[test]
    fn test_cognitive_file_write_weight() {
        let mut a = make_artifact(
            "source_risk_surface",
            vec!["cognitive_tampering:file_write"],
        );
        let score = score_artifact(&mut a);
        assert_eq!(score, 39); // 4 + 35
    }

    #[test]
    fn test_source_risk_surface_base_score() {
        let mut a = make_artifact("source_risk_surface", vec![]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 4);
    }

    #[test]
    fn test_structured_secret_signal_weight_replaces_generic_credential_weight() {
        let mut a = make_artifact(
            "agents_md",
            vec!["credential_exposure_signal", "secret:github:pat"],
        );
        let score = score_artifact(&mut a);
        assert_eq!(score, 33); // 8 + 25, without double counting the generic signal
        assert!(a.risk_reasons[0].contains("secret:github:pat"));
    }

    #[test]
    fn test_ssrf_metadata_signal_weight() {
        let mut a = make_artifact("prompt_config", vec!["ssrf:metadata:aws"]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 50); // unknown type base 5 + 45
    }

    #[test]
    fn test_cognitive_tampering_signal_weight() {
        let mut a = make_artifact("prompt_config", vec!["cognitive_tampering:role_override"]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 50); // unknown type base 5 + 45
    }

    #[test]
    fn test_extension_count() {
        let mut a = make_artifact("agents_md", vec!["extension_count:15"]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 18); // 8 + min(15,10)
    }

    #[test]
    fn test_mcp_server_count() {
        let mut a = make_artifact("agents_md", vec!["mcp_server_count:6"]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 28); // 8 + min(30,20)
    }

    #[test]
    fn test_cap_at_100() {
        let mut a = make_artifact(
            "mcp_config",
            vec![
                "dangerous_keyword:steal",
                "dangerous_keyword:exfiltrate",
                "dangerous_combo:shell+network+fs",
                "keyword:shell",
            ],
        );
        let score = score_artifact(&mut a);
        assert_eq!(score, 100);
    }

    #[test]
    fn test_risk_reasons_top_2() {
        let mut a = make_artifact(
            "agents_md",
            vec![
                "keyword:instructions",
                "keyword:shell",
                "dangerous_keyword:steal",
            ],
        );
        score_artifact(&mut a);
        assert_eq!(a.risk_reasons.len(), 2);
        assert!(a.risk_reasons[0].contains("steal"));
        assert!(a.risk_reasons[1].contains("shell"));
    }

    #[test]
    fn test_unknown_type_default_base() {
        let mut a = make_artifact("unknown_type", vec![]);
        let score = score_artifact(&mut a);
        assert_eq!(score, 5);
    }
}
