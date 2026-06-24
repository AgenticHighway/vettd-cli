//! Output dispatch — writes scan results as JSON, human-readable, or file,
//! and handles submission orchestration.

use std::fs;
use std::path::PathBuf;

use crate::contract::build_contract_payload;
use crate::contract_sync;
use crate::formatters::{print_human, print_overview, print_summary};
use crate::models::ScanReport;
use crate::submit::{
    load_auth_config, submit_contract_payload, AuthConfig, DEFAULT_PRODUCTION_ENDPOINT,
};

pub fn emit(
    report: &ScanReport,
    scan_duration_ms: u64,
    json_output: bool,
    out: &Option<Option<PathBuf>>,
    summary: bool,
    full: bool,
    cmd_name: &str,
) {
    if json_output {
        let payload = build_contract_payload(report, scan_duration_ms);
        match serde_json::to_string_pretty(&payload) {
            Ok(json) => println!("{json}"),
            Err(e) => eprintln!("Error serializing contract payload: {e}"),
        }
    } else if summary {
        print_summary(report, cmd_name);
    } else if full {
        print_human(report, cmd_name);
    } else {
        print_overview(report, cmd_name);
    }

    if let Some(maybe_path) = out {
        write_json_report(report, scan_duration_ms, maybe_path);
    }
}

pub(crate) fn write_json_report(
    report: &ScanReport,
    scan_duration_ms: u64,
    maybe_path: &Option<PathBuf>,
) {
    let dest = match maybe_path {
        Some(p) => p.clone(),
        None => PathBuf::from("vettd-report.json"),
    };
    let payload = build_contract_payload(report, scan_duration_ms);
    match serde_json::to_string_pretty(&payload) {
        Ok(json) => {
            if let Err(e) = fs::write(&dest, &json) {
                eprintln!("Error writing report to {}: {}", dest.display(), e);
            } else {
                eprintln!("Report written to {}", dest.display());
            }
        }
        Err(e) => eprintln!("Error serializing contract payload: {e}"),
    }
}

/// Resolve auth (from flags + config file).
///
/// When `submit_flag` contains an explicit URL (i.e. `--submit <URL>`), that URL is
/// validated via [`crate::network::ensure_endpoint_allowed`] and `allow_public`
/// controls whether public (non-local) hosts are permitted.
///
/// When the endpoint comes from the saved config file or the built-in default
/// (no explicit URL flag), it is assumed the user already opted in when they
/// ran `vettd auth` or `vettd setup`, so public hosts are allowed without
/// requiring the flag again.
pub fn resolve_submit_auth(
    submit_flag: &Option<Option<String>>,
    api_key_flag: Option<&str>,
    allow_public: bool,
) -> Result<AuthConfig, String> {
    let saved = load_auth_config();

    let endpoint = match submit_flag {
        Some(Some(url)) => {
            // Explicit URL supplied via --submit <URL> — enforce the flag.
            crate::network::ensure_endpoint_allowed(url, allow_public).map_err(|e| {
                format!("{e}\nPass --allow-public-endpoint to permit public endpoints.")
            })?;
            url.clone()
        }
        _ => {
            // From saved config or built-in default — trust the prior opt-in;
            // still validate scheme/hostname format for defence in depth.
            let ep = saved
                .as_ref()
                .map(|c| c.endpoint.clone())
                .unwrap_or_else(|| DEFAULT_PRODUCTION_ENDPOINT.to_string());
            crate::network::ensure_endpoint_allowed(&ep, true)
                .map_err(|e| format!("Saved endpoint is invalid: {e}"))?;
            ep
        }
    };

    let api_key = match api_key_flag {
        Some(k) => k.to_string(),
        None => match saved.as_ref().map(|c| c.api_key.clone()) {
            Some(k) => k,
            None => return Err(
                "No API key provided. Pass --api-key for automation or run `vettd auth` to save credentials.".to_string(),
            ),
        },
    };

    Ok(AuthConfig { endpoint, api_key })
}

fn preflight_submission(auth: &AuthConfig) -> Result<(), String> {
    match contract_sync::sync_contract(&auth.endpoint) {
        Ok(result) => {
            if result.was_updated {
                eprintln!("  Contract cache updated to v{}.", result.remote_version);
            }
            if !result.compiled_matches {
                return Err(format!(
                    "Contract mismatch: server expects v{}, this build produces v{}.\nRun `vettd update` to get a compatible version.",
                    result.remote_version,
                    contract_sync::COMPILED_CONTRACT_VERSION
                ));
            }
            Ok(())
        }
        Err(contract_sync::SyncError::Unreachable(_))
        | Err(contract_sync::SyncError::ServerError(_)) => Ok(()),
    }
}

/// POST the payload using the resolved auth config.
pub fn do_submit(payload_json: &str, auth: &AuthConfig) -> Result<(), String> {
    preflight_submission(auth)?;
    eprintln!("Submitting scan to {}...", auth.endpoint);
    match submit_contract_payload(payload_json, auth) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Submission failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_submit_auth_explicit_public_url_blocked_by_default() {
        let submit_flag = Some(Some("https://example.com/api".to_string()));
        let result = resolve_submit_auth(&submit_flag, Some("ah_test"), false);
        assert!(
            result.is_err(),
            "public URL without --allow-public-endpoint should be rejected"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("allow-public-endpoint"),
            "error should mention --allow-public-endpoint, got: {msg}"
        );
    }

    #[test]
    fn resolve_submit_auth_explicit_public_url_allowed_with_flag() {
        let submit_flag = Some(Some("https://example.com/api".to_string()));
        let result = resolve_submit_auth(&submit_flag, Some("ah_test"), true);
        assert!(
            result.is_ok(),
            "public URL with allow_public=true should pass"
        );
        assert_eq!(result.unwrap().endpoint, "https://example.com/api");
    }

    #[test]
    fn resolve_submit_auth_explicit_local_url_always_allowed() {
        let submit_flag = Some(Some("http://localhost:8080/ingest".to_string()));
        let result = resolve_submit_auth(&submit_flag, Some("ah_test"), false);
        assert!(result.is_ok(), "local URL should pass without flag");
    }
}
