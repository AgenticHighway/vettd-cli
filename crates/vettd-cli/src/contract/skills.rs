//! Skill building for the scanner data contract.

use crate::models::ArtifactReport;

use super::helpers::{declared_tools, first_path, make_id, qualified_name, read_artifact_head};
use super::types::{
    Agent, ExternalScannerResult, Skill, SkillConsumer, SkillDependencies, SkillPermission,
};

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

    let scanner_result = artifact
        .cached_scan_result
        .clone()
        .or_else(|| super::skill_scan::run_skill_scanner(artifact));
    let overall_grade = grade_from_scanner_result(scanner_result.as_ref()).to_string();
    let trust_level = trust_level_from_grade(&overall_grade).to_string();

    Skill {
        id,
        name,
        skill_type: "Local Function".to_string(),
        trust_level,
        overall_grade,
        execution_environment: "Local Process".to_string(),
        description: skill_artifact_description(artifact),
        permissions,
        dependencies: SkillDependencies {
            libraries: Vec::new(),
            binaries: skill_artifact_binaries(&capabilities),
            apis: skill_artifact_apis(&capabilities),
        },
        consumers: find_skill_consumers_by_path(source_path, agents),
        external_scanner_results: scanner_result.map(|r| vec![r]),
    }
}

/// Compute the overall grade from skill scanner findings.
///
/// Thresholds (worst wins):
/// - F: any critical, OR ≥ 3 highs
/// - C: any high (< 3), OR ≥ 3 mediums
/// - B: any medium (< 3), OR ≥ 4 lows
/// - A: < 4 lows, no mediums/highs/criticals
fn grade_from_scanner_result(result: Option<&ExternalScannerResult>) -> &'static str {
    let findings = match result.and_then(|r| r.findings.as_deref()) {
        Some(f) if !f.is_empty() => f,
        _ => return "A",
    };

    let mut critical = 0u32;
    let mut high = 0u32;
    let mut medium = 0u32;
    let mut low = 0u32;

    for f in findings {
        match f.severity.as_str() {
            "critical" => critical += 1,
            "high" => high += 1,
            "medium" => medium += 1,
            "low" => low += 1,
            _ => {}
        }
    }

    if critical > 0 || high >= 3 {
        "F"
    } else if high > 0 || medium >= 3 {
        "C"
    } else if medium > 0 || low >= 4 {
        "B"
    } else {
        "A"
    }
}

fn trust_level_from_grade(grade: &str) -> &'static str {
    match grade {
        "A" => "Trusted",
        "B" => "Conditional",
        "C" | "F" => "Untrusted",
        _ => "Conditional", // "pending" or unknown
    }
}

fn skill_artifact_description(artifact: &ArtifactReport) -> String {
    let path = first_path(artifact);
    if path != "unknown" {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Some(desc) = extract_frontmatter_description(&content) {
                return desc;
            }
        }
    }
    "Reusable agent skill instructions".to_string()
}

/// Extract the `description` field from SKILL.md YAML frontmatter.
///
/// Handles inline (`description: text`) and block-scalar values.
/// Returns `None` if the frontmatter is missing or the field is absent/empty.
fn extract_frontmatter_description(content: &str) -> Option<String> {
    let rest = content.strip_prefix("---\n")?;
    let close = rest.find("\n---")?;
    let raw = &rest[..close];

    let mut description = String::new();
    let mut collecting_block = false;

    for line in raw.lines() {
        if collecting_block {
            if line.starts_with(' ') || line.starts_with('\t') {
                if !description.is_empty() {
                    description.push(' ');
                }
                description.push_str(line.trim());
                continue;
            }
            collecting_block = false;
        }

        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("description:") {
            let inline = rest.trim().trim_matches('"').trim_matches('\'');
            if inline.is_empty() {
                collecting_block = true;
            } else {
                description = inline.to_string();
                break;
            }
        }
    }

    if description.is_empty() {
        None
    } else {
        Some(description)
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
            overall_grade: "pending".to_string(),
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
            external_scanner_results: None,
        });
    }
}

fn tool_to_skill(tool_name: &str, _artifact: &ArtifactReport, agents: &[Agent]) -> Skill {
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
        trust_level: trust_level_from_grade("pending").to_string(),
        overall_grade: "pending".to_string(),
        execution_environment: exec_env.to_string(),
        description: skill_description(tool_name),
        permissions,
        dependencies: SkillDependencies {
            libraries: Vec::new(),
            binaries,
            apis: Vec::new(),
        },
        consumers: find_skill_consumers(tool_name, agents),
        external_scanner_results: None,
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

    #[test]
    fn extract_frontmatter_description_inline() {
        let content = "---\nname: my-skill\ndescription: Does something useful\n---\nBody text\n";
        assert_eq!(
            extract_frontmatter_description(content),
            Some("Does something useful".to_string())
        );
    }

    #[test]
    fn extract_frontmatter_description_quoted() {
        let content = "---\ndescription: \"Quoted description here\"\n---\n";
        assert_eq!(
            extract_frontmatter_description(content),
            Some("Quoted description here".to_string())
        );
    }

    #[test]
    fn extract_frontmatter_description_block_scalar() {
        let content = "---\ndescription:\n  Multi-line\n  block value\n---\n";
        assert_eq!(
            extract_frontmatter_description(content),
            Some("Multi-line block value".to_string())
        );
    }

    #[test]
    fn extract_frontmatter_description_missing() {
        let content = "---\nname: my-skill\nauthor: me\n---\nBody\n";
        assert_eq!(extract_frontmatter_description(content), None);
    }

    #[test]
    fn extract_frontmatter_description_no_frontmatter() {
        let content = "Just a plain markdown file with no frontmatter.";
        assert_eq!(extract_frontmatter_description(content), None);
    }

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
    fn tool_to_skill_trust_level_is_conditional() {
        // tool_to_skill has no scanner result → grade is "pending" → trust_level "Conditional"
        let a = ArtifactReport::new("agents_md", 0.8);
        let skill = tool_to_skill("shell", &a, &[]);
        assert_eq!(skill.trust_level, "Conditional");
    }

    #[test]
    fn tool_to_skill_overall_grade_is_pending() {
        let a = ArtifactReport::new("agents_md", 0.8);
        let skill = tool_to_skill("shell", &a, &[]);
        assert_eq!(skill.overall_grade, "pending");
    }

    #[test]
    fn grade_from_scanner_result_thresholds() {
        use crate::contract::types::ExternalScannerFinding;

        let finding = |severity: &str| ExternalScannerFinding {
            rule_id: "VTD-0001".to_string(),
            category: "security".to_string(),
            severity: severity.to_string(),
            label: "test".to_string(),
            detail: None,
        };

        // No findings → A
        assert_eq!(grade_from_scanner_result(None), "A");

        // Any critical → F
        let r = ExternalScannerResult {
            source: "vettd".to_string(),
            version: None,
            status: "success".to_string(),
            verdict: None,
            raw_report: None,
            findings: Some(vec![finding("critical")]),
        };
        assert_eq!(grade_from_scanner_result(Some(&r)), "F");

        // 3 highs → F
        let r = ExternalScannerResult {
            findings: Some(vec![finding("high"), finding("high"), finding("high")]),
            ..r.clone()
        };
        assert_eq!(grade_from_scanner_result(Some(&r)), "F");

        // 2 highs → C
        let r = ExternalScannerResult {
            findings: Some(vec![finding("high"), finding("high")]),
            ..r.clone()
        };
        assert_eq!(grade_from_scanner_result(Some(&r)), "C");

        // 3 mediums → C
        let r = ExternalScannerResult {
            findings: Some(vec![
                finding("medium"),
                finding("medium"),
                finding("medium"),
            ]),
            ..r.clone()
        };
        assert_eq!(grade_from_scanner_result(Some(&r)), "C");

        // 2 mediums → B
        let r = ExternalScannerResult {
            findings: Some(vec![finding("medium"), finding("medium")]),
            ..r.clone()
        };
        assert_eq!(grade_from_scanner_result(Some(&r)), "B");

        // 4 lows → B
        let r = ExternalScannerResult {
            findings: Some(vec![
                finding("low"),
                finding("low"),
                finding("low"),
                finding("low"),
            ]),
            ..r.clone()
        };
        assert_eq!(grade_from_scanner_result(Some(&r)), "B");

        // 3 lows → A
        let r = ExternalScannerResult {
            findings: Some(vec![finding("low"), finding("low"), finding("low")]),
            ..r.clone()
        };
        assert_eq!(grade_from_scanner_result(Some(&r)), "A");

        // info only → A
        let r = ExternalScannerResult {
            findings: Some(vec![finding("info"), finding("info")]),
            ..r.clone()
        };
        assert_eq!(grade_from_scanner_result(Some(&r)), "A");
    }

    #[test]
    fn trust_level_from_grade_mapping() {
        assert_eq!(trust_level_from_grade("A"), "Trusted");
        assert_eq!(trust_level_from_grade("B"), "Conditional");
        assert_eq!(trust_level_from_grade("C"), "Untrusted");
        assert_eq!(trust_level_from_grade("F"), "Untrusted");
        assert_eq!(trust_level_from_grade("pending"), "Conditional");
    }

    #[test]
    fn artifact_to_skill_grade_from_scanner() {
        // A skill artifact with no files on disk gets a critical "Missing SKILL.md"
        // finding → grade F, trust_level Untrusted.
        let mut a = ArtifactReport::new("skill", 0.9);
        a.metadata.insert(
            "paths".to_string(),
            serde_json::json!(["/nonexistent/release-notes/SKILL.md"]),
        );
        a.compute_hash();

        let skills = build_skills(&[a], &[]);
        assert_eq!(skills[0].overall_grade, "F");
        assert_eq!(skills[0].trust_level, "Untrusted");
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
        assert_eq!(skills[0].skill_type, "Local Function");
        assert_eq!(skills[0].execution_environment, "Local Process");
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
