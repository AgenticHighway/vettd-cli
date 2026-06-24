use std::net::IpAddr;

/// Returns `true` when the hostname refers to the local machine or a
/// private / link-local address range.
pub fn is_local_or_private_host(hostname: &str) -> bool {
    let lower = hostname.to_ascii_lowercase();

    if lower == "localhost" || lower == "127.0.0.1" || lower == "::1" {
        return true;
    }

    match lower.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            if v4.is_loopback() {
                return true;
            }
            let octets = v4.octets();
            // 10.0.0.0/8
            if octets[0] == 10 {
                return true;
            }
            // 172.16.0.0/12
            if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                return true;
            }
            // 192.168.0.0/16
            if octets[0] == 192 && octets[1] == 168 {
                return true;
            }
            false
        }
        Ok(IpAddr::V6(v6)) => {
            if v6.is_loopback() {
                return true;
            }
            let segments = v6.segments();
            let first = segments[0];
            // fc00::/7 — unique local
            if (first & 0xfe00) == 0xfc00 {
                return true;
            }
            // fe80::/10 — link-local
            if (first & 0xffc0) == 0xfe80 {
                return true;
            }
            false
        }
        Err(_) => false,
    }
}

/// Validates that `endpoint` is an allowed URL.
///
/// Rules:
/// - Scheme must be `http` or `https`.
/// - Hostname must be present and non-empty.
/// - Unless `allow_public` is true the host must be local/private.
pub fn ensure_endpoint_allowed(endpoint: &str, allow_public: bool) -> Result<(), String> {
    let (scheme, rest) = endpoint
        .split_once("://")
        .ok_or_else(|| format!("Invalid endpoint (missing ://): {endpoint}"))?;

    match scheme {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "Unsupported scheme '{other}' in endpoint: {endpoint}"
            ));
        }
    }

    // Strip path / query / fragment to isolate host(:port).
    let authority = rest.split('/').next().unwrap_or("");
    let hostname = strip_port(authority);

    if hostname.is_empty() {
        return Err(format!("No hostname found in endpoint: {endpoint}"));
    }

    // Require HTTPS for public hosts. Local/private hosts may use plain HTTP
    // (dev servers, self-hosted VPC deployments with an internal TLS terminator).
    if scheme == "http" && !is_local_or_private_host(hostname) {
        return Err(format!(
            "Endpoint '{endpoint}' uses HTTP with a public host. \
             Use HTTPS to protect credentials in transit."
        ));
    }

    if !allow_public && !is_local_or_private_host(hostname) {
        return Err(format!(
            "Endpoint host '{hostname}' is not a local/private address. \
             Set allow_public to permit public endpoints."
        ));
    }

    Ok(())
}

/// Derive a sibling API URL from an ingest endpoint.
///
/// Strips the ingest-specific path segments and appends `resource`.
///
/// ```
/// # use vettd::network::derive_api_url;
/// assert_eq!(
///     derive_api_url("https://vettd.agentichighway.ai/api/scans/ingest", "directory"),
///     "https://vettd.agentichighway.ai/api/directory"
/// );
/// ```
pub fn derive_api_url(ingest_endpoint: &str, resource: &str) -> String {
    if let Some(base) = ingest_endpoint.strip_suffix("/scans/ingest") {
        format!("{base}/{resource}")
    } else if let Some(base) = ingest_endpoint.strip_suffix("/ingest") {
        format!("{base}/../{resource}").replace("/../", "/")
    } else {
        match ingest_endpoint.rfind("/api/") {
            Some(idx) => format!("{}/{resource}", &ingest_endpoint[..idx + 4]),
            None => format!("{}/api/{resource}", ingest_endpoint.trim_end_matches('/')),
        }
    }
}

/// Extract the `host` (and optional `:port`) portion of an endpoint URL for
/// display — strips the scheme and any trailing path.
///
/// ```
/// # use vettd::network::endpoint_display_host;
/// assert_eq!(endpoint_display_host("https://vettd.example.com/api/scans"), "vettd.example.com");
/// assert_eq!(endpoint_display_host("http://localhost:3000/ingest"), "localhost:3000");
/// ```
pub fn endpoint_display_host(endpoint: &str) -> &str {
    let rest = match endpoint.split_once("://") {
        Some((_, r)) => r,
        None => endpoint,
    };
    rest.split('/').next().unwrap_or(rest)
}

/// Remove an optional `:port` suffix, handling IPv6 bracket notation.
fn strip_port(authority: &str) -> &str {
    // [::1]:8080 → ::1
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or("");
    }
    // host:port  (only strip when there is exactly one colon → IPv4 / hostname)
    if authority.matches(':').count() == 1 {
        return authority.split(':').next().unwrap_or("");
    }
    authority
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- is_local_or_private_host ----

    #[test]
    fn localhost_variants() {
        assert!(is_local_or_private_host("localhost"));
        assert!(is_local_or_private_host("LOCALHOST"));
        assert!(is_local_or_private_host("127.0.0.1"));
        assert!(is_local_or_private_host("::1"));
    }

    #[test]
    fn private_ipv4() {
        assert!(is_local_or_private_host("10.0.0.1"));
        assert!(is_local_or_private_host("10.255.255.255"));
        assert!(is_local_or_private_host("172.16.0.1"));
        assert!(is_local_or_private_host("172.31.255.255"));
        assert!(is_local_or_private_host("192.168.0.1"));
        assert!(is_local_or_private_host("192.168.255.255"));
    }

    #[test]
    fn public_ipv4() {
        assert!(!is_local_or_private_host("8.8.8.8"));
        assert!(!is_local_or_private_host("172.32.0.1"));
        assert!(!is_local_or_private_host("192.169.0.1"));
    }

    #[test]
    fn private_ipv6() {
        assert!(is_local_or_private_host("fc00::1"));
        assert!(is_local_or_private_host("fd12:3456::1"));
        assert!(is_local_or_private_host("fe80::1"));
    }

    #[test]
    fn public_ipv6() {
        assert!(!is_local_or_private_host("2001:db8::1"));
    }

    #[test]
    fn arbitrary_hostname() {
        assert!(!is_local_or_private_host("example.com"));
    }

    // ---- ensure_endpoint_allowed ----

    #[test]
    fn valid_local_endpoint() {
        assert!(ensure_endpoint_allowed("http://localhost:8080/api", false).is_ok());
        assert!(ensure_endpoint_allowed("https://192.168.1.1/path", false).is_ok());
    }

    #[test]
    fn public_endpoint_blocked_by_default() {
        assert!(ensure_endpoint_allowed("https://example.com/api", false).is_err());
    }

    #[test]
    fn public_endpoint_allowed_when_flag_set() {
        assert!(ensure_endpoint_allowed("https://example.com/api", true).is_ok());
    }

    #[test]
    fn missing_scheme() {
        assert!(ensure_endpoint_allowed("localhost:8080", false).is_err());
    }

    #[test]
    fn bad_scheme() {
        assert!(ensure_endpoint_allowed("ftp://localhost", false).is_err());
    }

    #[test]
    fn empty_hostname() {
        assert!(ensure_endpoint_allowed("http:///path", false).is_err());
    }

    // ---- endpoint_display_host ----

    #[test]
    fn display_host_strips_scheme_and_path() {
        assert_eq!(
            endpoint_display_host("https://vettd.agentichighway.ai/api/scans/ingest"),
            "vettd.agentichighway.ai"
        );
        assert_eq!(
            endpoint_display_host("http://localhost:3000/api/ingest"),
            "localhost:3000"
        );
    }

    #[test]
    fn display_host_no_scheme() {
        assert_eq!(endpoint_display_host("localhost:3000"), "localhost:3000");
    }

    // ---- HTTPS enforcement ----

    #[test]
    fn public_http_rejected() {
        // Public host with plain HTTP must be rejected regardless of allow_public.
        assert!(ensure_endpoint_allowed("http://example.com/api", true).is_err());
        assert!(ensure_endpoint_allowed("http://8.8.8.8/api", true).is_err());
    }

    #[test]
    fn local_http_allowed() {
        // Local/private hosts may use HTTP (dev servers, VPC deployments).
        assert!(ensure_endpoint_allowed("http://localhost:8080/api", true).is_ok());
        assert!(ensure_endpoint_allowed("http://192.168.1.1/api", true).is_ok());
        assert!(ensure_endpoint_allowed("http://10.0.0.1/api", true).is_ok());
    }

    #[test]
    fn public_https_allowed_with_flag() {
        assert!(ensure_endpoint_allowed("https://example.com/api", true).is_ok());
    }

    // ---- derive_api_url ----

    #[test]
    fn derive_url_from_standard_ingest_endpoint() {
        assert_eq!(
            derive_api_url(
                "https://vettd.agentichighway.ai/api/scans/ingest",
                "directory"
            ),
            "https://vettd.agentichighway.ai/api/directory"
        );
        assert_eq!(
            derive_api_url(
                "https://vettd.agentichighway.ai/api/scans/ingest",
                "contract"
            ),
            "https://vettd.agentichighway.ai/api/contract"
        );
    }

    #[test]
    fn derive_url_from_localhost() {
        assert_eq!(
            derive_api_url("http://localhost:3000/api/scans/ingest", "directory"),
            "http://localhost:3000/api/directory"
        );
    }
}
