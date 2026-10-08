//! Registry of AI harnesses and where each keeps user-scoped assets.
//!
//! `scan quick` examines exactly the directories this table resolves to, so
//! supporting a new harness is a new row here, not new walking logic. Each row
//! lists locations relative to the user's home directory and to the
//! platform's user config directory, plus an optional environment variable the
//! harness documents for relocating its home.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub struct Harness {
    pub name: &'static str,
    /// Environment variable that relocates the harness's home directory.
    pub env_override: Option<&'static str>,
    /// Paths relative to the user's home directory, identical on every OS.
    pub home_dirs: &'static [&'static str],
    /// Paths relative to the platform user config directory (`~/.config`,
    /// `~/Library/Application Support` or `%APPDATA%`).
    pub config_dirs: &'static [&'static str],
}

const fn harness(
    name: &'static str,
    env_override: Option<&'static str>,
    home_dirs: &'static [&'static str],
    config_dirs: &'static [&'static str],
) -> Harness {
    Harness {
        name,
        env_override,
        home_dirs,
        config_dirs,
    }
}

pub const HARNESSES: &[Harness] = &[
    harness("claude", Some("CLAUDE_CONFIG_DIR"), &[".claude"], &[]),
    harness("cursor", None, &[".cursor"], &[]),
    harness("aider", None, &[".aider"], &[]),
    harness("ollama", None, &[".ollama"], &[]),
    harness("continue", None, &[".continue"], &[]),
    harness(
        "vscode",
        None,
        &[".vscode", ".vscode-insiders"],
        &["Code/User", "Code - Insiders/User", "Cursor/User"],
    ),
    harness("codex", Some("CODEX_HOME"), &[".codex"], &[]),
    harness("hermes", Some("HERMES_HOME"), &[".hermes"], &[]),
    harness("openclaw", Some("OPENCLAW_STATE_DIR"), &[".openclaw"], &[]),
    harness("openhands", None, &[".openhands"], &[]),
    harness("opencode", None, &[".config/opencode"], &["opencode"]),
    harness("agents", None, &[".agents"], &[]),
    harness("gemini", None, &[".gemini"], &[]),
    harness("copilot", None, &[".copilot"], &[]),
    harness("kiro", None, &[".kiro"], &[]),
    harness("windsurf", None, &[".windsurf", ".codeium/windsurf"], &[]),
    harness("cline", None, &[".cline"], &[]),
    harness("qwen", None, &[".qwen"], &[]),
];

/// Resolve every harness location that exists, in registry order.
///
/// `env` is injected so resolution is testable without touching the process
/// environment.
pub fn resolve_roots(
    home: &Path,
    config_dir: Option<&Path>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut roots = Vec::new();
    for harness in HARNESSES {
        let overridden = harness
            .env_override
            .and_then(env)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let candidates = overridden
            .into_iter()
            .chain(harness.home_dirs.iter().map(|rel| home.join(rel)))
            .chain(
                config_dir
                    .into_iter()
                    .flat_map(|base| harness.config_dirs.iter().map(move |rel| base.join(rel))),
            );
        for path in candidates {
            if path.exists() && seen.insert(path.clone()) {
                roots.push(path);
            }
        }
    }
    roots
}

/// The current user's harness roots, honoring the real process environment.
pub fn user_scoped_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    resolve_roots(&home, dirs::config_dir().as_deref(), &|key| {
        std::env::var_os(key)
    })
}

/// Names under the users directory that are not real user homes.
const NON_USER_HOME_DIRS: &[&str] = &[
    "Shared",
    "Guest",
    "Public",
    "Default",
    "Default User",
    "All Users",
    "Defaultuser0",
];

fn config_dir_in(home: &Path) -> PathBuf {
    match std::env::consts::OS {
        "macos" => home.join("Library").join("Application Support"),
        "windows" => home.join("AppData").join("Roaming"),
        _ => home.join(".config"),
    }
}

/// Other local users' homes: the siblings of `current_home` (`/home/*`,
/// `/Users/*`, `C:\\Users\\*`) and, on Linux, `/root`. Returns
/// `(readable, unreadable)`; a home this process may not list is reported
/// rather than silently treated as empty.
pub fn other_user_homes(current_home: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(entries) = current_home
        .parent()
        .and_then(|users_dir| std::fs::read_dir(users_dir).ok())
    {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let skip = name
                .to_str()
                .is_none_or(|n| n.starts_with('.') || NON_USER_HOME_DIRS.contains(&n));
            if !skip && entry.path() != current_home && entry.path().is_dir() {
                candidates.push(entry.path());
            }
        }
    }
    if std::env::consts::OS == "linux" {
        let root_home = PathBuf::from("/root");
        if root_home != current_home && !candidates.contains(&root_home) && root_home.exists() {
            candidates.push(root_home);
        }
    }
    candidates.sort();
    candidates
        .into_iter()
        .partition(|home| std::fs::read_dir(home).is_ok())
}

/// Harness roots for every other local user, for `scan quick --all-users`.
/// Environment overrides belong to the current user, so they are not applied.
/// Homes that cannot be read are named on stderr.
pub fn other_users_roots() -> Vec<PathBuf> {
    let Some(current) = dirs::home_dir() else {
        return Vec::new();
    };
    let (readable, unreadable) = other_user_homes(&current);
    for home in &unreadable {
        eprintln!("warning: skipped {}: not readable", home.display());
    }
    readable
        .iter()
        .flat_map(|home| resolve_roots(home, Some(&config_dir_in(home)), &|_| None))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn resolves_only_existing_home_relative_dirs() {
        let home = TempDir::new().unwrap();
        fs::create_dir(home.path().join(".codex")).unwrap();
        fs::create_dir_all(home.path().join(".config/opencode")).unwrap();

        let roots = resolve_roots(home.path(), None, &no_env);
        assert!(roots.contains(&home.path().join(".codex")));
        assert!(roots.contains(&home.path().join(".config/opencode")));
        assert!(!roots.contains(&home.path().join(".hermes")));
    }

    #[test]
    fn covers_harnesses_the_old_fixed_list_missed() {
        // Regression guard for #278: these harnesses hold user-scoped skills
        // but were absent from the hardcoded root list.
        let names: Vec<_> = HARNESSES.iter().map(|h| h.name).collect();
        for expected in [
            "codex",
            "hermes",
            "opencode",
            "openhands",
            "openclaw",
            "agents",
        ] {
            assert!(names.contains(&expected), "{expected} missing");
        }
    }

    #[test]
    fn env_override_relocates_a_harness_and_keeps_the_default() {
        let home = TempDir::new().unwrap();
        let moved = TempDir::new().unwrap();
        fs::create_dir(home.path().join(".claude")).unwrap();
        let moved_path = moved.path().to_path_buf();
        let env = move |key: &str| (key == "CLAUDE_CONFIG_DIR").then(|| moved_path.clone().into());

        let roots = resolve_roots(home.path(), None, &env);
        assert!(roots.contains(&moved.path().to_path_buf()));
        assert!(roots.contains(&home.path().join(".claude")));
    }

    #[test]
    fn missing_or_empty_env_override_is_ignored() {
        let home = TempDir::new().unwrap();
        let env = |key: &str| (key == "CODEX_HOME").then(OsString::new);
        assert!(resolve_roots(home.path(), None, &env).is_empty());
    }

    #[test]
    fn config_dir_entries_resolve_against_the_platform_config_dir() {
        let home = TempDir::new().unwrap();
        let config = TempDir::new().unwrap();
        fs::create_dir_all(config.path().join("Code/User")).unwrap();

        let roots = resolve_roots(home.path(), Some(config.path()), &no_env);
        assert_eq!(roots, vec![config.path().join("Code/User")]);
    }

    #[test]
    fn a_path_reachable_two_ways_is_reported_once() {
        let home = TempDir::new().unwrap();
        fs::create_dir(home.path().join(".codex")).unwrap();
        let codex = home.path().join(".codex");
        let env = move |key: &str| (key == "CODEX_HOME").then(|| codex.clone().into());

        let roots = resolve_roots(home.path(), None, &env);
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn other_user_homes_lists_siblings_and_skips_current_and_system_dirs() {
        let users = TempDir::new().unwrap();
        for name in ["me", "agent", "Shared", ".hidden"] {
            fs::create_dir(users.path().join(name)).unwrap();
        }
        fs::write(users.path().join("notes.txt"), "x").unwrap();

        let (readable, unreadable) = other_user_homes(&users.path().join("me"));
        let names: Vec<_> = readable
            .iter()
            .filter(|p| p.starts_with(users.path()))
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(names, vec!["agent"]);
        assert!(unreadable.iter().all(|p| !p.starts_with(users.path())));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_homes_are_reported_not_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let users = TempDir::new().unwrap();
        fs::create_dir(users.path().join("me")).unwrap();
        let locked = users.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let (_, unreadable) = other_user_homes(&users.path().join("me"));
        let still_locked = fs::read_dir(&locked).is_err();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // Running as root can read anything; only assert when the lock holds.
        if still_locked {
            assert!(unreadable.contains(&locked));
        }
    }
}
