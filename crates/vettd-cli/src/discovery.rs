//! Host and workdir surface discovery.
//!
//! Enumerates candidate files/directories from bounded host roots
//! or an explicit workspace path. Each candidate is tagged with its
//! origin ("host", "workdir", or "filesystem") so downstream detectors
//! and reports can distinguish them.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Guardrails – directory depth limit for bounded walks
// ---------------------------------------------------------------------------

pub const MAX_DEPTH: usize = 5;

/// Hard cap on candidates collected by `discover_root_surfaces` (the
/// `vettd scan full` walk from `/`). `scan full` has no directory-depth
/// bound like the other walkers, so this exists purely to keep the
/// in-memory candidate `Vec` and downstream detector pass bounded on a
/// very large or unusual filesystem.
pub const MAX_ROOT_SCAN_FILES: usize = 500_000;

// ---------------------------------------------------------------------------
// Excluded directory sets
// ---------------------------------------------------------------------------

const NON_FORENSIC_EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "venv",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".nox",
    ".idea",
    ".next",
    "target",
    ".cache",
    "cache",
    "Caches",
    ".cargo",
    ".rustup",
    ".npm",
    ".pnpm-store",
    ".yarn",
    ".gradle",
    ".m2",
    ".terraform",
    ".bundle",
    ".gem",
    ".nuget",
    ".swiftpm",
    ".build",
    "DerivedData",
    "vendor",
];

const FILESYSTEM_EXTRA_EXCLUDED: &[&str] = &[
    "proc",
    "sys",
    "dev",
    "run",
    "snap",
    "boot",
    "tmp",
    "private",
    "cores",
    "Volumes",
    "Network",
    "automount",
    "System",
    "Library",
];

const AI_CLI_CONFIG_DIRS: &[&str] = &[
    ".claude",
    ".cursor",
    ".aider",
    ".ollama",
    ".continue",
    ".vscode",
    ".vscode-insiders",
];

const FILESYSTEM_EXTRA_ROOTS: &[&str] = &["/Applications", "/opt/homebrew", "/usr/local"];

const MACOS_EDITOR_USER_DIRS: &[&str] = &["Code/User", "Code - Insiders/User", "Cursor/User"];
const LINUX_EDITOR_USER_DIRS: &[&str] = &["Code/User", "Code - Insiders/User", "Cursor/User"];
const WINDOWS_EDITOR_USER_DIRS: &[&str] = &["Code/User", "Code - Insiders/User", "Cursor/User"];

const MACOS_USER_SPACE_DIRS: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Developer",
    "Projects",
    "Code",
    "Workspace",
    "Work",
    "src",
    "GitHub",
];

const LINUX_USER_SPACE_DIRS: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "projects",
    "code",
    "workspace",
    "work",
    "src",
    "git",
    "GitHub",
];

const WINDOWS_USER_SPACE_DIRS: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Projects",
    "Code",
    "Workspace",
    "Source",
    "src",
    "GitHub",
];

// ---------------------------------------------------------------------------
// Candidate model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub origin: String,
}

// ---------------------------------------------------------------------------
// Excluded-dir helpers
// ---------------------------------------------------------------------------

fn nonforensic_excluded_set() -> HashSet<&'static str> {
    NON_FORENSIC_EXCLUDED_DIRS.iter().copied().collect()
}

fn filesystem_excluded_set() -> HashSet<&'static str> {
    let mut set = nonforensic_excluded_set();
    for d in FILESYSTEM_EXTRA_EXCLUDED {
        set.insert(d);
    }
    set
}

fn is_excluded_dir(entry: &walkdir::DirEntry, excluded: &HashSet<&str>) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    entry
        .file_name()
        .to_str()
        .is_some_and(|name| excluded.contains(name))
}

/// `.vscode` is a targeted exception to directory exclusion: the MCP
/// detector explicitly looks for `mcp.json`, and VS Code's per-project
/// config directory is a standard location for it. We still don't want
/// general editor noise (settings.json, launch.json, extensions/, etc.)
/// surfacing as candidates, so an organically-discovered `.vscode` is
/// always descended into (to see its direct children) but nothing *inside*
/// it is ever descended into further, and `is_vscode_noise_file` filters
/// its direct children down to `mcp.json` only. An explicitly-targeted
/// `.vscode` root (depth 0) is unaffected and keeps its full contents,
/// same as any other explicit root.
fn is_vscode_dir(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir() && entry.file_name().to_str() == Some(".vscode")
}

fn is_inside_organic_vscode_dir(entry: &walkdir::DirEntry, walk_root: &Path) -> bool {
    if walk_root.file_name().and_then(|n| n.to_str()) == Some(".vscode") {
        // The walk root itself is a .vscode directory — it was explicitly
        // targeted, so scan its full contents (matching the general
        // explicit-root-is-never-excluded rule) rather than applying the
        // organic-discovery noise heuristic anywhere within it.
        return false;
    }
    entry
        .path()
        .parent()
        .filter(|parent| *parent != walk_root)
        .is_some_and(|parent| parent.file_name().and_then(|n| n.to_str()) == Some(".vscode"))
}

fn should_descend(entry: &walkdir::DirEntry, excluded: &HashSet<&str>, walk_root: &Path) -> bool {
    // filter_entry runs on every entry (files included) — a `false` here
    // means "don't yield this entry at all", not just "don't descend into
    // it". The .vscode-subdirectory rule below must only ever prune
    // directories; files (e.g. `.vscode/mcp.json` itself) always need to
    // pass through here so `is_vscode_noise_file` gets a chance to filter
    // them by name afterward.
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }
    if is_inside_organic_vscode_dir(entry, walk_root) {
        // A directory (e.g. `extensions/`, or even a nested `.vscode/`)
        // inside an organically-found `.vscode`: never descend into it,
        // only the outer `.vscode`'s direct mcp.json child is ever a
        // candidate. Checked before is_vscode_dir below so a
        // pathologically nested `.vscode/.vscode/` doesn't re-open descent.
        return false;
    }
    if is_vscode_dir(entry) {
        return true;
    }
    !is_excluded_dir(entry, excluded)
}

fn is_regular_file(entry: &walkdir::DirEntry) -> bool {
    if entry.file_type().is_file() {
        return true;
    }
    if entry.file_type().is_symlink() {
        return entry
            .path()
            .metadata()
            .map(|m| m.is_file())
            .unwrap_or(false);
    }
    false
}

/// Filters out non-`mcp.json` direct children of a `.vscode` directory that
/// was reached organically during a walk (see `is_vscode_dir`), while
/// leaving an explicitly-targeted `.vscode` root's contents untouched.
/// Nested subdirectories of an organic `.vscode` never reach this check —
/// `should_descend`/`is_inside_organic_vscode_dir` already prunes descent
/// into them.
fn is_vscode_noise_file(entry: &walkdir::DirEntry, walk_root: &Path) -> bool {
    is_inside_organic_vscode_dir(entry, walk_root)
        && entry.path().file_name().and_then(|n| n.to_str()) != Some("mcp.json")
}

fn is_included_file(entry: &walkdir::DirEntry, walk_root: &Path) -> bool {
    is_regular_file(entry) && !is_vscode_noise_file(entry, walk_root)
}

// ---------------------------------------------------------------------------
// Platform-aware roots
// ---------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

fn existing_unique_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| path.exists())
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn default_user_space_dir_names() -> &'static [&'static str] {
    match std::env::consts::OS {
        "macos" => MACOS_USER_SPACE_DIRS,
        "windows" => WINDOWS_USER_SPACE_DIRS,
        _ => LINUX_USER_SPACE_DIRS,
    }
}

fn join_existing_relative_roots(base: &Path, relatives: &[&str]) -> Vec<PathBuf> {
    existing_unique_paths(relatives.iter().map(|relative| base.join(relative)))
}

pub fn host_roots() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let mut roots = ai_cli_config_roots();

    match std::env::consts::OS {
        "macos" => {
            let app_support = home.join("Library").join("Application Support");
            roots.extend(join_existing_relative_roots(
                &app_support,
                MACOS_EDITOR_USER_DIRS,
            ));
        }
        "windows" => {
            if let Some(config_dir) = dirs::config_dir() {
                roots.extend(join_existing_relative_roots(
                    &config_dir,
                    WINDOWS_EDITOR_USER_DIRS,
                ));
            }
        }
        _ => {
            let config_dir = home.join(".config");
            roots.extend(join_existing_relative_roots(
                &config_dir,
                LINUX_EDITOR_USER_DIRS,
            ));
        }
    }

    existing_unique_paths(roots)
}

pub fn browser_profile_roots() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let roots = match std::env::consts::OS {
        "macos" => {
            let app_support = home.join("Library").join("Application Support");
            vec![
                app_support.join("Google").join("Chrome"),
                app_support.join("Microsoft Edge"),
                app_support.join("BraveSoftware").join("Brave-Browser"),
                app_support.join("Arc").join("User Data"),
            ]
        }
        "linux" => {
            let config = home.join(".config");
            vec![
                config.join("google-chrome"),
                config.join("microsoft-edge"),
                config.join("BraveSoftware").join("Brave-Browser"),
            ]
        }
        _ => Vec::new(),
    };
    roots.into_iter().filter(|r| r.exists()).collect()
}

pub fn ai_cli_config_roots() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    AI_CLI_CONFIG_DIRS
        .iter()
        .map(|d| home.join(d))
        .filter(|p| p.exists())
        .collect()
}

pub fn default_user_space_roots() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    existing_unique_paths(
        default_user_space_dir_names()
            .iter()
            .map(|dir| home.join(dir)),
    )
}

// ---------------------------------------------------------------------------
// Walking functions
// ---------------------------------------------------------------------------

pub fn walk_bounded(root: &Path, origin: &str, on_tick: Option<&dyn Fn(&str)>) -> Vec<Candidate> {
    let excluded = nonforensic_excluded_set();
    let mut candidates = Vec::new();
    let mut count: usize = 0;
    let mut depth_cap_hit = false;

    let walker = WalkDir::new(root).max_depth(MAX_DEPTH).follow_links(false);
    let filtered = walker
        .into_iter()
        .filter_entry(|e| should_descend(e, &excluded, root));

    for entry in filtered.filter_map(|e| e.ok()) {
        if entry.depth() == MAX_DEPTH && entry.file_type().is_dir() {
            depth_cap_hit = true;
        }
        if !is_included_file(&entry, root) {
            continue;
        }
        candidates.push(Candidate {
            path: entry.into_path(),
            origin: origin.to_string(),
        });
        count += 1;
        if let Some(tick) = on_tick {
            if count % 5000 == 0 {
                tick(&format!("{count} files"));
            }
        }
    }

    if depth_cap_hit {
        eprintln!(
            "warning: scan depth capped at {MAX_DEPTH}; some files may have been skipped (use --deep for a full scan)"
        );
    }
    candidates
}

pub fn walk_deep_workdir(
    root: &Path,
    origin: &str,
    on_tick: Option<&dyn Fn(&str)>,
) -> Vec<Candidate> {
    let excluded = nonforensic_excluded_set();
    let mut candidates = Vec::new();
    let mut count: usize = 0;

    let walker = WalkDir::new(root).follow_links(false);
    let filtered = walker
        .into_iter()
        .filter_entry(|e| should_descend(e, &excluded, root));

    for entry in filtered.filter_map(|e| e.ok()) {
        if !is_included_file(&entry, root) {
            continue;
        }
        candidates.push(Candidate {
            path: entry.into_path(),
            origin: origin.to_string(),
        });
        count += 1;
        if let Some(tick) = on_tick {
            if count % 5000 == 0 {
                tick(&format!("{count} files"));
            }
        }
    }
    candidates
}

fn discover_direct_files(root: &Path, origin: &str) -> Vec<Candidate> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_file())
                .map(|_| Candidate {
                    path: entry.path(),
                    origin: origin.to_string(),
                })
        })
        .collect()
}

pub fn discover_direct_home_files() -> Vec<Candidate> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    discover_direct_files(&home, "home")
}

fn extend_unique_candidates(
    candidates: &mut Vec<Candidate>,
    seen: &mut HashSet<PathBuf>,
    incoming: Vec<Candidate>,
) {
    for candidate in incoming {
        if seen.insert(candidate.path.clone()) {
            candidates.push(candidate);
        }
    }
}

// ---------------------------------------------------------------------------
// High-level discovery entry points
// ---------------------------------------------------------------------------

pub fn discover_host_surfaces(on_tick: Option<&dyn Fn(&str)>) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for root in host_roots() {
        candidates.extend(walk_bounded(&root, "host", on_tick));
    }
    candidates
}

pub fn discover_scan_surfaces(on_tick: Option<&dyn Fn(&str)>) -> Vec<Candidate> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    extend_unique_candidates(
        &mut candidates,
        &mut seen,
        discover_direct_files(&home, "home"),
    );

    for root in host_roots() {
        extend_unique_candidates(
            &mut candidates,
            &mut seen,
            walk_bounded(&root, "host", on_tick),
        );
    }

    for root in default_user_space_roots() {
        extend_unique_candidates(
            &mut candidates,
            &mut seen,
            walk_bounded(&root, "home", on_tick),
        );
    }

    candidates
}

pub fn discover_workdir_surfaces(
    path: &Path,
    deep: bool,
    on_tick: Option<&dyn Fn(&str)>,
) -> Vec<Candidate> {
    let resolved = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    if !resolved.is_dir() {
        return Vec::new();
    }
    if deep {
        walk_deep_workdir(&resolved, "workdir", on_tick)
    } else {
        walk_bounded(&resolved, "workdir", on_tick)
    }
}

pub fn discover_filesystem_surfaces(on_tick: Option<&dyn Fn(&str)>) -> Vec<Candidate> {
    let excluded = filesystem_excluded_set();
    let mut candidates = Vec::new();
    let mut count: usize = 0;

    let mut scan_roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        scan_roots.push(home);
    }
    for extra in FILESYSTEM_EXTRA_ROOTS {
        let p = PathBuf::from(extra);
        if p.exists() {
            scan_roots.push(p);
        }
    }

    for root in &scan_roots {
        let walker = WalkDir::new(root).follow_links(false);
        let filtered = walker
            .into_iter()
            .filter_entry(|e| should_descend(e, &excluded, root));

        for entry in filtered.filter_map(|e| e.ok()) {
            if !is_included_file(&entry, root) {
                continue;
            }
            candidates.push(Candidate {
                path: entry.into_path(),
                origin: "filesystem".to_string(),
            });
            count += 1;
            if let Some(tick) = on_tick {
                if count % 10_000 == 0 {
                    tick(&format!("{count} files"));
                }
            }
        }
    }
    candidates
}

pub fn discover_home_surfaces(on_tick: Option<&dyn Fn(&str)>) -> Vec<Candidate> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    walk_deep_workdir(&home, "home", on_tick)
}

// Full scan: prune pseudo-filesystems (/proc, /sys, /dev, ...) and the same
// low-value dependency/cache/VCS directories every other walker excludes
// (node_modules, .git, .cargo, vendor, target, ...). Without this, `scan
// full` enumerates the entire tree — including virtual filesystems that can
// hang or grow unbounded — into one in-memory Vec, and floods results with
// vendored copies of files like AGENTS.md/.cursorrules weighted the same as
// first-party ones. On top of that, `cap` bounds total candidates so a huge
// disk can't grow the in-memory Vec without limit.
fn walk_root_with_cap(
    root: &Path,
    origin: &str,
    excluded: &HashSet<&str>,
    cap: usize,
    on_tick: Option<&dyn Fn(&str)>,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let mut count: usize = 0;

    if cap == 0 {
        return candidates;
    }

    let walker = WalkDir::new(root).follow_links(false);
    let filtered = walker
        .into_iter()
        .filter_entry(|e| should_descend(e, excluded, root));

    let mut cap_hit = false;
    for entry in filtered.filter_map(|e| e.ok()) {
        if !is_included_file(&entry, root) {
            continue;
        }
        candidates.push(Candidate {
            path: entry.into_path(),
            origin: origin.to_string(),
        });
        count += 1;
        if let Some(tick) = on_tick {
            if count % 10_000 == 0 {
                tick(&format!("{count} files"));
            }
        }
        if count >= cap {
            cap_hit = true;
            break;
        }
    }

    if cap_hit {
        eprintln!(
            "warning: full scan capped at {cap} files; results may be incomplete (use \
             `vettd scan repo`/`vettd scan folder` for a bounded, thorough scan of a specific \
             directory)"
        );
    }
    candidates
}

pub fn discover_root_surfaces(on_tick: Option<&dyn Fn(&str)>) -> Vec<Candidate> {
    let excluded = filesystem_excluded_set();
    let root = if cfg!(windows) {
        PathBuf::from("C:\\")
    } else {
        PathBuf::from("/")
    };
    walk_root_with_cap(&root, "root", &excluded, MAX_ROOT_SCAN_FILES, on_tick)
}

pub fn discover_file_surface(path: &Path) -> Vec<Candidate> {
    let resolved = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    if !resolved.is_file() {
        return Vec::new();
    }
    vec![Candidate {
        path: resolved,
        origin: "workdir".to_string(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn nonforensic_excluded_set_contains_expected_dirs() {
        let set = nonforensic_excluded_set();
        assert!(set.contains(".git"));
        assert!(set.contains("node_modules"));
        assert!(set.contains("target"));
        assert!(set.contains("__pycache__"));
        assert!(set.contains(".cargo"));
        assert!(set.contains("vendor"));
    }

    #[test]
    fn vscode_is_descended_but_not_dir_excluded() {
        // .vscode is intentionally absent from the exclusion set: it's a
        // targeted exception (see `is_vscode_dir`/`is_vscode_noise_file`)
        // so mcp.json is still found, rather than a general exclusion.
        let set = nonforensic_excluded_set();
        assert!(!set.contains(".vscode"));
    }

    #[test]
    fn filesystem_excluded_set_extends_nonforensic_set() {
        let deep = nonforensic_excluded_set();
        let fs_set = filesystem_excluded_set();
        for item in &deep {
            assert!(fs_set.contains(item));
        }
        assert!(fs_set.contains("proc"));
        assert!(fs_set.contains("sys"));
        assert!(fs_set.contains("Library"));
    }

    #[test]
    fn walk_bounded_finds_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("test.txt"), "hello").unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub").join("nested.txt"), "world").unwrap();

        let candidates = walk_bounded(tmp.path(), "test", None);
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|c| c.origin == "test"));
    }

    #[test]
    fn walk_bounded_skips_dirs() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let candidates = walk_bounded(tmp.path(), "test", None);
        assert!(candidates.is_empty());
    }

    #[test]
    fn walk_bounded_excludes_low_value_dirs() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("target")).unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("target").join("generated.txt"), "noise").unwrap();
        fs::write(tmp.path().join("src").join("main.rs"), "fn main() {}\n").unwrap();

        let candidates = walk_bounded(tmp.path(), "test", None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with("src/main.rs"));
    }

    #[test]
    fn walk_bounded_preserves_explicit_root_even_if_name_is_excluded() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join(".vscode");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("settings.json"), "{}\n").unwrap();

        let candidates = walk_bounded(&root, "host", None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with(".vscode/settings.json"));
    }

    #[test]
    fn walk_bounded_preserves_full_explicit_vscode_root_including_nested_subdirs() {
        // Same invariant as the walk_deep_workdir version, exercised
        // through the bounded (MAX_DEPTH-limited) walker used by default
        // workdir/host scans.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join(".vscode");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("mcp.json"), "{}\n").unwrap();
        fs::write(root.join("settings.json"), "{}\n").unwrap();
        let nested = root.join("extensions").join("foo");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("package.json"), "{}\n").unwrap();
        let inner_vscode = root.join(".vscode");
        fs::create_dir(&inner_vscode).unwrap();
        fs::write(inner_vscode.join("settings.json"), "{}\n").unwrap();

        let candidates = walk_bounded(&root, "host", None);
        assert_eq!(candidates.len(), 4, "found: {candidates:?}");
    }

    #[test]
    fn walk_deep_workdir_preserves_full_explicit_vscode_root_including_nested_subdirs() {
        // The organic-.vscode noise heuristic must not leak into an
        // explicitly-targeted `.vscode` root: if a user explicitly points
        // the scanner at `.vscode` (e.g. `vettd scan folder ~/.vscode`),
        // they want its full contents, including nested subdirectories and
        // even a pathological nested `.vscode/.vscode`, not just mcp.json.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join(".vscode");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("mcp.json"), "{}\n").unwrap();
        fs::write(root.join("settings.json"), "{}\n").unwrap();
        let nested = root.join("extensions").join("foo");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("package.json"), "{}\n").unwrap();
        let inner_vscode = root.join(".vscode");
        fs::create_dir(&inner_vscode).unwrap();
        fs::write(inner_vscode.join("settings.json"), "{}\n").unwrap();

        let candidates = walk_deep_workdir(&root, "host", None);
        assert_eq!(candidates.len(), 4, "found: {candidates:?}");
    }

    #[test]
    fn walk_bounded_finds_nested_vscode_mcp_json() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".vscode")).unwrap();
        fs::write(tmp.path().join(".vscode").join("mcp.json"), "{}\n").unwrap();

        let candidates = walk_bounded(tmp.path(), "workdir", None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with(".vscode/mcp.json"));
    }

    #[test]
    fn walk_bounded_ignores_other_files_in_nested_vscode_dir() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".vscode")).unwrap();
        fs::write(tmp.path().join(".vscode").join("mcp.json"), "{}\n").unwrap();
        fs::write(tmp.path().join(".vscode").join("settings.json"), "{}\n").unwrap();
        fs::write(tmp.path().join(".vscode").join("launch.json"), "{}\n").unwrap();

        let candidates = walk_bounded(tmp.path(), "workdir", None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with(".vscode/mcp.json"));
    }

    #[test]
    fn walk_deep_workdir_finds_nested_vscode_mcp_json() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".vscode")).unwrap();
        fs::write(tmp.path().join(".vscode").join("mcp.json"), "{}\n").unwrap();
        fs::write(tmp.path().join(".vscode").join("settings.json"), "{}\n").unwrap();

        let candidates = walk_deep_workdir(tmp.path(), "workdir", None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with(".vscode/mcp.json"));
    }

    #[test]
    fn walk_deep_workdir_does_not_descend_into_nested_vscode_subdirectories() {
        // The .vscode targeted exception must not become a general
        // un-exclusion: files nested *inside* an organically-discovered
        // .vscode directory (e.g. .vscode/extensions/foo/package.json)
        // must stay excluded, not just direct siblings of mcp.json.
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".vscode")).unwrap();
        fs::write(tmp.path().join(".vscode").join("mcp.json"), "{}\n").unwrap();
        let nested = tmp.path().join(".vscode").join("extensions").join("foo");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("package.json"), "{}\n").unwrap();

        let candidates = walk_deep_workdir(tmp.path(), "workdir", None);
        assert_eq!(candidates.len(), 1, "found: {candidates:?}");
        assert!(candidates[0].path.ends_with(".vscode/mcp.json"));
    }

    #[test]
    fn walk_deep_workdir_does_not_treat_pathologically_nested_vscode_as_a_new_root() {
        // A directory literally named `.vscode` nested inside an
        // organically-discovered `.vscode` must not re-open descent — the
        // "always descend into a directory named .vscode" rule is only
        // meant to apply once, to reach the outer .vscode's own direct
        // mcp.json, not recursively re-trigger on nested `.vscode/.vscode`.
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".vscode")).unwrap();
        fs::write(tmp.path().join(".vscode").join("mcp.json"), "{}\n").unwrap();
        let inner_vscode = tmp.path().join(".vscode").join(".vscode");
        fs::create_dir(&inner_vscode).unwrap();
        fs::write(inner_vscode.join("mcp.json"), "{}\n").unwrap();

        let candidates = walk_deep_workdir(tmp.path(), "workdir", None);
        assert_eq!(candidates.len(), 1, "found: {candidates:?}");
        assert!(candidates[0].path.ends_with(".vscode/mcp.json"));
        assert!(!candidates[0].path.ends_with(".vscode/.vscode/mcp.json"));
    }

    #[cfg(unix)]
    #[test]
    fn walk_bounded_finds_symlinked_files() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real.txt");
        let link = tmp.path().join("SKILL.md");
        fs::write(&real, "content").unwrap();
        symlink(&real, &link).unwrap();

        let candidates = walk_bounded(tmp.path(), "test", None);
        let paths: Vec<_> = candidates
            .iter()
            .map(|c| c.path.file_name().unwrap())
            .collect();
        assert!(
            paths.contains(&std::ffi::OsStr::new("SKILL.md")),
            "symlinked file should be found"
        );
    }

    #[cfg(unix)]
    #[test]
    fn walk_deep_workdir_finds_symlinked_files() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real.txt");
        let link = tmp.path().join("SKILL.md");
        fs::write(&real, "content").unwrap();
        symlink(&real, &link).unwrap();

        let candidates = walk_deep_workdir(tmp.path(), "test", None);
        let paths: Vec<_> = candidates
            .iter()
            .map(|c| c.path.file_name().unwrap())
            .collect();
        assert!(
            paths.contains(&std::ffi::OsStr::new("SKILL.md")),
            "symlinked file should be found"
        );
    }

    #[test]
    fn walk_deep_workdir_excludes_git() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".git").join("config"), "git data").unwrap();
        fs::write(tmp.path().join("real.txt"), "real file").unwrap();

        let candidates = walk_deep_workdir(tmp.path(), "workdir", None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with("real.txt"));
    }

    #[test]
    fn walk_deep_workdir_excludes_node_modules() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("node_modules")).unwrap();
        fs::write(tmp.path().join("node_modules").join("package.json"), "{}").unwrap();
        fs::write(tmp.path().join("index.js"), "code").unwrap();

        let candidates = walk_deep_workdir(tmp.path(), "workdir", None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with("index.js"));
    }

    #[test]
    fn walk_deep_workdir_excludes_dependency_cache_dirs() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".cargo").join("registry").join("src")).unwrap();
        fs::write(
            tmp.path()
                .join(".cargo")
                .join("registry")
                .join("src")
                .join("agents.md"),
            "cached dependency file",
        )
        .unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "real file").unwrap();

        let candidates = walk_deep_workdir(tmp.path(), "test", None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with("AGENTS.md"));
    }

    #[test]
    fn walk_root_with_cap_excludes_low_value_dirs_and_finds_vscode_mcp_json() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        fs::write(tmp.path().join("node_modules").join("agents.md"), "noise").unwrap();
        fs::create_dir(tmp.path().join(".vscode")).unwrap();
        fs::write(tmp.path().join(".vscode").join("mcp.json"), "{}").unwrap();
        fs::write(tmp.path().join(".vscode").join("settings.json"), "{}").unwrap();
        fs::write(tmp.path().join("real.txt"), "real file").unwrap();

        let excluded = filesystem_excluded_set();
        let candidates = walk_root_with_cap(tmp.path(), "root", &excluded, usize::MAX, None);

        let paths: Vec<_> = candidates.iter().map(|c| c.path.clone()).collect();
        assert_eq!(candidates.len(), 2, "found: {paths:?}");
        assert!(paths.iter().any(|p| p.ends_with("real.txt")));
        assert!(paths.iter().any(|p| p.ends_with(".vscode/mcp.json")));
    }

    #[test]
    fn walk_root_with_cap_stops_at_the_cap() {
        let tmp = TempDir::new().unwrap();
        for i in 0..5 {
            fs::write(tmp.path().join(format!("file{i}.txt")), "content").unwrap();
        }

        let excluded = filesystem_excluded_set();
        let candidates = walk_root_with_cap(tmp.path(), "root", &excluded, 2, None);

        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn discover_file_surface_single_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("agents.md");
        fs::write(&file, "# Agents").unwrap();

        let candidates = discover_file_surface(&file);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].origin, "workdir");
    }

    #[test]
    fn discover_file_surface_nonexistent() {
        let candidates = discover_file_surface(Path::new("/nonexistent/file.txt"));
        assert!(candidates.is_empty());
    }

    #[test]
    fn discover_file_surface_directory_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let candidates = discover_file_surface(tmp.path());
        assert!(candidates.is_empty());
    }

    #[test]
    fn discover_workdir_surfaces_nonexistent() {
        let candidates = discover_workdir_surfaces(Path::new("/nonexistent/path"), false, None);
        assert!(candidates.is_empty());
    }

    #[test]
    fn discover_workdir_surfaces_finds_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("test.md"), "hello").unwrap();

        let candidates = discover_workdir_surfaces(tmp.path(), false, None);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn discover_workdir_deep_excludes_git() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".git").join("HEAD"), "ref").unwrap();
        fs::write(tmp.path().join("code.rs"), "fn main() {}").unwrap();

        let candidates = discover_workdir_surfaces(tmp.path(), true, None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with("code.rs"));
    }

    #[test]
    fn host_roots_returns_existing_paths() {
        let roots = host_roots();
        for root in &roots {
            assert!(root.exists(), "{:?} should exist", root);
        }
    }

    #[test]
    fn ai_cli_config_roots_returns_existing_paths() {
        let roots = ai_cli_config_roots();
        for root in &roots {
            assert!(root.exists(), "{:?} should exist", root);
        }
    }

    #[test]
    fn default_user_space_dir_names_include_documents() {
        let dirs = default_user_space_dir_names();
        assert!(dirs.contains(&"Documents"));
    }

    #[test]
    fn discover_direct_files_only_collects_immediate_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("agents.md"), "hello").unwrap();
        fs::create_dir(tmp.path().join("nested")).unwrap();
        fs::write(tmp.path().join("nested").join("other.md"), "world").unwrap();

        let candidates = discover_direct_files(tmp.path(), "home");
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].path.ends_with("agents.md"));
        assert_eq!(candidates[0].origin, "home");
    }

    #[test]
    fn extend_unique_candidates_deduplicates_paths() {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        let path = PathBuf::from("/tmp/agents.md");

        extend_unique_candidates(
            &mut candidates,
            &mut seen,
            vec![Candidate {
                path: path.clone(),
                origin: "host".to_string(),
            }],
        );
        extend_unique_candidates(
            &mut candidates,
            &mut seen,
            vec![Candidate {
                path,
                origin: "home".to_string(),
            }],
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].origin, "host");
    }
}
