//! Submission side-effects — config persistence and HTTP dispatch.
//!
//! Handles the parts that touch the outside world: reading/writing config
//! files and posting contract payloads to the ingest API.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DEFAULT_PRODUCTION_ENDPOINT: &str = "https://vettd.agentichighway.ai/api/scans/ingest";

// ---------------------------------------------------------------------------
// Global auth config (~/.config/vettd/config.json)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub endpoint: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

/// Return the path to `~/.config/vettd/config.json`.
pub fn auth_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("vettd").join("config.json"))
}

/// Load the global auth config. Returns `None` if the file doesn't exist.
pub fn load_auth_config() -> Option<AuthConfig> {
    let path = auth_config_path()?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save the global auth config to `~/.config/vettd/config.json`.
pub fn save_auth_config(config: &AuthConfig) -> Result<(), String> {
    let path =
        auth_config_path().ok_or_else(|| "Could not determine config directory".to_string())?;
    save_auth_config_to_path(&path, config)
}

fn save_auth_config_to_path(path: &Path, config: &AuthConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("Failed to secure config directory: {e}"))?;
        }
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("Failed to open config file {}: {e}", path.display()))?;
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write config to {}: {e}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to secure config file {}: {e}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, json)
            .map_err(|e| format!("Failed to write config to {}: {e}", path.display()))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP submission with retry
// ---------------------------------------------------------------------------

/// Backoff schedule in seconds for transient failures.
const BACKOFF_SECONDS: [u64; 3] = [5, 30, 120];
const MAX_ATTEMPTS: usize = 3;

/// HTTP status codes considered transient (retryable).
fn is_retryable(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Submit the contract payload to the ingest endpoint.
///
/// Uses the global `AuthConfig` for the endpoint and bearer token.
/// Retries transient failures with exponential backoff.
pub fn submit_contract_payload(payload_json: &str, auth: &AuthConfig) -> Result<(), String> {
    let mut last_err = String::new();

    // All HTTP responses (including 4xx/5xx) come through Ok so we can read bodies.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();

    for (attempt, &backoff) in BACKOFF_SECONDS.iter().enumerate().take(MAX_ATTEMPTS) {
        if attempt > 0 {
            eprintln!("  Attempt {}/{MAX_ATTEMPTS}...", attempt + 1);
        }

        let result = agent
            .post(&auth.endpoint)
            .header("Content-Type", "application/json")
            .header("Authorization", &format!("Bearer {}", auth.api_key))
            .header("User-Agent", &crate::updater::user_agent_string())
            .send(payload_json.as_bytes());

        match result {
            Ok(mut response) => {
                let status = response.status().as_u16();
                match status {
                    201 => {
                        let body: Value = response.body_mut().read_json().unwrap_or(json!({}));
                        let scan_id = body
                            .get("scanId")
                            .or_else(|| body.get("scan_id"))
                            .or_else(|| body.get("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        eprintln!("Scan accepted: {scan_id}");
                        return Ok(());
                    }
                    200 | 202..=208 | 226 => {
                        // Other 2xx — treat as success
                        return Ok(());
                    }
                    409 => {
                        eprintln!("Scan already submitted (duplicate).");
                        return Ok(());
                    }
                    400 => {
                        let body = response.body_mut().read_to_string().unwrap_or_default();
                        return Err(format!(
                            "Server rejected payload (400): {body}\n\
                             This is likely a scanner bug — the payload doesn't match the contract."
                        ));
                    }
                    401 => {
                        return Err(
                            "Authentication failed (401). Run `vettd auth --key <your-key>` to configure credentials."
                                .into(),
                        );
                    }
                    413 => {
                        let size_kb = payload_json.len() / 1024;
                        return Err(format!(
                            "Payload too large (413): ~{size_kb} KB. Try reducing scan scope."
                        ));
                    }
                    s if is_retryable(s) => {
                        let wait = if s == 429 {
                            // Respect Retry-After header if present
                            response
                                .headers()
                                .get("retry-after")
                                .and_then(|v| v.to_str().ok())
                                .and_then(|v| v.parse::<u64>().ok())
                                .unwrap_or(backoff)
                        } else {
                            backoff
                        };
                        let body = response.body_mut().read_to_string().unwrap_or_default();
                        let detail = if body.trim().is_empty() {
                            "no details provided".to_string()
                        } else {
                            body
                        };
                        last_err = format!("Server returned {s}: {detail}");
                        if attempt < MAX_ATTEMPTS - 1 {
                            eprintln!("  Server returned {s}, retrying in {wait}s...");
                            thread::sleep(Duration::from_secs(wait));
                            continue;
                        }
                    }
                    _ => {
                        let body = response.body_mut().read_to_string().unwrap_or_default();
                        let detail = if body.trim().is_empty() {
                            "no details provided".to_string()
                        } else {
                            body
                        };
                        return Err(format!("Server error ({status}): {detail}"));
                    }
                }
            }
            Err(e) => {
                last_err = format!("Connection error: {e}");
                if attempt < MAX_ATTEMPTS - 1 {
                    let wait = BACKOFF_SECONDS[attempt];
                    eprintln!("  {last_err}, retrying in {wait}s...");
                    thread::sleep(Duration::from_secs(wait));
                    continue;
                }
            }
        }
    }

    Err(format!("Failed after {MAX_ATTEMPTS} attempts: {last_err}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_config_debug_redacts_api_key() {
        let auth = AuthConfig {
            endpoint: "https://example.com/api".to_string(),
            api_key: "super-secret-key".to_string(),
        };

        let debug = format!("{auth:?}");
        assert!(debug.contains("https://example.com/api"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("super-secret-key"));
    }

    #[test]
    fn save_auth_config_to_custom_path_writes_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        let path = config_dir.join("config.json");
        let auth = AuthConfig {
            endpoint: "https://example.com/api".to_string(),
            api_key: "ah_test".to_string(),
        };

        save_auth_config_to_path(&path, &auth).unwrap();

        let saved = fs::read_to_string(&path).unwrap();
        let loaded: AuthConfig = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded.endpoint, auth.endpoint);
        assert_eq!(loaded.api_key, auth.api_key);
    }

    #[cfg(unix)]
    #[test]
    fn save_auth_config_to_custom_path_secures_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        let path = config_dir.join("config.json");
        let auth = AuthConfig {
            endpoint: "https://example.com/api".to_string(),
            api_key: "ah_test".to_string(),
        };

        save_auth_config_to_path(&path, &auth).unwrap();

        let dir_mode = fs::metadata(&config_dir).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }
}
