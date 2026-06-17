//! Data types for the scanner data contract (v2).

use serde::{Deserialize, Serialize};

use crate::network_evidence::{EnvVarRef, HostNetworkInfo, NetworkEvidence};

// ═══════════════════════════════════════════════════════════════════════════
// Top-level payload
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractPayload {
    pub scan_meta: ScanMeta,
    pub prompts: Vec<Prompt>,
    pub skills: Vec<Skill>,
    pub mcp_servers: Vec<McpServer>,
    pub agents: Vec<Agent>,
    pub agentic_apps: Vec<AgenticApp>,
}

// ═══════════════════════════════════════════════════════════════════════════
// scanMeta
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanMeta {
    pub scan_id: String,
    pub endpoint_hostname: String,
    pub scanned_at: String,
    pub scanner_version: String,
    pub scan_duration_ms: u64,
    pub scan_roots: Vec<String>,
    pub host_network: HostNetworkInfo,
}

// ═══════════════════════════════════════════════════════════════════════════
// prompts
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    pub id: String,
    pub name: String,
    pub source_file_path: String,
    pub classification: String,
    pub tokens: u64,
    pub content_hash: String,
    pub last_changed_date: String,
    pub capabilities: Vec<PromptCapability>,
    pub secret_refs: Vec<SecretRef>,
    pub injection_surfaces: Vec<InjectionSurface>,
    pub dependencies: Vec<String>,
    pub risk_score: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCapability {
    pub text: String,
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRef {
    pub label: String,
    pub detail: String,
    pub tone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionSurface {
    pub text: String,
    pub severity: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// skills
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub skill_type: String,
    pub trust_level: String,
    pub execution_environment: String,
    pub description: String,
    pub permissions: Vec<SkillPermission>,
    pub dependencies: SkillDependencies,
    pub consumers: Vec<SkillConsumer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPermission {
    pub name: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDependencies {
    pub libraries: Vec<String>,
    pub binaries: Vec<String>,
    pub apis: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConsumer {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub consumer_type: String,
    pub invocations: u64,
}

// ═══════════════════════════════════════════════════════════════════════════
// mcpServers
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub network: String,
    pub auth: String,
    pub verified: bool,
    pub command: String,
    pub tools: Vec<McpTool>,
    pub dependent_agents: Vec<String>,
    pub network_evidence: Vec<NetworkEvidence>,
    pub env_vars: Vec<EnvVarRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub risk: String,
    pub description: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// agents
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub source_file_path: String,
    pub classification: String,
    pub execution_model: String,
    pub trust_score: i32,
    pub version: String,
    pub author: String,
    pub source_repo: String,
    pub capabilities: Vec<AgentCapability>,
    pub tools: Vec<AgentTool>,
    pub trust_breakdown: Vec<TrustFactor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapability {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTool {
    pub name: String,
    #[serde(rename = "type")]
    pub tool_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustFactor {
    pub label: String,
    pub delta: i32,
}

// ═══════════════════════════════════════════════════════════════════════════
// agenticApps
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgenticApp {
    pub id: String,
    pub name: String,
    pub source_file_path: String,
    pub framework: String,
    pub agent_count: u32,
    pub risk: String,
    pub review_status: String,
    pub description: String,
    pub agents: Vec<AppAgent>,
    pub tools_by_agent: Vec<Vec<String>>,
    pub workflow: Vec<WorkflowStep>,
    pub integrations: Vec<Integration>,
    pub verification_checks: Vec<String>,
    pub risk_tags: Vec<String>,
    pub risk_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppAgent {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub step: u32,
    pub agent: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Integration {
    pub name: String,
    #[serde(rename = "type")]
    pub integration_type: String,
    pub risk: String,
}
