//! Network evidence gathering for the scan contract.
//!
//! Collects factual data about host network configuration and MCP server
//! network exposure from:
//! - macOS Application Firewall rules
//! - MCP config transport types and URLs
//! - Environment variable references in MCP configs
//! - Application log files (VS Code, Cursor, Claude)
//! - Known package network behavior profiles

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// ═══════════════════════════════════════════════════════════════════════════
// Compiled regex patterns — compiled once at first use, reused everywhere
// ═══════════════════════════════════════════════════════════════════════════

/// HTTP/HTTPS URL pattern for MCP config scanning.
static HTTP_URL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"https?://[^\s"'\\,\]}>]+"#).expect("HTTP_URL_RE is a valid regex pattern")
});

/// HTTP/HTTPS URL pattern for log file scanning (excludes control characters).
static HTTP_URL_LOG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"https?://[^\s"'\\,\]}>)\x00-\x1f]+"#)
        .expect("HTTP_URL_LOG_RE is a valid regex pattern")
});

/// WebSocket URL pattern for MCP config scanning.
static WS_URL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"wss?://[^\s"'\\,\]}>]+"#).expect("WS_URL_RE is a valid regex pattern")
});

/// WebSocket URL pattern for log file scanning.
static WS_URL_LOG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"wss?://[^\s"'\\,\]}>)\x00-\x1f]+"#)
        .expect("WS_URL_LOG_RE is a valid regex pattern")
});

/// `${VAR_NAME}` interpolation pattern in MCP configs.
static ENV_VAR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\$\{([A-Z_][A-Z0-9_]*)\}").expect("ENV_VAR_RE is a valid regex pattern")
});

// ═══════════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostNetworkInfo {
    pub firewall_enabled: bool,
    pub firewall_mode: String,
    pub stealth_mode: bool,
    pub firewall_rules: Vec<FirewallRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRule {
    pub app_path: String,
    pub allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEvidence {
    /// Where the evidence came from: "config", "firewall", "logs", "package-info"
    pub source: String,
    /// What kind of evidence: "transport", "outbound-url", "local-endpoint",
    /// "websocket", "package-registry", "known-behavior", "firewall-match",
    /// "log-network-activity", "credential-in-env"
    pub category: String,
    /// Human-readable description of the finding.
    pub detail: String,
    /// The URL or address observed, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvVarRef {
    /// Name of the environment variable.
    pub name: String,
    /// Whether the variable is currently set on the host.
    pub is_set: bool,
    /// Where it was referenced ("env", "args", "config").
    pub source_key: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// Host-level: macOS Application Firewall
// ═══════════════════════════════════════════════════════════════════════════

const FIREWALL_BIN: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";

pub fn gather_host_network() -> HostNetworkInfo {
    #[cfg(target_os = "macos")]
    {
        if let Some(info) = read_macos_firewall() {
            return info;
        }
    }
    HostNetworkInfo {
        firewall_enabled: false,
        firewall_mode: "unknown".to_string(),
        stealth_mode: false,
        firewall_rules: Vec::new(),
    }
}

#[cfg(target_os = "macos")]
fn read_macos_firewall() -> Option<HostNetworkInfo> {
    use std::process::Command;

    // Global state
    let state_out = Command::new(FIREWALL_BIN)
        .arg("--getglobalstate")
        .output()
        .ok()?;
    let state_str = String::from_utf8_lossy(&state_out.stdout);
    let enabled = state_str.contains("enabled");
    let mode = if state_str.contains("State = 2") {
        "block-all"
    } else if state_str.contains("State = 1") {
        "specific-services"
    } else {
        "off"
    };

    // Stealth mode
    let stealth_out = Command::new(FIREWALL_BIN)
        .arg("--getstealth")
        .output()
        .ok()?;
    let stealth_str = String::from_utf8_lossy(&stealth_out.stdout);
    let stealth = stealth_str.contains("enabled");

    // App rules
    let apps_out = Command::new(FIREWALL_BIN).arg("--listapps").output().ok()?;
    let apps_str = String::from_utf8_lossy(&apps_out.stdout);
    let rules = parse_firewall_apps(&apps_str);

    Some(HostNetworkInfo {
        firewall_enabled: enabled,
        firewall_mode: mode.to_string(),
        stealth_mode: stealth,
        firewall_rules: rules,
    })
}

#[cfg(target_os = "macos")]
fn parse_firewall_apps(output: &str) -> Vec<FirewallRule> {
    let mut rules = Vec::new();
    let mut current_path = String::new();
    for line in output.lines() {
        let trimmed = line.trim();
        // Lines like: "1 : /usr/libexec/remoted"
        if trimmed.contains(" : /") {
            if let Some(path) = trimmed.split(" : ").nth(1) {
                current_path = path.trim().to_string();
            }
        }
        // Lines like: "   (Allow incoming connections)"
        if trimmed.starts_with('(') && !current_path.is_empty() {
            let allowed = trimmed.contains("Allow");
            rules.push(FirewallRule {
                app_path: current_path.clone(),
                allowed,
            });
            current_path.clear();
        }
    }
    rules
}

// ═══════════════════════════════════════════════════════════════════════════
// MCP transport inference
// ═══════════════════════════════════════════════════════════════════════════

pub fn infer_transport(server_val: &serde_json::Value) -> String {
    // Explicit type field (VS Code / Claude Desktop format)
    if let Some(t) = server_val.get("type").and_then(|v| v.as_str()) {
        return match t {
            "stdio" => "stdio",
            "sse" => "sse",
            "streamable-http" | "http" => "streamable-http",
            other => other,
        }
        .to_string();
    }
    // Infer from URL presence vs command presence
    if server_val.get("url").is_some() {
        return "sse".to_string();
    }
    if server_val.get("command").is_some() {
        return "stdio".to_string();
    }
    "unknown".to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// Per-server evidence gathering
// ═══════════════════════════════════════════════════════════════════════════

pub fn gather_server_evidence(
    name: &str,
    server_val: &serde_json::Value,
    transport: &str,
) -> Vec<NetworkEvidence> {
    let mut evidence = Vec::new();

    // 1. Transport type
    let transport_detail = match transport {
        "stdio" => "Local process pipe — no network listener",
        "sse" => "HTTP Server-Sent Events — requires network endpoint",
        "streamable-http" => "HTTP streaming — requires network endpoint",
        _ => "Unknown transport",
    };
    evidence.push(NetworkEvidence {
        source: "config".into(),
        category: "transport".into(),
        detail: format!("Transport: {transport} — {transport_detail}"),
        url: None,
    });

    // 2. URL endpoint (for sse/http servers)
    if let Some(url) = server_val.get("url").and_then(|v| v.as_str()) {
        let cat = classify_url(url);
        evidence.push(NetworkEvidence {
            source: "config".into(),
            category: cat.into(),
            detail: format!("Server endpoint: {url}"),
            url: Some(url.to_string()),
        });
    }

    // 3. URLs in args (e.g. --registry, --url flags)
    // Strip metadata-only keys that are not runtime config
    let mut runtime_val = server_val.clone();
    if let Some(obj) = runtime_val.as_object_mut() {
        for meta_key in &["gallery", "version", "$schema"] {
            obj.remove(*meta_key);
        }
    }
    let text = runtime_val.to_string();
    for m in HTTP_URL_RE.find_iter(&text) {
        let url = m.as_str();
        // Skip if already captured as the main endpoint
        if server_val.get("url").and_then(|v| v.as_str()) == Some(url) {
            continue;
        }
        evidence.push(NetworkEvidence {
            source: "config".into(),
            category: classify_url(url).into(),
            detail: format!("URL in server config: {url}"),
            url: Some(url.to_string()),
        });
    }

    // 4. WebSocket URLs
    for m in WS_URL_RE.find_iter(&text) {
        evidence.push(NetworkEvidence {
            source: "config".into(),
            category: "websocket".into(),
            detail: format!("WebSocket URL: {}", m.as_str()),
            url: Some(m.as_str().to_string()),
        });
    }

    // 5. Credential-bearing env vars
    if let Some(env_obj) = server_val.get("env").and_then(|v| v.as_object()) {
        for (key, val) in env_obj {
            let key_lower = key.to_lowercase();
            let is_credential = ["token", "key", "secret", "password", "auth", "credential"]
                .iter()
                .any(|kw| key_lower.contains(kw));
            if is_credential {
                let has_value = val.as_str().map(|s| !s.is_empty()).unwrap_or(false);
                evidence.push(NetworkEvidence {
                    source: "config".into(),
                    category: "credential-in-env".into(),
                    detail: format!(
                        "Credential env var '{key}' passed to server (value {})",
                        if has_value { "present" } else { "empty" }
                    ),
                    url: None,
                });
            }
        }
    }

    // 6. Known package behavior
    if let Some(pkg_ev) = known_package_profile(name, server_val) {
        evidence.push(pkg_ev);
    }

    evidence
}

fn classify_url(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    if lower.contains("registry.npmjs.org")
        || lower.contains("pypi.org")
        || lower.contains("api.mcp.github.com")
    {
        "package-registry"
    } else if lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("0.0.0.0")
    {
        "local-endpoint"
    } else {
        "outbound-url"
    }
}

fn known_package_profile(name: &str, server_val: &serde_json::Value) -> Option<NetworkEvidence> {
    let cmd = server_val
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let args = server_val
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let full = format!("{cmd} {args} {name}").to_lowercase();

    let (pkg, behavior) = if full.contains("playwright") {
        (
            "playwright",
            "Controls browser instances via CDP; browser may make arbitrary internet requests",
        )
    } else if full.contains("chrome-devtools") {
        (
            "chrome-devtools",
            "Connects to Chrome via DevTools Protocol (local WebSocket); browser may make internet requests",
        )
    } else if full.contains("github-mcp") || full.contains("github/github") {
        (
            "github-mcp-server",
            "Outbound HTTPS to api.github.com and api.githubcopilot.com",
        )
    } else if full.contains("next-devtools") {
        (
            "next-devtools-mcp",
            "Connects to local Next.js dev server; may proxy API calls to external services",
        )
    } else if full.contains("filesystem") || full.contains("fs-mcp") || full.contains("fs-server") {
        (
            "filesystem",
            "Local filesystem access only; no outbound network connections",
        )
    } else if full.contains("sqlite") {
        (
            "sqlite",
            "Local database access only; no outbound network connections",
        )
    } else if full.contains("puppeteer") {
        (
            "puppeteer",
            "Controls headless Chromium; browser may make arbitrary internet requests",
        )
    } else if full.contains("terraform") || full.contains("tfe") {
        (
            "terraform",
            "Outbound HTTPS to HCP Terraform / Terraform Enterprise API",
        )
    } else {
        return None;
    };

    Some(NetworkEvidence {
        source: "package-info".into(),
        category: "known-behavior".into(),
        detail: format!("Known package '{pkg}': {behavior}"),
        url: None,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Environment variable resolution
// ═══════════════════════════════════════════════════════════════════════════

pub fn resolve_env_refs(server_val: &serde_json::Value) -> Vec<EnvVarRef> {
    let mut refs = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let text = server_val.to_string();

    // ${VAR_NAME} patterns anywhere in the server config
    for cap in ENV_VAR_RE.captures_iter(&text) {
        let var_name = &cap[1];
        if seen.insert(var_name.to_string()) {
            refs.push(EnvVarRef {
                name: var_name.to_string(),
                is_set: std::env::var(var_name).is_ok(),
                source_key: "config".to_string(),
            });
        }
    }

    // Explicit env block keys
    if let Some(env_obj) = server_val.get("env").and_then(|v| v.as_object()) {
        for (key, val) in env_obj {
            if seen.insert(key.clone()) {
                refs.push(EnvVarRef {
                    name: key.clone(),
                    is_set: true, // explicitly set by config
                    source_key: "env".to_string(),
                });
            }
            // Check if the value itself references another env var
            if let Some(val_str) = val.as_str() {
                for cap in ENV_VAR_RE.captures_iter(val_str) {
                    let var_name = &cap[1];
                    if seen.insert(var_name.to_string()) {
                        refs.push(EnvVarRef {
                            name: var_name.to_string(),
                            is_set: std::env::var(var_name).is_ok(),
                            source_key: format!("env.{key}"),
                        });
                    }
                }
            }
        }
    }

    refs
}

// ═══════════════════════════════════════════════════════════════════════════
// MCP log scanning
// ═══════════════════════════════════════════════════════════════════════════

/// Known log directories for MCP-related applications.
fn mcp_log_directories() -> Vec<PathBuf> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    vec![
        home.join("Library/Logs/Claude"),
        home.join("Library/Application Support/Claude/logs"),
        home.join("Library/Application Support/Code/logs"),
        home.join("Library/Application Support/Code - Insiders/logs"),
        home.join("Library/Application Support/Cursor/logs"),
        home.join(".config/Code/logs"),
        home.join(".config/Cursor/logs"),
    ]
}

const MAX_LOG_FILES: usize = 15;
const MAX_LOG_TAIL_BYTES: usize = 32_768;

/// Scan known log directories for MCP-related network activity.
pub fn scan_mcp_logs() -> Vec<NetworkEvidence> {
    let mut evidence = Vec::new();
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(7 * 24 * 3600))
        .unwrap_or(std::time::UNIX_EPOCH);

    let mut seen_urls = std::collections::HashSet::new();
    let mut file_count = 0;

    for dir in mcp_log_directories() {
        if !dir.exists() || file_count >= MAX_LOG_FILES {
            break;
        }
        let walker = walkdir::WalkDir::new(&dir)
            .max_depth(5)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let is_log = name.ends_with(".log");
                let is_relevant = name.to_lowercase().contains("mcp")
                    || name.to_lowercase().contains("copilot")
                    || name.to_lowercase().contains("server")
                    || name.to_lowercase().contains("exthost");
                let is_recent = p
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t > cutoff)
                    .unwrap_or(false);
                is_log && is_relevant && is_recent
            });

        for entry in walker {
            if file_count >= MAX_LOG_FILES {
                break;
            }
            file_count += 1;
            let path = entry.path();
            let tail = match read_file_tail(path, MAX_LOG_TAIL_BYTES) {
                Some(t) => t,
                None => continue,
            };

            let log_name = path.display().to_string();

            // HTTP URLs
            for m in HTTP_URL_LOG_RE.find_iter(&tail) {
                let url = m.as_str();
                if is_noisy_log_url(url) {
                    continue;
                }
                if seen_urls.insert(url.to_string()) {
                    evidence.push(NetworkEvidence {
                        source: "logs".into(),
                        category: classify_url(url).into(),
                        detail: format!("URL observed in {log_name}"),
                        url: Some(url.to_string()),
                    });
                }
            }

            // WebSocket URLs
            for m in WS_URL_LOG_RE.find_iter(&tail) {
                let url = m.as_str();
                if seen_urls.insert(url.to_string()) {
                    evidence.push(NetworkEvidence {
                        source: "logs".into(),
                        category: "websocket".into(),
                        detail: format!("WebSocket observed in {log_name}"),
                        url: Some(url.to_string()),
                    });
                }
            }

            // Unix socket MCP servers (VS Code pattern)
            if tail.contains("mcp.sock") || tail.contains("MCP server started") {
                evidence.push(NetworkEvidence {
                    source: "logs".into(),
                    category: "local-endpoint".into(),
                    detail: format!("MCP server on unix socket observed in {log_name}"),
                    url: None,
                });
            }
        }
    }

    evidence
}

/// Filter out URLs that are infrastructure noise, not application behavior.
fn is_noisy_log_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("vscode-cdn")
        || lower.contains("update.code")
        || lower.contains("telemetry")
        || lower.contains("dc.services.visualstudio.com")
        || lower.contains("marketplace.visualstudio.com")
        || lower.contains("gallerycdn.vsassets.io")
        || lower.contains("vortex.data.microsoft.com")
}

fn read_file_tail(path: &Path, max_bytes: usize) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let start = bytes.len().saturating_sub(max_bytes);
    String::from_utf8(bytes[start..].to_vec()).ok()
}

// ═══════════════════════════════════════════════════════════════════════════
// Network classification (convenience — derived from evidence)
// ═══════════════════════════════════════════════════════════════════════════

/// Derive a network exposure classification from gathered evidence.
///
/// Returns one of: "Local Only", "LAN", "Internet Exposed".
/// The server should use the `networkEvidence` array for deeper analysis.
pub fn classify_from_evidence(transport: &str, evidence: &[NetworkEvidence]) -> String {
    // If any outbound-url evidence exists → Internet Exposed
    let has_outbound = evidence.iter().any(|e| e.category == "outbound-url");
    if has_outbound {
        return "Internet Exposed".to_string();
    }

    // HTTP/SSE transport with non-local URL → Internet Exposed
    if matches!(transport, "sse" | "streamable-http") {
        let has_non_local_endpoint = evidence
            .iter()
            .any(|e| e.category != "local-endpoint" && e.url.is_some());
        if has_non_local_endpoint {
            return "Internet Exposed".to_string();
        }
    }

    // Known behavior suggests internet access
    let known_internet = evidence.iter().any(|e| {
        e.category == "known-behavior"
            && (e.detail.contains("internet")
                || e.detail.contains("Outbound HTTPS")
                || e.detail.contains("api.github"))
    });
    if known_internet {
        return "Internet Exposed".to_string();
    }

    // Local endpoint or websocket → LAN
    let has_local = evidence
        .iter()
        .any(|e| e.category == "local-endpoint" || e.category == "websocket");
    if has_local {
        return "LAN".to_string();
    }

    // stdio transport with no network evidence → Local Only
    if transport == "stdio" {
        return "Local Only".to_string();
    }

    "Local Only".to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Regex compilation (fix #3) ──────────────────────────────────────────
    //
    // These tests verify that all five LazyLock regex patterns compile
    // successfully and match expected inputs. If any pattern were invalid,
    // the LazyLock initializer would panic on first access.

    #[test]
    fn http_url_re_matches_https_url() {
        let m = HTTP_URL_RE.find("command https://api.example.com/path end");
        assert!(m.is_some());
        assert_eq!(m.unwrap().as_str(), "https://api.example.com/path");
    }

    #[test]
    fn http_url_re_does_not_match_ws() {
        assert!(HTTP_URL_RE.find("ws://localhost:3000").is_none());
    }

    #[test]
    fn http_url_log_re_excludes_control_chars() {
        // Pattern must compile and match a normal URL
        let m = HTTP_URL_LOG_RE.find("https://internal.company.com/mcp");
        assert!(m.is_some());
    }

    #[test]
    fn ws_url_re_matches_websocket() {
        let m = WS_URL_RE.find("connecting to wss://realtime.example.com/stream");
        assert!(m.is_some());
        assert_eq!(m.unwrap().as_str(), "wss://realtime.example.com/stream");
    }

    #[test]
    fn ws_url_log_re_matches_plain_ws() {
        let m = WS_URL_LOG_RE.find("ws://localhost:9229");
        assert!(m.is_some());
    }

    #[test]
    fn env_var_re_captures_var_name() {
        let caps = ENV_VAR_RE.captures("value: ${GITHUB_TOKEN}");
        assert!(caps.is_some());
        assert_eq!(&caps.unwrap()[1], "GITHUB_TOKEN");
    }

    #[test]
    fn env_var_re_rejects_lowercase() {
        // Pattern requires uppercase + underscore names only
        assert!(ENV_VAR_RE.find("${lowercase_var}").is_none());
    }

    // ── gather_server_evidence integration ─────────────────────────────────

    #[test]
    fn server_evidence_detects_http_transport() {
        let val = serde_json::json!({"type": "http", "url": "https://api.example.com/mcp"});
        let transport = infer_transport(&val);
        assert_eq!(transport, "streamable-http");
        let evidence = gather_server_evidence("test-server", &val, &transport);
        let has_transport = evidence.iter().any(|e| e.category == "transport");
        let has_url = evidence.iter().any(|e| e.category == "outbound-url");
        assert!(has_transport, "should have transport evidence");
        assert!(
            has_url,
            "should have outbound-url evidence for https endpoint"
        );
    }

    #[test]
    fn server_evidence_detects_credential_env_var() {
        let val =
            serde_json::json!({"type": "stdio", "command": "npx", "env": {"API_KEY": "secret"}});
        let evidence = gather_server_evidence("test", &val, "stdio");
        let has_cred = evidence.iter().any(|e| e.category == "credential-in-env");
        assert!(has_cred, "should detect credential in env block");
    }

    #[test]
    fn resolve_env_refs_finds_interpolated_vars() {
        let val = serde_json::json!({"args": ["--token", "${MY_SECRET_TOKEN}"]});
        let refs = resolve_env_refs(&val);
        assert!(refs.iter().any(|r| r.name == "MY_SECRET_TOKEN"));
    }

    #[test]
    fn classify_from_evidence_local_only_for_stdio_no_urls() {
        let evidence = vec![NetworkEvidence {
            source: "config".into(),
            category: "transport".into(),
            detail: "Transport: stdio — local process".into(),
            url: None,
        }];
        assert_eq!(classify_from_evidence("stdio", &evidence), "Local Only");
    }

    #[test]
    fn classify_from_evidence_internet_exposed_for_outbound_url() {
        let evidence = vec![NetworkEvidence {
            source: "config".into(),
            category: "outbound-url".into(),
            detail: "Server endpoint: https://api.example.com".into(),
            url: Some("https://api.example.com".into()),
        }];
        assert_eq!(classify_from_evidence("sse", &evidence), "Internet Exposed");
    }
}
