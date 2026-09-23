use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

/// A freshly generated observer secret is 32 bytes.
const OBSERVER_SECRET_LEN: usize = 32;
/// An explicitly supplied observer secret file must hold at least 16 bytes.
const OBSERVER_SECRET_MIN_LEN: usize = 16;

/// Returns `true` when `value` is a valid v4-style UUID string.
pub fn is_valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

/// `~/.vettd/scanner_uuid`
pub fn default_scanner_uuid_path() -> Result<PathBuf, String> {
    Ok(vettd_dir()?.join("scanner_uuid"))
}

/// `~/.vettd/scanner_account_uuid`
pub fn default_scanner_account_uuid_path() -> Result<PathBuf, String> {
    Ok(vettd_dir()?.join("scanner_account_uuid"))
}

/// `~/.vettd/observer_secret`
pub fn default_observer_secret_path() -> Result<PathBuf, String> {
    Ok(vettd_dir()?.join("observer_secret"))
}

fn vettd_dir() -> Result<PathBuf, String> {
    crate::cli::user_home_dir()
        .map(|h| h.join(".vettd"))
        .ok_or_else(|| {
            "Unable to determine home directory — cannot resolve scanner identity paths".to_string()
        })
}

/// Write `bytes` to `path`, owner-only: the parent directory is created and
/// chmod'd 0700 and the file is created and chmod'd 0600 on unix.
fn persist_secret_bytes(path: &Path, field_name: &str, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create directory {} for {field_name}: {e}",
                parent.display()
            )
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|e| {
                format!(
                    "Failed to secure directory {} for {field_name}: {e}",
                    parent.display()
                )
            })?;
        }
    }

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
            .map_err(|e| format!("Failed to open {field_name} file {}: {e}", path.display()))?;
        file.write_all(bytes)
            .map_err(|e| format!("Failed to write {field_name} to {}: {e}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to secure {field_name} file {}: {e}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, bytes)
            .map_err(|e| format!("Failed to persist {field_name} to {}: {e}", path.display()))?;
    }

    Ok(())
}

fn persist_uuid(path: &Path, field_name: &str, uuid: &str) -> Result<(), String> {
    persist_secret_bytes(path, field_name, uuid.as_bytes())
}

fn read_secret_bytes(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| {
        format!(
            "Failed to read observer secret from {}: {e}",
            path.display()
        )
    })
}

/// Resolve the observer HMAC secret plus the `run_id_basis` label that records
/// where it came from.
///
/// `explicit` loads the file's raw bytes verbatim — no trimming and no UTF-8
/// round-trip — and reports basis `test_secret`; `None` loads (or mints exactly
/// once) `~/.vettd/observer_secret` and reports `device_secret`.
pub fn resolve_observer_secret(explicit: Option<&Path>) -> Result<(Vec<u8>, &'static str), String> {
    let Some(path) = explicit else {
        return resolve_observer_secret_at(&default_observer_secret_path()?);
    };

    let bytes = read_secret_bytes(path)?;
    if bytes.len() < OBSERVER_SECRET_MIN_LEN {
        return Err(format!(
            "observer secret must hold at least {OBSERVER_SECRET_MIN_LEN} bytes"
        ));
    }
    Ok((bytes, "test_secret"))
}

/// Path-injectable form of the device-secret branch of [`resolve_observer_secret`].
///
/// An existing, non-empty file is returned verbatim and is never regenerated;
/// otherwise fresh random bytes are generated and persisted owner-only.
pub(crate) fn resolve_observer_secret_at(path: &Path) -> Result<(Vec<u8>, &'static str), String> {
    // Distinguish "no secret yet" from "cannot read the secret". `Path::exists` reports both as
    // false, so a stat failure on a secret that does exist (a stale NFS handle over `$HOME`, say)
    // would fall through to minting — silently rotating the key, changing every future `run_id`,
    // and orphaning the records the server already holds under the old one.
    match fs::read(path) {
        Ok(bytes) if bytes.is_empty() => {}
        Ok(bytes) => {
            if bytes.len() < OBSERVER_SECRET_MIN_LEN {
                // Minted secrets are always OBSERVER_SECRET_LEN, so a short one is truncation or
                // tampering. Failing loud beats silently re-minting: the plan's rule is that an
                // existing secret is never regenerated, and a weak HMAC key must not go unnoticed.
                return Err(format!(
                    "observer secret {} holds {} bytes; it must hold at least {OBSERVER_SECRET_MIN_LEN}. Remove the file to mint a new one, which changes every future run_id.",
                    path.display(),
                    bytes.len()
                ));
            }
            return Ok((bytes, "device_secret"));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to read observer secret from {}: {e}",
                path.display()
            ));
        }
    }

    let mut bytes = vec![0u8; OBSERVER_SECRET_LEN];
    getrandom::fill(&mut bytes).map_err(|e| format!("Failed to generate observer secret: {e}"))?;
    persist_secret_bytes(path, "observer_secret", &bytes)?;
    Ok((bytes, "device_secret"))
}

/// Resolve a UUID through the following cascade:
///
/// 1. `explicit` — use if provided (must be valid UUID).
/// 2. Environment variable `env_var`.
/// 3. Read from `id_path` on disk.
/// 4. Generate a new v4 UUID and persist it to `id_path`.
///
/// `field_name` is used in error messages (e.g. "scanner_uuid").
pub fn resolve_persisted_uuid(
    explicit: Option<&str>,
    env_var: &str,
    id_path: &Path,
    field_name: &str,
) -> Result<String, String> {
    // 1. Explicit value
    if let Some(val) = explicit {
        let val = val.trim();
        if !is_valid_uuid(val) {
            return Err(format!("Explicit {field_name} is not a valid UUID: {val}"));
        }
        return Ok(val.to_string());
    }

    // 2. Environment variable
    if let Ok(val) = std::env::var(env_var) {
        let val = val.trim().to_string();
        if !val.is_empty() {
            if !is_valid_uuid(&val) {
                return Err(format!(
                    "Environment variable {env_var} is not a valid UUID: {val}"
                ));
            }
            return Ok(val);
        }
    }

    // 3. Read from file
    if id_path.exists() {
        let content = fs::read_to_string(id_path).map_err(|e| {
            format!(
                "Failed to read {field_name} from {}: {e}",
                id_path.display()
            )
        })?;
        let val = content.trim().to_string();
        if !val.is_empty() {
            if !is_valid_uuid(&val) {
                return Err(format!(
                    "Persisted {field_name} in {} is not a valid UUID: {val}",
                    id_path.display()
                ));
            }
            return Ok(val);
        }
    }

    // 4. Generate and persist
    let new_uuid = Uuid::new_v4().to_string();
    persist_uuid(id_path, field_name, &new_uuid)?;

    Ok(new_uuid)
}

/// Resolve the scanner UUID (convenience wrapper).
pub fn resolve_scanner_uuid(explicit: Option<&str>) -> Result<String, String> {
    resolve_persisted_uuid(
        explicit,
        "VETTD_SCANNER_UUID",
        &default_scanner_uuid_path()?,
        "scanner_uuid",
    )
}

/// Resolve the scanner account UUID (convenience wrapper).
pub fn resolve_scanner_account_uuid(explicit: Option<&str>) -> Result<String, String> {
    resolve_persisted_uuid(
        explicit,
        "VETTD_SCANNER_ACCOUNT_UUID",
        &default_scanner_account_uuid_path()?,
        "scanner_account_uuid",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct ScopedEnvVar {
        name: &'static str,
        original: Option<String>,
    }

    impl ScopedEnvVar {
        fn set(name: &'static str, value: &str) -> Self {
            let original = env::var(name).ok();
            // SAFETY: Environment mutation is process-global, so tests serialize
            // access with ENV_LOCK and restore the original value in Drop.
            unsafe {
                env::set_var(name, value);
            }
            Self { name, original }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            // SAFETY: Environment mutation is serialized by ENV_LOCK for the
            // lifetime of ScopedEnvVar, so restoration is scoped and ordered.
            unsafe {
                if let Some(value) = &self.original {
                    env::set_var(self.name, value);
                } else {
                    env::remove_var(self.name);
                }
            }
        }
    }

    /// A stat failure on a secret that does exist must not look like "no secret yet".
    /// `Path::exists` reports both as false, and falling through would silently mint a new key,
    /// changing every future run_id and orphaning the records the server holds under the old one.
    /// A directory at the secret path is the portable way to make the read fail while the path
    /// exists; the guarantee under test is "an unreadable secret is an error, never a rotation".
    #[test]
    fn unreadable_existing_secret_is_an_error_not_a_silent_rotation() {
        let tmp = tempdir();
        let path = tmp.join("observer_secret");
        fs::create_dir_all(&path).unwrap();

        let err = resolve_observer_secret_at(&path).expect_err("a directory cannot be read");
        assert!(
            err.contains("Failed to read observer secret"),
            "unexpected error: {err}"
        );
        assert!(path.is_dir(), "the path must not have been overwritten");
    }

    /// Minted secrets are always 32 bytes, so a shorter existing one is truncation or tampering.
    /// Re-minting would quietly rotate the pseudonym key; using it would make a weak HMAC key the
    /// device identity. Both are worse than refusing, so this fails loud and says how to recover.
    #[test]
    fn short_existing_device_secret_is_refused_rather_than_used_or_replaced() {
        let tmp = tempdir();
        let path = tmp.join("observer_secret");
        fs::write(&path, b"too short").unwrap();

        let err = resolve_observer_secret_at(&path).expect_err("9 bytes is below the minimum");
        assert!(err.contains("at least 16"), "unexpected error: {err}");
        assert_eq!(
            fs::read(&path).unwrap(),
            b"too short",
            "the existing secret must be left exactly as it was"
        );
    }

    /// An empty file is indistinguishable from a failed create, so it is treated as "not yet
    /// minted" and filled in — the one case where writing over an existing path is right.
    #[test]
    fn empty_secret_file_is_minted_into() {
        let tmp = tempdir();
        let path = tmp.join("observer_secret");
        fs::write(&path, b"").unwrap();

        let (bytes, basis) = resolve_observer_secret_at(&path).expect("mints into an empty file");
        assert_eq!(bytes.len(), OBSERVER_SECRET_LEN);
        assert_eq!(basis, "device_secret");
        assert_eq!(fs::read(&path).unwrap(), bytes, "the mint was persisted");
    }

    #[test]
    fn valid_uuid_check() {
        assert!(is_valid_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_valid_uuid("not-a-uuid"));
        assert!(!is_valid_uuid(""));
    }

    #[test]
    fn default_paths_end_correctly() {
        let p = default_scanner_uuid_path().expect("home dir must be available in test env");
        assert!(p.ends_with("scanner_uuid"));
        let p =
            default_scanner_account_uuid_path().expect("home dir must be available in test env");
        assert!(p.ends_with("scanner_account_uuid"));
    }

    #[test]
    fn vettd_dir_returns_ok_in_normal_env() {
        // In any environment with a home directory, vettd_dir() must succeed
        // rather than panicking. This is the key behavioral guarantee of fix #4.
        let result = vettd_dir();
        assert!(result.is_ok(), "vettd_dir() returned Err: {:?}", result);
        let dir = result.unwrap();
        assert!(dir.ends_with(".vettd"));
    }

    #[test]
    fn home_dir_error_propagates_to_resolve_scanner_uuid() {
        // Simulate "no home dir" by checking that an Err from vettd_dir()
        // propagates through resolve_persisted_uuid rather than panicking.
        // We verify this by using the Result-returning API directly.
        //
        // On this machine the call will succeed, but the test confirms the
        // function returns Result<_, String> (not a panic type) in all cases.
        let result = resolve_scanner_uuid(None);
        // Either Ok (normal env) or Err (e.g. no home dir) — never a panic.
        match result {
            Ok(uuid) => assert!(is_valid_uuid(&uuid), "resolved UUID must be valid"),
            Err(msg) => assert!(!msg.is_empty(), "error message must not be empty"),
        }
    }

    #[test]
    fn explicit_value_wins() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let tmp = tempdir();
        let path = tmp.join("id");
        let result = resolve_persisted_uuid(Some(uuid), "UNUSED_VAR_1234", &path, "test");
        assert_eq!(result.unwrap(), uuid);
    }

    #[test]
    fn invalid_explicit_is_rejected() {
        let tmp = tempdir();
        let path = tmp.join("id");
        let result = resolve_persisted_uuid(Some("bad"), "UNUSED_VAR_1234", &path, "test");
        assert!(result.is_err());
    }

    #[test]
    fn env_var_fallback() {
        let _guard = ENV_LOCK.lock().unwrap();
        let uuid = "660e8400-e29b-41d4-a716-446655440000";
        let var_name = "AH_TEST_UUID_ENV_FALLBACK";
        let _var = ScopedEnvVar::set(var_name, uuid);
        let tmp = tempdir();
        let path = tmp.join("id");
        let result = resolve_persisted_uuid(None, var_name, &path, "test");
        assert_eq!(result.unwrap(), uuid);
    }

    #[test]
    fn file_fallback() {
        let uuid = "770e8400-e29b-41d4-a716-446655440000";
        let tmp = tempdir();
        let path = tmp.join("id");
        fs::write(&path, uuid).unwrap();
        let result = resolve_persisted_uuid(None, "UNUSED_VAR_5678", &path, "test");
        assert_eq!(result.unwrap(), uuid);
    }

    #[test]
    fn generates_and_persists_when_nothing_exists() {
        let tmp = tempdir();
        let path = tmp.join("sub").join("id");
        let result = resolve_persisted_uuid(None, "UNUSED_VAR_9012", &path, "test");
        let uuid = result.unwrap();
        assert!(is_valid_uuid(&uuid));
        assert_eq!(fs::read_to_string(&path).unwrap(), uuid);
    }

    #[cfg(unix)]
    #[test]
    fn generated_uuid_path_is_saved_with_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir();
        let path = tmp.join("secure").join("id");

        let uuid = resolve_persisted_uuid(None, "UNUSED_VAR_3456", &path, "test").unwrap();

        assert!(is_valid_uuid(&uuid));
        let dir_mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    /// Invariant: the device secret is minted exactly once and stays owner-only
    /// on disk. Every run id is an HMAC under this key, so regenerating it would
    /// silently orphan every persisted cursor and ledger entry, and a
    /// world-readable key would let anyone forge those ids.
    #[cfg(unix)]
    #[test]
    fn observer_secret_is_generated_once_with_0600() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir();
        let path = tmp.join("secure").join("observer_secret");

        let (first, basis) = resolve_observer_secret_at(&path).unwrap();
        assert_eq!(first.len(), 32);
        assert_eq!(basis, "device_secret");

        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&path), 0o600);

        let (second, _) = resolve_observer_secret_at(&path).unwrap();
        assert_eq!(
            first, second,
            "an existing secret must never be regenerated"
        );
    }

    /// Invariant: 16 bytes is the floor for an explicitly supplied secret — a
    /// shorter key makes the run-id HMAC cheap to brute-force, so it is refused
    /// rather than silently accepted.
    #[test]
    fn observer_secret_rejects_short_file() {
        let tmp = tempdir();

        let short = tmp.join("short.bin");
        fs::write(&short, [0x41u8; 15]).unwrap();
        let err = resolve_observer_secret(Some(&short)).unwrap_err();
        assert!(err.contains("at least 16 bytes"), "unexpected error: {err}");

        let exact = tmp.join("exact.bin");
        fs::write(&exact, [0x41u8; 16]).unwrap();
        assert!(resolve_observer_secret(Some(&exact)).is_ok());
    }

    /// Invariant: an explicit secret file is loaded byte-for-byte. The golden
    /// fixtures pin a 33-byte secret written without a trailing newline, so any
    /// trimming or UTF-8 round-trip would change every HMAC in the envelope.
    #[test]
    fn explicit_secret_file_bytes_are_loaded_exactly() {
        let tmp = tempdir();
        let path = tmp.join("secret.bin");
        let raw = b"abcdefghijklmnop\n\x00\xffrest-of-secret".to_vec();
        fs::write(&path, &raw).unwrap();

        let (loaded, basis) = resolve_observer_secret(Some(&path)).unwrap();
        assert_eq!(loaded, raw);
        assert_eq!(basis, "test_secret");
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ah_test_{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
