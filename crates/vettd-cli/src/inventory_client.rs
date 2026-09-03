//! Authenticated HTTP GET client for the user's private inventory routes.
//!
//! Always sets an `Authorization: Bearer <api_key>` header. If no API key is
//! configured, requests fail locally with `InventoryError::Unauthenticated`
//! rather than being sent unauthenticated.

use serde::de::DeserializeOwned;

const REQUEST_TIMEOUT_SECS: u64 = 10;

/// Errors from an authenticated inventory request.
#[derive(Debug)]
pub enum InventoryError {
    /// No API key is configured — the caller should tell the user to run `vettd auth`.
    Unauthenticated,
    /// Resource not found (HTTP 404).
    NotFound,
    /// Rate limited (HTTP 429). Surfaced to stderr; the process exits before returning.
    RateLimited,
    /// Server responded with a non-success status other than 404 or 429.
    ServerError(u16),
    /// Network or DNS failure — the server was not reachable.
    Unreachable(String),
    /// The response body could not be decoded.
    Decode(String),
}

impl std::fmt::Display for InventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthenticated => {
                write!(f, "not authenticated — run `vettd auth` to configure")
            }
            Self::NotFound => write!(f, "not found (404)"),
            Self::RateLimited => write!(f, "rate limited (429)"),
            Self::ServerError(s) => write!(f, "server error ({s})"),
            Self::Unreachable(msg) => write!(f, "unreachable: {msg}"),
            Self::Decode(msg) => write!(f, "decode error: {msg}"),
        }
    }
}

/// Perform an authenticated GET against the user's inventory and decode the
/// JSON response body as `T`.
///
/// Requires a configured API key (`vettd auth`). If none is configured,
/// returns `InventoryError::Unauthenticated` without making a network call.
pub fn fetch_json<T: DeserializeOwned>(url: &str) -> Result<T, InventoryError> {
    let auth = match crate::submit::load_auth_config() {
        Some(a) => a,
        None => return Err(InventoryError::Unauthenticated),
    };

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .http_status_as_error(false)
        .build()
        .into();

    match agent
        .get(url)
        .header("User-Agent", &crate::updater::user_agent_string())
        .header("Authorization", &format!("Bearer {}", auth.api_key))
        .call()
    {
        Ok(mut response) => {
            let status = response.status().as_u16();
            if status == 429 {
                eprintln!(
                    "Error: rate limited by the server (HTTP 429). Please wait and try again."
                );
                std::process::exit(1);
            }
            if status == 404 {
                return Err(InventoryError::NotFound);
            }
            if status != 200 {
                return Err(InventoryError::ServerError(status));
            }
            response
                .body_mut()
                .read_json::<T>()
                .map_err(|e| InventoryError::Decode(e.to_string()))
        }
        Err(e) => Err(InventoryError::Unreachable(e.to_string())),
    }
}

/// Returns true if an API key is currently configured (used by the CLI to
/// fail fast with exit code 3 before attempting any inventory subcommand).
pub fn is_authenticated() -> bool {
    crate::submit::load_auth_config().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_error_display_unauthenticated() {
        let msg = InventoryError::Unauthenticated.to_string();
        assert!(msg.contains("vettd auth"));
    }

    #[test]
    fn inventory_error_display_not_found() {
        assert_eq!(InventoryError::NotFound.to_string(), "not found (404)");
    }

    #[test]
    fn inventory_error_display_server_error() {
        assert_eq!(
            InventoryError::ServerError(500).to_string(),
            "server error (500)"
        );
    }
}
