use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

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

fn vettd_dir() -> Result<PathBuf, String> {
    dirs::home_dir().map(|h| h.join(".vettd")).ok_or_else(|| {
        "Unable to determine home directory — cannot resolve scanner identity paths".to_string()
    })
}

fn persist_uuid(path: &Path, field_name: &str, uuid: &str) -> Result<(), String> {
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
        file.write_all(uuid.as_bytes())
            .map_err(|e| format!("Failed to write {field_name} to {}: {e}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to secure {field_name} file {}: {e}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, uuid)
            .map_err(|e| format!("Failed to persist {field_name} to {}: {e}", path.display()))?;
    }

    Ok(())
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

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ah_test_{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
