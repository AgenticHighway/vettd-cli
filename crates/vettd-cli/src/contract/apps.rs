//! AgenticApp building for the scanner data contract.

use crate::capabilities::derive_capabilities;
use crate::models::ArtifactReport;

use super::helpers::{first_path, make_id, qualified_name};
use super::types::{Agent, AgenticApp, AppAgent, Integration, WorkflowStep};

pub fn build_agentic_apps(
    container_artifacts: &[&ArtifactReport],
    agents: &[Agent],
) -> Vec<AgenticApp> {
    container_artifacts
        .iter()
        .filter_map(|a| {
            let local_agents = find_local_agents(first_path(a), agents);
            if is_agentic_container(a, &local_agents) {
                Some(container_to_app(a, &local_agents))
            } else {
                None
            }
        })
        .collect()
}

fn container_to_app(a: &ArtifactReport, local_agents: &[&Agent]) -> AgenticApp {
    let source_path = first_path(a).to_string();
    let name = qualified_name(&source_path);
    let id = make_id(&source_path, &a.artifact_hash);

    let framework = detect_framework(a);
    let risk = risk_level(a.risk_score);
    let review_status = review_status_label(&a.verification_status);

    let app_agents = build_app_agents(local_agents);
    let tools_by_agent = build_tools_by_agent(local_agents);
    let workflow = build_workflow(local_agents);
    let integrations = build_integrations(a);
    let verification_checks = build_verification_checks(a);
    let risk_tags = build_risk_tags(a);
    let risk_summary = build_risk_summary(&name, a, risk);
    let description = build_app_description(a, &framework, local_agents);

    AgenticApp {
        id,
        name,
        source_file_path: source_path,
        framework,
        agent_count: app_agents.len() as u32,
        risk: risk.to_string(),
        review_status: review_status.to_string(),
        description,
        agents: app_agents,
        tools_by_agent,
        workflow,
        integrations,
        verification_checks,
        risk_tags,
        risk_summary,
    }
}

fn is_agentic_container(a: &ArtifactReport, local_agents: &[&Agent]) -> bool {
    !local_agents.is_empty() || metadata_bool(a, "direct_agentic_evidence")
}

fn risk_level(score: i32) -> &'static str {
    if score >= 70 {
        "High"
    } else if score >= 40 {
        "Medium"
    } else {
        "Low"
    }
}

fn review_status_label(status: &str) -> &'static str {
    match status {
        "pass" => "Reviewed",
        "fail" => "Flagged",
        _ => "Unreviewed",
    }
}

fn find_local_agents<'a>(source_path: &str, agents: &'a [Agent]) -> Vec<&'a Agent> {
    let container_dir = std::path::Path::new(source_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    agents
        .iter()
        .filter(|ag| ag.source_file_path.starts_with(&container_dir))
        .collect()
}

fn build_app_agents(local_agents: &[&Agent]) -> Vec<AppAgent> {
    local_agents
        .iter()
        .map(|ag| AppAgent {
            id: ag.id.clone(),
            name: ag.name.clone(),
        })
        .collect()
}

fn build_tools_by_agent(local_agents: &[&Agent]) -> Vec<Vec<String>> {
    local_agents
        .iter()
        .map(|ag| ag.tools.iter().map(|t| t.name.clone()).collect())
        .collect()
}

fn build_workflow(local_agents: &[&Agent]) -> Vec<WorkflowStep> {
    local_agents
        .iter()
        .enumerate()
        .map(|(i, ag)| WorkflowStep {
            step: (i + 1) as u32,
            agent: ag.name.clone(),
            action: format!(
                "Execute {} tasks using {} tools",
                ag.classification,
                ag.tools.len()
            ),
        })
        .collect()
}

fn build_app_description(a: &ArtifactReport, framework: &str, local_agents: &[&Agent]) -> String {
    let agent_count = local_agents.len();
    let artifact_label = artifact_label(a);
    let has_direct_agentic_evidence = metadata_bool(a, "direct_agentic_evidence");

    if agent_count > 0 {
        let unique_classes: std::collections::BTreeSet<&str> = local_agents
            .iter()
            .map(|ag| ag.classification.as_str())
            .collect();
        format!(
            "{artifact_label} for a {framework} workflow with {agent_count} agent(s) performing {} tasks",
            unique_classes
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
                .to_lowercase()
        )
    } else if has_direct_agentic_evidence {
        format!("{artifact_label} for a {framework} workflow")
    } else {
        format!("{artifact_label} associated with a {framework} workflow")
    }
}

fn detect_framework(a: &ArtifactReport) -> String {
    for signal in &a.signals {
        let s = signal.to_lowercase();
        if s.contains("langchain") || s.contains("langgraph") {
            return "LangGraph".to_string();
        }
        if s.contains("crewai") {
            return "CrewAI".to_string();
        }
        if s.contains("autogen") {
            return "AutoGen".to_string();
        }
    }
    "Custom".to_string()
}

fn build_integrations(a: &ArtifactReport) -> Vec<Integration> {
    let endpoints: Vec<String> = a
        .metadata
        .get("api_endpoints")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    endpoints
        .iter()
        .filter(|ep| {
            let lower = ep.to_lowercase();
            !(lower.contains("docs.") || lower.contains("/docs/") || lower.contains("readme"))
        })
        .map(|ep| {
            let (name, itype, risk) = classify_endpoint(ep);
            Integration {
                name,
                integration_type: itype,
                risk,
            }
        })
        .collect()
}

fn classify_endpoint(ep: &str) -> (String, String, String) {
    let lower = ep.to_lowercase();
    if lower.contains("github") {
        (
            "GitHub API".to_string(),
            "REST API".to_string(),
            "Medium".to_string(),
        )
    } else if lower.contains("openai") {
        (
            "OpenAI API".to_string(),
            "REST API".to_string(),
            "Medium".to_string(),
        )
    } else if lower.contains("anthropic") {
        (
            "Anthropic API".to_string(),
            "REST API".to_string(),
            "Medium".to_string(),
        )
    } else {
        (ep.to_string(), "REST API".to_string(), "Medium".to_string())
    }
}

fn build_verification_checks(a: &ArtifactReport) -> Vec<String> {
    let mut checks = vec![
        format!("{} present", artifact_label(a)),
        "Risk score computed".to_string(),
    ];

    if a.verification_status == "pass" {
        checks.push("Verification passed".to_string());
    }
    if metadata_bool(a, "direct_agentic_evidence") {
        checks.push("Direct agentic content detected".to_string());
    }
    if a.signals.iter().any(|s| s == "ai_artifact_proximity") {
        checks.push("AI artifact proximity detected".to_string());
    }

    checks
}

fn build_risk_tags(a: &ArtifactReport) -> Vec<String> {
    let mut tags = Vec::new();
    let caps = derive_capabilities(a);

    if caps
        .iter()
        .any(|c| c == "shell_execution" || c == "code_execution")
    {
        tags.push("Autonomous Code Execution".to_string());
    }
    if a.signals.iter().any(|s| s == "credential_exposure_signal") {
        tags.push("Credential Exposure".to_string());
    }
    if caps
        .iter()
        .any(|c| c == "network_access" || c == "external_api_calls")
    {
        tags.push("External Network Access".to_string());
    }
    if a.signals
        .iter()
        .any(|s| s.starts_with("dangerous_keyword:"))
    {
        tags.push("Dangerous Instructions".to_string());
    }

    tags
}

fn build_risk_summary(app_name: &str, a: &ArtifactReport, risk_level: &str) -> String {
    let reasons: Vec<&str> = a.risk_reasons.iter().map(|r| r.as_str()).collect();
    let artifact_label = artifact_label(a).to_lowercase();

    if reasons.is_empty() {
        format!(
            "{risk_level}-risk {artifact_label} '{app_name}'. No specific risk drivers identified."
        )
    } else {
        format!(
            "{risk_level}-risk {artifact_label} '{app_name}'. Key risk drivers: {}.",
            reasons.join(", ")
        )
    }
}

fn artifact_kind(a: &ArtifactReport) -> &str {
    a.metadata
        .get("container_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("service_orchestration")
}

fn artifact_label(a: &ArtifactReport) -> &'static str {
    match artifact_kind(a) {
        "image_definition" => "Container image definition",
        _ => "Service orchestration configuration",
    }
}

fn metadata_bool(a: &ArtifactReport, key: &str) -> bool {
    a.metadata
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::super::types::{AgentCapability, AgentTool, TrustFactor};
    use super::*;
    use crate::models::ArtifactReport;

    fn make_agent(name: &str, source_file_path: &str, classification: &str) -> Agent {
        Agent {
            id: format!("agent-{name}"),
            name: name.to_string(),
            source_file_path: source_file_path.to_string(),
            classification: classification.to_string(),
            execution_model: "local".to_string(),
            trust_score: 80,
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            source_repo: "unknown".to_string(),
            capabilities: vec![AgentCapability {
                name: "tool_use".to_string(),
                enabled: true,
            }],
            tools: vec![AgentTool {
                name: "shell".to_string(),
                tool_type: "builtin".to_string(),
            }],
            trust_breakdown: vec![TrustFactor {
                label: "test".to_string(),
                delta: 10,
            }],
        }
    }

    #[test]
    fn risk_level_high() {
        assert_eq!(risk_level(70), "High");
        assert_eq!(risk_level(100), "High");
    }

    #[test]
    fn risk_level_medium() {
        assert_eq!(risk_level(40), "Medium");
        assert_eq!(risk_level(69), "Medium");
    }

    #[test]
    fn risk_level_low() {
        assert_eq!(risk_level(0), "Low");
        assert_eq!(risk_level(39), "Low");
    }

    #[test]
    fn review_status_pass() {
        assert_eq!(review_status_label("pass"), "Reviewed");
    }

    #[test]
    fn review_status_fail() {
        assert_eq!(review_status_label("fail"), "Flagged");
    }

    #[test]
    fn review_status_unknown() {
        assert_eq!(review_status_label("pending"), "Unreviewed");
        assert_eq!(review_status_label("conditional_pass"), "Unreviewed");
    }

    #[test]
    fn detect_framework_langchain() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.signals = vec!["uses_langchain_framework".to_string()];
        assert_eq!(detect_framework(&a), "LangGraph");
    }

    #[test]
    fn detect_framework_crewai() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.signals = vec!["uses_crewai".to_string()];
        assert_eq!(detect_framework(&a), "CrewAI");
    }

    #[test]
    fn detect_framework_custom_default() {
        let a = ArtifactReport::new("container_config", 0.8);
        assert_eq!(detect_framework(&a), "Custom");
    }

    #[test]
    fn classify_endpoint_github() {
        let (name, itype, _risk) = classify_endpoint("https://api.github.com/repos");
        assert_eq!(name, "GitHub API");
        assert_eq!(itype, "REST API");
    }

    #[test]
    fn classify_endpoint_openai() {
        let (name, _, _) = classify_endpoint("https://api.openai.com/v1/chat");
        assert_eq!(name, "OpenAI API");
    }

    #[test]
    fn classify_endpoint_anthropic() {
        let (name, _, _) = classify_endpoint("https://api.anthropic.com/v1/messages");
        assert_eq!(name, "Anthropic API");
    }

    #[test]
    fn classify_endpoint_unknown() {
        let (name, _, _) = classify_endpoint("https://example.com/api");
        assert_eq!(name, "https://example.com/api");
    }

    #[test]
    fn build_verification_checks_basic() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.metadata.insert(
            "container_kind".to_string(),
            serde_json::json!("image_definition"),
        );
        let checks = build_verification_checks(&a);
        assert!(checks.contains(&"Container image definition present".to_string()));
        assert!(checks.contains(&"Risk score computed".to_string()));
    }

    #[test]
    fn build_verification_checks_pass() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.verification_status = "pass".to_string();
        let checks = build_verification_checks(&a);
        assert!(checks.contains(&"Verification passed".to_string()));
    }

    #[test]
    fn build_verification_checks_ai_proximity() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.metadata.insert(
            "container_kind".to_string(),
            serde_json::json!("service_orchestration"),
        );
        a.signals = vec!["ai_artifact_proximity".to_string()];
        let checks = build_verification_checks(&a);
        assert!(checks.contains(&"AI artifact proximity detected".to_string()));
    }

    #[test]
    fn build_verification_checks_direct_agentic_content() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.metadata.insert(
            "direct_agentic_evidence".to_string(),
            serde_json::json!(true),
        );
        let checks = build_verification_checks(&a);
        assert!(checks.contains(&"Direct agentic content detected".to_string()));
    }

    #[test]
    fn build_risk_tags_code_execution() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.signals = vec!["keyword:shell".to_string()];
        let tags = build_risk_tags(&a);
        assert!(tags.contains(&"Autonomous Code Execution".to_string()));
    }

    #[test]
    fn build_risk_tags_credential_exposure() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.signals = vec!["credential_exposure_signal".to_string()];
        let tags = build_risk_tags(&a);
        assert!(tags.contains(&"Credential Exposure".to_string()));
    }

    #[test]
    fn build_risk_tags_dangerous_instructions() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.signals = vec!["dangerous_keyword:exfiltrate".to_string()];
        let tags = build_risk_tags(&a);
        assert!(tags.contains(&"Dangerous Instructions".to_string()));
    }

    #[test]
    fn build_risk_summary_no_reasons() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.metadata.insert(
            "container_kind".to_string(),
            serde_json::json!("service_orchestration"),
        );
        let summary = build_risk_summary("my-app", &a, "Low");
        assert!(summary.contains("Low-risk"));
        assert!(summary.contains("my-app"));
        assert!(summary.contains("No specific risk drivers"));
    }

    #[test]
    fn build_risk_summary_with_reasons() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.metadata.insert(
            "container_kind".to_string(),
            serde_json::json!("image_definition"),
        );
        a.risk_reasons = vec!["shell access".to_string(), "network calls".to_string()];
        let summary = build_risk_summary("my-app", &a, "High");
        assert!(summary.contains("High-risk"));
        assert!(summary.contains("shell access, network calls"));
    }

    #[test]
    fn build_agentic_apps_skips_container_candidates() {
        let mut a = ArtifactReport::new("container_candidate", 0.8);
        a.metadata
            .insert("paths".to_string(), serde_json::json!(["/tmp/Dockerfile"]));
        let apps = build_agentic_apps(&[&a], &[]);
        assert!(apps.is_empty());
    }

    #[test]
    fn build_agentic_apps_skips_non_agentic_container_config() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.metadata
            .insert("paths".to_string(), serde_json::json!(["/tmp/Dockerfile"]));
        a.metadata
            .insert("direct_ai_evidence".to_string(), serde_json::json!(true));
        a.metadata.insert(
            "direct_agentic_evidence".to_string(),
            serde_json::json!(false),
        );

        let apps = build_agentic_apps(&[&a], &[]);
        assert!(apps.is_empty());
    }

    #[test]
    fn build_agentic_apps_promotes_local_agents_even_for_candidate() {
        let mut a = ArtifactReport::new("container_candidate", 0.55);
        a.metadata.insert(
            "paths".to_string(),
            serde_json::json!(["/tmp/project/Dockerfile"]),
        );
        a.metadata.insert(
            "container_kind".to_string(),
            serde_json::json!("image_definition"),
        );

        let agent = make_agent("planner", "/tmp/project/agents.md", "planner");
        let apps = build_agentic_apps(&[&a], &[agent]);

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].agent_count, 1);
        assert!(apps[0].description.contains("Container image definition"));
    }

    #[test]
    fn build_agentic_apps_promotes_direct_agentic_content_without_local_agents() {
        let mut a = ArtifactReport::new("container_config", 0.8);
        a.metadata.insert(
            "paths".to_string(),
            serde_json::json!(["/tmp/project/docker-compose.yml"]),
        );
        a.metadata.insert(
            "container_kind".to_string(),
            serde_json::json!("service_orchestration"),
        );
        a.metadata.insert(
            "direct_agentic_evidence".to_string(),
            serde_json::json!(true),
        );
        a.signals = vec!["ai_token:langgraph".to_string()];

        let apps = build_agentic_apps(&[&a], &[]);

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].framework, "LangGraph");
        assert!(apps[0]
            .description
            .contains("Service orchestration configuration"));
    }
}
