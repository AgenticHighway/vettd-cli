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
}
