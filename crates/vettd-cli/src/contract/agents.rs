//! Agent building for the scanner data contract.

use crate::capabilities::derive_capabilities;
use crate::models::ArtifactReport;

use super::helpers::{
    declared_tools, detect_source_repo, first_path, is_same_tool_scope, make_id, qualified_name,
    read_artifact_head,
};
use super::types::{Agent, AgentCapability, AgentTool, TrustFactor};

pub fn build_agents(
    agent_artifacts: &[&ArtifactReport],
    mcp_artifacts: &[&ArtifactReport],
) -> Vec<Agent> {
    agent_artifacts
        .iter()
        .map(|a| artifact_to_agent(a, mcp_artifacts))
        .collect()
}

fn artifact_to_agent(a: &ArtifactReport, mcp_artifacts: &[&ArtifactReport]) -> Agent {
    let source_path = first_path(a).to_string();
    let name = qualified_name(&source_path);
    let id = make_id(&source_path, &a.artifact_hash);

    let caps = derive_capabilities(a);
    let classification = infer_classification(&caps, a);
    let execution_model = infer_execution_model(a);
    let trust_score = (100 - a.risk_score).clamp(0, 100);
    let capabilities = build_capability_flags(&caps);
    let mut tools = build_declared_tools(a);

    link_mcp_tools(&source_path, mcp_artifacts, &mut tools);

    let trust_breakdown = build_trust_breakdown(a);
    let source_repo = detect_source_repo(&source_path);

    Agent {
        id,
        name,
        source_file_path: source_path,
        classification,
        execution_model,
        trust_score,
        version: "unknown".to_string(),
        author: "unknown".to_string(),
        source_repo,
        capabilities,
        tools,
        trust_breakdown,
    }
}

fn build_capability_flags(caps: &[String]) -> Vec<AgentCapability> {
    ["Filesystem", "Browser", "Network", "Shell", "Database"]
        .iter()
        .map(|cap_name| {
            let enabled = caps.iter().any(|c| {
                matches!(
                    (cap_name, c.as_str()),
                    (&"Filesystem", "filesystem_access")
                        | (&"Browser", "browser_access")
                        | (&"Network", "network_access" | "external_api_calls")
                        | (&"Shell", "shell_execution" | "code_execution")
                        | (&"Database", "database_access")
                )
            });
            AgentCapability {
                name: cap_name.to_string(),
                enabled,
            }
        })
        .collect()
}

fn build_declared_tools(a: &ArtifactReport) -> Vec<AgentTool> {
    declared_tools(a)
        .into_iter()
        .map(|t| AgentTool {
            name: t,
            tool_type: "skill".to_string(),
        })
        .collect()
}

fn link_mcp_tools(
    source_path: &str,
    mcp_artifacts: &[&ArtifactReport],
    tools: &mut Vec<AgentTool>,
) {
    let agent_dir = std::path::Path::new(source_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut seen_mcp_names = std::collections::HashSet::new();

    for mcp in mcp_artifacts {
        let mcp_path = first_path(mcp);
        let mcp_dir = std::path::Path::new(mcp_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let related = agent_dir.starts_with(&mcp_dir)
            || mcp_dir.starts_with(&agent_dir)
            || is_same_tool_scope(&agent_dir, &mcp_dir);

        if !related {
            continue;
        }

        if let Some(content) = read_artifact_head(mcp) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                let servers = val
                    .get("mcpServers")
                    .or_else(|| val.get("servers"))
                    .and_then(|v| v.as_object());
                if let Some(servers) = servers {
                    for (server_name, _) in servers {
                        if seen_mcp_names.insert(server_name.clone()) {
                            tools.push(AgentTool {
                                name: server_name.clone(),
                                tool_type: "mcp".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn infer_classification(caps: &[String], a: &ArtifactReport) -> String {
    if caps
        .iter()
        .any(|c| c == "code_execution" || c == "shell_execution")
    {
        return "Code".to_string();
    }
    if caps
        .iter()
        .any(|c| c == "container_runtime" || c == "dependency_execution")
    {
        return "Automation".to_string();
    }
    if caps
        .iter()
        .any(|c| c == "browser_access" || c == "external_api_calls")
    {
        return "Research".to_string();
    }
    if a.artifact_type == "agents_md" {
        return "System".to_string();
    }
    "System".to_string()
}

fn infer_execution_model(a: &ArtifactReport) -> String {
    let has_dangerous = a
        .signals
        .iter()
        .any(|s| s.starts_with("dangerous_keyword:") || s == "dangerous_combo:shell+network+fs");

    if has_dangerous {
        "Autonomous".to_string()
    } else {
        "User-in-the-loop".to_string()
    }
}

fn build_trust_breakdown(a: &ArtifactReport) -> Vec<TrustFactor> {
    let mut factors = Vec::new();

    let base = match a.artifact_type.as_str() {
        "agents_md" => 10,
        "cursor_rules" => 5,
        _ => 0,
    };
    if base > 0 {
        factors.push(TrustFactor {
            label: format!("Known artifact type: {}", a.artifact_type.replace('_', " ")),
            delta: base,
        });
    }

    for signal in &a.signals {
        let (delta, label) = match signal.as_str() {
            "credential_exposure_signal" => (
                -25,
                "Hardcoded credential or secret pattern detected".to_string(),
            ),
            "dangerous_combo:shell+network+fs" => (
                -30,
                "Combined shell, network, and filesystem access — high exfiltration risk"
                    .to_string(),
            ),
            s if s.starts_with("dangerous_keyword:") => {
                let kw = s.strip_prefix("dangerous_keyword:").unwrap_or(s);
                (-15, format!("Dangerous keyword detected: '{kw}' command"))
            }
            "execution_tokens_present" => (
                -10,
                "Execution tokens present in MCP configuration".to_string(),
            ),
            "shell_access_detected" => (
                -10,
                "Shell access detected in MCP configuration".to_string(),
            ),
            _ => (0, String::new()),
        };
        if delta != 0 {
            factors.push(TrustFactor { label, delta });
        }
    }

    if a.verification_status == "pass" {
        factors.push(TrustFactor {
            label: "Verification passed".to_string(),
            delta: 15,
        });
    }

    factors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ArtifactReport;

    #[test]
    fn infer_classification_code_for_shell() {
        let caps = vec!["shell_execution".to_string()];
        let a = ArtifactReport::new("agents_md", 0.8);
        assert_eq!(infer_classification(&caps, &a), "Code");
    }

    #[test]
    fn infer_classification_code_for_code_execution() {
        let caps = vec!["code_execution".to_string()];
        let a = ArtifactReport::new("agents_md", 0.8);
        assert_eq!(infer_classification(&caps, &a), "Code");
    }

    #[test]
    fn infer_classification_automation() {
        let caps = vec!["container_runtime".to_string()];
        let a = ArtifactReport::new("agents_md", 0.8);
        assert_eq!(infer_classification(&caps, &a), "Automation");
    }

    #[test]
    fn infer_classification_research() {
        let caps = vec!["browser_access".to_string()];
        let a = ArtifactReport::new("agents_md", 0.8);
        assert_eq!(infer_classification(&caps, &a), "Research");
    }

    #[test]
    fn infer_classification_system_for_agents_md() {
        let caps: Vec<String> = vec![];
        let a = ArtifactReport::new("agents_md", 0.8);
        assert_eq!(infer_classification(&caps, &a), "System");
    }

    #[test]
    fn infer_classification_system_default() {
        let caps: Vec<String> = vec![];
        let a = ArtifactReport::new("prompt_config", 0.8);
        assert_eq!(infer_classification(&caps, &a), "System");
    }

    #[test]
    fn execution_model_autonomous_for_dangerous_keyword() {
        let mut a = ArtifactReport::new("agents_md", 0.8);
        a.signals = vec!["dangerous_keyword:exfiltrate".to_string()];
        assert_eq!(infer_execution_model(&a), "Autonomous");
    }

    #[test]
    fn execution_model_autonomous_for_dangerous_combo() {
        let mut a = ArtifactReport::new("agents_md", 0.8);
        a.signals = vec!["dangerous_combo:shell+network+fs".to_string()];
        assert_eq!(infer_execution_model(&a), "Autonomous");
    }

    #[test]
    fn execution_model_user_in_loop_for_safe_signals() {
        let mut a = ArtifactReport::new("agents_md", 0.8);
        a.signals = vec!["keyword:shell".to_string()];
        assert_eq!(infer_execution_model(&a), "User-in-the-loop");
    }

    #[test]
    fn capability_flags_enable_matching_caps() {
        let caps = vec![
            "filesystem_access".to_string(),
            "shell_execution".to_string(),
        ];
        let flags = build_capability_flags(&caps);
        assert_eq!(flags.len(), 5);

        let fs_flag = flags.iter().find(|f| f.name == "Filesystem").unwrap();
        assert!(fs_flag.enabled);

        let shell_flag = flags.iter().find(|f| f.name == "Shell").unwrap();
        assert!(shell_flag.enabled);

        let browser_flag = flags.iter().find(|f| f.name == "Browser").unwrap();
        assert!(!browser_flag.enabled);
    }

    #[test]
    fn capability_flags_network_enabled_by_external_api() {
        let caps = vec!["external_api_calls".to_string()];
        let flags = build_capability_flags(&caps);
        let net = flags.iter().find(|f| f.name == "Network").unwrap();
        assert!(net.enabled);
    }

    #[test]
    fn trust_breakdown_agents_md_base() {
        let a = ArtifactReport::new("agents_md", 0.8);
        let factors = build_trust_breakdown(&a);
        assert!(factors
            .iter()
            .any(|f| f.delta == 10 && f.label.contains("agents md")));
    }

    #[test]
    fn trust_breakdown_credential_exposure() {
        let mut a = ArtifactReport::new("agents_md", 0.8);
        a.signals = vec!["credential_exposure_signal".to_string()];
        let factors = build_trust_breakdown(&a);
        assert!(factors.iter().any(|f| f.delta == -25));
    }

    #[test]
    fn trust_breakdown_dangerous_combo() {
        let mut a = ArtifactReport::new("agents_md", 0.8);
        a.signals = vec!["dangerous_combo:shell+network+fs".to_string()];
        let factors = build_trust_breakdown(&a);
        assert!(factors.iter().any(|f| f.delta == -30));
    }

    #[test]
    fn trust_breakdown_pass_bonus() {
        let mut a = ArtifactReport::new("agents_md", 0.8);
        a.verification_status = "pass".to_string();
        let factors = build_trust_breakdown(&a);
        assert!(factors
            .iter()
            .any(|f| f.delta == 15 && f.label == "Verification passed"));
    }

    #[test]
    fn build_declared_tools_from_metadata() {
        let mut a = ArtifactReport::new("agents_md", 0.8);
        a.metadata.insert(
            "declared_tools".to_string(),
            serde_json::json!(["shell", "browser"]),
        );
        let tools = build_declared_tools(&a);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "shell");
        assert_eq!(tools[0].tool_type, "skill");
    }
}
