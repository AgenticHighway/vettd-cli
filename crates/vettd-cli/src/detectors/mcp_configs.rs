use crate::discovery::Candidate;
use crate::models::{check_for_secrets, gather_file_primitives, ArtifactReport};

use super::base::Detector;
use super::read_utf8_head;
use serde_json::{json, Value};
use std::sync::LazyLock;

static URL_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"https?://[^\s"'\\,\]}>]+"#).unwrap());

const MAX_READ_BYTES: usize = 8192;

const MCP_EXACT_NAMES: &[&str] = &[
    "mcp.json",
    "mcp_config.json",
    "claude_desktop_config.json",
    "mcp-config.json",
    "mcp_settings.json",
];

const EXECUTION_TOKENS: &[&str] = &["/bin/", "npx", "uvx", "node", "python", "deno"];
const SHELL_TOKENS: &[&str] = &["shell", "bash", "sh -c", "zsh"];

const CREDENTIAL_SIGNALS: &[&str] = &[
    "api_key",
    "apikey",
    "secret",
    "token",
    "password",
    "credential",
    "auth",
];

pub struct MCPConfigDetector;

impl Detector for MCPConfigDetector {
    fn name(&self) -> &str {
        "mcp_configs"
    }

    fn detect(&self, candidates: &[Candidate], _deep: bool) -> Vec<ArtifactReport> {
        let mut results = Vec::new();
        for candidate in candidates
            .iter()
            .filter(|candidate| is_mcp_config_candidate(candidate))
        {
            if let Some(report) = classify_candidate(candidate) {
                results.push(report);
            }
        }
        results
    }
}

fn is_mcp_config_candidate(candidate: &Candidate) -> bool {
    candidate
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| MCP_EXACT_NAMES.contains(&name))
}

fn classify_candidate(candidate: &Candidate) -> Option<ArtifactReport> {
    let content = read_utf8_head(&candidate.path, MAX_READ_BYTES)?;
    let mut signals = Vec::new();
    let mut metadata = serde_json::Map::new();

    // File primitives — gather once, avoid re-reads downstream
    let file_prims = gather_file_primitives(&candidate.path);
    metadata.extend(file_prims);

    metadata.insert("paths".into(), json!([candidate.path.to_string_lossy()]));

    let confidence = match parse_mcp_json(&content) {
        Some(parsed) => {
            apply_parsed_signals(&parsed, &mut signals, &mut metadata);
            0.85
        }
        None => 0.75,
    };

    signals.extend(check_for_secrets(&content));

    let mut report = ArtifactReport::new("mcp_config", confidence);
    report.signals = signals;
    report.metadata = metadata;
    report.artifact_scope = candidate.origin.clone();
    report.compute_hash();
    Some(report)
}

fn parse_mcp_json(content: &str) -> Option<Value> {
    let val: Value = serde_json::from_str(content).ok()?;
    let obj = val.as_object()?;
    // Must contain mcpServers or servers
    if obj.contains_key("mcpServers") || obj.contains_key("servers") {
        Some(val)
    } else {
        None
    }
}

fn apply_parsed_signals(
    parsed: &Value,
    signals: &mut Vec<String>,
    metadata: &mut serde_json::Map<String, Value>,
) {
    let text = parsed.to_string().to_lowercase();

    let exec_found = scan_tokens(&text, EXECUTION_TOKENS);
    if !exec_found.is_empty() {
        signals.push("execution_tokens_present".to_string());
        metadata.insert("execution_tokens".into(), json!(exec_found));
    }

    let shell_found = scan_tokens(&text, SHELL_TOKENS);
    if !shell_found.is_empty() {
        signals.push("shell_access_detected".to_string());
        metadata.insert("shell_tokens".into(), json!(shell_found));
    }

    let endpoints = extract_endpoints(&parsed.to_string());
    if !endpoints.is_empty() {
        metadata.insert("api_endpoints".into(), json!(endpoints));
    }

    let cred_found = scan_tokens(&text, CREDENTIAL_SIGNALS);
    if !cred_found.is_empty() {
        signals.push("credential_references".to_string());
    }

    let server_count = count_servers(parsed);
    metadata.insert("server_count".into(), json!(server_count));

    // Extract individual server names for downstream cross-referencing
    let server_names = extract_server_names(parsed);
    if !server_names.is_empty() {
        metadata.insert("server_names".into(), json!(server_names));
    }
}

fn scan_tokens(text: &str, tokens: &[&str]) -> Vec<String> {
    tokens
        .iter()
        .filter(|t| text.contains(**t))
        .map(|s| s.to_string())
        .collect()
}

fn extract_endpoints(text: &str) -> Vec<String> {
    URL_REGEX
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn count_servers(val: &Value) -> usize {
    let servers_obj = val
        .get("mcpServers")
        .or_else(|| val.get("servers"))
        .and_then(|v| v.as_object());
    servers_obj.map_or(0, |m| m.len())
}

fn extract_server_names(val: &Value) -> Vec<String> {
    val.get("mcpServers")
        .or_else(|| val.get("servers"))
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mcp_json_valid() {
        let json = r#"{"mcpServers": {"fs": {"command": "npx"}}}
"#;
        assert!(parse_mcp_json(json).is_some());
    }

    #[test]
    fn parse_mcp_json_no_servers_key() {
        assert!(parse_mcp_json(r#"{"other": 1}"#).is_none());
    }

    #[test]
    fn parse_mcp_json_invalid_json() {
        assert!(parse_mcp_json("not json").is_none());
    }

    #[test]
    fn extract_server_names_from_mcp_servers() {
        let val: Value = serde_json::from_str(
            r#"{"mcpServers": {"filesystem": {}, "github": {}}}
"#,
        )
        .unwrap();
        let mut names = extract_server_names(&val);
        names.sort();
        assert_eq!(names, vec!["filesystem", "github"]);
    }

    #[test]
    fn extract_server_names_empty() {
        let val: Value = serde_json::from_str(r#"{"other": 1}"#).unwrap();
        assert!(extract_server_names(&val).is_empty());
    }

    #[test]
    fn count_servers_counts_mcp_servers() {
        let val: Value =
            serde_json::from_str(r#"{"mcpServers": {"a": {}, "b": {}, "c": {}}}"#).unwrap();
        assert_eq!(count_servers(&val), 3);
    }

    #[test]
    fn extract_endpoints_finds_urls() {
        let text = r#""url": "https://api.example.com/v1""#;
        let eps = extract_endpoints(text);
        assert_eq!(eps, vec!["https://api.example.com/v1"]);
    }

    #[test]
    fn scan_tokens_finds_matches() {
        let found = scan_tokens("uses npx and python", &["npx", "node", "python"]);
        assert_eq!(found, vec!["npx", "python"]);
    }

    #[test]
    fn is_mcp_config_candidate_matches_expected_names() {
        let candidate = Candidate {
            path: std::path::PathBuf::from("/tmp/mcp.json"),
            origin: "host".to_string(),
        };
        let other = Candidate {
            path: std::path::PathBuf::from("/tmp/config.json"),
            origin: "host".to_string(),
        };

        assert!(is_mcp_config_candidate(&candidate));
        assert!(!is_mcp_config_candidate(&other));
    }
}
