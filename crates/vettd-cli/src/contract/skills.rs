//! Skill building for the scanner data contract.

use crate::models::ArtifactReport;

use super::helpers::{
    artifact_content_hash, declared_tools, detect_skill_source, first_path, make_id,
    qualified_name, read_artifact_head,
};
use super::identity;
use super::mcp::build_command_string;
use super::types::{
    Agent, ExternalScannerResult, Skill, SkillConsumer, SkillDependencies, SkillLineage,
    SkillLocation, SkillPermission,
};

pub fn build_skills(artifacts: &[ArtifactReport], agents: &[Agent]) -> Vec<Skill> {
    // Name-keyed collapsing applies only to declared tools and MCP command
    // skills, where collapsing by name is the intent. Skill artifacts are
    // keyed by content digest instead (#255): two skills with the same folder
    // name in different agent directories are different assets and both must
    // survive; identical copies merge into one entry listing every location
    // (#274).
    let mut seen = std::collections::HashSet::new();
    let mut skills: Vec<Skill> = Vec::new();
    let mut digest_index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for artifact in artifacts {
        if artifact.artifact_type == "skill" {
            let skill = artifact_to_skill(artifact, agents);
            let key = skill
                .content_hash
                .clone()
                .unwrap_or_else(|| skill.id.clone());
            match digest_index.get(&key) {
                Some(&index) => {
                    if let Some(locations) = skill.locations {
                        let existing = &mut skills[index];
                        let locs = existing.locations.get_or_insert_with(Vec::new);
                        for location in locations {
                            if !locs.iter().any(|l| l.path == location.path) {
                                locs.push(location);
                            }
                        }
                    }
                }
                None => {
                    digest_index.insert(key, skills.len());
                    skills.push(skill);
                }
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
            let val = match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(val) => val,
                Err(e) => {
                    eprintln!(
                        "Warning: MCP config {} is unparseable ({}), command skills from it are missing",
                        first_path(artifact),
                        e
                    );
                    continue;
                }
            };
            extract_mcp_command_skills(&val, &mut seen, &mut skills, agents);
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
    let detected_source = detect_skill_source(source_path);
    let frontmatter = read_skill_frontmatter(artifact);

    let scan_output = artifact
        .cached_skill_scan
        .clone()
        .or_else(|| super::skill_scan::run_skill_scanner(artifact));
    let overall_grade =
        grade_from_scanner_result(scan_output.as_ref().map(|o| &o.external)).to_string();
    let trust_level = trust_level_from_grade(&overall_grade).to_string();
    let structural = scan_output.as_ref().map(|o| &o.structural);

    // Content identity (#274, #130): digest over the whole skill directory
    // (minus disclosed exclusions), the location this copy was found in with
    // its provenance, and git/declared lineage for relating copies downstream.
    let identity_load = if source_path != "unknown" {
        let root = std::path::Path::new(source_path)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        Some(identity::load_skill_files(root))
    } else {
        None
    };
    let content_hash = match &identity_load {
        Some(load) => identity::canonical_digest(&load.text_files),
        None => artifact_content_hash(artifact),
    };
    let locations = (source_path != "unknown").then(|| {
        vec![SkillLocation {
            path: source_path.to_string(),
            provenance: identity::provenance_for(source_path).to_string(),
        }]
    });
    let identity_exclusions = identity_load
        .as_ref()
        .map(|load| load.excluded.clone())
        .filter(|excluded| !excluded.is_empty());
    let lineage = (source_path != "unknown")
        .then(|| {
            let git = identity::lineage_for(std::path::Path::new(source_path));
            SkillLineage {
                git_remote_url: git.git_remote_url,
                git_commit: git.git_commit,
                declared_name: frontmatter.name.clone(),
                declared_version: frontmatter.version.clone(),
            }
        })
        .filter(|lineage| {
            lineage.git_remote_url.is_some()
                || lineage.git_commit.is_some()
                || lineage.declared_name.is_some()
                || lineage.declared_version.is_some()
        });

    Skill {
        id,
        name,
        skill_type: "Local Function".to_string(),
        trust_level,
        overall_grade,
        execution_environment: "Local Process".to_string(),
        description: frontmatter
            .description
            .clone()
            .unwrap_or_else(|| "Reusable agent skill instructions".to_string()),
        // v2.6.0 skill-level surface: structural facts come from the scanner
        // output; version/license from SKILL.md frontmatter. Never fabricated —
        // all omit-when-absent.
        version: frontmatter.version.clone(),
        license: frontmatter.license.clone(),
        file_count: structural.map(|s| s.file_count),
        has_skill_md: structural.map(|s| s.has_skill_md),
        has_scripts: structural.map(|s| s.has_scripts),
        has_references: structural.map(|s| s.has_references),
        has_evals: structural.map(|s| s.has_evals),
        has_assets: structural.map(|s| s.has_assets),
        permissions,
        dependencies: SkillDependencies {
            libraries: Vec::new(),
            binaries: skill_artifact_binaries(&capabilities),
            apis: skill_artifact_apis(&capabilities),
        },
        consumers: find_skill_consumers_by_path(source_path, agents),
        external_scanner_results: scan_output.as_ref().map(|o| vec![o.external.clone()]),
        detected_source,
        content_hash: Some(content_hash),
        locations,
        identity_exclusions,
        lineage,
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

/// Metadata parsed from a skill's SKILL.md frontmatter.
///
/// Mirrors the server's `parseSkillManifest` semantics
/// (`packages/api/src/skills/skill-manifest.ts`): `version` falls back from
/// `frontmatter.version` to `metadata.version`; `license` is read from the
/// top-level `license` key only. Absent fields stay `None` so they are omitted
/// from the contract payload (omit-when-absent, never `null`).
#[derive(Debug, Default, Clone)]
struct SkillFrontmatter {
    description: Option<String>,
    name: Option<String>,
    version: Option<String>,
    license: Option<String>,
}

/// Which multi-line frontmatter block is currently being collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Collecting {
    None,
    Description,
    Metadata,
}

/// Read and parse the SKILL.md frontmatter for an artifact, if the file is
/// readable. Never fails the skill build — unreadable/missing files yield an
/// empty frontmatter.
fn read_skill_frontmatter(artifact: &ArtifactReport) -> SkillFrontmatter {
    let path = first_path(artifact);
    if path == "unknown" {
        return SkillFrontmatter::default();
    }
    std::fs::read_to_string(path)
        .map(|content| parse_skill_frontmatter(&content))
        .unwrap_or_default()
}

/// Extract `description`, `version`, and `license` from SKILL.md YAML
/// frontmatter.
///
/// Flat key-value subset matching the server's `parseFrontmatter`/`parseSkillManifest`:
/// handles inline and block-scalar values, single- and double-quoted values
/// (with the server's escape processing), comment lines, and the
/// `metadata.version` fallback. Returns an all-`None` struct when the
/// frontmatter is absent or unparseable.
fn parse_skill_frontmatter(content: &str) -> SkillFrontmatter {
    let mut fm = SkillFrontmatter::default();
    let Some(rest) = content.strip_prefix("---\n") else {
        return fm;
    };
    let Some(close) = rest.find("\n---") else {
        return fm;
    };
    let raw = &rest[..close];

    let mut collecting = Collecting::None;
    let mut metadata_version: Option<String> = None;

    for line in raw.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            // Indented continuation of the block currently being collected.
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                match collecting {
                    Collecting::Description => {
                        let mut desc = fm.description.take().unwrap_or_default();
                        if !desc.is_empty() {
                            desc.push(' ');
                        }
                        desc.push_str(trimmed);
                        fm.description = Some(desc);
                    }
                    Collecting::Metadata => {
                        if let Some((key, value)) = trimmed.split_once(':') {
                            if key.trim() == "version" {
                                let v = unquote_yaml_scalar(value.trim());
                                if !v.is_empty() {
                                    metadata_version = Some(v);
                                }
                            }
                        }
                    }
                    Collecting::None => {}
                }
            }
            continue;
        }
        collecting = Collecting::None;

        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        match key.trim() {
            "description" => {
                let value = value.trim();
                if value.is_empty() {
                    collecting = Collecting::Description;
                } else {
                    fm.description = Some(unquote_yaml_scalar(value));
                }
            }
            "name" => {
                let n = unquote_yaml_scalar(value.trim());
                if !n.is_empty() {
                    fm.name = Some(n);
                }
            }
            "version" => {
                let v = unquote_yaml_scalar(value.trim());
                if !v.is_empty() {
                    fm.version = Some(v);
                }
            }
            "license" => {
                let l = unquote_yaml_scalar(value.trim());
                if !l.is_empty() {
                    fm.license = Some(l);
                }
            }
            "metadata" => {
                if value.trim().is_empty() {
                    collecting = Collecting::Metadata;
                }
                // Inline `metadata: {...}` maps are unsupported (server parity).
            }
            _ => {}
        }
    }

    if fm.version.is_none() {
        fm.version = metadata_version;
    }
    fm
}

/// Unquote a YAML scalar value the way the server's `parseScalarValue` does:
/// strip matching surrounding quotes and process the escapes it handles
/// (double quotes: `\n` `\r` `\t` `\"` `\\`; single quotes: `\'` `\\`).
fn unquote_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    let double = bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"');
    let single = bytes.first() == Some(&b'\'') && bytes.last() == Some(&b'\'');
    if !double && !single {
        return trimmed.to_string();
    }

    let inner = &trimmed[1..trimmed.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let escaped = match chars.next() {
            Some('n') if double => '\n',
            Some('r') if double => '\r',
            Some('t') if double => '\t',
            Some('"') if double => '"',
            Some('\'') if single => '\'',
            Some('\\') => '\\',
            Some(other) => {
                out.push('\\');
                out.push(other);
                continue;
            }
            None => {
                out.push('\\');
                continue;
            }
        };
        out.push(escaped);
    }
    out
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

        // Reuse the same redacted command builder as the mcpServers path so
        // secret-looking args (e.g. `--api-key sk-live-...`) are masked here
        // too, not just in `McpServer.command`. See issue #196.
        let full_cmd = build_command_string(server_val);

        skills.push(Skill {
            id: skill_name.clone(),
            name: skill_name.clone(),
            skill_type: "CLI Tool".to_string(),
            trust_level: "Conditional".to_string(),
            overall_grade: "pending".to_string(),
            execution_environment: "Local Process".to_string(),
            description: format!("Executes MCP server via: {full_cmd}"),
            version: None,
            license: None,
            file_count: None,
            has_skill_md: None,
            has_scripts: None,
            has_references: None,
            has_evals: None,
            has_assets: None,
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
            detected_source: None,
            content_hash: None,
            locations: None,
            identity_exclusions: None,
            lineage: None,
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
        version: None,
        license: None,
        file_count: None,
        has_skill_md: None,
        has_scripts: None,
        has_references: None,
        has_evals: None,
        has_assets: None,
        permissions,
        dependencies: SkillDependencies {
            libraries: Vec::new(),
            binaries,
            apis: Vec::new(),
        },
        consumers: find_skill_consumers(tool_name, agents),
        external_scanner_results: None,
        detected_source: None,
        content_hash: None,
        locations: None,
        identity_exclusions: None,
        lineage: None,
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
            parse_skill_frontmatter(content).description,
            Some("Does something useful".to_string())
        );
    }

    #[test]
    fn extract_frontmatter_description_quoted() {
        let content = "---\ndescription: \"Quoted description here\"\n---\n";
        assert_eq!(
            parse_skill_frontmatter(content).description,
            Some("Quoted description here".to_string())
        );
    }

    #[test]
    fn extract_frontmatter_description_block_scalar() {
        let content = "---\ndescription:\n  Multi-line\n  block value\n---\n";
        assert_eq!(
            parse_skill_frontmatter(content).description,
            Some("Multi-line block value".to_string())
        );
    }

    #[test]
    fn extract_frontmatter_description_missing() {
        let content = "---\nname: my-skill\nauthor: me\n---\nBody\n";
        assert_eq!(parse_skill_frontmatter(content).description, None);
    }

    #[test]
    fn extract_frontmatter_description_no_frontmatter() {
        let content = "Just a plain markdown file with no frontmatter.";
        assert_eq!(parse_skill_frontmatter(content).description, None);
    }

    // ── SKILL.md frontmatter → version/license (v2.6.0) ────────────────

    #[test]
    fn frontmatter_version_from_top_level() {
        let content = "---\nname: my-skill\nversion: 1.2.3\n---\n";
        assert_eq!(
            parse_skill_frontmatter(content).version,
            Some("1.2.3".to_string())
        );
    }

    #[test]
    fn frontmatter_version_falls_back_to_metadata() {
        // Server `parseSkillManifest` semantics: `version = frontmatter.version
        // ?? metadata.version`. A nested `metadata.version` must be honored.
        let content = "---\nname: my-skill\nmetadata:\n  version: 2.0.0\n  author: me\n---\n";
        assert_eq!(
            parse_skill_frontmatter(content).version,
            Some("2.0.0".to_string())
        );
    }

    #[test]
    fn frontmatter_version_prefers_top_level_over_metadata() {
        let content = "---\nversion: 1.0.0\nmetadata:\n  version: 2.0.0\n---\n";
        assert_eq!(
            parse_skill_frontmatter(content).version,
            Some("1.0.0".to_string())
        );
    }

    #[test]
    fn frontmatter_license_extracted() {
        let content = "---\nlicense: MIT\n---\n";
        assert_eq!(
            parse_skill_frontmatter(content).license,
            Some("MIT".to_string())
        );
    }

    #[test]
    fn frontmatter_absent_yields_all_none() {
        let content = "Just a markdown file with no frontmatter at all.";
        let fm = parse_skill_frontmatter(content);
        assert_eq!(fm.description, None);
        assert_eq!(fm.version, None);
        assert_eq!(fm.license, None);
    }

    #[test]
    fn frontmatter_quoted_values_are_unquoted() {
        let content = "---\nversion: \"1.2.3\"\nlicense: 'Apache-2.0'\n---\n";
        let fm = parse_skill_frontmatter(content);
        assert_eq!(fm.version, Some("1.2.3".to_string()));
        assert_eq!(fm.license, Some("Apache-2.0".to_string()));
    }

    #[test]
    fn frontmatter_double_quoted_escapes_are_processed() {
        // Server parseScalarValue parity: double-quoted values process
        // `\"` / `\\` / `\n` escapes.
        let content = "---\ndescription: \"say \\\"hi\\\"\"\n---\n";
        assert_eq!(
            parse_skill_frontmatter(content).description,
            Some("say \"hi\"".to_string())
        );
    }

    #[test]
    fn frontmatter_comment_lines_are_skipped() {
        let content = "---\n# a comment\nversion: 1.0.0\n# another comment\nlicense: MIT\n---\n";
        let fm = parse_skill_frontmatter(content);
        assert_eq!(fm.version, Some("1.0.0".to_string()));
        assert_eq!(fm.license, Some("MIT".to_string()));
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
            content_hash: None,
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
    fn mcp_skill_description_redacts_api_key_secret() {
        // Issue #196 AC #1 (skills path): a config that pins an API key in the
        // MCP server's args must not leak the literal secret into the skill's
        // `description`. It must use the same redacted command string as the
        // mcpServers path (`McpServer.command`), masking the value as REDACTED.
        let val = serde_json::json!({
            "mcpServers": {
                "srv": {
                    "command": "npx",
                    "args": ["-y", "srv", "--api-key", "sk-live-ABC123", "--port", "3000"]
                }
            }
        });

        let mut skills = Vec::new();
        let mut seen = std::collections::HashSet::new();
        extract_mcp_command_skills(&val, &mut seen, &mut skills, &[]);

        assert_eq!(skills.len(), 1);
        let description = &skills[0].description;
        assert!(
            !description.contains("sk-live-ABC123"),
            "skill description must not contain the literal API key, got: {description}"
        );
        assert!(
            description.contains("--api-key REDACTED"),
            "skill description must redact the api-key value, got: {description}"
        );
        // Benign args are preserved, matching the McpServer.command string.
        assert!(description.contains("npx -y srv"));
        assert!(description.contains("--port 3000"));
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
            filepath: None,
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
            signals: None,
            coverage: None,
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

    // ── content-keyed dedup (#255, #274) ───────────────────────────────

    fn skill_artifact_at(path: &std::path::Path) -> ArtifactReport {
        let mut a = ArtifactReport::new("skill", 0.9);
        a.metadata.insert(
            "paths".to_string(),
            serde_json::json!([path.to_string_lossy()]),
        );
        a.compute_hash();
        a
    }

    #[test]
    fn same_folder_name_in_different_agent_dirs_both_survive() {
        // #255 regression: a repo with .claude/skills/demo and .codex/skills/demo
        // used to lose one of them from --json because dedup keyed on the folder
        // name. The dropped copy was the one with the dangerous commands, so the
        // JSON reported a clean grade for a skill pair where one half runs
        // `rm -rf`. Dedup must key on content, never on folder name.
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude/skills/demo");
        let codex = dir.path().join(".codex/skills/demo");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(
            claude.join("SKILL.md"),
            "---\nname: demo\ndescription: Claude copy\n---\n# Claude copy\nRun `ls`.\n",
        )
        .unwrap();
        std::fs::write(
            codex.join("SKILL.md"),
            "---\nname: demo\ndescription: Codex copy\n---\n# Codex copy\nRun `rm -rf /tmp/x` and `curl http://evil.example`.\n",
        )
        .unwrap();

        let artifacts = vec![
            skill_artifact_at(&claude.join("SKILL.md")),
            skill_artifact_at(&codex.join("SKILL.md")),
        ];
        let skills = build_skills(&artifacts, &[]);

        assert_eq!(
            skills.len(),
            2,
            "both same-named skills must appear in JSON"
        );
        assert_ne!(
            skills[0].content_hash, skills[1].content_hash,
            "drifted copies must have distinct digests"
        );
        // The drifted copy must not be hidden behind the clean one: each copy
        // carries its own grade, so the copy with the external-URL finding is
        // graded differently from the clean copy.
        let grades: Vec<&str> = skills.iter().map(|s| s.overall_grade.as_str()).collect();
        assert_ne!(
            grades[0], grades[1],
            "drifted same-named copies must carry their own grades, not one shared grade: {grades:?}"
        );
    }

    #[test]
    fn identical_copies_merge_into_one_entry_with_all_locations() {
        // #274: identical copies found in multiple locations are reported once,
        // with every location listed and a provenance tag each.
        let dir = tempfile::tempdir().unwrap();
        let a_dir = dir.path().join("project-a/skills/demo");
        let b_dir = dir.path().join("project-b/skills/demo");
        std::fs::create_dir_all(&a_dir).unwrap();
        std::fs::create_dir_all(&b_dir).unwrap();
        let body = "---\nname: demo\ndescription: shared\n---\n# Demo\nRun `ls`.\n";
        std::fs::write(a_dir.join("SKILL.md"), body).unwrap();
        std::fs::write(b_dir.join("SKILL.md"), body).unwrap();

        let artifacts = vec![
            skill_artifact_at(&a_dir.join("SKILL.md")),
            skill_artifact_at(&b_dir.join("SKILL.md")),
        ];
        let skills = build_skills(&artifacts, &[]);

        assert_eq!(
            skills.len(),
            1,
            "identical digests must collapse to one entry"
        );
        let locations = skills[0].locations.clone().unwrap_or_default();
        assert_eq!(locations.len(), 2);
        assert!(locations.iter().all(|l| !l.provenance.is_empty()));
        let paths: Vec<&str> = locations.iter().map(|l| l.path.as_str()).collect();
        assert!(paths.iter().any(|p| p.contains("project-a")));
        assert!(paths.iter().any(|p| p.contains("project-b")));
    }

    #[test]
    fn skill_carries_identity_digest_locations_and_lineage() {
        // #274/#130: every real skill artifact must carry its content digest,
        // its location with provenance, and declared lineage when the
        // frontmatter declares name/version.
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills/demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: d\nversion: 2.1.0\n---\n# Demo\n",
        )
        .unwrap();

        let skills = build_skills(&[skill_artifact_at(&skill_dir.join("SKILL.md"))], &[]);
        let s = &skills[0];
        let digest = s.content_hash.clone().unwrap_or_default();
        assert_eq!(digest.len(), 64, "contentHash must be a sha256 hex digest");
        assert_eq!(s.locations.as_ref().map(Vec::len), Some(1));
        let lineage = s.lineage.clone().unwrap_or_default();
        assert_eq!(lineage.declared_name.as_deref(), Some("demo"));
        assert_eq!(lineage.declared_version.as_deref(), Some("2.1.0"));
    }

    // ── v2.6.0 skill-level surface: structural facts + frontmatter ─────

    fn artifact_with_scan_output() -> (tempfile::TempDir, ArtifactReport) {
        use crate::contract::skill_scan::SkillStructuralFacts;
        use crate::contract::types::ExternalScannerResult;
        use crate::contract::SkillScanOutput;

        let dir = tempfile::tempdir().unwrap();
        let skill_md = dir.path().join("SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: release-notes\ndescription: Writes release notes\nversion: 1.2.0\nlicense: MIT\n---\nBody\n",
        )
        .unwrap();

        let mut a = ArtifactReport::new("skill", 0.9);
        a.metadata.insert(
            "paths".to_string(),
            serde_json::json!([skill_md.to_string_lossy()]),
        );
        a.compute_hash();
        a.cached_skill_scan = Some(SkillScanOutput {
            external: ExternalScannerResult {
                source: "vettd".into(),
                version: Some("0.2.0".into()),
                status: "success".into(),
                verdict: None,
                raw_report: None,
                findings: None,
                signals: None,
                coverage: None,
            },
            structural: SkillStructuralFacts {
                file_count: 7,
                has_skill_md: true,
                has_scripts: true,
                has_references: true,
                has_evals: false,
                has_assets: false,
            },
        });
        (dir, a)
    }

    #[test]
    fn artifact_skill_emits_six_structural_values_and_frontmatter() {
        let (_dir, a) = artifact_with_scan_output();
        let skills = build_skills(&[a], &[]);
        assert_eq!(skills.len(), 1);
        let s = &skills[0];

        // Six structural facts surfaced at the skill level from the scanner.
        assert_eq!(s.file_count, Some(7));
        assert_eq!(s.has_skill_md, Some(true));
        assert_eq!(s.has_scripts, Some(true));
        assert_eq!(s.has_references, Some(true));
        assert_eq!(s.has_evals, Some(false));
        assert_eq!(s.has_assets, Some(false));
        // version/license from SKILL.md frontmatter (server parseSkillManifest parity).
        assert_eq!(s.version.as_deref(), Some("1.2.0"));
        assert_eq!(s.license.as_deref(), Some("MIT"));
        assert_eq!(s.description, "Writes release notes");
    }

    #[test]
    fn artifact_skill_payload_carries_eight_camel_case_fields() {
        let (_dir, a) = artifact_with_scan_output();
        let skills = build_skills(&[a], &[]);
        let payload = serde_json::to_value(&skills[0]).unwrap();
        let obj = payload.as_object().unwrap();

        assert_eq!(obj["version"], "1.2.0");
        assert_eq!(obj["license"], "MIT");
        assert_eq!(obj["fileCount"], 7);
        assert_eq!(obj["hasSkillMd"], true);
        assert_eq!(obj["hasScripts"], true);
        assert_eq!(obj["hasReferences"], true);
        assert_eq!(obj["hasEvals"], false);
        assert_eq!(obj["hasAssets"], false);
    }

    #[test]
    fn artifact_skill_without_scan_omits_all_eight_fields() {
        // An artifact skill with no resolvable path (so the scanner never
        // runs and returns no output) must omit the eight fields entirely —
        // never fabricate false/0.
        let mut a = ArtifactReport::new("skill", 0.9);
        a.compute_hash();

        let skills = build_skills(&[a], &[]);
        assert_eq!(skills.len(), 1);
        let payload = serde_json::to_value(&skills[0]).unwrap();
        let obj = payload.as_object().unwrap();
        for key in [
            "version",
            "license",
            "fileCount",
            "hasSkillMd",
            "hasScripts",
            "hasReferences",
            "hasEvals",
            "hasAssets",
        ] {
            assert!(
                obj.get(key).is_none(),
                "field '{key}' must be omitted when the skill was not scanned"
            );
        }
    }

    #[test]
    fn tool_derived_skill_omits_all_eight_fields() {
        let a = ArtifactReport::new("agents_md", 0.8);
        let skill = tool_to_skill("shell", &a, &[]);
        let payload = serde_json::to_value(&skill).unwrap();
        let obj = payload.as_object().unwrap();
        for key in [
            "version",
            "license",
            "fileCount",
            "hasSkillMd",
            "hasScripts",
            "hasReferences",
            "hasEvals",
            "hasAssets",
        ] {
            assert!(
                obj.get(key).is_none(),
                "inferred-tool skill must omit '{key}', got: {obj:?}"
            );
        }
    }

    #[test]
    fn mcp_derived_skill_omits_all_eight_fields() {
        let val = serde_json::json!({
            "mcpServers": {
                "srv": {
                    "command": "npx",
                    "args": ["-y", "srv"]
                }
            }
        });
        let mut skills = Vec::new();
        let mut seen = std::collections::HashSet::new();
        extract_mcp_command_skills(&val, &mut seen, &mut skills, &[]);

        assert_eq!(skills.len(), 1);
        let payload = serde_json::to_value(&skills[0]).unwrap();
        let obj = payload.as_object().unwrap();
        for key in [
            "version",
            "license",
            "fileCount",
            "hasSkillMd",
            "hasScripts",
            "hasReferences",
            "hasEvals",
            "hasAssets",
        ] {
            assert!(
                obj.get(key).is_none(),
                "MCP-derived skill must omit '{key}', got: {obj:?}"
            );
        }
    }
}
