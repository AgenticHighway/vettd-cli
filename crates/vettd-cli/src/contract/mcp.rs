//! MCP server building for the scanner data contract.

use crate::models::ArtifactReport;
use crate::network_evidence;

use super::helpers::{first_path, read_artifact_head, short_hash};
use super::types::{McpServer, McpTool};

pub fn build_mcp_servers(artifacts: &[&ArtifactReport]) -> Vec<McpServer> {
    let mut servers = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    for artifact in artifacts {
        let content = match read_artifact_head(artifact) {
            Some(c) => c,
            None => continue,
        };
        let val: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let server_map = match mcp_server_map(&val) {
            Some(m) => m,
            None => continue,
        };

        for (name, server_val) in server_map {
            if seen_names.insert(name.clone()) {
                servers.push(mcp_entry_to_server(name, server_val, artifact));
            }
        }
    }

    servers
}

fn mcp_server_map(val: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    val.get("mcpServers")
        .or_else(|| val.get("servers"))
        .and_then(|v| v.as_object())
}

fn mcp_entry_to_server(
    name: &str,
    val: &serde_json::Value,
    artifact: &ArtifactReport,
) -> McpServer {
    let transport = network_evidence::infer_transport(val);
    let network_ev = network_evidence::gather_server_evidence(name, val, &transport);
    let env_vars = network_evidence::resolve_env_refs(val);
    let network = network_evidence::classify_from_evidence(&transport, &network_ev);

    let auth = infer_auth(val);
    let verified = artifact.verification_status == "pass";
    let full_command = build_command_string(val);
    let tools = extract_mcp_tools(val, name);

    let source_path = first_path(artifact);
    let id = format!("{}-{}", name, short_hash(source_path));

    McpServer {
        id,
        name: name.to_string(),
        transport,
        network,
        auth,
        verified,
        command: full_command,
        tools,
        dependent_agents: Vec::new(),
        network_evidence: network_ev,
        env_vars,
    }
}

fn infer_auth(val: &serde_json::Value) -> String {
    let server_text = val.to_string().to_lowercase();
    let has_env_pattern = server_text.contains("${")
        || server_text.contains("process.env")
        || server_text.contains("os.environ");
    let has_cred_key = [
        "api_key",
        "apikey",
        "secret",
        "token",
        "password",
        "credential",
        "auth",
    ]
    .iter()
    .any(|kw| server_text.contains(kw));
    if has_cred_key || has_env_pattern {
        "API Key".to_string()
    } else {
        "None".to_string()
    }
}

fn build_command_string(val: &serde_json::Value) -> String {
    let command_str = val.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let args: Vec<&str> = val
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    if args.is_empty() {
        command_str.to_string()
    } else {
        format!("{} {}", command_str, args.join(" "))
    }
}

fn extract_mcp_tools(server_val: &serde_json::Value, server_name: &str) -> Vec<McpTool> {
    let mut tools = Vec::new();

    // Explicit tools array
    if let Some(tool_arr) = server_val.get("tools").and_then(|v| v.as_array()) {
        for tool in tool_arr {
            let tool_name = tool
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let desc = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            tools.push(McpTool {
                name: tool_name.to_string(),
                risk: "Medium".to_string(),
                description: desc.to_string(),
            });
        }
    }

    // Infer from command/args when no explicit tools
    if tools.is_empty() {
        let command = server_val
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let args: Vec<&str> = server_val
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        let has_shell = command.contains("sh") || args.iter().any(|a| a.contains("sh"));
        if has_shell {
            tools.push(McpTool {
                name: "run_shell_command".to_string(),
                risk: "High".to_string(),
                description: format!("Shell execution via {server_name}"),
            });
        }

        if command.contains("filesystem") || server_name.contains("filesystem") {
            tools.push(McpTool {
                name: "read_file".to_string(),
                risk: "Medium".to_string(),
                description: "Read file contents".to_string(),
            });
            tools.push(McpTool {
                name: "write_file".to_string(),
                risk: "Medium".to_string(),
                description: "Write file contents".to_string(),
            });
        }
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn infer_auth_api_key_from_env_pattern() {
        let val = json!({"command": "node", "env": {"TOKEN": "${SECRET}"}});
        assert_eq!(infer_auth(&val), "API Key");
    }

    #[test]
    fn infer_auth_api_key_from_keyword() {
        let val = json!({"command": "node", "api_key": "xxx"});
        assert_eq!(infer_auth(&val), "API Key");
    }

    #[test]
    fn infer_auth_none_when_no_creds() {
        let val = json!({"command": "node", "args": ["server.js"]});
        assert_eq!(infer_auth(&val), "None");
    }

    #[test]
    fn build_command_string_no_args() {
        let val = json!({"command": "npx"});
        assert_eq!(build_command_string(&val), "npx");
    }

    #[test]
    fn build_command_string_with_args() {
        let val = json!({"command": "npx", "args": ["-y", "@modelcontextprotocol/server"]});
        assert_eq!(
            build_command_string(&val),
            "npx -y @modelcontextprotocol/server"
        );
    }

    #[test]
    fn build_command_string_empty() {
        let val = json!({});
        assert_eq!(build_command_string(&val), "");
    }

    #[test]
    fn extract_mcp_tools_explicit() {
        let val = json!({
            "tools": [
                {"name": "read_file", "description": "Read a file"},
                {"name": "write_file", "description": "Write a file"}
            ]
        });
        let tools = extract_mcp_tools(&val, "test-server");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[1].name, "write_file");
    }

    #[test]
    fn extract_mcp_tools_inferred_shell() {
        let val = json!({"command": "bash", "args": ["-c", "server"]});
        let tools = extract_mcp_tools(&val, "shell-server");
        assert!(tools.iter().any(|t| t.name == "run_shell_command"));
        assert!(tools.iter().any(|t| t.risk == "High"));
    }

    #[test]
    fn extract_mcp_tools_inferred_filesystem() {
        let val = json!({"command": "filesystem-server"});
        let tools = extract_mcp_tools(&val, "test");
        assert!(tools.iter().any(|t| t.name == "read_file"));
        assert!(tools.iter().any(|t| t.name == "write_file"));
    }

    #[test]
    fn extract_mcp_tools_inferred_filesystem_from_name() {
        let val = json!({"command": "npx"});
        let tools = extract_mcp_tools(&val, "filesystem");
        assert!(tools.iter().any(|t| t.name == "read_file"));
    }

    #[test]
    fn extract_mcp_tools_empty_when_no_match() {
        let val = json!({"command": "node", "args": ["index.js"]});
        let tools = extract_mcp_tools(&val, "custom-server");
        assert!(tools.is_empty());
    }

    #[test]
    fn mcp_server_map_finds_mcp_servers_key() {
        let val = json!({"mcpServers": {"test": {}}});
        assert!(mcp_server_map(&val).is_some());
    }

    #[test]
    fn mcp_server_map_finds_servers_key() {
        let val = json!({"servers": {"test": {}}});
        assert!(mcp_server_map(&val).is_some());
    }

    #[test]
    fn mcp_server_map_none_when_missing() {
        let val = json!({"other": "data"});
        assert!(mcp_server_map(&val).is_none());
    }
}
