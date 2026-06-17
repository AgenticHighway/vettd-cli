//! Transforms a [`ScanReport`] into the scanner data contract (v2).
//!
//! The contract defines the exact payload shape the ingestion endpoint
//! expects:  `scanMeta`, `prompts`, `skills`, `mcpServers`, `agents`,
//! and `agenticApps`.

mod agents;
mod apps;
mod helpers;
mod mcp;
mod prompts;
mod skills;
pub mod types;

pub use types::*;

use crate::models::ScanReport;
use crate::network_evidence;

pub fn build_contract_payload(report: &ScanReport, scan_duration_ms: u64) -> ContractPayload {
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    let host_network = network_evidence::gather_host_network();

    let scan_meta = ScanMeta {
        scan_id: uuid::Uuid::new_v4().to_string(),
        endpoint_hostname: hostname,
        scanned_at: report.timestamp.clone(),
        scanner_version: env!("CARGO_PKG_VERSION").to_string(),
        scan_duration_ms,
        scan_roots: vec![report.scanned_path.clone()],
        host_network,
    };

    // Partition artifacts by type
    let (prompt_artifacts, mcp_artifacts, container_artifacts, agent_artifacts) =
        partition_artifacts(report);

    let prompts_out = prompts::build_prompts(&prompt_artifacts);
    let agents_out = agents::build_agents(&agent_artifacts, &mcp_artifacts);
    let skills_out = skills::build_skills(&report.artifacts, &agents_out);
    let agentic_apps = apps::build_agentic_apps(&container_artifacts, &agents_out);

    let mcp_servers = build_mcp_with_links(&mcp_artifacts, &agents_out);

    ContractPayload {
        scan_meta,
        prompts: prompts_out,
        skills: skills_out,
        mcp_servers,
        agents: agents_out,
        agentic_apps,
    }
}

type ArtifactPartition<'a> = (
    Vec<&'a crate::models::ArtifactReport>,
    Vec<&'a crate::models::ArtifactReport>,
    Vec<&'a crate::models::ArtifactReport>,
    Vec<&'a crate::models::ArtifactReport>,
);

fn partition_artifacts(report: &ScanReport) -> ArtifactPartition<'_> {
    let mut prompts = Vec::new();
    let mut mcps = Vec::new();
    let mut containers = Vec::new();
    let mut agents = Vec::new();

    for artifact in &report.artifacts {
        match artifact.artifact_type.as_str() {
            "cursor_rules" | "prompt_config" => prompts.push(artifact),
            "agents_md" => {
                prompts.push(artifact);
                agents.push(artifact);
            }
            "mcp_config" => mcps.push(artifact),
            "container_config" | "container_candidate" => containers.push(artifact),
            _ => {}
        }
    }

    (prompts, mcps, containers, agents)
}

fn build_mcp_with_links(
    mcp_artifacts: &[&crate::models::ArtifactReport],
    agents_out: &[Agent],
) -> Vec<McpServer> {
    // Map: MCP server name → agent IDs that reference it
    let mut agent_ids_by_mcp: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for agent in agents_out {
        for tool in &agent.tools {
            if tool.tool_type == "mcp" {
                agent_ids_by_mcp
                    .entry(tool.name.clone())
                    .or_default()
                    .push(agent.id.clone());
            }
        }
    }

    let mut servers = mcp::build_mcp_servers(mcp_artifacts);
    for server in &mut servers {
        if let Some(ids) = agent_ids_by_mcp.get(&server.name) {
            let mut seen = std::collections::HashSet::new();
            server.dependent_agents = ids
                .iter()
                .filter(|id| seen.insert((*id).clone()))
                .cloned()
                .collect();
        }
    }
    servers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ArtifactReport, ScanReport};

    fn make_artifact(atype: &str) -> ArtifactReport {
        ArtifactReport::new(atype, 0.8)
    }

    fn make_report(artifacts: Vec<ArtifactReport>) -> ScanReport {
        ScanReport {
            scanned_path: "/tmp/test".to_string(),
            run_id: "test-run".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            artifacts,
        }
    }

    #[test]
    fn partition_prompt_config() {
        let report = make_report(vec![make_artifact("prompt_config")]);
        let (prompts, mcps, containers, agents) = partition_artifacts(&report);
        assert_eq!(prompts.len(), 1);
        assert!(mcps.is_empty());
        assert!(containers.is_empty());
        assert!(agents.is_empty());
    }

    #[test]
    fn partition_cursor_rules_as_prompt() {
        let report = make_report(vec![make_artifact("cursor_rules")]);
        let (prompts, _, _, agents) = partition_artifacts(&report);
        assert_eq!(prompts.len(), 1);
        assert!(agents.is_empty());
    }

    #[test]
    fn partition_agents_md_goes_to_both() {
        let report = make_report(vec![make_artifact("agents_md")]);
        let (prompts, _, _, agents) = partition_artifacts(&report);
        assert_eq!(prompts.len(), 1);
        assert_eq!(agents.len(), 1);
    }

    #[test]
    fn partition_mcp_config() {
        let report = make_report(vec![make_artifact("mcp_config")]);
        let (prompts, mcps, _, _) = partition_artifacts(&report);
        assert!(prompts.is_empty());
        assert_eq!(mcps.len(), 1);
    }

    #[test]
    fn partition_container_types() {
        let report = make_report(vec![
            make_artifact("container_config"),
            make_artifact("container_candidate"),
        ]);
        let (_, _, containers, _) = partition_artifacts(&report);
        assert_eq!(containers.len(), 2);
    }

    #[test]
    fn partition_unknown_type_ignored() {
        let report = make_report(vec![make_artifact("unknown_type")]);
        let (prompts, mcps, containers, agents) = partition_artifacts(&report);
        assert!(prompts.is_empty());
        assert!(mcps.is_empty());
        assert!(containers.is_empty());
        assert!(agents.is_empty());
    }

    #[test]
    fn partition_mixed_artifacts() {
        let report = make_report(vec![
            make_artifact("prompt_config"),
            make_artifact("agents_md"),
            make_artifact("mcp_config"),
            make_artifact("container_config"),
            make_artifact("browser_footprint"),
        ]);
        let (prompts, mcps, containers, agents) = partition_artifacts(&report);
        assert_eq!(prompts.len(), 2); // prompt_config + agents_md
        assert_eq!(mcps.len(), 1);
        assert_eq!(containers.len(), 1);
        assert_eq!(agents.len(), 1);
    }
}
