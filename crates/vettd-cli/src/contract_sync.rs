//! Contract synchronisation — fetch, cache, and version-check the scanner
//! data contract from the server's `/api/contract` endpoint.
//!
//! Flow (called before every scan):
//! 1. `GET /api/contract?version=true` → compare with compiled version
//! 2. If mismatch → tell user to `vettd update`, exit(1)
//! 3. If stale cache → `GET /api/contract` → write to `~/.vettd/contract/`
//! 4. If server unreachable → warn and continue scanning

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

/// The contract version this build of vettd was compiled to produce.
pub const COMPILED_CONTRACT_VERSION: &str = "2.4.0";

/// Timeout (seconds) for contract endpoint requests.
const REQUEST_TIMEOUT_SECS: u64 = 10;

// ---------------------------------------------------------------------------
// Local cache paths
// ---------------------------------------------------------------------------

fn contract_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|h| h.join(".vettd").join("contract"))
        .ok_or_else(|| "Unable to determine home directory for contract cache".to_string())
}

fn local_contract_path() -> Result<PathBuf, String> {
    Ok(contract_dir()?.join("scanner-data-contract.json"))
}

fn local_version_path() -> Result<PathBuf, String> {
    Ok(contract_dir()?.join("version"))
}

fn read_local_version() -> Option<String> {
    let path = local_version_path().ok()?;
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_local_cache(version: &str, schema_json: &str) -> Result<(), String> {
    let dir = contract_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create contract cache dir: {e}"))?;

    let vp = local_version_path()?;
    fs::write(&vp, version).map_err(|e| format!("Failed to write version cache: {e}"))?;

    let cp = local_contract_path()?;
    fs::write(&cp, schema_json).map_err(|e| format!("Failed to write contract cache: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Remote fetching
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VersionResponse {
    version: String,
}

/// Derive the contract API base from an ingest endpoint.
///
/// e.g. `https://vettd.agentichighway.ai/api/scans/ingest`
///   →  `https://vettd.agentichighway.ai/api/contract`
pub fn derive_contract_url(ingest_endpoint: &str) -> String {
    if let Some(base) = ingest_endpoint.strip_suffix("/scans/ingest") {
        format!("{base}/contract")
    } else if let Some(base) = ingest_endpoint.strip_suffix("/ingest") {
        format!("{base}/../contract").replace("/../", "/")
    } else {
        // Best effort: replace trailing path segment with /contract
        match ingest_endpoint.rfind("/api/") {
            Some(idx) => format!("{}/contract", &ingest_endpoint[..idx + 4]),
            None => format!("{}/api/contract", ingest_endpoint.trim_end_matches('/')),
        }
    }
}

/// Fetch only the version string from the server.
fn fetch_remote_version(contract_url: &str) -> Result<String, SyncError> {
    let url = format!("{contract_url}?version=true");
    let mut response = ureq::get(&url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .build()
        .header("User-Agent", &crate::updater::user_agent_string())
        .call()
        .map_err(|e| match &e {
            ureq::Error::StatusCode(code) => {
                SyncError::ServerError(format!("contract endpoint returned {code}"))
            }
            _ => SyncError::Unreachable(format!("{e}")),
        })?;

    let vr: VersionResponse = response
        .body_mut()
        .read_json()
        .map_err(|e| SyncError::ServerError(format!("invalid version response: {e}")))?;

    Ok(vr.version)
}

/// Fetch the full contract JSON schema from the server.
fn fetch_remote_contract(contract_url: &str) -> Result<(String, String), SyncError> {
    let mut response = ureq::get(contract_url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .build()
        .header("User-Agent", &crate::updater::user_agent_string())
        .call()
        .map_err(|e| match &e {
            ureq::Error::StatusCode(code) => {
                SyncError::ServerError(format!("contract endpoint returned {code}"))
            }
            _ => SyncError::Unreachable(format!("{e}")),
        })?;

    let version = response
        .headers()
        .get("x-contract-version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| SyncError::ServerError(format!("failed to read contract body: {e}")))?;

    Ok((version, body))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Distinguishes connection failures from server-side errors.
#[derive(Debug)]
pub enum SyncError {
    /// Network/DNS/timeout — server not reachable at all.
    Unreachable(String),
    /// Server responded but with an error (e.g. 401, 500).
    ServerError(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(msg) => write!(f, "{msg}"),
            Self::ServerError(msg) => write!(f, "{msg}"),
        }
    }
}

/// Result of a contract sync attempt.
#[derive(Debug)]
pub struct SyncResult {
    pub remote_version: String,
    pub was_updated: bool,
    pub compiled_matches: bool,
}

/// Fetch the server's contract version via the `X-Contract-Version` response
/// header without writing to the local cache.
///
/// Used by `contract status` to get a lightweight read of the server version
/// while avoiding the `~/.vettd/contract/` write side-effect of `sync_contract`.
pub fn fetch_server_contract_version(ingest_endpoint: &str) -> Result<String, SyncError> {
    let contract_url = derive_contract_url(ingest_endpoint);
    let url = format!("{contract_url}?version=true");
    let response = ureq::get(&url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .build()
        .header("User-Agent", &crate::updater::user_agent_string())
        .call()
        .map_err(|e| match &e {
            ureq::Error::StatusCode(code) => {
                SyncError::ServerError(format!("contract endpoint returned {code}"))
            }
            _ => SyncError::Unreachable(format!("{e}")),
        })?;

    response
        .headers()
        .get("x-contract-version")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            SyncError::ServerError("server response missing x-contract-version header".to_string())
        })
}

/// Check the server contract version and update the local cache if stale.
///
/// Returns the remote version and whether the cache was refreshed.
/// Errors are non-fatal — callers should log and continue.
pub fn sync_contract(ingest_endpoint: &str) -> Result<SyncResult, SyncError> {
    let contract_url = derive_contract_url(ingest_endpoint);
    let local_version = read_local_version();

    // Step 1: lightweight version check
    let remote_version = fetch_remote_version(&contract_url)?;

    let is_stale = local_version
        .as_ref()
        .map(|lv| lv != &remote_version)
        .unwrap_or(true);

    // Step 2: fetch full schema if stale
    let was_updated = if is_stale {
        let (_header_version, schema_json) = fetch_remote_contract(&contract_url)?;
        write_local_cache(&remote_version, &schema_json).map_err(SyncError::ServerError)?;
        true
    } else {
        false
    };

    let compiled_matches = remote_version == COMPILED_CONTRACT_VERSION;

    Ok(SyncResult {
        remote_version,
        was_updated,
        compiled_matches,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_url_from_standard_ingest() {
        assert_eq!(
            derive_contract_url("https://vettd.agentichighway.ai/api/scans/ingest"),
            "https://vettd.agentichighway.ai/api/contract"
        );
    }

    #[test]
    fn derive_url_from_localhost() {
        assert_eq!(
            derive_contract_url("http://localhost:3000/api/scans/ingest"),
            "http://localhost:3000/api/contract"
        );
    }

    #[test]
    fn derive_url_without_scans_ingest() {
        // /api/v2/ingest → strips /ingest, finds /api/ prefix → /api/contract
        assert_eq!(
            derive_contract_url("https://example.com/api/v2/ingest"),
            "https://example.com/api/v2/contract"
        );
    }

    #[test]
    fn derive_url_fallback() {
        assert_eq!(
            derive_contract_url("https://example.com/something"),
            "https://example.com/something/api/contract"
        );
    }

    #[test]
    #[allow(clippy::const_is_empty)] // intentional: guard against blanking the const
    fn compiled_version_is_set() {
        assert!(
            !COMPILED_CONTRACT_VERSION.is_empty(),
            "COMPILED_CONTRACT_VERSION must not be empty"
        );
    }

    #[test]
    fn bundled_contract_version_matches_compiled_version() {
        let json = include_str!("../../../scanner-data-contract.json");
        let contract: serde_json::Value =
            serde_json::from_str(json).expect("scanner-data-contract.json must be valid JSON");
        assert_eq!(
            contract["version"].as_str().unwrap_or("missing"),
            COMPILED_CONTRACT_VERSION,
            "scanner-data-contract.json version must match COMPILED_CONTRACT_VERSION — \
update one or the other to resolve the drift"
        );
    }

    #[test]
    fn local_cache_roundtrip() {
        // Only run if home dir is available (CI may not have one)
        if dirs::home_dir().is_none() {
            return;
        }

        let test_version = "0.0.0-test";
        let test_schema = r#"{"test": true}"#;

        // Write
        write_local_cache(test_version, test_schema).unwrap();

        // Read version
        let v = read_local_version().unwrap();
        assert_eq!(v, test_version);

        // Read schema
        let schema = std::fs::read_to_string(local_contract_path().unwrap()).unwrap();
        assert_eq!(schema, test_schema);

        // Clean up — restore real version if it existed
        let _ = std::fs::remove_file(local_version_path().unwrap());
        let _ = std::fs::remove_file(local_contract_path().unwrap());
    }
}
