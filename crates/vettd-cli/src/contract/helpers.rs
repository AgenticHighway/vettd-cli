//! Shared helpers used across contract submodules.

use sha2::{Digest, Sha256};

use super::types::DetectedSkillSource;
use crate::models::ArtifactReport;
pub fn first_path(a: &ArtifactReport) -> &str {
    a.metadata
        .get("paths")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
}

/// Build a project-qualified display name from an absolute path.
///
/// E.g. `/Users/example/project/foo/agents.md` → `foo/agents`
///      `/Users/example/bar/.cursorrules`      → `bar/.cursorrules`
pub fn qualified_name(path: &str) -> String {
    let p = std::path::Path::new(path);
    let file_name = p.file_name().and_then(|s| s.to_str()).unwrap_or("unknown");
    let parent_name = p
        .parent()
        .and_then(|pp| pp.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);

    // For dotfiles, keep the full filename
    if file_name.starts_with('.') {
        format!("{parent_name}/{file_name}")
    } else {
        format!("{parent_name}/{stem}")
    }
}

/// Build a deterministic ID from source path + content hash.
pub fn make_id(source_path: &str, artifact_hash: &str) -> String {
    if !artifact_hash.is_empty() {
        format!(
            "{}:{}",
            source_path,
            &artifact_hash[..12.min(artifact_hash.len())]
        )
    } else {
        format!("{}:{}", source_path, short_hash(source_path))
    }
}

pub fn short_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = format!("{:x}", hasher.finalize());
    result[..12].to_string()
}

pub fn compute_file_hash(path: &str) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            format!("{:x}", hasher.finalize())
        }
        Err(_) => String::new(),
    }
}

/// Try to find the git remote origin URL for a file path.
///
/// The raw URL is sanitized before being returned — see [`sanitize_git_remote_url`].
pub fn detect_source_repo(file_path: &str) -> String {
    let mut dir = std::path::Path::new(file_path).parent();
    while let Some(d) = dir {
        let git_config = d.join(".git").join("config");
        if git_config.exists() {
            if let Some(raw) = read_remote_url(&git_config) {
                return sanitize_git_remote_url(&raw);
            }
        }
        dir = d.parent();
    }
    "unknown".to_string()
}

/// Detect a candidate source for a skill by walking up from its path to find
/// an enclosing git working tree, then extracting the remote URL, current
/// branch, and the skill's path relative to the repo root.
///
/// Local-only, informational detection — see [`DetectedSkillSource`]'s doc
/// comment for why this must never be treated as verified. Returns `None`
/// when no usable git remote is found, so `Skill.detected_source` is omitted
/// entirely rather than set to a placeholder.
pub fn detect_skill_source(skill_path: &str) -> Option<DetectedSkillSource> {
    let raw_path = std::path::Path::new(skill_path);
    // Skills are often reported by their SKILL.md file path; detection
    // should walk from the skill's directory, not treat the file as a root.
    let start = if raw_path.is_file() {
        raw_path.parent()?
    } else {
        raw_path
    };

    let mut dir = Some(start);
    while let Some(d) = dir {
        let git_dir = d.join(".git");
        if git_dir.is_dir() {
            let raw_remote = read_remote_url(&git_dir.join("config"))?;
            let sanitized = sanitize_git_remote_url(&raw_remote);
            if sanitized == "unknown" {
                return None;
            }

            let relative_path = start
                .strip_prefix(d)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();

            return Some(if is_github_remote(&sanitized) {
                DetectedSkillSource::GitHub {
                    repo_url: sanitized,
                    branch: read_current_branch(&git_dir).unwrap_or_else(|| "main".to_string()),
                    path: relative_path,
                }
            } else {
                DetectedSkillSource::UnsupportedRemote {
                    remote_url: sanitized,
                }
            });
        }
        dir = d.parent();
    }
    None
}

/// Extract the raw (unsanitized) `url = ` line from a `.git/config` file.
fn read_remote_url(git_config: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(git_config).ok()?;
    for line in content.lines() {
        if let Some(raw) = line.trim().strip_prefix("url = ") {
            return Some(raw.to_string());
        }
    }
    None
}

fn is_github_remote(sanitized_url: &str) -> bool {
    sanitized_url.contains("github.com")
}

/// Read the current branch name from `.git/HEAD`. Returns `None` for a
/// detached HEAD (bare commit SHA) rather than fabricating a branch name;
/// callers fall back to a sensible default.
fn read_current_branch(git_dir: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    content
        .trim()
        .strip_prefix("ref: refs/heads/")
        .map(|s| s.to_string())
}

/// Strip credentials and normalize a raw Git remote URL so it is safe to include
/// in emitted/submitted payloads.
///
/// Rules applied:
/// - **HTTPS/HTTP**: remove any `user:password@` or `token@` userinfo component.
/// - **SSH SCP syntax** (`git@host:org/repo.git`): convert to `ssh://host/org/repo.git`
///   (no userinfo leaked).
/// - **`ssh://` URL**: strip any userinfo.
/// - **`git://` URL**: pass through unchanged (no credentials possible in this scheme).
/// - **Local paths or unrecognised formats**: return `"unknown"` to avoid leaking
///   internal filesystem layout.
pub fn sanitize_git_remote_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return "unknown".to_string();
    }

    // SSH SCP-like syntax has no "://" — e.g. git@github.com:org/repo.git
    if !url.contains("://") {
        if let Some(rest) = url.strip_prefix("git@") {
            if let Some(colon_pos) = rest.find(':') {
                let host = &rest[..colon_pos];
                let path = rest[colon_pos + 1..].trim_start_matches('/');
                return format!("ssh://{host}/{path}");
            }
        }
        // Local path or unknown format — do not emit.
        return "unknown".to_string();
    }

    // Standard scheme://[userinfo@]host/path form
    let scheme_end = url.find("://").unwrap();
    let scheme = &url[..scheme_end];
    let after_scheme = &url[scheme_end + 3..];

    // Strip userinfo (everything up to and including the first '@' that precedes the
    // first path separator, if any).
    let authority_and_path = {
        let slash_pos = after_scheme.find('/').unwrap_or(after_scheme.len());
        let at_pos = after_scheme[..slash_pos].find('@');
        match at_pos {
            Some(pos) => &after_scheme[pos + 1..],
            None => after_scheme,
        }
    };

    match scheme {
        "https" | "http" | "ssh" | "git" => format!("{scheme}://{authority_and_path}"),
        // Reject any other scheme (e.g. file://, custom internal schemes).
        _ => "unknown".to_string(),
    }
}

/// Check if two directories share the same tool scope.
pub fn is_same_tool_scope(dir_a: &str, dir_b: &str) -> bool {
    let scope_markers = [
        ".vscode",
        ".vscode-insiders",
        ".cursor",
        ".claude",
        "Code/User",
        "Cursor/User",
    ];
    for marker in &scope_markers {
        if dir_a.contains(marker) && dir_b.contains(marker) {
            return true;
        }
    }
    false
}

pub fn declared_tools(a: &ArtifactReport) -> Vec<String> {
    a.metadata
        .get("declared_tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub fn capability_level(cap: &str) -> &'static str {
    match cap {
        "shell_execution" | "code_execution" | "container_runtime" => "danger",
        "network_access" | "external_api_calls" | "browser_access" | "secret_references" => "warn",
        _ => "info",
    }
}

pub fn humanize_capability(cap: &str) -> String {
    match cap {
        "shell_execution" => "Shell execution".to_string(),
        "browser_access" => "Browser access".to_string(),
        "external_api_calls" => "External API calls".to_string(),
        "filesystem_access" => "Filesystem read/write".to_string(),
        "network_access" => "Network access".to_string(),
        "code_execution" => "Code execution".to_string(),
        "container_runtime" => "Container runtime".to_string(),
        "system_prompt" => "System prompt control".to_string(),
        "permission_scope" => "Permission scope declarations".to_string(),
        "dependency_execution" => "Dependency execution".to_string(),
        "tool_declarations" => "Tool declarations".to_string(),
        "secret_references" => "Secret references".to_string(),
        other => other.replace('_', " "),
    }
}

const MAX_READ_BYTES: usize = 8192;

pub fn read_artifact_head(a: &ArtifactReport) -> Option<String> {
    let path_str = first_path(a);
    if path_str == "unknown" {
        return None;
    }
    let path = std::path::Path::new(path_str);
    if !crate::models::is_content_read_allowed(path) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let len = bytes.len().min(MAX_READ_BYTES);
    String::from_utf8(bytes[..len].to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_artifact_with_path(path: &str) -> ArtifactReport {
        let mut a = ArtifactReport::new("test", 0.8);
        a.metadata.insert("paths".to_string(), json!([path]));
        a
    }

    #[test]
    fn first_path_returns_first_element() {
        let a = make_artifact_with_path("/tmp/foo.md");
        assert_eq!(first_path(&a), "/tmp/foo.md");
    }

    #[test]
    fn first_path_returns_unknown_when_missing() {
        let a = ArtifactReport::new("test", 0.8);
        assert_eq!(first_path(&a), "unknown");
    }

    #[test]
    fn qualified_name_regular_file() {
        assert_eq!(
            qualified_name("/Users/example/project/agents.md"),
            "project/agents"
        );
    }

    #[test]
    fn qualified_name_dotfile() {
        assert_eq!(
            qualified_name("/Users/example/bar/.cursorrules"),
            "bar/.cursorrules"
        );
    }

    #[test]
    fn make_id_with_hash() {
        let id = make_id("/some/path.md", "abcdef123456789");
        assert_eq!(id, "/some/path.md:abcdef123456");
    }

    #[test]
    fn make_id_without_hash_falls_back_to_short_hash() {
        let id = make_id("/some/path.md", "");
        assert!(id.starts_with("/some/path.md:"));
        assert_eq!(id.len(), "/some/path.md:".len() + 12);
    }

    #[test]
    fn short_hash_is_deterministic() {
        let h1 = short_hash("hello");
        let h2 = short_hash("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
    }

    #[test]
    fn short_hash_differs_for_different_inputs() {
        assert_ne!(short_hash("hello"), short_hash("world"));
    }

    #[test]
    fn is_same_tool_scope_vscode() {
        assert!(is_same_tool_scope(
            "/home/user/.vscode/extensions",
            "/home/user/.vscode/settings"
        ));
    }

    #[test]
    fn is_same_tool_scope_different_scopes() {
        assert!(!is_same_tool_scope(
            "/home/user/.vscode/extensions",
            "/home/user/project/src"
        ));
    }

    #[test]
    fn is_same_tool_scope_cursor() {
        assert!(is_same_tool_scope(
            "/home/user/.cursor/rules",
            "/home/user/.cursor/config"
        ));
    }

    #[test]
    fn declared_tools_extracts_from_metadata() {
        let mut a = ArtifactReport::new("test", 0.8);
        a.metadata.insert(
            "declared_tools".to_string(),
            json!(["shell", "browser", "api"]),
        );
        assert_eq!(declared_tools(&a), vec!["shell", "browser", "api"]);
    }

    #[test]
    fn declared_tools_empty_when_missing() {
        let a = ArtifactReport::new("test", 0.8);
        assert!(declared_tools(&a).is_empty());
    }

    #[test]
    fn capability_level_danger() {
        assert_eq!(capability_level("shell_execution"), "danger");
        assert_eq!(capability_level("code_execution"), "danger");
        assert_eq!(capability_level("container_runtime"), "danger");
    }

    #[test]
    fn capability_level_warn() {
        assert_eq!(capability_level("network_access"), "warn");
        assert_eq!(capability_level("external_api_calls"), "warn");
        assert_eq!(capability_level("secret_references"), "warn");
    }

    #[test]
    fn capability_level_info_default() {
        assert_eq!(capability_level("filesystem_access"), "info");
        assert_eq!(capability_level("unknown_thing"), "info");
    }

    #[test]
    fn humanize_capability_known() {
        assert_eq!(humanize_capability("shell_execution"), "Shell execution");
        assert_eq!(humanize_capability("browser_access"), "Browser access");
    }

    #[test]
    fn humanize_capability_unknown_replaces_underscores() {
        assert_eq!(humanize_capability("my_custom_thing"), "my custom thing");
    }

    // ── sanitize_git_remote_url ──────────────────────────────────────────────

    #[test]
    fn sanitize_https_strips_user_and_password() {
        assert_eq!(
            sanitize_git_remote_url("https://user:s3cr3t@github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
    }

    #[test]
    fn sanitize_https_strips_token_credential() {
        assert_eq!(
            sanitize_git_remote_url("https://ghp_token123@github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
    }

    #[test]
    fn sanitize_https_no_credentials_unchanged() {
        assert_eq!(
            sanitize_git_remote_url("https://github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
    }

    #[test]
    fn sanitize_http_strips_credentials() {
        assert_eq!(
            sanitize_git_remote_url("http://admin:pass@internal.corp/org/repo.git"),
            "http://internal.corp/org/repo.git"
        );
    }

    #[test]
    fn sanitize_ssh_scp_git_at_syntax() {
        assert_eq!(
            sanitize_git_remote_url("git@github.com:org/repo.git"),
            "ssh://github.com/org/repo.git"
        );
    }

    #[test]
    fn sanitize_ssh_scp_internal_host() {
        assert_eq!(
            sanitize_git_remote_url("git@gitlab.internal.corp:team/project.git"),
            "ssh://gitlab.internal.corp/team/project.git"
        );
    }

    #[test]
    fn sanitize_ssh_url_strips_userinfo() {
        assert_eq!(
            sanitize_git_remote_url("ssh://git@github.com/org/repo.git"),
            "ssh://github.com/org/repo.git"
        );
    }

    #[test]
    fn sanitize_ssh_url_no_userinfo_unchanged() {
        assert_eq!(
            sanitize_git_remote_url("ssh://github.com/org/repo.git"),
            "ssh://github.com/org/repo.git"
        );
    }

    #[test]
    fn sanitize_git_protocol_passthrough() {
        assert_eq!(
            sanitize_git_remote_url("git://github.com/org/repo.git"),
            "git://github.com/org/repo.git"
        );
    }

    #[test]
    fn sanitize_local_path_returns_unknown() {
        assert_eq!(sanitize_git_remote_url("/home/user/repo.git"), "unknown");
    }

    #[test]
    fn sanitize_relative_path_returns_unknown() {
        assert_eq!(sanitize_git_remote_url("../sibling/repo.git"), "unknown");
    }

    #[test]
    fn sanitize_file_scheme_returns_unknown() {
        assert_eq!(
            sanitize_git_remote_url("file:///home/user/repo.git"),
            "unknown"
        );
    }

    #[test]
    fn sanitize_empty_string_returns_unknown() {
        assert_eq!(sanitize_git_remote_url(""), "unknown");
    }

    #[test]
    fn sanitize_whitespace_only_returns_unknown() {
        assert_eq!(sanitize_git_remote_url("   "), "unknown");
    }

    #[test]
    fn sanitize_https_internal_host_strips_credentials() {
        assert_eq!(
            sanitize_git_remote_url("https://ci-bot:token@git.internal.corp/team/repo.git"),
            "https://git.internal.corp/team/repo.git"
        );
    }

    #[test]
    fn sanitize_at_sign_in_path_not_treated_as_userinfo() {
        // '@' appearing only after the first '/' is path data, not userinfo.
        assert_eq!(
            sanitize_git_remote_url("https://github.com/org/repo@v2.git"),
            "https://github.com/org/repo@v2.git"
        );
    }

    fn write_git_remote(repo_dir: &std::path::Path, url: &str) {
        let git_dir = repo_dir.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("config"),
            format!("[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"),
        )
        .unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    #[test]
    fn detect_skill_source_github_remote() {
        let dir = tempfile::tempdir().unwrap();
        write_git_remote(dir.path(), "https://github.com/acme/skills-repo.git");
        let skill_dir = dir.path().join("skills").join("foo");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let result = detect_skill_source(skill_dir.to_str().unwrap());
        match result {
            Some(DetectedSkillSource::GitHub {
                repo_url,
                branch,
                path,
            }) => {
                assert_eq!(repo_url, "https://github.com/acme/skills-repo.git");
                assert_eq!(branch, "main");
                assert_eq!(path, "skills/foo");
            }
            other => panic!("expected GitHub variant, got {other:?}"),
        }
    }

    #[test]
    fn detect_skill_source_from_skill_md_file_path() {
        // Detection should walk from the skill's directory even when given
        // the SKILL.md file path directly, not the directory.
        let dir = tempfile::tempdir().unwrap();
        write_git_remote(dir.path(), "https://github.com/acme/skills-repo.git");
        let skill_dir = dir.path().join("skills").join("foo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_md = skill_dir.join("SKILL.md");
        std::fs::write(&skill_md, "# foo").unwrap();

        let result = detect_skill_source(skill_md.to_str().unwrap());
        match result {
            Some(DetectedSkillSource::GitHub { path, .. }) => {
                assert_eq!(path, "skills/foo");
            }
            other => panic!("expected GitHub variant, got {other:?}"),
        }
    }

    #[test]
    fn detect_skill_source_non_github_remote_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        write_git_remote(dir.path(), "https://gitlab.com/acme/skills-repo.git");
        let skill_dir = dir.path().join("skills").join("foo");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let result = detect_skill_source(skill_dir.to_str().unwrap());
        match result {
            Some(DetectedSkillSource::UnsupportedRemote { remote_url }) => {
                assert_eq!(remote_url, "https://gitlab.com/acme/skills-repo.git");
            }
            other => panic!("expected UnsupportedRemote variant, got {other:?}"),
        }
    }

    #[test]
    fn detect_skill_source_no_git_repo_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("foo");
        std::fs::create_dir_all(&skill_dir).unwrap();

        assert!(detect_skill_source(skill_dir.to_str().unwrap()).is_none());
    }

    #[test]
    fn detect_skill_source_detached_head_falls_back_to_main() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("config"),
            "[remote \"origin\"]\n\turl = https://github.com/acme/repo.git\n",
        )
        .unwrap();
        // Detached HEAD: a bare SHA, no "ref:" prefix.
        std::fs::write(git_dir.join("HEAD"), "a1b2c3d4e5f6\n").unwrap();

        let result = detect_skill_source(dir.path().to_str().unwrap());
        match result {
            Some(DetectedSkillSource::GitHub { branch, .. }) => {
                assert_eq!(branch, "main");
            }
            other => panic!("expected GitHub variant, got {other:?}"),
        }
    }

    #[test]
    fn detect_skill_source_credentials_are_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        write_git_remote(
            dir.path(),
            "https://user:s3cr3t@github.com/acme/skills-repo.git",
        );

        let result = detect_skill_source(dir.path().to_str().unwrap());
        match result {
            Some(DetectedSkillSource::GitHub { repo_url, .. }) => {
                assert!(!repo_url.contains("s3cr3t"));
                assert_eq!(repo_url, "https://github.com/acme/skills-repo.git");
            }
            other => panic!("expected GitHub variant, got {other:?}"),
        }
    }
}
