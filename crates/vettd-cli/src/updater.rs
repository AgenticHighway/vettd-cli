//! Self-update mechanism — check for new versions and replace the binary.
//!
//! Architecture:
//!   1. `check_for_update()` — GET the manifest and detached signature envelope
//!   2. Verify the manifest with the embedded KMS-backed public key
//!   3. Compare semver and return the matching artifact
//!   4. `perform_update()` — download, verify SHA-256, backup, replace
//!
//! All downloads are over HTTPS. The manifest must verify before Vettd trusts
//! artifact hashes or URLs. The binary is never executed during the update —
//! only extracted, verified, and placed.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature as EcdsaSignature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Vettd endpoint for the current signed release manifest.
const MANIFEST_URL: &str = "https://vettd.agentichighway.ai/api/scanner/latest";

/// Detached AWS KMS signature envelope for the current release manifest.
const MANIFEST_SIGNATURE_URL: &str = "https://vettd.agentichighway.ai/api/scanner/latest/signature";

/// Build-time SPKI DER public key used to verify official update manifests.
const UPDATE_PUBLIC_KEY_DER_B64: Option<&str> = option_env!("PROOV_UPDATE_PUBLIC_KEY_DER_B64");

/// Signing algorithm emitted by the AWS KMS release signer.
const KMS_SIGNATURE_ALGORITHM: &str = "ECDSA_SHA_256";

/// HTTP timeout for update-check requests.
const CHECK_TIMEOUT_SECS: u64 = 3;

/// HTTP timeout for the active download.
const DOWNLOAD_TIMEOUT_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// The expanded `latest.json` manifest served from S3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    #[serde(rename = "manifestVersion", default)]
    pub manifest_version: Option<u32>,
    pub version: String,
    pub date: String,
    #[serde(default)]
    pub artifacts: std::collections::HashMap<String, ArtifactInfo>,
}

/// One platform's downloadable artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactInfo {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/// Detached signature envelope stored alongside the signed manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestSignatureEnvelope {
    pub algorithm: String,
    #[serde(rename = "keyId", default)]
    pub key_id: Option<String>,
    pub signature: String,
}

/// Result of comparing the manifest version to the running binary.
#[derive(Debug)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub is_newer: bool,
    pub artifact: Option<ArtifactInfo>,
}

// ---------------------------------------------------------------------------
// Platform key
// ---------------------------------------------------------------------------

/// Map the running OS + arch to the artifact key in `latest.json`.
pub fn platform_key() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-amd64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        ("windows", "x86_64") => Ok("windows-amd64"),
        (os, arch) => Err(format!("Unsupported platform: {os}/{arch}")),
    }
}

// ---------------------------------------------------------------------------
// Version comparison (simple semver: major.minor.patch)
// ---------------------------------------------------------------------------

fn is_version_newer(current: &str, latest: &str) -> bool {
    crate::semver::cmp(current, latest)
        .map(|ord| ord == std::cmp::Ordering::Less)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn vettd_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|h| h.join(".vettd"))
        .ok_or_else(|| "Cannot determine home directory".to_string())
}

fn downloads_dir() -> Result<PathBuf, String> {
    Ok(vettd_dir()?.join("downloads"))
}

fn backup_path() -> Result<PathBuf, String> {
    Ok(vettd_dir()?.join("vettd.backup"))
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

fn update_public_key_der_b64() -> Result<&'static str, String> {
    UPDATE_PUBLIC_KEY_DER_B64
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "This build does not include an embedded Vettd update verification \
             key. Self-update is only available in official signed builds. \
             Rebuild with PROOV_UPDATE_PUBLIC_KEY_DER_B64 set or install an official \
             release."
                .to_string()
        })
}

fn fetch_url_bytes(url: &str, timeout_secs: u64) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .build()
        .into();

    let mut response = agent
        .get(url)
        .header("User-Agent", &user_agent_string())
        .call()
        .map_err(|e| format!("Failed to fetch {url}: {e}"))?;

    response
        .body_mut()
        .read_to_vec()
        .map_err(|e| format!("Failed to read {url}: {e}"))
}

fn fetch_manifest_bytes(timeout_secs: u64) -> Result<Vec<u8>, String> {
    fetch_url_bytes(MANIFEST_URL, timeout_secs)
        .map_err(|e| format!("Failed to fetch update manifest: {e}"))
}

fn fetch_manifest_signature(timeout_secs: u64) -> Result<ManifestSignatureEnvelope, String> {
    let bytes = fetch_url_bytes(MANIFEST_SIGNATURE_URL, timeout_secs)
        .map_err(|e| format!("Failed to fetch manifest signature: {e}"))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("Manifest signature envelope was not valid JSON: {e}"))
}

fn decode_update_public_key(public_key_der_b64: &str) -> Result<VerifyingKey, String> {
    let public_key_der = BASE64_STANDARD
        .decode(public_key_der_b64.trim())
        .map_err(|e| format!("Embedded update verification key was not valid base64: {e}"))?;
    VerifyingKey::from_public_key_der(&public_key_der)
        .map_err(|e| format!("Embedded update verification key is invalid: {e}"))
}

fn verify_manifest_signature(
    manifest_bytes: &[u8],
    envelope: &ManifestSignatureEnvelope,
    public_key_der_b64: &str,
) -> Result<(), String> {
    if envelope.algorithm != KMS_SIGNATURE_ALGORITHM {
        return Err(format!(
            "Unsupported manifest signature algorithm: {}. Expected {}.",
            envelope.algorithm, KMS_SIGNATURE_ALGORITHM
        ));
    }

    let verifying_key = decode_update_public_key(public_key_der_b64)?;
    let signature_bytes = BASE64_STANDARD
        .decode(envelope.signature.trim())
        .map_err(|e| format!("Manifest signature was not valid base64: {e}"))?;
    let signature = EcdsaSignature::from_der(&signature_bytes)
        .map_err(|e| format!("Manifest signature was not valid DER: {e}"))?;

    verifying_key
        .verify(manifest_bytes, &signature)
        .map_err(|e| {
            format!(
            "The update manifest signature did not verify: {e}. Refusing to trust update metadata."
        )
        })
}

fn parse_manifest_bytes(manifest_bytes: &[u8]) -> Result<UpdateManifest, String> {
    serde_json::from_slice::<UpdateManifest>(manifest_bytes)
        .map_err(|e| format!("Failed to parse update manifest: {e}"))
}

fn fetch_manifest(timeout_secs: u64) -> Result<UpdateManifest, String> {
    let public_key_der_b64 = update_public_key_der_b64()?;
    let manifest_bytes = fetch_manifest_bytes(timeout_secs)?;
    let signature_envelope = fetch_manifest_signature(timeout_secs)?;

    verify_manifest_signature(&manifest_bytes, &signature_envelope, public_key_der_b64)?;
    parse_manifest_bytes(&manifest_bytes)
}

fn download_to_file(url: &str, dest: &Path) -> Result<u64, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS)))
        .build()
        .into();

    let mut response = agent
        .get(url)
        .header("User-Agent", &user_agent_string())
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create download directory: {e}"))?;
    }

    let mut file =
        fs::File::create(dest).map_err(|e| format!("Failed to create download file: {e}"))?;

    let mut reader = response.body_mut().as_reader();
    let total = io::copy(&mut reader, &mut file)
        .map_err(|e| format!("Write error during download: {e}"))?;

    Ok(total)
}

// ---------------------------------------------------------------------------
// SHA-256 verification
// ---------------------------------------------------------------------------

fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("Cannot open file for verification: {e}"))?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];

    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Read error during verification: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let actual = format!("{:x}", hasher.finalize());
    let expected_lower = expected.to_lowercase();

    if actual != expected_lower {
        return Err(format!(
            "SHA-256 mismatch!\n  Expected: {expected_lower}\n  Got:      {actual}\n\
             The downloaded file may be corrupted or tampered with."
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// User-Agent string
// ---------------------------------------------------------------------------

pub fn user_agent_string() -> String {
    format!(
        "vettd/{} ({}/{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

// ---------------------------------------------------------------------------
// Public API: check
// ---------------------------------------------------------------------------

/// Fetch the update manifest and compare against the running version.
pub fn check_for_update(timeout_secs: u64) -> Result<UpdateCheckResult, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let manifest = fetch_manifest(timeout_secs)?;
    let platform = platform_key()?;

    let is_newer = is_version_newer(&current, &manifest.version);
    let artifact = manifest.artifacts.get(platform).cloned();

    let result = UpdateCheckResult {
        current_version: current,
        latest_version: manifest.version,
        is_newer,
        artifact,
    };
    Ok(result)
}

// ---------------------------------------------------------------------------
// Public API: update
// ---------------------------------------------------------------------------

/// Download, verify, backup, and replace the running binary.
pub fn perform_update(force: bool) -> Result<(), String> {
    let result = check_for_update(CHECK_TIMEOUT_SECS)?;

    if !result.is_newer {
        eprintln!(
            "You are already running the latest version ({}). Nothing to do.",
            result.current_version
        );
        return Ok(());
    }

    let artifact = result.artifact.ok_or_else(|| {
        let plat = platform_key().unwrap_or("unknown");
        format!(
            "No artifact available for your platform ({plat}) in version {}.\n\
             Please download manually from the GitHub Releases page.",
            result.latest_version
        )
    })?;

    eprintln!(
        "Update available: {} → {}",
        result.current_version, result.latest_version
    );

    if !force {
        eprint!("Proceed with update? [Y/n] ");
        let _ = io::stderr().flush();
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| format!("Failed to read input: {e}"))?;
        let input = input.trim().to_lowercase();
        if input == "n" || input == "no" {
            eprintln!("Update cancelled.");
            return Ok(());
        }
    }

    // 1. Determine paths
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("Cannot determine current binary path: {e}"))?;
    let dl_dir = downloads_dir()?;
    let dl_path = dl_dir.join(format!(
        "vettd-{}.tmp",
        result.latest_version.replace('/', "-")
    ));
    let backup = backup_path()?;

    // 2. Download
    eprintln!("Downloading {}...", artifact.url);
    let bytes = download_to_file(&artifact.url, &dl_path)?;
    eprintln!("  Downloaded {} bytes.", bytes);

    // 3. Verify SHA-256
    eprintln!("Verifying integrity (SHA-256)...");
    if let Err(e) = verify_sha256(&dl_path, &artifact.sha256) {
        // Clean up tainted download
        let _ = fs::remove_file(&dl_path);
        return Err(e);
    }
    eprintln!("  Checksum verified.");

    // 4. Backup current binary
    if let Some(parent) = backup.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::copy(&current_exe, &backup).map_err(|e| {
        format!(
            "Failed to backup current binary to {}: {e}",
            backup.display()
        )
    })?;
    eprintln!("  Backed up current binary to {}", backup.display());

    // 5. Extract and replace
    match extract_and_replace(&dl_path, &current_exe) {
        Ok(()) => {
            // Clean up download
            let _ = fs::remove_file(&dl_path);
            eprintln!(
                "Updated vettd {} → {}.",
                result.current_version, result.latest_version
            );

            // macOS quarantine notice
            if cfg!(target_os = "macos") {
                eprintln!();
                eprintln!("  Note: macOS may quarantine the new binary.");
                eprintln!("  If you see a \"cannot be opened\" warning, run:");
                eprintln!(
                    "    xattr -d com.apple.quarantine {}",
                    current_exe.display()
                );
                eprintln!();
            }
            Ok(())
        }
        Err(e) => {
            // Restore from backup
            eprintln!("Update failed: {e}");
            eprintln!("Restoring previous version from backup...");
            if let Err(restore_err) = fs::copy(&backup, &current_exe) {
                eprintln!(
                    "CRITICAL: Failed to restore backup: {restore_err}\n\
                     Your backup is at: {}\n\
                     Manually copy it to: {}",
                    backup.display(),
                    current_exe.display()
                );
            } else {
                eprintln!("  Restored previous version successfully.");
            }
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Extract + replace
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
fn extract_and_replace(archive_path: &Path, dest: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let file =
        fs::File::open(archive_path).map_err(|e| format!("Cannot open downloaded archive: {e}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    // Find the binary inside the tar (expect a single file named "vettd")
    let mut found = false;
    for entry in archive
        .entries()
        .map_err(|e| format!("Failed to read tar entries: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("Bad tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Bad path in tar: {e}"))?
            .to_path_buf();

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if name == "vettd" {
            // Extract to a temp file next to dest, then atomic rename
            let tmp = dest.with_extension("new");
            let mut out =
                fs::File::create(&tmp).map_err(|e| format!("Cannot create temp file: {e}"))?;
            io::copy(&mut entry, &mut out).map_err(|e| format!("Failed to extract binary: {e}"))?;
            drop(out);

            // Set executable permission
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))
                    .map_err(|e| format!("Failed to set permissions: {e}"))?;
            }

            // Atomic rename
            fs::rename(&tmp, dest).map_err(|e| {
                format!(
                    "Failed to replace binary (rename {} → {}): {e}",
                    tmp.display(),
                    dest.display()
                )
            })?;
            found = true;
            break;
        }
    }

    if !found {
        return Err("Downloaded archive does not contain the vettd binary.".into());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn extract_and_replace(downloaded: &Path, dest: &Path) -> Result<(), String> {
    // Stage the new binary first so replacing the current executable leaves the
    // smallest practical window where `dest` does not exist.
    let old = dest.with_extension("old.exe");
    let staged = dest.with_extension("new.exe");
    let _ = fs::remove_file(&old);
    let _ = fs::remove_file(&staged);

    fs::copy(downloaded, &staged).map_err(|e| format!("Cannot stage new binary: {e}"))?;
    fs::rename(dest, &old).map_err(|e| format!("Cannot rename current binary: {e}"))?;

    match fs::rename(&staged, dest) {
        Ok(()) => {
            let _ = fs::remove_file(&old);
            Ok(())
        }
        Err(e) => {
            let _ = fs::rename(&old, dest);
            let _ = fs::remove_file(&staged);
            Err(format!("Cannot replace current binary: {e}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Public API: print version
// ---------------------------------------------------------------------------

pub fn print_version() {
    println!("vettd {}", env!("CARGO_PKG_VERSION"));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::EncodePublicKey;

    fn test_signature_fixture(message: &[u8]) -> (String, ManifestSignatureEnvelope) {
        let signing_key = SigningKey::from_bytes((&[7u8; 32]).into()).unwrap();
        let public_key_der = signing_key.verifying_key().to_public_key_der().unwrap();
        let signature: EcdsaSignature = signing_key.sign(message);

        (
            BASE64_STANDARD.encode(public_key_der.as_ref()),
            ManifestSignatureEnvelope {
                algorithm: KMS_SIGNATURE_ALGORITHM.to_string(),
                key_id: Some("alias/proov-release-signing".to_string()),
                signature: BASE64_STANDARD.encode(signature.to_der().as_bytes()),
            },
        )
    }

    #[test]
    fn test_parse_semver_basic() {
        assert_eq!(crate::semver::parse("0.3.0"), Some((0, 3, 0)));
        assert_eq!(crate::semver::parse("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(crate::semver::parse("10.20.30"), Some((10, 20, 30)));
    }

    #[test]
    fn test_parse_semver_invalid() {
        assert_eq!(crate::semver::parse(""), None);
        assert_eq!(crate::semver::parse("1.2"), None);
        assert_eq!(crate::semver::parse("not-a-version"), None);
        assert_eq!(crate::semver::parse("1.2.x"), None);
    }

    #[test]
    fn test_is_version_newer_true() {
        assert!(is_version_newer("0.3.0", "0.4.0"));
        assert!(is_version_newer("0.3.0", "1.0.0"));
        assert!(is_version_newer("0.3.0", "v0.3.1"));
        assert!(is_version_newer("1.0.0", "1.0.1"));
    }

    #[test]
    fn test_is_version_newer_false() {
        assert!(!is_version_newer("0.3.0", "0.3.0")); // same
        assert!(!is_version_newer("0.4.0", "0.3.0")); // older
        assert!(!is_version_newer("1.0.0", "0.9.9")); // older
    }

    #[test]
    fn test_is_version_newer_with_v_prefix() {
        assert!(is_version_newer("v0.3.0", "v0.4.0"));
        assert!(is_version_newer("0.3.0", "v0.4.0"));
        assert!(is_version_newer("v0.3.0", "0.4.0"));
    }

    #[test]
    fn test_is_version_newer_invalid_returns_false() {
        assert!(!is_version_newer("bad", "0.4.0"));
        assert!(!is_version_newer("0.3.0", "bad"));
        assert!(!is_version_newer("bad", "bad"));
    }

    #[test]
    fn test_platform_key_returns_ok() {
        // Should not error on any CI/dev platform
        let result = platform_key();
        assert!(result.is_ok(), "platform_key() failed: {:?}", result);
        let key = result.unwrap();
        assert!(
            [
                "darwin-arm64",
                "darwin-amd64",
                "linux-arm64",
                "linux-amd64",
                "windows-amd64"
            ]
            .contains(&key),
            "Unexpected platform key: {key}"
        );
    }

    #[test]
    fn test_user_agent_string_format() {
        let ua = user_agent_string();
        assert!(
            ua.starts_with("vettd/"),
            "UA should start with vettd/: {ua}"
        );
        assert!(ua.contains('/'), "UA should contain OS/ARCH: {ua}");
        assert!(ua.contains('('), "UA should contain parens: {ua}");
    }

    #[test]
    fn test_verify_sha256_correct() {
        let dir = std::env::temp_dir().join("vettd-test-sha256");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test-verify.bin");
        fs::write(&path, b"hello world").unwrap();

        // SHA-256 of "hello world"
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert!(verify_sha256(&path, expected).is_ok());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_sha256_mismatch() {
        let dir = std::env::temp_dir().join("vettd-test-sha256-bad");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test-verify-bad.bin");
        fs::write(&path, b"hello world").unwrap();

        let bad_hash = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = verify_sha256(&path, bad_hash);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SHA-256 mismatch"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_sha256_case_insensitive() {
        let dir = std::env::temp_dir().join("vettd-test-sha256-case");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test-verify-case.bin");
        fs::write(&path, b"hello world").unwrap();

        let expected = "B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9";
        assert!(verify_sha256(&path, expected).is_ok());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_manifest_deserialization() {
        let json = r#"{
            "manifestVersion": 1,
            "version": "v0.4.0",
            "date": "2026-03-21T00:00:00Z",
            "artifacts": {
                "darwin-arm64": {
                    "url": "https://example.com/vettd-darwin-arm64.tar.gz",
                    "sha256": "abcdef1234567890",
                    "size": 1234
                }
            }
        }"#;
        let manifest: UpdateManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.manifest_version, Some(1));
        assert_eq!(manifest.version, "v0.4.0");
        assert_eq!(manifest.artifacts.len(), 1);
        assert!(manifest.artifacts.contains_key("darwin-arm64"));
        assert_eq!(
            manifest.artifacts["darwin-arm64"].sha256,
            "abcdef1234567890"
        );
        assert_eq!(manifest.artifacts["darwin-arm64"].size, Some(1234));
    }

    #[test]
    fn test_manifest_deserialization_no_artifacts() {
        // Backwards-compatible with old latest.json format
        let json = r#"{"version":"v0.3.0","date":"2026-03-20T00:00:00Z"}"#;
        let manifest: UpdateManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.manifest_version, None);
        assert_eq!(manifest.version, "v0.3.0");
        assert!(manifest.artifacts.is_empty());
    }

    #[test]
    fn test_verify_manifest_signature_accepts_valid_ecdsa_signature() {
        let (public_key_der_b64, envelope) = test_signature_fixture(b"test");

        assert!(verify_manifest_signature(b"test", &envelope, &public_key_der_b64).is_ok());
    }

    #[test]
    fn test_verify_manifest_signature_rejects_tampered_manifest() {
        let (public_key_der_b64, envelope) = test_signature_fixture(b"test");

        let result = verify_manifest_signature(b"Test", &envelope, &public_key_der_b64);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("update manifest signature did not verify"));
    }

    #[test]
    fn test_verify_manifest_signature_rejects_unsupported_algorithm() {
        let (public_key_der_b64, mut envelope) = test_signature_fixture(b"test");
        envelope.algorithm = "RSA_SHA_256".to_string();

        let result = verify_manifest_signature(b"test", &envelope, &public_key_der_b64);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Unsupported manifest signature algorithm"));
    }

    #[test]
    fn test_verify_manifest_signature_rejects_invalid_signature_encoding() {
        let (public_key_der_b64, mut envelope) = test_signature_fixture(b"test");
        envelope.signature = "!not-base64!".to_string();

        let result = verify_manifest_signature(b"test", &envelope, &public_key_der_b64);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Manifest signature was not valid base64"));
    }

    #[test]
    fn test_decode_update_public_key_rejects_invalid_key() {
        let result = decode_update_public_key("not-base64");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Embedded update verification key was not valid base64"));
    }

    #[test]
    fn test_parse_manifest_bytes_rejects_invalid_json() {
        let result = parse_manifest_bytes(br#"{"version":"v1.0.0""#);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Failed to parse update manifest"));
    }

    #[test]
    fn test_print_version_does_not_panic() {
        // Just ensure it doesn't panic; output goes to stdout
        print_version();
    }

    #[test]
    fn test_is_version_newer_same_version_is_false() {
        // Regression: after upgrading the binary the cached is_newer flag
        // would still be true. The fix re-evaluates against the current
        // binary version, so equal versions must return false.
        let current = env!("CARGO_PKG_VERSION");
        assert!(
            !is_version_newer(current, current),
            "same version should not be considered newer (got true for {current})"
        );
    }
}
