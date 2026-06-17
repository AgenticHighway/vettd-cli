//! Pure-logic payload builder for the ingest API.
//!
//! This module contains **no I/O** — every function is a deterministic
//! transformation from input data to `serde_json::Value`.

use serde_json::{json, Value};

use crate::capabilities::derive_capabilities;
use crate::models::{ArtifactReport, ScanReport};
use crate::network::is_local_or_private_host;

pub const PAYLOAD_SCHEMA_VERSION: &str = "v1";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Clamp `status` to one of the three accepted values.
pub fn normalize_status(status: &str) -> &str {
    match status {
        "pass" | "conditional_pass" | "fail" => status,
        _ => "conditional_pass",
    }
}

/// Extract the first path from an artifact's metadata, or `"unknown"`.
fn first_location(artifact: &ArtifactReport) -> &str {
    artifact
        .metadata
        .get("paths")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
}

// ---------------------------------------------------------------------------
// Per-artifact record
// ---------------------------------------------------------------------------

/// Convert a single `ArtifactReport` into the ingest-API record shape.
pub fn artifact_to_ingest_record(artifact: &ArtifactReport) -> Value {
    let location = first_location(artifact);
    let hash = if artifact.artifact_hash.is_empty() {
        &artifact.artifact_id
    } else {
        &artifact.artifact_hash
    };
    let status = normalize_status(&artifact.verification_status);

    json!({
        "artifact_hash": hash,
        "name": format!("{}:{}", artifact.artifact_type, location),
        "kind": artifact.artifact_type,
        "schema": PAYLOAD_SCHEMA_VERSION,
        "capabilities": derive_capabilities(artifact),
        "signals": artifact.signals,
        "risk_reasons": artifact.risk_reasons,
        "confidence": artifact.confidence,
        "risk_score": artifact.risk_score,
        "status": status,
        "registry_candidate": artifact.registry_eligible,
        "location": location,
        "metadata": artifact.metadata,
        "evidence": json!({
            "artifact_id": artifact.artifact_id,
            "artifact_scope": artifact.artifact_scope,
            "registry_eligible": artifact.registry_eligible,
        }),
    })
}

// ---------------------------------------------------------------------------
// Endpoint details (pure URL parsing — no network calls)
// ---------------------------------------------------------------------------

/// Build an `endpoint_details_*` block by parsing `endpoint` manually.
///
/// No DNS resolution or network I/O is performed — only the hostname string
/// is checked against `is_local_or_private_host`.
pub fn endpoint_details(endpoint: Option<&str>, source: &str) -> Value {
    let ep = match endpoint {
        Some(e) if !e.is_empty() => e,
        _ => {
            return json!({
                "endpoint_details_url": Value::Null,
                "endpoint_details_hostname": Value::Null,
                "endpoint_details_is_local": true,
                "endpoint_details_source": source,
            });
        }
    };

    let (scheme, rest) = ep.split_once("://").unwrap_or(("unknown", ep));
    let authority = rest.split('/').next().unwrap_or("");
    let hostname = strip_port(authority);
    let is_local = is_local_or_private_host(hostname);

    json!({
        "endpoint_details_url": ep,
        "endpoint_details_scheme": scheme,
        "endpoint_details_hostname": hostname,
        "endpoint_details_is_local": is_local,
        "endpoint_details_source": source,
    })
}

/// Remove an optional `:port` suffix, handling `[ipv6]:port` notation.
fn strip_port(authority: &str) -> &str {
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or("");
    }
    if authority.matches(':').count() == 1 {
        return authority.split(':').next().unwrap_or("");
    }
    authority
}

// ---------------------------------------------------------------------------
// Full ingest payload
// ---------------------------------------------------------------------------

/// Assemble the complete ingest API payload from a finished `ScanReport`.
///
/// # Arguments
///
/// * `report`                  — completed scan report.
/// * `include_informational`   — when `false`, only `registry_eligible` artifacts are included.
/// * `endpoint`                — optional ingest endpoint URL (for metadata).
/// * `source`                  — human label for the submission source (e.g. `"cli"`).
/// * `scanner_uuid`            — resolved scanner identity.
/// * `scanner_account_uuid`    — resolved account identity.
/// * `client_emitted_at`       — optional ISO-8601 timestamp override.
/// * `lite_mode_locked_summary`— optional pre-computed lite-mode summary to embed.
#[allow(clippy::too_many_arguments)]
pub fn build_ingest_payload(
    report: &ScanReport,
    include_informational: bool,
    endpoint: Option<&str>,
    source: &str,
    scanner_uuid: &str,
    scanner_account_uuid: &str,
    client_emitted_at: Option<&str>,
    lite_mode_locked_summary: Option<&Value>,
) -> Value {
    // -- filter artifacts ---------------------------------------------------
    let artifacts: Vec<Value> = report
        .artifacts
        .iter()
        .filter(|a| include_informational || a.registry_eligible)
        .map(artifact_to_ingest_record)
        .collect();

    let total_count = report.artifacts.len();
    let registry_count = report
        .artifacts
        .iter()
        .filter(|a| a.registry_eligible)
        .count();
    let informational_count = total_count - registry_count;

    // -- client details -----------------------------------------------------
    let emitted_at = client_emitted_at.unwrap_or(&report.timestamp).to_string();

    let client_details = json!({
        "client_details_scanner_name": "vettd",
        "client_details_scanner_version": env!("CARGO_PKG_VERSION"),
        "client_details_platform_os": std::env::consts::OS,
        "client_details_platform_arch": std::env::consts::ARCH,
        "client_details_total_artifacts": total_count,
        "client_details_registry_artifacts": registry_count,
        "client_details_informational_artifacts": informational_count,
    });

    // -- endpoint details ---------------------------------------------------
    let ep_details = endpoint_details(endpoint, source);

    // -- top-level payload --------------------------------------------------
    let mut payload = json!({
        "schema_version": PAYLOAD_SCHEMA_VERSION,
        "run_id": report.run_id,
        "scanned_path": report.scanned_path,
        "timestamp": report.timestamp,
        "client_emitted_at": emitted_at,
        "scanner_uuid": scanner_uuid,
        "scanner_account_uuid": scanner_account_uuid,
        "source": source,
        "artifacts": artifacts,
        "client_details": client_details,
        "endpoint_details": ep_details,
    });

    if let Some(summary) = lite_mode_locked_summary {
        payload
            .as_object_mut()
            .expect("payload is an object")
            .insert("lite_mode_summary".to_string(), summary.clone());
    }

    payload
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ArtifactReport, ScanReport};

    fn sample_artifact(eligible: bool) -> ArtifactReport {
        let mut a = ArtifactReport::new("mcp_config", 0.9);
        a.verification_status = "pass".to_string();
        a.registry_eligible = eligible;
        a.artifact_hash = "abc123".to_string();
        a.artifact_id = "id456".to_string();
        a.metadata
            .insert("paths".to_string(), serde_json::json!(["/tmp/mcp.json"]));
        a
    }

    #[test]
    fn normalize_status_known_values() {
        assert_eq!(normalize_status("pass"), "pass");
        assert_eq!(normalize_status("conditional_pass"), "conditional_pass");
        assert_eq!(normalize_status("fail"), "fail");
    }

    #[test]
    fn normalize_status_unknown_falls_back() {
        assert_eq!(normalize_status("unknown"), "conditional_pass");
        assert_eq!(normalize_status(""), "conditional_pass");
    }

    #[test]
    fn artifact_record_uses_hash_when_present() {
        let a = sample_artifact(true);
        let rec = artifact_to_ingest_record(&a);
        assert_eq!(rec["artifact_hash"], "abc123");
    }

    #[test]
    fn artifact_record_falls_back_to_id() {
        let mut a = sample_artifact(true);
        a.artifact_hash = String::new();
        let rec = artifact_to_ingest_record(&a);
        assert_eq!(rec["artifact_hash"], "id456");
    }

    #[test]
    fn endpoint_details_none() {
        let d = endpoint_details(None, "cli");
        assert_eq!(d["endpoint_details_is_local"], true);
        assert_eq!(d["endpoint_details_source"], "cli");
    }

    #[test]
    fn endpoint_details_localhost() {
        let d = endpoint_details(Some("http://localhost:3000/api"), "cli");
        assert_eq!(d["endpoint_details_hostname"], "localhost");
        assert_eq!(d["endpoint_details_is_local"], true);
    }

    #[test]
    fn endpoint_details_public() {
        let d = endpoint_details(Some("https://api.example.com/ingest"), "ci");
        assert_eq!(d["endpoint_details_hostname"], "api.example.com");
        assert_eq!(d["endpoint_details_is_local"], false);
    }

    #[test]
    fn build_payload_filters_non_eligible() {
        let mut report = ScanReport::new("/tmp");
        report.artifacts.push(sample_artifact(true));
        report.artifacts.push(sample_artifact(false));

        let payload =
            build_ingest_payload(&report, false, None, "test", "uuid1", "acct1", None, None);
        let arts = payload["artifacts"].as_array().unwrap();
        assert_eq!(arts.len(), 1);
    }

    #[test]
    fn build_payload_includes_all_when_flag_set() {
        let mut report = ScanReport::new("/tmp");
        report.artifacts.push(sample_artifact(true));
        report.artifacts.push(sample_artifact(false));

        let payload =
            build_ingest_payload(&report, true, None, "test", "uuid1", "acct1", None, None);
        let arts = payload["artifacts"].as_array().unwrap();
        assert_eq!(arts.len(), 2);
    }

    #[test]
    fn build_payload_embeds_lite_mode_summary() {
        let report = ScanReport::new("/tmp");
        let summary = json!({"total": 0});
        let payload =
            build_ingest_payload(&report, true, None, "test", "u", "a", None, Some(&summary));
        assert_eq!(payload["lite_mode_summary"]["total"], 0);
    }
}
