//! Skill building for the scanner data contract.

use crate::models::ArtifactReport;

use super::helpers::{declared_tools, first_path, make_id, qualified_name, read_artifact_head};
use super::types::{Agent, Skill, SkillConsumer, SkillDependencies, SkillPermission};

pub fn build_skills(artifacts: &[ArtifactReport], agents: &[Agent]) -> Vec<Skill> {
    let mut seen = std::collections::HashSet::new();
    let mut skills = Vec::new();

    for artifact in artifacts {
        if artifact.artifact_type == "skill" {
            let skill = artifact_to_skill(artifact, agents);
            if seen.insert(skill.name.clone()) {
                skills.push(skill);
            }
        }

        let tools = declared_tools(artifact);
        for tool in tools {
            if seen.insert(tool.clone()) {
                skills.push(tool_to_skill(&tool, artifact, agents));
            }
        }
    }

    // Add skills from MCP server tool commands
    for artifact in artifacts.iter().filter(|a| a.artifact_type == "mcp_config") {
        if let Some(content) = read_artifact_head(artifact) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                extract_mcp_command_skills(&val, &mut seen, &mut skills, agents);
            }
        }
    }

    skills
}

fn artifact_to_skill(artifact: &ArtifactReport, agents: &[Agent]) -> Skill {
    let source_path = first_path(artifact);
    let name = qualified_name(source_path);
    let id = make_id(source_path, &artifact.artifact_hash);
    let capabilities = crate::capabilities::derive_capabilities(artifact);
    let permissions = infer_permissions_from_capabilities(&capabilities);

    Skill {
        id,
        name,
        skill_type: "Instruction Skill".to_string(),
        trust_level: trust_level(artifact).to_string(),
        execution_environment: "Agent Runtime".to_string(),
        description: skill_artifact_description(artifact),
        permissions,
        dependencies: SkillDependencies {
            libraries: Vec::new(),
            binaries: skill_artifact_binaries(&capabilities),
            apis: skill_artifact_apis(&capabilities),
        },
        consumers: find_skill_consumers_by_path(source_path, agents),
    }
}

fn trust_level(artifact: &ArtifactReport) -> &'static str {
    if artifact.risk_score >= 70 {
        "Untrusted"
    } else if artifact.risk_score >= 40 {
        "Conditional"
    } else {
        "Trusted"
    }
}

fn skill_artifact_description(artifact: &ArtifactReport) -> String {
    let path = first_path(artifact);
    if path == "unknown" {
        "Reusable agent skill instructions".to_string()
    } else {
        format!("Reusable agent skill instructions from {path}")
    }
}

fn infer_permissions_from_capabilities(capabilities: &[String]) -> Vec<SkillPermission> {
    let mut permissions = Vec::new();
    let mut push = |name: &str| {
        if !permissions
            .iter()
            .any(|permission: &SkillPermission| permission.name == name)
        {
            permissions.push(SkillPermission {
                name: name.to_string(),
                required: true,
            });
        }
    };

    for capability in capabilities {
        match capability.as_str() {
            "shell_execution" | "code_execution" => push("Shell execution"),
            "filesystem_access" => push("Filesystem read/write"),
            "network_access" | "external_api_calls" | "browser_access" => push("Network access"),
            "container_runtime" => {
                push("Shell execution");
                push("Network access");
            }
            "secret_references" => push("Secret access"),
            _ => {}
        }
    }

    permissions
}

fn skill_artifact_binaries(capabilities: &[String]) -> Vec<String> {
    let mut binaries = Vec::new();
    if capabilities
        .iter()
        .any(|cap| cap == "shell_execution" || cap == "code_execution")
    {
        binaries.push("shell".to_string());
    }
    if capabilities.iter().any(|cap| cap == "container_runtime") {
        binaries.push("docker".to_string());
    }
    binaries
}

fn skill_artifact_apis(capabilities: &[String]) -> Vec<String> {
    if capabilities
        .iter()
        .any(|cap| cap == "external_api_calls" || cap == "network_access")
    {
        vec!["HTTP".to_string()]
    } else {
        Vec::new()
    }
}

fn find_skill_consumers_by_path(source_path: &str, agents: &[Agent]) -> Vec<SkillConsumer> {
    agents
        .iter()
        .filter(|agent| agent.source_file_path == source_path)
        .map(|agent| SkillConsumer {
            id: agent.id.clone(),
            name: agent.name.clone(),
            consumer_type: "Agent".to_string(),
            invocations: 0,
        })
        .collect()
}

fn extract_mcp_command_skills(
    val: &serde_json::Value,
    seen: &mut std::collections::HashSet<String>,
    skills: &mut Vec<Skill>,
    agents: &[Agent],
) {
    let servers = val
        .get("mcpServers")
        .or_else(|| val.get("servers"))
        .and_then(|v| v.as_object());
    let servers = match servers {
        Some(s) => s,
        None => return,
    };

    for (_server_name, server_val) in servers {
        let cmd = match server_val.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => continue,
        };
        let skill_name = cmd.split('/').next_back().unwrap_or(cmd).to_string();
        if !seen.insert(skill_name.clone()) {
            continue;
        }

        let args_str = server_val
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let full_cmd = if args_str.is_empty() {
            cmd.to_string()
        } else {
            format!("{cmd} {args_str}")
        };

        skills.push(Skill {
            id: skill_name.clone(),
            name: skill_name.clone(),
            skill_type: "CLI Tool".to_string(),
            trust_level: "Conditional".to_string(),
            execution_environment: "Local Process".to_string(),
            description: format!("Executes MCP server via: {full_cmd}"),
            permissions: vec![SkillPermission {
                name: "Shell execution".to_string(),
                required: true,
            }],
            dependencies: SkillDependencies {
                libraries: Vec::new(),
                binaries: vec![cmd.to_string()],
                apis: Vec::new(),
            },
            consumers: find_skill_consumers(&skill_name, agents),
        });
    }
}

fn tool_to_skill(tool_name: &str, artifact: &ArtifactReport, agents: &[Agent]) -> Skill {
    let (skill_type, exec_env) = match tool_name {
        "shell" | "bash" => ("CLI Tool", "Local Process"),
        "browser" => ("HTTP Integration", "Remote API"),
        "api" => ("HTTP Integration", "Remote API"),
        "docker" => ("CLI Tool", "Container"),
        "python" | "node" => ("CLI Tool", "Local Process"),
        "filesystem" => ("Local Function", "Local Process"),
        _ => ("Local Function", "Local Process"),
    };

    let permissions = infer_permissions(tool_name);

    let binaries: Vec<String> = match tool_name {
        "shell" | "bash" => vec!["bash".to_string()],
        "python" => vec!["python".to_string()],
        "node" => vec!["node".to_string()],
        "docker" => vec!["docker".to_string()],
        _ => Vec::new(),
    };

    Skill {
        id: tool_name.to_string(),
        name: tool_name.to_string(),
        skill_type: skill_type.to_string(),
        trust_level: trust_level(artifact).to_string(),
        execution_environment: exec_env.to_string(),
        description: skill_description(tool_name),
        permissions,
        dependencies: SkillDependencies {
            libraries: Vec::new(),
            binaries,
            apis: Vec::new(),
        },
        consumers: find_skill_consumers(tool_name, agents),
    }
}

fn skill_description(tool_name: &str) -> String {
    match tool_name {
        "shell" | "bash" => {
            "Executes shell commands via local bash interpreter with unrestricted system access"
                .to_string()
        }
        "python" => {
            "Executes Python scripts via local interpreter with unrestricted filesystem access"
                .to_string()
        }
        "node" => "Executes Node.js scripts via local runtime with unrestricted filesystem access"
            .to_string(),
        "filesystem" => "Reads and writes files on the local filesystem".to_string(),
        "browser" => "Controls a browser instance for web navigation and interaction".to_string(),
        "api" => "Makes HTTP requests to external API services".to_string(),
        "docker" => "Manages Docker containers and images via the Docker CLI".to_string(),
        other => format!("Provides {} functionality", other.replace('_', " ")),
    }
}

fn infer_permissions(tool_name: &str) -> Vec<SkillPermission> {
    let mut perms = Vec::new();
    match tool_name {
        "shell" | "bash" => {
            perms.push(SkillPermission {
                name: "Shell execution".to_string(),
                required: true,
            });
            perms.push(SkillPermission {
                name: "Filesystem read/write".to_string(),
                required: true,
            });
        }
        "filesystem" => {
            perms.push(SkillPermission {
                name: "Filesystem read/write".to_string(),
                required: true,
            });
        }
        "browser" | "api" => {
            perms.push(SkillPermission {
                name: "Network access".to_string(),
                required: true,
            });
        }
        "docker" => {
            perms.push(SkillPermission {
                name: "Shell execution".to_string(),
                required: true,
            });
            perms.push(SkillPermission {
                name: "Network access".to_string(),
                required: true,
            });
        }
        "python" | "node" => {
            perms.push(SkillPermission {
                name: "Shell execution".to_string(),
                required: true,
            });
        }
        _ => {}
    }
    perms
}

fn find_skill_consumers(tool_name: &str, agents: &[Agent]) -> Vec<SkillConsumer> {
    agents
        .iter()
        .filter(|agent| agent.tools.iter().any(|t| t.name == tool_name))
        .map(|agent| SkillConsumer {
            id: agent.id.clone(),
            name: agent.name.clone(),
            consumer_type: "Agent".to_string(),
            invocations: 0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::types::AgentTool;
    use crate::models::ArtifactReport;

    fn make_agent(name: &str, tools: Vec<&str>) -> Agent {
        Agent {
            id: format!("agent-{name}"),
            name: name.to_string(),
            source_file_path: String::new(),
            classification: "System".to_string(),
            execution_model: "User-in-the-loop".to_string(),
            trust_score: 80,
            version: "unknown".to_string(),
            author: "unknown".to_string(),
            source_repo: "unknown".to_string(),
            capabilities: Vec::new(),
            tools: tools
                .into_iter()
                .map(|t| AgentTool {
                    name: t.to_string(),
                    tool_type: "skill".to_string(),
                })
                .collect(),
            trust_breakdown: Vec::new(),
        }
    }

    #[test]
    fn skill_description_known_tools() {
        assert!(skill_description("shell").contains("shell commands"));
        assert!(skill_description("bash").contains("shell commands"));
        assert!(skill_description("python").contains("Python"));
        assert!(skill_description("node").contains("Node.js"));
        assert!(skill_description("filesystem").contains("filesystem"));
        assert!(skill_description("browser").contains("browser"));
        assert!(skill_description("api").contains("HTTP"));
        assert!(skill_description("docker").contains("Docker"));
    }

    #[test]
    fn skill_description_unknown_replaces_underscores() {
        let desc = skill_description("my_custom_tool");
        assert_eq!(desc, "Provides my custom tool functionality");
    }

    #[test]
    fn infer_permissions_shell() {
        let perms = infer_permissions("shell");
        assert_eq!(perms.len(), 2);
        assert!(perms.iter().any(|p| p.name == "Shell execution"));
        assert!(perms.iter().any(|p| p.name == "Filesystem read/write"));
    }

    #[test]
    fn infer_permissions_filesystem() {
        let perms = infer_permissions("filesystem");
        assert_eq!(perms.len(), 1);
        assert_eq!(perms[0].name, "Filesystem read/write");
    }

    #[test]
    fn infer_permissions_browser() {
        let perms = infer_permissions("browser");
        assert_eq!(perms.len(), 1);
        assert_eq!(perms[0].name, "Network access");
    }

    #[test]
    fn infer_permissions_docker() {
        let perms = infer_permissions("docker");
        assert_eq!(perms.len(), 2);
        assert!(perms.iter().any(|p| p.name == "Shell execution"));
        assert!(perms.iter().any(|p| p.name == "Network access"));
    }

    #[test]
    fn infer_permissions_unknown_empty() {
        let perms = infer_permissions("custom_tool");
        assert!(perms.is_empty());
    }

    #[test]
    fn find_skill_consumers_matches_agents() {
        let agents = vec![
            make_agent("coder", vec!["shell", "filesystem"]),
            make_agent("researcher", vec!["browser", "api"]),
        ];
        let consumers = find_skill_consumers("shell", &agents);
        assert_eq!(consumers.len(), 1);
        assert_eq!(consumers[0].name, "coder");
        assert_eq!(consumers[0].consumer_type, "Agent");
    }

    #[test]
    fn find_skill_consumers_no_match() {
        let agents = vec![make_agent("coder", vec!["shell"])];
        let consumers = find_skill_consumers("browser", &agents);
        assert!(consumers.is_empty());
    }

    #[test]
    fn tool_to_skill_shell_type() {
        let a = ArtifactReport::new("agents_md", 0.8);
        let skill = tool_to_skill("shell", &a, &[]);
        assert_eq!(skill.skill_type, "CLI Tool");
        assert_eq!(skill.execution_environment, "Local Process");
        assert!(skill.dependencies.binaries.contains(&"bash".to_string()));
    }

    #[test]
    fn tool_to_skill_browser_type() {
        let a = ArtifactReport::new("agents_md", 0.8);
        let skill = tool_to_skill("browser", &a, &[]);
        assert_eq!(skill.skill_type, "HTTP Integration");
        assert_eq!(skill.execution_environment, "Remote API");
    }

    #[test]
    fn tool_to_skill_trust_level_by_risk() {
        let mut a = ArtifactReport::new("agents_md", 0.8);
        a.risk_score = 80;
        let skill = tool_to_skill("shell", &a, &[]);
        assert_eq!(skill.trust_level, "Untrusted");

        a.risk_score = 50;
        let skill = tool_to_skill("shell", &a, &[]);
        assert_eq!(skill.trust_level, "Conditional");

        a.risk_score = 10;
        let skill = tool_to_skill("shell", &a, &[]);
        assert_eq!(skill.trust_level, "Trusted");
    }

    #[test]
    fn build_skills_deduplicates() {
        let mut a1 = ArtifactReport::new("agents_md", 0.8);
        a1.metadata.insert(
            "declared_tools".to_string(),
            serde_json::json!(["shell", "browser"]),
        );
        let mut a2 = ArtifactReport::new("agents_md", 0.8);
        a2.metadata.insert(
            "declared_tools".to_string(),
            serde_json::json!(["shell", "api"]),
        );
        let skills = build_skills(&[a1, a2], &[]);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"browser"));
        assert!(names.contains(&"api"));
        // "shell" should appear only once
        assert_eq!(names.iter().filter(|n| **n == "shell").count(), 1);
    }

    #[test]
    fn build_skills_includes_skill_artifacts() {
        let mut a = ArtifactReport::new("skill", 0.9);
        a.metadata.insert(
            "paths".to_string(),
            serde_json::json!(["/repo/skills/release-notes/SKILL.md"]),
        );
        a.signals = vec!["keyword:shell".to_string(), "keyword:api".to_string()];
        a.compute_hash();

        let skills = build_skills(&[a], &[]);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "release-notes/SKILL");
        assert_eq!(skills[0].skill_type, "Instruction Skill");
        assert_eq!(skills[0].execution_environment, "Agent Runtime");
        assert!(skills[0]
            .permissions
            .iter()
            .any(|permission| permission.name == "Shell execution"));
        assert!(skills[0]
            .permissions
            .iter()
            .any(|permission| permission.name == "Network access"));
    }
}
