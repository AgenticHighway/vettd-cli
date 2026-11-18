//! Repository context for a local skill scan (vettd#1271).
//!
//! The scanner can only tell shared repository content from the skill's own folder when it is
//! told where the skill sits in its repository. A local scan knows that from disk: the nearest
//! `.git` ancestor is the repository root, and a bounded walk of it gives the path list the
//! scanner classifies references against. Without a repository there is no context, and the
//! scanner treats a `../` escape as unverifiable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest, Sha256};

/// Same bound the hosted side applies to the repository path list.
const MAX_REPO_PATHS: usize = 20_000;
/// Directory entries visited, so a path-free tree cannot walk forever.
const MAX_WALK_ENTRIES: usize = 200_000;

/// Owned repository context for one skill.
#[derive(Debug, Clone)]
pub(crate) struct OwnedRepoContext {
    /// The skill directory relative to the repository root; empty when it is the root.
    pub bundle_path: String,
    /// The skill directory's own name.
    pub skill_dir_name: Option<String>,
    /// Every repository path, relative to the root, capped.
    pub repo_paths: Arc<Vec<String>>,
    /// Digest of the path list, folded into the assessment fingerprint so a cached assessment
    /// cannot outlive a change in what the skill's references resolve to.
    pub repo_digest: String,
}

impl OwnedRepoContext {
    pub(crate) fn as_scanner_context(&self) -> vettd_skill_scanner::RepoContext<'_> {
        vettd_skill_scanner::RepoContext {
            bundle_path: &self.bundle_path,
            repo_paths: &self.repo_paths,
            skill_dir_name: self.skill_dir_name.as_deref(),
        }
    }
}

type WalkCache = Mutex<HashMap<PathBuf, (Arc<Vec<String>>, String)>>;

fn cache() -> &'static WalkCache {
    static CACHE: OnceLock<WalkCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Context for the skill at `skill_root`, or `None` when it is not inside a git work tree.
pub(crate) fn for_skill(skill_root: &Path) -> Option<OwnedRepoContext> {
    let (repo_root, _) = super::identity::find_git_dir(skill_root)?;
    let bundle_path = skill_root
        .strip_prefix(&repo_root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    let skill_dir_name = skill_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .or_else(|| {
            skill_root
                .canonicalize()
                .ok()?
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        });
    let (repo_paths, repo_digest) = walk_cached(&repo_root);
    Some(OwnedRepoContext {
        bundle_path,
        skill_dir_name,
        repo_paths,
        repo_digest,
    })
}

fn walk_cached(root: &Path) -> (Arc<Vec<String>>, String) {
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(root).cloned()) {
        return hit;
    }
    let paths = walk_repo(root);
    let mut hasher = Sha256::new();
    for p in &paths {
        hasher.update(p.as_bytes());
        hasher.update([0]);
    }
    let entry = (Arc::new(paths), format!("{:x}", hasher.finalize()));
    if let Ok(mut c) = cache().lock() {
        c.insert(root.to_path_buf(), entry.clone());
    }
    entry
}

/// Deterministic bounded walk: files only, no `.git`, no symlinks, sorted.
fn walk_repo(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut visited = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            visited += 1;
            if visited > MAX_WALK_ENTRIES || out.len() >= MAX_REPO_PATHS {
                out.sort();
                return out;
            }
            if entry.file_name() == ".git" {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(entry.path());
            } else if let Ok(rel) = entry.path().strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

/// Fold the repository classification into a loader fingerprint.
pub(crate) fn fold_fingerprint(load_fingerprint: &str, ctx: Option<&OwnedRepoContext>) -> String {
    match ctx {
        None => load_fingerprint.to_string(),
        Some(c) => {
            let mut hasher = Sha256::new();
            hasher.update(load_fingerprint.as_bytes());
            hasher.update(c.bundle_path.as_bytes());
            hasher.update([0]);
            hasher.update(c.skill_dir_name.as_deref().unwrap_or("").as_bytes());
            hasher.update([0]);
            hasher.update(c.repo_digest.as_bytes());
            format!("{:x}", hasher.finalize())
        }
    }
}
