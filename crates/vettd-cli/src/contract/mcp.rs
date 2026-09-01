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
    let verified = artifact.verification_status == "info";
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

pub(crate) fn build_command_string(val: &serde_json::Value) -> String {
    let command_str = val.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let args: Vec<&str> = val
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let redacted = redact_command_args(&args);
    if redacted.is_empty() {
        command_str.to_string()
    } else {
        format!("{} {}", command_str, redacted.join(" "))
    }
}

/// Mask values that follow secret-looking CLI flags in a command's args list,
/// secret-looking `KEY=VALUE` env assignments, and bare credential-bearing
/// values (e.g. `Authorization: Bearer <token>`).
///
/// Handles `--flag value`, `--flag=value`, `-k value`/`-k=value`, `KEY=value`
/// env-assignment, and a standalone value that embeds a `Bearer ` credential.
/// Non-secret args and the binary name are returned untouched. Uses the same
/// keyword set as [`infer_auth`] so "what counts as secret" stays consistent
/// across the contract module.
///
/// The redaction literal matches the one used by [`redact_url_credentials`]
/// (in `network_evidence.rs`): the bare word `REDACTED`.
fn redact_command_args(args: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut expect_secret_value = false;

    for arg in args {
        if expect_secret_value {
            out.push("REDACTED".to_string());
            expect_secret_value = false;
            continue;
        }

        // `--flag=value` / `-x=value` form: redact the value if secret-shaped.
        if let Some((flag, _value)) = split_flag_equals(arg) {
            if is_secret_flag(flag) {
                out.push(format!("{flag}=REDACTED"));
                continue;
            }
            out.push(arg.to_string());
            continue;
        }

        // `KEY=VALUE` env-assignment (non-flag): redact the value if the KEY is
        // secret-shaped (e.g. `API_KEY=secret`, `GITHUB_TOKEN=ghp_...`).
        if let Some((key, _value)) = split_env_assignment(arg) {
            if is_secret_env_key(key) {
                out.push(format!("{key}=REDACTED"));
                continue;
            }
            out.push(arg.to_string());
            continue;
        }

        // A standalone argument that itself embeds a credential — e.g.
        // `--header "Authorization: Bearer eyJ..."` — is redacted wholesale so
        // the token never reaches the payload.
        if is_secret_value_like(arg) {
            out.push("REDACTED".to_string());
            continue;
        }

        // `--flag value` / `-k value` form: if this arg is a secret-shaped
        // flag, the next arg is its value and should be masked.
        if is_secret_flag(arg) {
            out.push(arg.to_string());
            expect_secret_value = true;
            continue;
        }

        out.push(arg.to_string());
    }

    out
}

/// True if `flag` (with or without `--`/`-` prefix) looks like a credential
/// key, or is a short single-letter credential flag (`-k`, `-p`, `-s`, `-t`).
///
/// Mirrors the keyword list in [`infer_auth`] and adds a few common aliases
/// for MCP/CLI contexts (bearer, access-token, key, cred) plus the conventional
/// short flags for key/password/secret/token.
fn is_secret_flag(flag: &str) -> bool {
    let lower = flag.to_lowercase();
    let bare = lower.strip_prefix("--").unwrap_or(&lower);
    // Strip leading `-` once more (e.g. `-k` short flags).
    let bare = bare.strip_prefix('-').unwrap_or(bare);
    let keywords = [
        "api_key",
        "apikey",
        "secret",
        "token",
        "password",
        "credential",
        "auth",
        "bearer",
        "access_token",
        "key",
        "cred",
        "passphrase",
    ];
    if keywords.iter().any(|kw| bare.contains(kw)) {
        return true;
    }
    // Short single-letter credential flag. Only `-k` is unambiguously a
    // key/credential short flag; broad short flags like `-s` (silent), `-p`
    // (port), and `-t` (tag/type) are intentionally NOT treated as secret to
    // avoid over-redacting benign values. The value is a display-only
    // redaction (never leaks), so erring toward redaction here is still safe.
    matches!(bare, "k")
}

/// Split a `KEY=VALUE` env-assignment argument (non-flag) into `(key, value)`.
///
/// Returns `Some` only for strings that do not start with `-` (so flags are
/// handled by [`split_flag_equals`] instead) and have a non-empty key before
/// the first `=`. e.g. `API_KEY=secret` → `("API_KEY", "secret")`.
fn split_env_assignment(s: &str) -> Option<(&str, &str)> {
    if s.starts_with('-') {
        return None;
    }
    let eq = s.find('=')?;
    if eq == 0 {
        return None;
    }
    Some((&s[..eq], &s[eq + 1..]))
}

/// True if an env-assignment key looks secret-shaped (issue #196 AC #1).
///
/// `API_KEY`, `ANTHROPIC_API_KEY`, `GITHUB_TOKEN`, `SECRET`, `PASSWORD`, etc.
/// `.is_secret_flag` is reused because the env-assignment key and the flag body
/// share the same keyword semantics.
fn is_secret_env_key(key: &str) -> bool {
    is_secret_flag(key)
}

/// True if a standalone argument embeds an inline credential value that must
/// be redacted as a whole — e.g. `Authorization: Bearer <token>`.
fn is_secret_value_like(arg: &str) -> bool {
    let lower = arg.to_lowercase();
    lower.contains("bearer ")
}

/// Split a `--flag=value` / `-x=value` argument into `(flag, value)`. Returns
/// `None` if the string is not in that form (no `=` after the flag token, or
/// the argument doesn't start with `-`). The returned `flag` preserves the
/// `-`/`--` prefix so the caller can use it verbatim in output.
fn split_flag_equals(s: &str) -> Option<(&str, &str)> {
    if !s.starts_with('-') {
        return None;
    }
    let eq = s.find('=')?;
    if eq <= 1 {
        return None;
    }
    // Preserve the `-`/`--` prefix in the flag so output matches the original.
    Some((&s[..eq], &s[eq + 1..]))
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

    // ── redaction ────────────────────────────────────────────────────────────

    #[test]
    fn redact_command_args_api_key_space_separated() {
        let result = redact_command_args(&["--api-key", "sk-live-ABC123"]);
        assert_eq!(result, vec!["--api-key", "REDACTED"]);
    }

    #[test]
    fn redact_command_args_api_key_equals_form() {
        let result = redact_command_args(&["--api-key=sk-live-ABC123"]);
        assert_eq!(result, vec!["--api-key=REDACTED"]);
    }

    #[test]
    fn redact_command_args_token_flag() {
        let result = redact_command_args(&["--token", "ghp_abc123xyz"]);
        assert_eq!(result, vec!["--token", "REDACTED"]);
    }

    #[test]
    fn redact_command_args_password_flag() {
        let result = redact_command_args(&["--password", "s3cret!"]);
        assert_eq!(result, vec!["--password", "REDACTED"]);
    }

    #[test]
    fn redact_command_args_bearer_token() {
        let result = redact_command_args(&["--bearer", "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0"]);
        assert_eq!(result, vec!["--bearer", "REDACTED"]);
    }

    #[test]
    fn redact_command_args_access_token_flag() {
        let result = redact_command_args(&["--access-token", "ghp_xxxxxxxxxxxxxxxxxxxx"]);
        assert_eq!(result, vec!["--access-token", "REDACTED"]);
    }

    #[test]
    fn redact_command_args_secret_flag() {
        let result = redact_command_args(&["--secret", "akia-1234567890"]);
        assert_eq!(result, vec!["--secret", "REDACTED"]);
    }

    #[test]
    fn redact_command_args_keeps_benign_args() {
        let result = redact_command_args(&["--port", "3000", "--host", "localhost"]);
        assert_eq!(result, vec!["--port", "3000", "--host", "localhost"]);
    }

    #[test]
    fn redact_command_args_no_secrets_unchanged() {
        let result = redact_command_args(&["-y", "@modelcontextprotocol/server"]);
        assert_eq!(result, vec!["-y", "@modelcontextprotocol/server"]);
    }

    #[test]
    fn redact_command_args_empty_args() {
        let empty: &[&str] = &[];
        let result = redact_command_args(empty);
        assert!(result.is_empty());
    }

    #[test]
    fn build_command_string_redacts_secret_in_payload() {
        // End-to-end: a config that pins an API key should NOT leak the literal
        // value in the submitted command string. This maps to issue #196 AC #1.
        let val = json!({
            "command": "npx",
            "args": ["-y", "srv", "--api-key", "sk-live-ABC"]
        });
        let result = build_command_string(&val);
        assert!(
            !result.contains("sk-live-ABC"),
            "command string still contains literal secret: {result}"
        );
        assert_eq!(result, "npx -y srv --api-key REDACTED");
    }

    #[test]
    fn build_command_string_equals_form_redacts() {
        let val = json!({
            "command": "node",
            "args": ["server.js", "--token=ghp_xxxxxxxxxxxx"]
        });
        let result = build_command_string(&val);
        assert!(
            !result.contains("ghp_xxxxxxxxxxxx"),
            "command string still contains literal secret: {result}"
        );
        assert_eq!(result, "node server.js --token=REDACTED");
    }

    #[test]
    fn build_command_string_no_secrets_unchanged() {
        let val = json!({"command": "npx", "args": ["-y", "@modelcontextprotocol/server"]});
        assert_eq!(
            build_command_string(&val),
            "npx -y @modelcontextprotocol/server"
        );
    }

    // ── #196 AC #1 expanded redaction forms ────────────────────────────────

    #[test]
    fn redact_command_args_env_assignment_secret_key() {
        // Issue #196 AC #1: env-assignment form `API_KEY=secret` must redact
        // the value after `=`, even though it has no `--flag` prefix.
        let result = redact_command_args(&["API_KEY=sk-live-ABC", "--port", "3000"]);
        assert_eq!(result, vec!["API_KEY=REDACTED", "--port", "3000"]);
    }

    #[test]
    fn redact_command_args_env_assignment_benign_key_kept() {
        // A non-secret env assignment (e.g. a host or mode) must be left intact.
        let result = redact_command_args(&["HOST=localhost", "DEBUG=1"]);
        assert_eq!(result, vec!["HOST=localhost", "DEBUG=1"]);
    }

    #[test]
    fn redact_command_args_short_flag_k() {
        // `-k secret` (short key flag) must redact the following value.
        let result = redact_command_args(&["-k", "akia-1234567890"]);
        assert_eq!(result, vec!["-k", "REDACTED"]);
    }

    #[test]
    fn redact_command_args_short_flag_equals() {
        // `-k=secret` short-flag equals form.
        let result = redact_command_args(&["-k=akia-1234567890"]);
        assert_eq!(result, vec!["-k=REDACTED"]);
    }

    #[test]
    fn redact_command_args_bearer_header_value() {
        // `--header "Authorization: Bearer <token>"` — the header value embeds
        // a bearer credential and must be redacted wholesale.
        let result = redact_command_args(&[
            "--header",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
        ]);
        assert_eq!(result, vec!["--header", "REDACTED"]);
        assert!(
            !result.iter().any(|a| a.contains("eyJhbGci")),
            "bearer token must never appear in the redacted args"
        );
    }

    #[test]
    fn redact_command_args_keeps_benign_flags_with_equals() {
        // Benign non-secret flags/values stay untouched.
        let result = redact_command_args(&["--port=3000", "--host=localhost", "-v"]);
        assert_eq!(result, vec!["--port=3000", "--host=localhost", "-v"]);
    }

    #[test]
    fn build_command_string_redacts_env_assignment_form() {
        // End-to-end: a config that pins an API key via env-assignment must not
        // leak the literal value in the submitted command string.
        let val = json!({
            "command": "node",
            "args": ["server.js", "API_KEY=sk-live-ABC", "--port", "3000"]
        });
        let result = build_command_string(&val);
        assert!(
            !result.contains("sk-live-ABC"),
            "command string still contains literal env secret: {result}"
        );
        assert_eq!(result, "node server.js API_KEY=REDACTED --port 3000");
    }

    #[test]
    fn build_command_string_redacts_bearer_header() {
        let val = json!({
            "command": "curl",
            "args": ["-s", "--header", "Authorization: Bearer ghp_SECRETTOKEN", "https://api"]
        });
        let result = build_command_string(&val);
        assert!(
            !result.contains("ghp_SECRETTOKEN"),
            "bearer token must be redacted from the command string: {result}"
        );
        assert_eq!(result, "curl -s --header REDACTED https://api");
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
