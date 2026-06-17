use crate::discovery::Candidate;
use crate::models::ArtifactReport;
use chrono::Utc;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const CACHE_SCHEMA_VERSION: &str = "scan-v1";

#[derive(Debug, Clone)]
pub struct FileStateSnapshot {
    pub canonical_path: String,
    pub origin: String,
    pub stable_file_id: Option<String>,
    pub size_bytes: u64,
    pub modified_ns: Option<i64>,
    pub state_key: String,
}

#[derive(Debug, Clone)]
pub struct CachedCandidate {
    pub candidate: Candidate,
    pub file_state: Option<FileStateSnapshot>,
}

#[derive(Debug, Clone)]
pub struct ScanCacheProfile {
    pub profile_key: String,
    pub mode: String,
    pub roots: String,
    pub detector_fingerprint: String,
    pub rule_fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct CachedDetectorBundle {
    pub detector_fingerprint: String,
    pub file_state_key: String,
    pub artifacts: Vec<ArtifactReport>,
}

#[derive(Debug, Clone)]
pub struct RootCursor {
    pub backend_type: String,
    pub cursor_token: String,
}

pub struct ScanCache {
    conn: Connection,
}

impl ScanCache {
    pub fn open_default() -> Result<Self, String> {
        let db_path = default_db_path()?;
        Self::open_at(&db_path)
    }

    pub fn open_at(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create scan-cache directory: {e}"))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| format!("Failed to open scan-cache database {}: {e}", path.display()))?;
        let cache = Self { conn };
        cache.ensure_schema()?;
        Ok(cache)
    }

    fn ensure_schema(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS scan_profiles (
                    profile_key TEXT PRIMARY KEY,
                    mode TEXT NOT NULL,
                    roots TEXT NOT NULL,
                    detector_fingerprint TEXT NOT NULL,
                    rule_fingerprint TEXT NOT NULL,
                    completed_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS file_states (
                    canonical_path TEXT PRIMARY KEY,
                    origin_tier TEXT NOT NULL,
                    stable_file_id TEXT,
                    size_bytes INTEGER NOT NULL,
                    modified_ns INTEGER,
                    content_hash TEXT,
                    last_seen_profile TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS artifacts (
                    profile_key TEXT NOT NULL,
                    detector_name TEXT NOT NULL,
                    canonical_path TEXT NOT NULL,
                    detector_fingerprint TEXT NOT NULL,
                    file_state_key TEXT NOT NULL,
                    artifacts_json TEXT NOT NULL,
                    artifact_hash TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (profile_key, detector_name, canonical_path)
                );

                CREATE TABLE IF NOT EXISTS root_cursors (
                    root_path TEXT PRIMARY KEY,
                    backend_type TEXT NOT NULL,
                    cursor_token TEXT NOT NULL
                );
                ",
            )
            .map_err(|e| format!("Failed to initialize scan-cache schema: {e}"))
    }

    pub fn upsert_profile(&self, profile: &ScanCacheProfile) -> Result<(), String> {
        self.conn
            .execute(
                "
                INSERT INTO scan_profiles (
                    profile_key, mode, roots, detector_fingerprint, rule_fingerprint, completed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(profile_key) DO UPDATE SET
                    mode = excluded.mode,
                    roots = excluded.roots,
                    detector_fingerprint = excluded.detector_fingerprint,
                    rule_fingerprint = excluded.rule_fingerprint,
                    completed_at = excluded.completed_at
                ",
                params![
                    profile.profile_key,
                    profile.mode,
                    profile.roots,
                    profile.detector_fingerprint,
                    profile.rule_fingerprint,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(|e| format!("Failed to store scan profile: {e}"))?;
        Ok(())
    }

    pub fn upsert_file_states(
        &mut self,
        profile_key: &str,
        candidates: &[CachedCandidate],
    ) -> Result<(), String> {
        let transaction = self
            .conn
            .transaction()
            .map_err(|e| format!("Failed to start scan-cache transaction: {e}"))?;

        for candidate in candidates {
            let Some(file_state) = &candidate.file_state else {
                continue;
            };
            transaction
                .execute(
                    "
                    INSERT INTO file_states (
                        canonical_path,
                        origin_tier,
                        stable_file_id,
                        size_bytes,
                        modified_ns,
                        content_hash,
                        last_seen_profile
                    ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)
                    ON CONFLICT(canonical_path) DO UPDATE SET
                        origin_tier = excluded.origin_tier,
                        stable_file_id = excluded.stable_file_id,
                        size_bytes = excluded.size_bytes,
                        modified_ns = excluded.modified_ns,
                        last_seen_profile = excluded.last_seen_profile
                    ",
                    params![
                        file_state.canonical_path,
                        file_state.origin,
                        file_state.stable_file_id,
                        file_state.size_bytes as i64,
                        file_state.modified_ns,
                        profile_key,
                    ],
                )
                .map_err(|e| format!("Failed to upsert file state: {e}"))?;
        }

        transaction
            .commit()
            .map_err(|e| format!("Failed to commit file-state transaction: {e}"))
    }

    pub fn load_detector_bundles(
        &self,
        profile_key: &str,
        detector_name: &str,
    ) -> Result<HashMap<String, CachedDetectorBundle>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "
                SELECT canonical_path, detector_fingerprint, file_state_key, artifacts_json
                FROM artifacts
                WHERE profile_key = ?1 AND detector_name = ?2
                ",
            )
            .map_err(|e| format!("Failed to prepare scan-cache read: {e}"))?;

        let mut rows = stmt
            .query(params![profile_key, detector_name])
            .map_err(|e| format!("Failed to query scan-cache: {e}"))?;

        let mut bundles = HashMap::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| format!("Failed to read scan-cache row: {e}"))?
        {
            let canonical_path: String = row
                .get(0)
                .map_err(|e| format!("Failed to decode scan-cache path: {e}"))?;
            let detector_fingerprint: String = row
                .get(1)
                .map_err(|e| format!("Failed to decode detector fingerprint: {e}"))?;
            let file_state_key: String = row
                .get(2)
                .map_err(|e| format!("Failed to decode file-state key: {e}"))?;
            let artifacts_json: String = row
                .get(3)
                .map_err(|e| format!("Failed to decode artifact payload: {e}"))?;
            let artifacts = serde_json::from_str::<Vec<ArtifactReport>>(&artifacts_json)
                .map_err(|e| format!("Failed to decode cached artifact payload: {e}"))?;
            bundles.insert(
                canonical_path,
                CachedDetectorBundle {
                    detector_fingerprint,
                    file_state_key,
                    artifacts,
                },
            );
        }

        Ok(bundles)
    }

    pub fn persist_detector_results(
        &mut self,
        profile_key: &str,
        detector_name: &str,
        detector_fingerprint: &str,
        candidates: &[CachedCandidate],
        artifacts_by_path: &HashMap<String, Vec<ArtifactReport>>,
    ) -> Result<(), String> {
        let transaction = self
            .conn
            .transaction()
            .map_err(|e| format!("Failed to start detector-result transaction: {e}"))?;

        for candidate in candidates {
            let Some(file_state) = &candidate.file_state else {
                continue;
            };

            let artifacts = artifacts_by_path
                .get(&file_state.canonical_path)
                .cloned()
                .unwrap_or_default();
            let artifacts_json = serde_json::to_string(&artifacts)
                .map_err(|e| format!("Failed to serialize cached artifacts: {e}"))?;
            let artifact_hash = hex_sha256(artifacts_json.as_bytes());

            transaction
                .execute(
                    "
                    INSERT INTO artifacts (
                        profile_key,
                        detector_name,
                        canonical_path,
                        detector_fingerprint,
                        file_state_key,
                        artifacts_json,
                        artifact_hash,
                        updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    ON CONFLICT(profile_key, detector_name, canonical_path) DO UPDATE SET
                        detector_fingerprint = excluded.detector_fingerprint,
                        file_state_key = excluded.file_state_key,
                        artifacts_json = excluded.artifacts_json,
                        artifact_hash = excluded.artifact_hash,
                        updated_at = excluded.updated_at
                    ",
                    params![
                        profile_key,
                        detector_name,
                        file_state.canonical_path,
                        detector_fingerprint,
                        file_state.state_key,
                        artifacts_json,
                        artifact_hash,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(|e| format!("Failed to persist detector results: {e}"))?;
        }

        transaction
            .commit()
            .map_err(|e| format!("Failed to commit detector-result transaction: {e}"))
    }

    pub fn load_root_cursor(
        &self,
        root_path: &str,
        backend_type: &str,
    ) -> Result<Option<RootCursor>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "
                SELECT backend_type, cursor_token
                FROM root_cursors
                WHERE root_path = ?1 AND backend_type = ?2
                ",
            )
            .map_err(|e| format!("Failed to prepare root-cursor read: {e}"))?;

        let mut rows = stmt
            .query(params![root_path, backend_type])
            .map_err(|e| format!("Failed to query root cursor: {e}"))?;

        let Some(row) = rows
            .next()
            .map_err(|e| format!("Failed to read root cursor row: {e}"))?
        else {
            return Ok(None);
        };

        let backend_type: String = row
            .get(0)
            .map_err(|e| format!("Failed to decode root-cursor backend: {e}"))?;
        let cursor_token: String = row
            .get(1)
            .map_err(|e| format!("Failed to decode root-cursor token: {e}"))?;

        Ok(Some(RootCursor {
            backend_type,
            cursor_token,
        }))
    }

    pub fn upsert_root_cursor(
        &self,
        root_path: &str,
        backend_type: &str,
        cursor_token: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "
                INSERT INTO root_cursors (root_path, backend_type, cursor_token)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(root_path) DO UPDATE SET
                    backend_type = excluded.backend_type,
                    cursor_token = excluded.cursor_token
                ",
                params![root_path, backend_type, cursor_token],
            )
            .map_err(|e| format!("Failed to store root cursor: {e}"))?;
        Ok(())
    }

    pub fn load_profile_candidates_for_root(
        &self,
        profile_key: &str,
        root_path: &Path,
    ) -> Result<Vec<CachedCandidate>, String> {
        let root = root_path.to_string_lossy().to_string();
        let root_prefix = format!("{root}/%");
        let mut stmt = self
            .conn
            .prepare(
                "
                SELECT DISTINCT
                    f.canonical_path,
                    f.origin_tier,
                    f.stable_file_id,
                    f.size_bytes,
                    f.modified_ns
                FROM artifacts a
                INNER JOIN file_states f ON f.canonical_path = a.canonical_path
                WHERE a.profile_key = ?1
                  AND (a.canonical_path = ?2 OR a.canonical_path LIKE ?3)
                ORDER BY f.canonical_path ASC
                ",
            )
            .map_err(|e| format!("Failed to prepare cached-candidate read: {e}"))?;

        let mut rows = stmt
            .query(params![profile_key, root, root_prefix])
            .map_err(|e| format!("Failed to query cached candidates: {e}"))?;

        let mut candidates = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| format!("Failed to read cached candidate row: {e}"))?
        {
            let canonical_path: String = row
                .get(0)
                .map_err(|e| format!("Failed to decode cached candidate path: {e}"))?;
            let origin: String = row
                .get(1)
                .map_err(|e| format!("Failed to decode cached candidate origin: {e}"))?;
            let stable_file_id: Option<String> = row
                .get(2)
                .map_err(|e| format!("Failed to decode cached candidate stable id: {e}"))?;
            let size_bytes: i64 = row
                .get(3)
                .map_err(|e| format!("Failed to decode cached candidate size: {e}"))?;
            let modified_ns: Option<i64> = row
                .get(4)
                .map_err(|e| format!("Failed to decode cached candidate mtime: {e}"))?;
            let file_state = file_state_from_row(
                &canonical_path,
                &origin,
                stable_file_id,
                size_bytes.max(0) as u64,
                modified_ns,
            );
            let candidate = Candidate {
                path: PathBuf::from(&canonical_path),
                origin,
            };
            candidates.push(CachedCandidate {
                candidate,
                file_state: Some(file_state),
            });
        }

        Ok(candidates)
    }

    pub fn prune_profile_root_artifacts(
        &mut self,
        profile_key: &str,
        root_path: &Path,
        keep_paths: &HashSet<String>,
    ) -> Result<(), String> {
        let root = root_path.to_string_lossy().to_string();
        let root_prefix = format!("{root}/%");
        let existing_paths = self.profile_root_paths(profile_key, &root, &root_prefix)?;
        let stale_paths: Vec<String> = existing_paths
            .into_iter()
            .filter(|path| !keep_paths.contains(path))
            .collect();
        if stale_paths.is_empty() {
            return Ok(());
        }

        let transaction = self
            .conn
            .transaction()
            .map_err(|e| format!("Failed to start root-prune transaction: {e}"))?;
        for path in stale_paths {
            transaction
                .execute(
                    "
                    DELETE FROM artifacts
                    WHERE profile_key = ?1 AND canonical_path = ?2
                    ",
                    params![profile_key, path],
                )
                .map_err(|e| format!("Failed to prune stale root artifact rows: {e}"))?;
        }
        transaction
            .commit()
            .map_err(|e| format!("Failed to commit root-prune transaction: {e}"))
    }

    fn profile_root_paths(
        &self,
        profile_key: &str,
        root: &str,
        root_prefix: &str,
    ) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "
                SELECT DISTINCT canonical_path
                FROM artifacts
                WHERE profile_key = ?1
                  AND (canonical_path = ?2 OR canonical_path LIKE ?3)
                ",
            )
            .map_err(|e| format!("Failed to prepare root-path read: {e}"))?;
        let mut rows = stmt
            .query(params![profile_key, root, root_prefix])
            .map_err(|e| format!("Failed to query root-path rows: {e}"))?;

        let mut paths = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| format!("Failed to read root-path row: {e}"))?
        {
            let canonical_path: String = row
                .get(0)
                .map_err(|e| format!("Failed to decode root-path row: {e}"))?;
            paths.push(canonical_path);
        }
        Ok(paths)
    }
}

pub fn cache_enabled_for_mode(mode: &str) -> bool {
    matches!(mode, "host" | "scan" | "workdir" | "file")
}

pub fn cacheable_detector(mode: &str, detector_name: &str) -> bool {
    matches!(detector_name, "custom_rules" | "containers" | "mcp_configs")
        || (mode == "file" && detector_name == "source_risks")
}

pub fn detector_fingerprint(detector_name: &str) -> String {
    hex_sha256(format!("{}:{detector_name}", env!("CARGO_PKG_VERSION")).as_bytes())
}

pub fn build_profile(
    mode: &str,
    deep: bool,
    roots: &str,
    detector_names: &[String],
    rule_fingerprint: &str,
) -> ScanCacheProfile {
    let detector_fingerprint = detector_names.join(",");
    let profile_key = hex_sha256(
        format!(
            "{CACHE_SCHEMA_VERSION}:{}:{mode}:{deep}:{roots}:{detector_fingerprint}:{rule_fingerprint}",
            env!("CARGO_PKG_VERSION")
        )
        .as_bytes(),
    );

    ScanCacheProfile {
        profile_key,
        mode: mode.to_string(),
        roots: roots.to_string(),
        detector_fingerprint,
        rule_fingerprint: rule_fingerprint.to_string(),
    }
}

pub fn snapshot_candidates(candidates: &[Candidate]) -> Vec<CachedCandidate> {
    candidates
        .iter()
        .cloned()
        .map(|candidate| {
            let file_state = build_file_state_snapshot(&candidate);
            CachedCandidate {
                candidate,
                file_state,
            }
        })
        .collect()
}

fn file_state_from_row(
    canonical_path: &str,
    origin: &str,
    stable_file_id: Option<String>,
    size_bytes: u64,
    modified_ns: Option<i64>,
) -> FileStateSnapshot {
    let state_key = state_key_for(
        canonical_path,
        origin,
        stable_file_id.as_deref(),
        size_bytes,
        modified_ns,
    );
    FileStateSnapshot {
        canonical_path: canonical_path.to_string(),
        origin: origin.to_string(),
        stable_file_id,
        size_bytes,
        modified_ns,
        state_key,
    }
}

fn build_file_state_snapshot(candidate: &Candidate) -> Option<FileStateSnapshot> {
    let metadata = fs::metadata(&candidate.path).ok()?;
    let canonical_path = candidate.path.to_string_lossy().to_string();
    let stable_file_id = stable_file_id(&metadata);
    let size_bytes = metadata.len();
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64);
    Some(file_state_from_row(
        &canonical_path,
        &candidate.origin,
        stable_file_id,
        size_bytes,
        modified_ns,
    ))
}

fn default_db_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| {
            home.join(".vettd")
                .join("scan-cache")
                .join("scan-v1.sqlite3")
        })
        .ok_or_else(|| "Unable to determine home directory for scan-cache".to_string())
}

#[cfg(unix)]
fn stable_file_id(metadata: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn stable_file_id(_metadata: &fs::Metadata) -> Option<String> {
    None
}

fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn state_key_for(
    canonical_path: &str,
    origin: &str,
    stable_file_id: Option<&str>,
    size_bytes: u64,
    modified_ns: Option<i64>,
) -> String {
    hex_sha256(
        format!(
            "{}:{}:{}:{}:{}",
            canonical_path,
            origin,
            stable_file_id.unwrap_or(""),
            size_bytes,
            modified_ns.unwrap_or_default(),
        )
        .as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn candidate(path: &Path, origin: &str) -> Candidate {
        Candidate {
            path: path.to_path_buf(),
            origin: origin.to_string(),
        }
    }

    #[test]
    fn snapshot_state_key_changes_when_file_changes() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("agents.md");
        fs::write(&file, b"one").unwrap();

        let first = snapshot_candidates(&[candidate(&file, "host")]);
        let first_key = first[0].file_state.as_ref().unwrap().state_key.clone();

        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(&file, b"two two").unwrap();

        let second = snapshot_candidates(&[candidate(&file, "host")]);
        let second_key = second[0].file_state.as_ref().unwrap().state_key.clone();

        assert_ne!(first_key, second_key);
    }

    #[test]
    fn detector_results_round_trip_through_cache() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("scan-v1.sqlite3");
        let file = dir.path().join("mcp.json");
        fs::write(&file, br#"{"mcpServers":{"fs":{"command":"npx"}}}"#).unwrap();

        let mut cache = ScanCache::open_at(&db_path).unwrap();
        let profile = build_profile(
            "host",
            false,
            "~",
            &["custom_rules".to_string(), "containers".to_string()],
            "rules",
        );
        cache.upsert_profile(&profile).unwrap();

        let candidates = snapshot_candidates(&[candidate(&file, "host")]);
        cache
            .upsert_file_states(&profile.profile_key, &candidates)
            .unwrap();

        let mut artifact = ArtifactReport::new("mcp_config", 0.9);
        artifact.metadata.insert(
            "paths".into(),
            serde_json::json!([file.to_string_lossy().to_string()]),
        );
        artifact.compute_hash();

        let mut by_path = HashMap::new();
        by_path.insert(file.to_string_lossy().to_string(), vec![artifact.clone()]);

        cache
            .persist_detector_results(
                &profile.profile_key,
                "mcp_configs",
                &detector_fingerprint("mcp_configs"),
                &candidates,
                &by_path,
            )
            .unwrap();

        let bundles = cache
            .load_detector_bundles(&profile.profile_key, "mcp_configs")
            .unwrap();
        let bundle = bundles.get(file.to_string_lossy().as_ref()).unwrap();

        assert_eq!(bundle.artifacts.len(), 1);
        assert_eq!(bundle.artifacts[0].artifact_type, artifact.artifact_type);
        assert_eq!(bundle.artifacts[0].artifact_hash, artifact.artifact_hash);
    }

    #[test]
    fn cache_enabled_for_workdir_mode() {
        assert!(cache_enabled_for_mode("host"));
        assert!(cache_enabled_for_mode("scan"));
        assert!(cache_enabled_for_mode("workdir"));
        assert!(cache_enabled_for_mode("file"));
        assert!(!cache_enabled_for_mode("root"));
    }

    #[test]
    fn source_risks_is_cacheable_only_for_file_mode() {
        assert!(cacheable_detector("file", "source_risks"));
        assert!(!cacheable_detector("workdir", "source_risks"));
        assert!(cacheable_detector("file", "mcp_configs"));
    }

    #[test]
    fn build_profile_distinguishes_workdir_depth() {
        let shallow = build_profile(
            "workdir",
            false,
            "/tmp/project",
            &["custom_rules".to_string()],
            "rules",
        );
        let deep = build_profile(
            "workdir",
            true,
            "/tmp/project",
            &["custom_rules".to_string()],
            "rules",
        );

        assert_ne!(shallow.profile_key, deep.profile_key);
    }

    #[test]
    fn root_cursor_round_trips() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("scan-v1.sqlite3");
        let cache = ScanCache::open_at(&db_path).unwrap();

        cache
            .upsert_root_cursor("/tmp/root", "macos_fsevents_v1", r#"{"last_event_id":12}"#)
            .unwrap();

        let cursor = cache
            .load_root_cursor("/tmp/root", "macos_fsevents_v1")
            .unwrap()
            .unwrap();

        assert_eq!(cursor.backend_type, "macos_fsevents_v1");
        assert_eq!(cursor.cursor_token, r#"{"last_event_id":12}"#);
    }

    #[test]
    fn load_profile_candidates_for_root_uses_profile_membership() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("scan-v1.sqlite3");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("mcp.json");
        fs::write(&file, br#"{"mcpServers":{"fs":{"command":"npx"}}}"#).unwrap();

        let mut cache = ScanCache::open_at(&db_path).unwrap();
        let host_profile = build_profile("host", false, "~", &["mcp_configs".to_string()], "rules");
        let other_profile = build_profile(
            "workdir",
            true,
            root.to_string_lossy().as_ref(),
            &["mcp_configs".to_string()],
            "rules",
        );
        cache.upsert_profile(&host_profile).unwrap();
        cache.upsert_profile(&other_profile).unwrap();

        let candidates = snapshot_candidates(&[candidate(&file, "host")]);
        cache
            .upsert_file_states(&host_profile.profile_key, &candidates)
            .unwrap();

        let mut artifact = ArtifactReport::new("mcp_config", 0.9);
        artifact.metadata.insert(
            "paths".into(),
            serde_json::json!([file.to_string_lossy().to_string()]),
        );
        artifact.compute_hash();
        let mut by_path = HashMap::new();
        by_path.insert(file.to_string_lossy().to_string(), vec![artifact]);

        cache
            .persist_detector_results(
                &other_profile.profile_key,
                "mcp_configs",
                &detector_fingerprint("mcp_configs"),
                &candidates,
                &by_path,
            )
            .unwrap();

        let loaded = cache
            .load_profile_candidates_for_root(&host_profile.profile_key, &root)
            .unwrap();
        assert!(loaded.is_empty());

        let loaded = cache
            .load_profile_candidates_for_root(&other_profile.profile_key, &root)
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].candidate.path, file);
    }

    #[test]
    fn prune_profile_root_artifacts_removes_deleted_paths() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("scan-v1.sqlite3");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("first.json");
        let second = root.join("second.json");
        fs::write(&first, "{}").unwrap();
        fs::write(&second, "{}").unwrap();

        let mut cache = ScanCache::open_at(&db_path).unwrap();
        let profile = build_profile("scan", false, "~", &["mcp_configs".to_string()], "rules");
        cache.upsert_profile(&profile).unwrap();

        let candidates =
            snapshot_candidates(&[candidate(&first, "home"), candidate(&second, "home")]);
        cache
            .upsert_file_states(&profile.profile_key, &candidates)
            .unwrap();

        let mut by_path = HashMap::new();
        for path in [&first, &second] {
            let mut artifact = ArtifactReport::new("mcp_config", 0.8);
            artifact.metadata.insert(
                "paths".into(),
                serde_json::json!([path.to_string_lossy().to_string()]),
            );
            artifact.compute_hash();
            by_path.insert(path.to_string_lossy().to_string(), vec![artifact]);
        }

        cache
            .persist_detector_results(
                &profile.profile_key,
                "mcp_configs",
                &detector_fingerprint("mcp_configs"),
                &candidates,
                &by_path,
            )
            .unwrap();

        let keep_paths = HashSet::from([first.to_string_lossy().to_string()]);
        cache
            .prune_profile_root_artifacts(&profile.profile_key, &root, &keep_paths)
            .unwrap();

        let loaded = cache
            .load_profile_candidates_for_root(&profile.profile_key, &root)
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].candidate.path, first);
    }
}
