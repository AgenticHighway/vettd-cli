//! Unauthenticated HTTP GET client for public read routes.
//!
//! **Never** sets an `Authorization` header. All directory reads and the
//! reachability probe in `auth status` are public — this module enforces that
//! at the transport layer.

use serde::de::DeserializeOwned;

const REQUEST_TIMEOUT_SECS: u64 = 10;

/// Errors from a public read request.
#[derive(Debug)]
pub enum ReadError {
    /// Resource not found (HTTP 404).
    NotFound,
    /// Rate limited (HTTP 429). The error is surfaced to stderr; callers do
    /// not need to handle this variant — the process exits before it is returned.
    RateLimited,
    /// Server responded with a non-success status other than 404 or 429.
    ServerError(u16),
    /// Network or DNS failure — the server was not reachable.
    Unreachable(String),
    /// The response body could not be decoded.
    Decode(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found (404)"),
            Self::RateLimited => write!(f, "rate limited (429)"),
            Self::ServerError(s) => write!(f, "server error ({s})"),
            Self::Unreachable(msg) => write!(f, "unreachable: {msg}"),
            Self::Decode(msg) => write!(f, "decode error: {msg}"),
        }
    }
}

/// Perform an unauthenticated GET and decode the JSON response body as `T`.
///
/// On HTTP 429, prints a message to stderr and exits immediately — `RateLimited`
/// is never returned to the caller. On any other non-200 status or network
/// failure, the appropriate `ReadError` variant is returned.
///
/// # No auth header
///
/// This function never sets `Authorization`. It never calls `load_auth_config`
/// or `resolve_submit_auth`. All public read routes must go through this
/// function and not through the submit path.
pub fn fetch_json<T: DeserializeOwned>(url: &str) -> Result<T, ReadError> {
    // IMPORTANT: No Authorization header is set here. This is intentional.
    // All routes using this client are public. If you ever need auth on a
    // new route, do NOT add it here — create a separate authenticated client.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .http_status_as_error(false)
        .build()
        .into();

    match agent
        .get(url)
        .header("User-Agent", &crate::updater::user_agent_string())
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
                return Err(ReadError::NotFound);
            }
            if status != 200 {
                return Err(ReadError::ServerError(status));
            }
            response
                .body_mut()
                .read_json::<T>()
                .map_err(|e| ReadError::Decode(e.to_string()))
        }
        Err(e) => Err(ReadError::Unreachable(e.to_string())),
    }
}

/// Perform an unauthenticated GET and return the raw response body string.
///
/// Handles 404/429/non-200 the same way as `fetch_json`. Use this when you
/// need access to the raw body (e.g. for contract status reachability probes).
pub fn fetch_raw(url: &str) -> Result<String, ReadError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .http_status_as_error(false)
        .build()
        .into();

    match agent
        .get(url)
        .header("User-Agent", &crate::updater::user_agent_string())
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
                return Err(ReadError::NotFound);
            }
            if status != 200 {
                return Err(ReadError::ServerError(status));
            }
            response
                .body_mut()
                .read_to_string()
                .map_err(|e| ReadError::Decode(e.to_string()))
        }
        Err(e) => Err(ReadError::Unreachable(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_error_display_not_found() {
        assert_eq!(ReadError::NotFound.to_string(), "not found (404)");
    }

    #[test]
    fn read_error_display_server_error() {
        assert_eq!(
            ReadError::ServerError(500).to_string(),
            "server error (500)"
        );
    }

    #[test]
    fn read_error_display_unreachable() {
        let e = ReadError::Unreachable("connection refused".to_string());
        assert!(e.to_string().contains("unreachable"));
    }

    #[test]
    fn read_error_display_decode() {
        let e = ReadError::Decode("expected object".to_string());
        assert!(e.to_string().contains("decode error"));
    }
}
