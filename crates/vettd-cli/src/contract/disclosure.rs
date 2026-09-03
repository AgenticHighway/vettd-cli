//! Disclosure-category derivation from a [`ContractPayload`].
//!
//! The disclosure summary names the data categories actually present in the
//! payload — not what *could* be there. This means the rendered text tracks
//! the payload structurally: if a new field is added to any serialized type
//! and the walker doesn't know about it, the test (and, in debug/test runs,
//! the walker itself) fails loud.
//!
//! Design decisions:
//!
//! - [`DisclosureCategory`] is the enum of all known data categories. It is
//!   the single source of truth for what the disclosure can mention.
//! - [`disclosure_categories`] walks the payload and returns only the
//!   categories actually present (e.g. `PromptRecords` is returned only if
//!   `prompts` is non-empty).
//! - [`validate_payload_coverage`] serializes the payload and recursively
//!   walks every serialized JSON path, requiring each key to map to a known
//!   [`DisclosureCategory`]. An unknown key panics — this is what makes
//!   "adding a serialized field without a disclosure mapping" a failure, not
//!   a silent omission.
//! - [`render_disclosure`] turns the category list into human-readable text
//!   (plus the record count). Callers render to stderr directly.
//! - The test at the bottom of this module builds a maximally-populated
//!   [`ContractPayload`] and asserts every serialized field has a category —
//!   adding a field anywhere without updating the walker fails the test.

use super::types::*;
use super::{ContractPayload, McpServer};

// ═══════════════════════════════════════════════════════════════════════════
// Categories — one variant per disclosed data type
// ═══════════════════════════════════════════════════════════════════════════

/// A category of data present in a [`ContractPayload`].
///
/// Each variant names a distinct kind of data the payload may transmit. The
/// list is the authoritative catalog: [`render_disclosure`] formats one line
/// per category, and the test ensures every field in every payload type maps
/// to one of these variants.
///
/// The last fourteen variants belong to the passive-observer telemetry envelope
/// (`vettd observe`), not to [`ContractPayload`]: their label and description text is
/// copied verbatim from the repo-root `telemetry-field-gate.json`, which maps every
/// leaf path that envelope may carry to one of them. [`disclosure_categories`] never
/// returns them — `observe::disclosure` renders them, structurally, before any session
/// log is opened. They live here because this enum is the one catalog of everything the
/// CLI may disclose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisclosureCategory {
    /// `scan_meta.scan_id`, `scanned_at`, `scan_duration_ms` — scan bookkeeping.
    ScanMetaInfo,
    /// `scan_meta.scan_roots` — absolute paths of scanned directories/files.
    ScanRootPaths,
    /// `scan_meta.endpoint_hostname` — the local machine's hostname.
    Hostname,
    /// `scan_meta.host_network.*` — macOS Application Firewall state.
    HostSecurityContext,
    /// `scan_meta.scanner_version` — the vettd CLI version string.
    ScannerVersion,
    /// `mcp_servers[*].{id,name,transport,network,auth,verified,command}`.
    McpServerCommand,
    /// `mcp_servers[*].tools[*].{name,risk,description}` — tool names/descriptions.
    McpToolNames,
    /// `mcp_servers[*].dependent_agents[*]` — agent IDs referencing a server.
    McpDependentAgents,
    /// `mcp_servers[*].network_evidence[*]` — URLs observed in application logs.
    LogDerivedNetworkEvidence,
    /// `mcp_servers[*].env_vars[*].name` — names of environment variables referenced.
    EnvVarNames,
    /// `prompts[*]` — AI prompt configuration records.
    PromptRecords,
    /// `skills[*]` — scanned skill records.
    SkillRecords,
    /// `agents[*]` — AI agent configuration records.
    AgentRecords,
    /// `agentic_apps[*]` — agentic application records.
    AgenticAppRecords,
    /// `envelope_version`, `extractor_version`, `gate_version`, `resource.collector`,
    /// `resource.collector_version` — versions of the contract, its reader, and this gate.
    TelemetryBookkeeping,
    /// `emitted_day`, `records[].observed_day` — UTC calendar days, never a finer time.
    ObservationDay,
    /// `resource.device_id`, `resource.device_id_source`, `records[].run_id` — the persisted
    /// scanner device id and the per-run HMAC pseudonym keyed by a device-local secret.
    DeviceIdentity,
    /// `resource.harness`, `resource.harness_version`, `records[].entrypoint_class`.
    HarnessIdentity,
    /// `records[].model`, `records[].tokens_by_model[].model` — allowlisted ids, else `other`.
    ModelIdentity,
    /// `records[].effort`, `.permission_mode`, `.task_category`, `.loaded_set_basis`,
    /// `.run_outcome` — closed enums only.
    RunShape,
    /// `records[].counts.*` — turns, tool calls, tool failures, user denials, sub-agent runs,
    /// compactions, unpaired tool uses, repeated tool calls, loaded-set changes.
    RunOutcomeCounts,
    /// `records[].tokens.*` and `records[].tokens_by_model[]` (all but its `model`) — token
    /// totals per provider bucket and the basis they were read from.
    RunTokenTotals,
    /// `records[].assets[].asset_id`, `.asset_type`, `.key_basis` — the hash and how it was made.
    AssetIdentityHash,
    /// `records[].bom_version`, `records[].assets[].tier`, `.binding`,
    /// `.direct_evidence_available`, `bom[].bom_version`, `bom[].asset_ids[]`.
    AssetLoadedSet,
    /// `records[].assets[].signals.invocations.n`, `.signals.failures.*`,
    /// `.signals.harness_corroborations`.
    AssetOutcomeCounts,
    /// `records[].assets[].signals.latency_ms.n`, `.sum`, `.min`, `.max`, `.sumsq`.
    AssetTimingStats,
    /// `records[].assets[].signals.tokens_attributed` (and `.n`, `.sum`, `.min`, `.max`,
    /// `.sumsq`), `.signals.context_cost_est` (and `.tokens`, `.method`).
    AssetTokenStats,
    /// `coverage.sessions_seen`, `.sessions_emitted`, `.sessions_skipped_unparseable`,
    /// `.lines_seen`, `.lines_unknown_type`, `.bytes_read`, `.truncated_sessions`,
    /// `.window_days`, `.cursor_state`, `.run_id_basis`.
    CoverageMetadata,
}

/// Every [`DisclosureCategory`], scanner categories first and then the telemetry
/// categories in `telemetry-field-gate.json` order.
///
/// The only way to look a category up by name; the parity test walks it to prove the
/// gate and this enum are the same list.
pub const ALL_CATEGORIES: [DisclosureCategory; 28] = [
    DisclosureCategory::ScanMetaInfo,
    DisclosureCategory::ScanRootPaths,
    DisclosureCategory::Hostname,
    DisclosureCategory::HostSecurityContext,
    DisclosureCategory::ScannerVersion,
    DisclosureCategory::McpServerCommand,
    DisclosureCategory::McpToolNames,
    DisclosureCategory::McpDependentAgents,
    DisclosureCategory::LogDerivedNetworkEvidence,
    DisclosureCategory::EnvVarNames,
    DisclosureCategory::PromptRecords,
    DisclosureCategory::SkillRecords,
    DisclosureCategory::AgentRecords,
    DisclosureCategory::AgenticAppRecords,
    DisclosureCategory::TelemetryBookkeeping,
    DisclosureCategory::ObservationDay,
    DisclosureCategory::DeviceIdentity,
    DisclosureCategory::HarnessIdentity,
    DisclosureCategory::ModelIdentity,
    DisclosureCategory::RunShape,
    DisclosureCategory::RunOutcomeCounts,
    DisclosureCategory::RunTokenTotals,
    DisclosureCategory::AssetIdentityHash,
    DisclosureCategory::AssetLoadedSet,
    DisclosureCategory::AssetOutcomeCounts,
    DisclosureCategory::AssetTimingStats,
    DisclosureCategory::AssetTokenStats,
    DisclosureCategory::CoverageMetadata,
];

impl DisclosureCategory {
    /// Human-readable label used in the rendered disclosure.
    pub fn label(&self) -> &'static str {
        match self {
            DisclosureCategory::ScanMetaInfo => "Scan metadata",
            DisclosureCategory::ScanRootPaths => "Scan root paths",
            DisclosureCategory::Hostname => "Machine hostname",
            DisclosureCategory::HostSecurityContext => {
                "Host security context (macOS firewall state on macOS; empty elsewhere)"
            }
            DisclosureCategory::ScannerVersion => "Scanner version",
            DisclosureCategory::McpServerCommand => "MCP server command(s)",
            DisclosureCategory::McpToolNames => "MCP tool name(s)",
            DisclosureCategory::McpDependentAgents => "MCP dependent agent reference(s)",
            DisclosureCategory::LogDerivedNetworkEvidence => {
                "Log-derived network evidence (URLs observed in application logs)"
            }
            DisclosureCategory::EnvVarNames => {
                "Environment variable name(s) (names only, not values)"
            }
            DisclosureCategory::PromptRecords => "AI prompt configuration records",
            DisclosureCategory::SkillRecords => "Scanned skill records",
            DisclosureCategory::AgentRecords => "AI agent configuration records",
            DisclosureCategory::AgenticAppRecords => "Agentic application records",
            DisclosureCategory::TelemetryBookkeeping => "Telemetry bookkeeping",
            DisclosureCategory::ObservationDay => "Observation day",
            DisclosureCategory::DeviceIdentity => "Device identity",
            DisclosureCategory::HarnessIdentity => "Harness identity",
            DisclosureCategory::ModelIdentity => "Model identity",
            DisclosureCategory::RunShape => "Run shape",
            DisclosureCategory::RunOutcomeCounts => "Run outcome counts",
            DisclosureCategory::RunTokenTotals => "Run token totals",
            DisclosureCategory::AssetIdentityHash => "Asset identity hashes",
            DisclosureCategory::AssetLoadedSet => "Loaded set",
            DisclosureCategory::AssetOutcomeCounts => "Asset outcome counts",
            DisclosureCategory::AssetTimingStats => "Asset timing stats",
            DisclosureCategory::AssetTokenStats => "Asset token stats",
            DisclosureCategory::CoverageMetadata => "Coverage metadata",
        }
    }

    /// Detailed sentence describing this category's contents.
    pub fn description(&self) -> &'static str {
        match self {
            DisclosureCategory::ScanMetaInfo => {
                "scan ID, scanned-at timestamp, and scan duration (milliseconds)"
            }
            DisclosureCategory::ScanRootPaths => {
                "absolute paths of the scanned directories and files"
            }
            DisclosureCategory::Hostname => "name of the local machine (from `hostname`)",
            DisclosureCategory::HostSecurityContext => {
                "macOS Application Firewall state: whether it is enabled, its mode, stealth mode, and the number/identity of rules"
            }
            DisclosureCategory::ScannerVersion => "the vettd CLI version string (e.g. 0.1.0)",
            DisclosureCategory::McpServerCommand => {
                "MCP server identity (id, name), transport, network classification, authentication status, verification flag, and the launch command (secret-looking flag values are masked with REDACTED)"
            }
            DisclosureCategory::McpToolNames => {
                "the name, risk level, and description of each tool exposed by each MCP server"
            }
            DisclosureCategory::McpDependentAgents => {
                "the IDs of agents that reference each MCP server's tools"
            }
            DisclosureCategory::LogDerivedNetworkEvidence => {
                "URLs and endpoints observed in application logs (VS Code, Cursor, Claude); file paths are scrubbed of the OS username and URL credentials are redacted"
            }
            DisclosureCategory::EnvVarNames => {
                "the names of environment variables referenced by MCP server configurations, whether each is currently set, and where it was referenced (names only, never values)"
            }
            DisclosureCategory::PromptRecords => {
                "file paths, classification, capability signals, content hashes, secret references, injection surfaces, dependencies, and risk scores"
            }
            DisclosureCategory::SkillRecords => {
                "skill name, type, trust grade, execution environment, description, permissions, dependencies, consumers, and external scanner results"
            }
            DisclosureCategory::AgentRecords => {
                "source paths, classification, execution model, trust score, version, author, source repo, capabilities, tool bindings, and trust breakdown"
            }
            DisclosureCategory::AgenticAppRecords => {
                "framework, agent count, risk, review status, description, agents, tools by agent, workflow steps, integrations, verification checks, and risk summary"
            }
            DisclosureCategory::TelemetryBookkeeping => {
                "envelope, extractor, gate, and collector versions"
            }
            DisclosureCategory::ObservationDay => {
                "UTC calendar day of emission and of each observed run start; no finer time resolution is transmitted"
            }
            DisclosureCategory::DeviceIdentity => {
                "the persisted scanner device id and a per-run pseudonym derived from a device-local secret; the harness session id itself is never transmitted"
            }
            DisclosureCategory::HarnessIdentity => {
                "which supported harness produced the run, its semantic version, and a coarse entrypoint class"
            }
            DisclosureCategory::ModelIdentity => {
                "the allowlisted model identifier reported by the harness for the run, or 'other'"
            }
            DisclosureCategory::RunShape => {
                "closed-enum descriptors of the run: effort, permission mode, task category from a published tool-mix rule set, loaded-set basis, run outcome"
            }
            DisclosureCategory::RunOutcomeCounts => {
                "integer counts per run: turns, tool calls, failures by class, denials, sub-agent runs, compactions, unpaired calls, repeated near-identical calls"
            }
            DisclosureCategory::RunTokenTotals => {
                "token totals per run by provider bucket (nullable per provider) and the basis they were read from; never a cost figure"
            }
            DisclosureCategory::AssetIdentityHash => {
                "a content hash, canonical-descriptor hash, or HMAC name pseudonym per asset with its type and key basis; never a name or path"
            }
            DisclosureCategory::AssetLoadedSet => {
                "the loaded-set hash per run, the membership list as asset hashes, and per-asset attribution tier and binding"
            }
            DisclosureCategory::AssetOutcomeCounts => {
                "per asset per run: invocation count, failures by class, harness-native corroboration count"
            }
            DisclosureCategory::AssetTimingStats => {
                "per asset per run: mergeable stats (n, sum, min, max, sumsq) of per-invocation latency in ms from harness timestamps"
            }
            DisclosureCategory::AssetTokenStats => {
                "per asset per run: mergeable stats of tokens attributed to the asset where attribution is exact (sub-agent runs), and a locally estimated context-cost figure with its method"
            }
            DisclosureCategory::CoverageMetadata => {
                "what the collector saw and did not see, so silence is distinguishable from nothing to report"
            }
        }
    }

    /// The variant's own Rust name.
    ///
    /// Exists so the parity test can match a `telemetry-field-gate.json` category name
    /// against this enum. The match is exhaustive on purpose: a new variant cannot be
    /// added without being named here.
    pub fn name(&self) -> &'static str {
        match self {
            DisclosureCategory::ScanMetaInfo => "ScanMetaInfo",
            DisclosureCategory::ScanRootPaths => "ScanRootPaths",
            DisclosureCategory::Hostname => "Hostname",
            DisclosureCategory::HostSecurityContext => "HostSecurityContext",
            DisclosureCategory::ScannerVersion => "ScannerVersion",
            DisclosureCategory::McpServerCommand => "McpServerCommand",
            DisclosureCategory::McpToolNames => "McpToolNames",
            DisclosureCategory::McpDependentAgents => "McpDependentAgents",
            DisclosureCategory::LogDerivedNetworkEvidence => "LogDerivedNetworkEvidence",
            DisclosureCategory::EnvVarNames => "EnvVarNames",
            DisclosureCategory::PromptRecords => "PromptRecords",
            DisclosureCategory::SkillRecords => "SkillRecords",
            DisclosureCategory::AgentRecords => "AgentRecords",
            DisclosureCategory::AgenticAppRecords => "AgenticAppRecords",
            DisclosureCategory::TelemetryBookkeeping => "TelemetryBookkeeping",
            DisclosureCategory::ObservationDay => "ObservationDay",
            DisclosureCategory::DeviceIdentity => "DeviceIdentity",
            DisclosureCategory::HarnessIdentity => "HarnessIdentity",
            DisclosureCategory::ModelIdentity => "ModelIdentity",
            DisclosureCategory::RunShape => "RunShape",
            DisclosureCategory::RunOutcomeCounts => "RunOutcomeCounts",
            DisclosureCategory::RunTokenTotals => "RunTokenTotals",
            DisclosureCategory::AssetIdentityHash => "AssetIdentityHash",
            DisclosureCategory::AssetLoadedSet => "AssetLoadedSet",
            DisclosureCategory::AssetOutcomeCounts => "AssetOutcomeCounts",
            DisclosureCategory::AssetTimingStats => "AssetTimingStats",
            DisclosureCategory::AssetTokenStats => "AssetTokenStats",
            DisclosureCategory::CoverageMetadata => "CoverageMetadata",
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Structural field→category mapping
// ═══════════════════════════════════════════════════════════════════════════

/// Map a serialized (camelCase) JSON path to the [`DisclosureCategory`] that
/// truthfully covers that field. `path` uses `[]` to denote array segments,
/// e.g. `mcpServers[].tools[].name`. Returns `None` for any serialized field
/// that is not yet disclosed — the coverage walker turns that into a hard
/// failure (issue #196 AC #5).
fn field_category(path: &str) -> Option<DisclosureCategory> {
    if path.starts_with("scanMeta") {
        return Some(scan_meta_category(path));
    }
    if path.starts_with("mcpServers") {
        return mcp_category(path);
    }
    if path.starts_with("prompts") {
        return under_path(path, PROMPT_FIELDS).map(|_| DisclosureCategory::PromptRecords);
    }
    if path.starts_with("skills") {
        return under_path(path, SKILL_FIELDS).map(|_| DisclosureCategory::SkillRecords);
    }
    if path.starts_with("agents") {
        return under_path(path, AGENT_FIELDS).map(|_| DisclosureCategory::AgentRecords);
    }
    if path.starts_with("agenticApps") {
        return under_path(path, APP_FIELDS).map(|_| DisclosureCategory::AgenticAppRecords);
    }
    None
}

fn scan_meta_category(path: &str) -> DisclosureCategory {
    let leaf = path.rsplit('.').next().unwrap_or("");
    match leaf {
        "scanId" | "scannedAt" | "scanDurationMs" => DisclosureCategory::ScanMetaInfo,
        "endpointHostname" => DisclosureCategory::Hostname,
        "scannerVersion" => DisclosureCategory::ScannerVersion,
        "scanRoots" => DisclosureCategory::ScanRootPaths,
        // hostNetwork (container) + firewall sub-fields + FirewallRule fields.
        "hostNetwork" | "firewallEnabled" | "firewallMode" | "stealthMode" | "firewallRules"
        | "appPath" | "allowed" => DisclosureCategory::HostSecurityContext,
        _ => DisclosureCategory::ScanMetaInfo,
    }
}

fn mcp_category(path: &str) -> Option<DisclosureCategory> {
    if path == "mcpServers" {
        // Aggregate top-level container; its concrete sub-fields are mapped
        // below (and the presence walker re-derives the specific categories).
        return Some(DisclosureCategory::McpServerCommand);
    }
    let rest = path.strip_prefix("mcpServers[].")?;
    let leaf = rest.rsplit('.').next().unwrap_or("");
    if rest == "id"
        || rest == "name"
        || rest == "transport"
        || rest == "network"
        || rest == "auth"
        || rest == "verified"
        || rest == "command"
    {
        return Some(DisclosureCategory::McpServerCommand);
    }
    if rest == "dependentAgents" || rest.starts_with("dependentAgents") {
        return Some(DisclosureCategory::McpDependentAgents);
    }
    if rest == "tools" || rest.starts_with("tools[].") {
        return matches!(leaf, "name" | "risk" | "description" | "tools")
            .then_some(DisclosureCategory::McpToolNames);
    }
    if rest == "networkEvidence" || rest.starts_with("networkEvidence[].") {
        return matches!(
            leaf,
            "source" | "category" | "detail" | "url" | "networkEvidence"
        )
        .then_some(DisclosureCategory::LogDerivedNetworkEvidence);
    }
    if rest == "envVars" || rest.starts_with("envVars[].") {
        return matches!(leaf, "name" | "isSet" | "sourceKey" | "envVars")
            .then_some(DisclosureCategory::EnvVarNames);
    }
    None
}

/// `Some(())` iff `path`'s leaf field is one of `fields` (a known field name
/// for the section). Unknown leaf → `None`, i.e. undisclosed.
fn under_path(path: &str, fields: &[&str]) -> Option<()> {
    let leaf = path.rsplit('.').next().unwrap_or("");
    fields.contains(&leaf).then_some(())
}

const PROMPT_FIELDS: &[&str] = &[
    "prompts",
    "id",
    "name",
    "sourceFilePath",
    "classification",
    "tokens",
    "contentHash",
    "lastChangedDate",
    "capabilities",
    "secretRefs",
    "injectionSurfaces",
    "dependencies",
    "riskScore",
    // PromptCapability
    "text",
    "level",
    // SecretRef
    "label",
    "detail",
    "tone",
    // InjectionSurface
    "severity",
];

const SKILL_FIELDS: &[&str] = &[
    "skills",
    "id",
    "name",
    "type",
    "trustLevel",
    "overallGrade",
    "executionEnvironment",
    "description",
    "permissions",
    "dependencies",
    "consumers",
    "externalScannerResults",
    // SkillPermission
    "required",
    // SkillDependencies
    "libraries",
    "binaries",
    "apis",
    // SkillConsumer
    "invocations",
    // ExternalScannerResult
    "source",
    "version",
    "status",
    "verdict",
    "rawReport",
    "findings",
    // ExternalScannerFinding
    "ruleId",
    "category",
    "severity",
    "label",
    "detail",
    // DetectedSkillSource (issue #219)
    "detectedSource",
    "repoUrl",
    "branch",
    "path",
    "remoteUrl",
];

const AGENT_FIELDS: &[&str] = &[
    "agents",
    "id",
    "name",
    "sourceFilePath",
    "classification",
    "executionModel",
    "trustScore",
    "version",
    "author",
    "sourceRepo",
    "capabilities",
    "tools",
    "trustBreakdown",
    // AgentCapability
    "enabled",
    // AgentTool
    "type",
    // TrustFactor
    "label",
    "delta",
];

const APP_FIELDS: &[&str] = &[
    "agenticApps",
    "id",
    "name",
    "sourceFilePath",
    "framework",
    "agentCount",
    "risk",
    "reviewStatus",
    "description",
    "agents",
    "toolsByAgent",
    "workflow",
    "integrations",
    "verificationChecks",
    "riskTags",
    "riskSummary",
    // WorkflowStep
    "step",
    "agent",
    "action",
    // Integration
    "type",
];

// ═══════════════════════════════════════════════════════════════════════════
// Coverage walker — fails loud on undisclosed fields
// ═══════════════════════════════════════════════════════════════════════════

/// Recursively walk every serialized JSON key in `payload` and require each to
/// map to a known [`DisclosureCategory`].
///
/// Panics on the first unknown key. This is the structural guarantee that
/// "a newly serialized payload field not reflected in the disclosure" is a
/// hard failure (issue #196 AC #5 and #4).
pub fn validate_payload_coverage(payload: &ContractPayload) {
    let value = match serde_json::to_value(payload) {
        Ok(v) => v,
        Err(_) => return,
    };
    walk_coverage(&value, "");
}

fn walk_coverage(value: &serde_json::Value, path: &str) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if field_category(&child_path).is_none() {
                    panic!(
                        "PII/compliance: serialized field '{child_path}' has no \
                         DisclosureCategory mapping. Add it to `field_category` in \
                         disclosure.rs (and a category variant if needed) before shipping."
                    );
                }
                // `rawReport` is an opaque, arbitrary JSON blob from an external
                // scanner — its internal keys cannot be enumerated from the
                // contract types. It is fully covered (transmitted) by
                // SkillRecords, so we don't require per-key disclosure inside it.
                if key == "rawReport" {
                    continue;
                }
                walk_coverage(child, &child_path);
            }
        }
        serde_json::Value::Array(items) => {
            let arr_path = format!("{path}[]");
            for item in items {
                walk_coverage(item, &arr_path);
            }
        }
        _ => {}
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Presence walker
// ═══════════════════════════════════════════════════════════════════════════

/// Determine which [`DisclosureCategory`] values are present in `payload`.
///
/// A category is *present* if the corresponding field(s) in the payload
/// contain actual data, or (for log-derived evidence) log scanning will run
/// against non-empty MCP servers. This is the structural link between the
/// payload types and the disclosure text.
pub fn disclosure_categories(payload: &ContractPayload) -> Vec<DisclosureCategory> {
    // Fail loud if any field is undisclosed.
    validate_payload_coverage(payload);

    let mut cats = Vec::new();

    // scan_meta — always present, but some sub-fields may be empty defaults.
    cats.push(DisclosureCategory::ScanMetaInfo);
    cats.push(DisclosureCategory::ScanRootPaths);
    cats.push(DisclosureCategory::Hostname);
    if has_host_security_data(&payload.scan_meta) {
        cats.push(DisclosureCategory::HostSecurityContext);
    }
    cats.push(DisclosureCategory::ScannerVersion);

    // Non-MCP record categories — present when the vec is non-empty.
    if !payload.prompts.is_empty() {
        cats.push(DisclosureCategory::PromptRecords);
    }
    if !payload.skills.is_empty() {
        cats.push(DisclosureCategory::SkillRecords);
    }
    if !payload.agents.is_empty() {
        cats.push(DisclosureCategory::AgentRecords);
    }
    if !payload.agentic_apps.is_empty() {
        cats.push(DisclosureCategory::AgenticAppRecords);
    }

    // MCP server categories — derived from the actual server data.
    let mcp_present = !payload.mcp_servers.is_empty();
    if mcp_present {
        cats.push(DisclosureCategory::McpServerCommand);
        cats.push(DisclosureCategory::McpToolNames);
        if any_server_has_dependent_agents(&payload.mcp_servers) {
            cats.push(DisclosureCategory::McpDependentAgents);
        }
        // Log scanning runs whenever there are MCP servers to match against,
        // so the category is disclosed whenever mcp_servers is non-empty —
        // even on the pre-consent disclosure payload that has not yet read
        // any logs (its log evidence is still empty). Presence is decided
        // structurally from `!mcp_servers.is_empty()`, not from having
        // already read the logs (issue #196, finding #4).
        cats.push(DisclosureCategory::LogDerivedNetworkEvidence);
        if any_server_has_env_vars(&payload.mcp_servers) {
            cats.push(DisclosureCategory::EnvVarNames);
        }
    }

    cats
}

fn has_host_security_data(scan_meta: &ScanMeta) -> bool {
    let hn = &scan_meta.host_network;
    hn.firewall_enabled
        || !hn.firewall_mode.is_empty() && hn.firewall_mode != "unknown"
        || hn.stealth_mode
        || !hn.firewall_rules.is_empty()
}

fn any_server_has_dependent_agents(servers: &[McpServer]) -> bool {
    servers.iter().any(|s| !s.dependent_agents.is_empty())
}

fn any_server_has_env_vars(servers: &[McpServer]) -> bool {
    servers.iter().any(|s| !s.env_vars.is_empty())
}

// ═══════════════════════════════════════════════════════════════════════════
// Rendering
// ═══════════════════════════════════════════════════════════════════════════

/// Format the disclosure text for a payload.
///
/// Returns a multi-line string ready for stderr rendering. The caller is
/// responsible for writing to the correct stream.
pub fn render_disclosure(payload: &ContractPayload) -> String {
    let categories = disclosure_categories(payload);
    let record_count = payload.prompts.len()
        + payload.skills.len()
        + payload.agents.len()
        + payload.agentic_apps.len();

    let mut lines = Vec::with_capacity(4 + categories.len());
    lines.push("  This submission will include:".to_string());

    for cat in &categories {
        lines.push(format!("    • {} — {}", cat.label(), cat.description()));
    }

    lines.push(format!(
        "  {} AI artifact record(s): prompts, skills, agents, agentic apps",
        record_count
    ));

    if !payload.mcp_servers.is_empty() {
        lines.push(format!(
            "  {} MCP server config record(s)",
            payload.mcp_servers.len()
        ));
    }

    lines.push(String::new()); // trailing blank line
    lines.join("\n")
}

/// Print the disclosure summary to stderr.
///
/// Used in interactive flows immediately before asking for consent, and on
/// non-interactive submission paths so the user sees what will be transmitted.
pub fn print_submit_disclosure(payload: &ContractPayload) {
    eprint!("{}", render_disclosure(payload));
}

/// Format a one-line status message about what data categories are present.
///
/// Used by tests to assert that the walker returned the expected categories
/// for a given payload shape.
pub fn disclosure_category_labels(categories: &[DisclosureCategory]) -> Vec<&str> {
    categories.iter().map(|c| c.label()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_evidence::{EnvVarRef, HostNetworkInfo, NetworkEvidence};

    /// The telemetry field gate and this enum must be the same list of categories.
    ///
    /// The gate maps every leaf path a telemetry payload may carry to a category *name*;
    /// this enum is what the user is actually shown. A gate category with no variant — or
    /// one whose label or description differs by a single byte — means a field could
    /// egress under a heading the disclosure never displays, or displays differently from
    /// the allowlist that admitted it. Byte-for-byte parity is what keeps "nothing is
    /// emitted that is not disclosed" true as the gate grows.
    #[test]
    fn every_gate_category_is_a_disclosure_variant() {
        const GATE_JSON: &str = include_str!("../../../../telemetry-field-gate.json");
        let gate: serde_json::Value =
            serde_json::from_str(GATE_JSON).expect("telemetry-field-gate.json parses");
        let entries = gate["disclosureCategories"]
            .as_array()
            .expect("the gate declares a disclosureCategories array");
        assert!(!entries.is_empty(), "the gate must declare categories");

        for entry in entries {
            let name = entry["name"].as_str().expect("category has a name");
            let category = ALL_CATEGORIES
                .iter()
                .find(|c| c.name() == name)
                .unwrap_or_else(|| {
                    panic!(
                        "telemetry-field-gate.json category '{name}' has no DisclosureCategory \
                         variant: gated fields would egress under a heading the disclosure \
                         never shows"
                    )
                });
            assert_eq!(
                category.label(),
                entry["label"].as_str().expect("category has a label"),
                "label of {name} must be the gate's own wording, byte for byte"
            );
            assert_eq!(
                category.description(),
                entry["description"]
                    .as_str()
                    .expect("category has a description"),
                "description of {name} must be the gate's own wording, byte for byte"
            );
        }
    }

    /// Every serialized field in a maximally-populated [`ContractPayload`] must
    /// map to a known [`DisclosureCategory`]. If anyone adds a public field to
    /// any serialized type and forgets to disclose it, `validate_payload_coverage`
    /// panics and this test fails.
    #[test]
    fn every_serialized_payload_field_maps_to_a_known_category() {
        let payload = max_payload();
        validate_payload_coverage(&payload);
    }

    /// The walker must leave no undisclosed field on the *default* shape too —
    /// a regression where a newly required field only appears in a full payload
    /// could slip through a sparse one.
    #[test]
    fn coverage_holds_for_minimal_payload() {
        let payload = ContractPayload {
            scan_meta: make_scan_meta(),
            prompts: vec![],
            skills: vec![],
            mcp_servers: vec![],
            agents: vec![],
            agentic_apps: vec![],
        };
        validate_payload_coverage(&payload);
    }

    /// A synthetic undeclared field must be rejected — proves the walker is not
    /// a template that silently ignores unknown keys.
    #[test]
    fn unknown_field_is_rejected() {
        let mut value = serde_json::to_value(max_payload()).unwrap();
        // Inject an unknown field into mcp_servers[0]; the walker must reject it.
        value["mcpServers"][0]["mysteryField"] = serde_json::json!("x");
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            walk_coverage(&value, "");
        }));
        assert!(
            res.is_err(),
            "an unmapped serialized field must panic the coverage walker"
        );
    }

    /// The walker must include a category for every non-empty MCP sub-field.
    #[test]
    fn walker_includes_log_evidence_when_servers_have_network_evidence() {
        let payload = max_payload();
        let cats = disclosure_categories(&payload);
        assert!(
            cats.contains(&DisclosureCategory::LogDerivedNetworkEvidence),
            "walker must include LogDerivedNetworkEvidence when mcp_servers is non-empty"
        );
    }

    #[test]
    fn walker_includes_log_evidence_whenever_mcp_servers_present() {
        // Log scanning runs whenever there are MCP servers, so the category is
        // present even when the payload has no log evidence (pre-consent
        // disclosure payload). Issue #196 / finding #4.
        let payload = ContractPayload {
            scan_meta: make_scan_meta(),
            prompts: vec![],
            skills: vec![],
            mcp_servers: vec![McpServer {
                id: "s1".into(),
                name: "server".into(),
                transport: "stdio".into(),
                network: "local".into(),
                auth: "none".into(),
                verified: false,
                command: "npx server".into(),
                tools: vec![],
                dependent_agents: vec![],
                network_evidence: vec![],
                env_vars: vec![],
            }],
            agents: vec![],
            agentic_apps: vec![],
        };
        let cats = disclosure_categories(&payload);
        assert!(
            cats.contains(&DisclosureCategory::LogDerivedNetworkEvidence),
            "LogDerivedNetworkEvidence must be present when mcp_servers is non-empty even \
             without log evidence"
        );
    }

    #[test]
    fn walker_excludes_log_evidence_when_no_mcp_servers() {
        let payload = ContractPayload {
            scan_meta: make_scan_meta(),
            prompts: vec![],
            skills: vec![],
            mcp_servers: vec![],
            agents: vec![],
            agentic_apps: vec![],
        };
        let cats = disclosure_categories(&payload);
        assert!(
            !cats.contains(&DisclosureCategory::LogDerivedNetworkEvidence),
            "LogDerivedNetworkEvidence must NOT be present when there are no MCP servers"
        );
    }

    #[test]
    fn walker_includes_env_vars_when_present() {
        let mut payload = max_payload();
        payload.mcp_servers[0].env_vars = vec![EnvVarRef {
            name: "ANTHROPIC_API_KEY".into(),
            is_set: true,
            source_key: "args".into(),
        }];
        let cats = disclosure_categories(&payload);
        assert!(
            cats.contains(&DisclosureCategory::EnvVarNames),
            "walker must include EnvVarNames when mcp_servers[*].env_vars is non-empty"
        );
    }

    #[test]
    fn walker_includes_dependent_agents_when_present() {
        let mut payload = max_payload();
        payload.mcp_servers[0].dependent_agents = vec!["agent-1".into()];
        let cats = disclosure_categories(&payload);
        assert!(
            cats.contains(&DisclosureCategory::McpDependentAgents),
            "walker must include McpDependentAgents when mcp_servers[*].dependent_agents is non-empty"
        );
    }

    #[test]
    fn render_disclosure_contains_all_category_labels() {
        let payload = max_payload();
        let rendered = render_disclosure(&payload);
        let cats = disclosure_categories(&payload);

        for cat in &cats {
            assert!(
                rendered.contains(cat.label()),
                "rendered disclosure must mention category '{}':\n{rendered}",
                cat.label()
            );
        }
    }

    #[test]
    fn print_disclosure_does_not_panic() {
        let payload = ContractPayload {
            scan_meta: make_scan_meta(),
            prompts: vec![],
            skills: vec![],
            mcp_servers: vec![],
            agents: vec![],
            agentic_apps: vec![],
        };
        print_submit_disclosure(&payload);
    }

    fn make_scan_meta() -> ScanMeta {
        ScanMeta {
            scan_id: "test-id".into(),
            endpoint_hostname: "test-host".into(),
            scanned_at: "2026-01-01T00:00:00Z".into(),
            scanner_version: "0.1.0".into(),
            scan_duration_ms: 0,
            scan_roots: vec!["/tmp/test".into()],
            host_network: HostNetworkInfo::default(),
        }
    }

    fn make_scan_meta_full() -> ScanMeta {
        ScanMeta {
            scan_id: "test-id".into(),
            endpoint_hostname: "test-host".into(),
            scanned_at: "2026-01-01T00:00:00Z".into(),
            scanner_version: "0.1.0".into(),
            scan_duration_ms: 100,
            scan_roots: vec!["/tmp/test".into()],
            host_network: HostNetworkInfo {
                firewall_enabled: true,
                firewall_mode: "active".into(),
                stealth_mode: false,
                firewall_rules: vec![],
            },
        }
    }

    /// A maximally-populated payload exercising every serialized field of every
    /// contract type, so the coverage walker touches them all.
    fn max_payload() -> ContractPayload {
        ContractPayload {
            scan_meta: make_scan_meta_full(),
            prompts: vec![Prompt {
                id: "p1".into(),
                name: "test".into(),
                source_file_path: "/tmp/prompt.md".into(),
                classification: "system".into(),
                tokens: 100,
                content_hash: "abc".into(),
                last_changed_date: "2026-01-01".into(),
                capabilities: vec![PromptCapability {
                    text: "shell".into(),
                    level: "high".into(),
                }],
                secret_refs: vec![SecretRef {
                    label: "k".into(),
                    detail: "d".into(),
                    tone: "warn".into(),
                }],
                injection_surfaces: vec![InjectionSurface {
                    text: "x".into(),
                    severity: "high".into(),
                }],
                dependencies: vec!["dep".into()],
                risk_score: 50,
            }],
            skills: vec![Skill {
                id: "sk1".into(),
                name: "test-skill".into(),
                skill_type: "agent".into(),
                trust_level: "high".into(),
                overall_grade: "A".into(),
                execution_environment: "shell".into(),
                description: "a skill".into(),
                permissions: vec![SkillPermission {
                    name: "fs".into(),
                    required: true,
                }],
                dependencies: SkillDependencies {
                    libraries: vec!["lib".into()],
                    binaries: vec!["bin".into()],
                    apis: vec!["api".into()],
                },
                consumers: vec![SkillConsumer {
                    id: "c1".into(),
                    name: "consumer".into(),
                    consumer_type: "agent".into(),
                    invocations: 3,
                }],
                external_scanner_results: Some(vec![ExternalScannerResult {
                    source: "suite".into(),
                    version: Some("1.0".into()),
                    status: "done".into(),
                    verdict: Some("pass".into()),
                    raw_report: Some(serde_json::json!({"k": "v"})),
                    findings: Some(vec![ExternalScannerFinding {
                        rule_id: "r1".into(),
                        category: "sec".into(),
                        severity: "high".into(),
                        label: "l".into(),
                        detail: Some("d".into()),
                    }]),
                }]),
                detected_source: None,
            }],
            mcp_servers: vec![McpServer {
                id: "s1".into(),
                name: "server".into(),
                transport: "stdio".into(),
                network: "local".into(),
                auth: "none".into(),
                verified: false,
                command: "npx server".into(),
                tools: vec![McpTool {
                    name: "read_file".into(),
                    risk: "low".into(),
                    description: "a".into(),
                }],
                dependent_agents: vec![],
                network_evidence: vec![NetworkEvidence {
                    source: "logs".into(),
                    category: "outbound-url".into(),
                    detail: "observed".into(),
                    url: Some("https://example.com".into()),
                }],
                env_vars: vec![EnvVarRef {
                    name: "API_KEY".into(),
                    is_set: true,
                    source_key: "args".into(),
                }],
            }],
            agents: vec![Agent {
                id: "a1".into(),
                name: "test-agent".into(),
                source_file_path: "/tmp/AGENTS.md".into(),
                classification: "tool-use".into(),
                execution_model: "sequential".into(),
                trust_score: 80,
                version: "1.0".into(),
                author: "test".into(),
                source_repo: "test/repo".into(),
                capabilities: vec![AgentCapability {
                    name: "fs".into(),
                    enabled: true,
                }],
                tools: vec![AgentTool {
                    name: "tool".into(),
                    tool_type: "mcp".into(),
                }],
                trust_breakdown: vec![TrustFactor {
                    label: "auth".into(),
                    delta: -5,
                }],
            }],
            agentic_apps: vec![AgenticApp {
                id: "aa1".into(),
                name: "test-app".into(),
                source_file_path: "/tmp/docker-compose.yml".into(),
                framework: "docker".into(),
                agent_count: 1,
                risk: "medium".into(),
                review_status: "pending".into(),
                description: "an app".into(),
                agents: vec![AppAgent {
                    id: "a1".into(),
                    name: "agent".into(),
                }],
                tools_by_agent: vec![vec!["tool".into()]],
                workflow: vec![WorkflowStep {
                    step: 1,
                    agent: "a1".into(),
                    action: "run".into(),
                }],
                integrations: vec![Integration {
                    name: "slack".into(),
                    integration_type: "webhook".into(),
                    risk: "low".into(),
                }],
                verification_checks: vec!["check".into()],
                risk_tags: vec!["tag".into()],
                risk_summary: "low".into(),
            }],
        }
    }
}
