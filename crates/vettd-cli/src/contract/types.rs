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
#[serde(tag = "status", rename_all = "camelCase")]
pub enum DetectedSkillSource {
    #[serde(rename_all = "camelCase")]
    GitHub {
        repo_url: String,
        branch: String,
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    UnsupportedRemote { remote_url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub skill_type: String,
    pub trust_level: String,
    pub overall_grade: String,
    pub execution_environment: String,
    pub description: String,
    // v2.6.0 addition — skill-level metadata surfaced from the skill scanner
    // and SKILL.md frontmatter (see scanner-field-gate.json). Optional and
    // omitted when absent; never fabricated as `false`/`0` for assets that
    // were not scanned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_skill_md: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_scripts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_references: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_evals: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_assets: Option<bool>,
    pub permissions: Vec<SkillPermission>,
    pub dependencies: SkillDependencies,
    pub consumers: Vec<SkillConsumer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_scanner_results: Option<Vec<ExternalScannerResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_source: Option<DetectedSkillSource>,
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

// v2.2.0 addition — optional external scanner results attached to a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalScannerResult {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_report: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub findings: Option<Vec<ExternalScannerFinding>>,
    /// Non-finding signals emitted by the scanner (display-only). Never mapped
    /// into `ExternalScannerFinding` or the local grade — see `skill_scan.rs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signals: Option<Vec<ScannerSignal>>,
    /// Scan coverage / attestation entries (display-only). See `skill_scan.rs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Vec<ScannerCoverage>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalScannerFinding {
    pub rule_id: String,
    pub category: String,
    pub severity: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

// v2.5.0 addition — scanner signal/coverage output surfaced additively.
// Shapes mirror the vettd-skill-scanner `Signal`/`CoverageEntry` wire format
// (camelCase, open strings, optional fields omitted when absent).

/// A single non-finding signal produced by the skill scanner for one asset.
/// Display-only: never feeds grade or verdict computation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannerSignal {
    pub data_category: String,
    pub source_class: String,
    pub rule_id: String,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_num: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_size: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub synthetic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Map<String, serde_json::Value>>,
}

/// One scan coverage / attestation entry, kept separate from findings and
/// signals. Display-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannerCoverage {
    pub kind: String,
    pub rule_id: String,
    pub label: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
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
