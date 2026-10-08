//! Content identity for scanned assets (#274, #130).
//!
//! Provides a canonical content digest for a skill directory, provenance for
//! each location a copy was found in, and lineage metadata (git remote and
//! HEAD commit) read directly from `.git` files — no subprocesses, no network.
//!
//! The digest replicates the hub's `computeCanonicalSkillHash()`
//! (AgenticHighway/vettd `packages/api/src/skills/skill-hash.ts`): SHA-256 over
//! the JSON serialization of `[path, content]` pairs sorted bytewise by path.
//! Keeping both sides on one formula gives the CLI and the dashboard a single
//! canonical asset identity. Escaping is identical for ASCII content; non-ASCII
//! text may serialize differently (JS escapes U+2028/U+2029, serde does not).

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Parity with the server-side GitHub fetcher limits (vettd-cli#204): 250 text
/// files and 10 MB of total text content.
pub(crate) const MAX_SKILL_FILES: usize = 250;
pub(crate) const MAX_SKILL_TOTAL_BYTES: u64 = 10 * 1024 * 1024;
/// Hard stop for the directory walk so a pathological tree cannot hang a scan.
const WALK_SAFETY_LIMIT: usize = 10_000;

/// Directories excluded from identity: version-control metadata.
const VCS_DIRS: [&str; 3] = [".git", ".hg", ".svn"];
/// Files excluded from identity: operating-system junk.
const OS_JUNK: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];
/// Path markers that make a home-directory location a user "installed" asset.
const INSTALLED_MARKERS: [&str; 6] = [
    ".claude/", ".codex/", ".cursor/", ".vscode/", ".config/", ".agents/",
];

/// Result of loading a skill directory for scanning and identity.
pub(crate) struct SkillFileLoad {
    /// Text files included in the scan and the digest, keyed by relative path.
    pub text_files: HashMap<String, String>,
    /// Every included file path (relative), including non-text files.
    pub all_paths: Vec<String>,
    /// Text-extension files that could not be decoded — reported, never silent.
    pub skipped: Vec<String>,
    /// Paths dropped from identity (VCS dirs, OS junk, own .gitignore matches).
    pub excluded: Vec<String>,
    /// True when a size/count cap cut the scan short.
    pub partial: bool,
}

/// Walk a skill directory deterministically, apply disclosed exclusions, and
/// load full text content under the server-parity caps.
pub(crate) fn load_skill_files(root: &Path) -> SkillFileLoad {
    let mut files: Vec<String> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    let mut rules: Vec<IgnoreRule> = Vec::new();
    collect_files(root, "", &mut files, &mut excluded, &mut rules);
    files.sort();
    excluded.sort();

    let mut partial = false;
    if files.len() > WALK_SAFETY_LIMIT {
        partial = true;
        files.truncate(WALK_SAFETY_LIMIT);
    }

    let mut text_files: HashMap<String, String> = HashMap::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut total_bytes: u64 = 0;
    for rel in &files {
        if !is_likely_text(Path::new(rel)) {
            continue;
        }
        if text_files.len() >= MAX_SKILL_FILES {
            partial = true;
            continue;
        }
        match fs::read_to_string(root.join(rel)) {
            Ok(content) => {
                if total_bytes + content.len() as u64 > MAX_SKILL_TOTAL_BYTES {
                    partial = true;
                    continue;
                }
                total_bytes += content.len() as u64;
                text_files.insert(rel.clone(), content);
            }
            Err(_) => skipped.push(rel.clone()),
        }
    }

    SkillFileLoad {
        text_files,
        all_paths: files,
        skipped,
        excluded,
        partial,
    }
}

/// Canonical content digest over a text-file map, matching the hub's
/// `computeCanonicalSkillHash`: entries sorted bytewise by path, serialized as
/// a JSON array of `[path, content]` pairs, hashed with SHA-256.
pub(crate) fn canonical_digest(text_files: &HashMap<String, String>) -> String {
    let mut entries: Vec<(&String, &String)> = text_files.iter().collect();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let serialized = serde_json::to_string(&entries).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Classify where a found copy lives. One ordered rules table (#274).
pub(crate) fn provenance_for(path: &str) -> &'static str {
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_lowercase();
    if lower.contains("/.trash/") || lower.ends_with("/.trash") || lower.contains("/tmp/") {
        return "trash";
    }
    if lower.contains("/.cache/") || lower.contains("/cache/") || lower.contains("/caches/") {
        return "cache";
    }
    if lower.contains("/node_modules/")
        || lower.contains("/vendor/")
        || lower.contains("/site-packages/")
        || lower.contains("/pods/")
        || lower.contains("/target/")
    {
        return "vendored";
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty()
            && normalized.starts_with(home.as_str())
            && INSTALLED_MARKERS.iter().any(|m| lower.contains(m))
        {
            return "installed";
        }
    }
    "bundled"
}

/// Lineage metadata for relating copies of the same asset across versions.
#[derive(Debug, Clone, Default)]
pub(crate) struct AssetLineage {
    pub git_remote_url: Option<String>,
    pub git_commit: Option<String>,
}

/// Read lineage from the nearest enclosing `.git` directory (files only).
pub(crate) fn lineage_for(path: &Path) -> AssetLineage {
    let Some(git_dir) = find_git_dir(path) else {
        return AssetLineage::default();
    };
    AssetLineage {
        git_remote_url: read_git_remote(&git_dir),
        git_commit: read_git_head_commit(&git_dir),
    }
}

fn find_git_dir(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    loop {
        let candidate = current.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn read_git_remote(git_dir: &Path) -> Option<String> {
    let config = fs::read_to_string(git_dir.join("config")).ok()?;
    for line in config.lines() {
        if let Some((_, url)) = line.split_once("url = ") {
            let url = url.trim();
            if !url.is_empty() {
                return Some(super::helpers::sanitize_git_remote_url(url));
            }
        }
    }
    None
}

fn read_git_head_commit(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(rest) = head.strip_prefix("ref: ") {
        let refname = rest.trim();
        if let Ok(content) = fs::read_to_string(git_dir.join(refname)) {
            let sha = content.trim().to_string();
            if is_sha(&sha) {
                return Some(sha);
            }
        }
        return read_packed_ref(git_dir, refname);
    }
    if is_sha(head) {
        return Some(head.to_string());
    }
    None
}

fn read_packed_ref(git_dir: &Path, refname: &str) -> Option<String> {
    let content = fs::read_to_string(git_dir.join("packed-refs")).ok()?;
    for line in content.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((sha, r#ref)) = line.split_once(' ') {
            if r#ref.trim() == refname && is_sha(sha) {
                return Some(sha.to_string());
            }
        }
    }
    None
}

fn is_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Extension heuristic for "is this a text file we should scan and hash".
pub(crate) fn is_likely_text(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
        // Extension-less files (Dockerfile, Makefile, LICENSE) are text.
        return true;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "md" | "txt"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "sh"
            | "bash"
            | "zsh"
            | "py"
            | "js"
            | "ts"
            | "mjs"
            | "cjs"
            | "rs"
            | "go"
            | "rb"
            | "php"
            | "java"
            | "kt"
            | "swift"
            | "c"
            | "cpp"
            | "h"
            | "cs"
            | "html"
            | "xml"
            | "css"
            | "sql"
            | "env"
            | "ini"
            | "cfg"
            | "conf"
            | "lock"
    )
}

// --- own .gitignore matching (minimal subset) ---------------------------------

struct IgnoreRule {
    base: String,
    pattern: String,
    anchored: bool,
}

fn parse_gitignore(content: &str, base: &str) -> Vec<IgnoreRule> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                return None;
            }
            let anchored = line.starts_with('/');
            let pattern = line.trim_matches('/').trim().to_string();
            if pattern.is_empty() {
                return None;
            }
            Some(IgnoreRule {
                base: base.to_string(),
                pattern,
                anchored,
            })
        })
        .collect()
}

fn matches_ignore(rel: &str, rules: &[IgnoreRule]) -> bool {
    rules.iter().any(|rule| matches_rule(rule, rel))
}

fn matches_rule(rule: &IgnoreRule, rel: &str) -> bool {
    if !rel.starts_with(&rule.base) {
        return false;
    }
    let rest = &rel[rule.base.len()..];
    let pattern_parts: Vec<&str> = rule.pattern.split('/').filter(|p| !p.is_empty()).collect();
    let rel_parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    if pattern_parts.is_empty() || rel_parts.is_empty() {
        return false;
    }
    let starts: Vec<usize> = if rule.anchored {
        vec![0]
    } else {
        (0..rel_parts.len()).collect()
    };
    starts.into_iter().any(|start| {
        rel_parts.len() - start == pattern_parts.len()
            && pattern_parts
                .iter()
                .zip(rel_parts[start..].iter())
                .all(|(p, r)| glob_match(p, r))
    })
}

fn glob_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let Some((first, middle_and_last)) = parts.split_first() else {
        return true;
    };
    let Some((last, middle)) = middle_and_last.split_last() else {
        return true;
    };
    if !text.starts_with(first) || !text.ends_with(last) {
        return false;
    }
    let mut remaining = &text[first.len()..text.len() - last.len()];
    middle.iter().all(|segment| match remaining.find(segment) {
        Some(index) => {
            remaining = &remaining[index + segment.len()..];
            true
        }
        None => false,
    })
}

// --- walk --------------------------------------------------------------------

fn collect_files(
    dir: &Path,
    rel_prefix: &str,
    files: &mut Vec<String>,
    excluded: &mut Vec<String>,
    rules: &mut Vec<IgnoreRule>,
) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<fs::DirEntry> = read_dir.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_string());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if VCS_DIRS.contains(&name.as_str()) || matches_ignore(&rel, rules) {
                excluded.push(format!("{rel}/"));
                continue;
            }
            collect_files(&entry.path(), &rel, files, excluded, rules);
        } else if file_type.is_file() {
            if OS_JUNK.iter().any(|junk| junk.eq_ignore_ascii_case(&name)) {
                excluded.push(rel);
                continue;
            }
            if matches_ignore(&rel, rules) {
                excluded.push(rel);
                continue;
            }
            if name == ".gitignore" {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    rules.extend(parse_gitignore(&content, rel_prefix));
                }
            }
            files.push(rel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vettd-identity-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn canonical_digest_matches_hub_compute_canonical_skill_hash() {
        // Fixture cross-checked against the hub formula:
        // sha256(JSON.stringify([["SKILL.md",...],["references/guide.md",...]]))
        // computed independently with python3 json.dumps(separators=(',',':')).
        let mut files = HashMap::new();
        files.insert(
            "SKILL.md".to_string(),
            "---\nname: demo\ndescription: A demo skill\n---\n\n# Demo\n\nRun `ls`.\n".to_string(),
        );
        files.insert(
            "references/guide.md".to_string(),
            "# Guide\n\nStep one.\n".to_string(),
        );
        assert_eq!(
            canonical_digest(&files),
            "215ae3b9961f76cf7ef0e536fe4c9a4e2200f9420491651cfe6b7c5f0d423f5a"
        );
    }

    #[test]
    fn digest_changes_when_bytes_past_8kb_differ() {
        // Why this matters: the old loader truncated every file at 8192 chars,
        // so a payload hidden past 8KB was invisible to scanning AND to any
        // content hash. A byte change past the old cap must move the digest.
        let base = "a".repeat(9_000);
        let mut changed = base.clone();
        changed.replace_range(8_500..8_501, "b");
        let mut files_a = HashMap::new();
        files_a.insert("SKILL.md".to_string(), base);
        let mut files_b = HashMap::new();
        files_b.insert("SKILL.md".to_string(), changed);
        assert_ne!(canonical_digest(&files_a), canonical_digest(&files_b));
    }

    #[test]
    fn load_excludes_vcs_dirs_and_os_junk_and_lists_them() {
        let root = fixture_root("exclusions");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "[remote]\n").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/.DS_Store"), "junk").unwrap();
        fs::write(root.join("sub/keep.md"), "keep").unwrap();

        let load = load_skill_files(&root);
        assert!(load.excluded.contains(&".git/".to_string()));
        assert!(load.excluded.contains(&"sub/.DS_Store".to_string()));
        assert!(load.all_paths.contains(&"sub/keep.md".to_string()));
        assert!(!load.all_paths.contains(&"sub/.DS_Store".to_string()));
        assert!(!load.text_files.contains_key(".git/config"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn load_honours_the_assets_own_gitignore() {
        let root = fixture_root("gitignore");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::write(root.join(".gitignore"), "generated/\n*.bak\n").unwrap();
        fs::create_dir_all(root.join("generated")).unwrap();
        fs::write(root.join("generated/blob.md"), "ignored").unwrap();
        fs::write(root.join("notes.bak"), "ignored").unwrap();
        fs::write(root.join("notes.md"), "kept").unwrap();

        let load = load_skill_files(&root);
        assert!(load.excluded.contains(&"generated/".to_string()));
        assert!(load.excluded.contains(&"notes.bak".to_string()));
        assert!(load.text_files.contains_key("notes.md"));
        assert!(!load.text_files.contains_key("generated/blob.md"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn load_caps_at_250_text_files_deterministically_and_flags_partial() {
        let root = fixture_root("cap");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        for i in 0..300 {
            fs::write(root.join(format!("f{i:03}.md")), format!("file {i}")).unwrap();
        }

        let load = load_skill_files(&root);
        assert!(load.partial);
        // Deterministic: the 250 lowest sorted paths survive, never a random
        // read_dir-order subset.
        assert_eq!(load.text_files.len(), MAX_SKILL_FILES);
        assert!(load.text_files.contains_key("f000.md"));
        assert!(load.text_files.contains_key("f248.md"));
        assert!(!load.text_files.contains_key("f249.md"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn load_reports_non_utf8_files_instead_of_skipping_silently() {
        let root = fixture_root("nonutf8");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::write(root.join("payload.sh"), [0xff_u8, 0xfe, 0x00, 0x80]).unwrap();

        let load = load_skill_files(&root);
        assert_eq!(load.skipped, vec!["payload.sh".to_string()]);
        assert!(!load.text_files.contains_key("payload.sh"));
        assert!(load.all_paths.contains(&"payload.sh".to_string()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn provenance_rules_table() {
        assert_eq!(
            provenance_for("/home/u/.Trash/skills/demo/SKILL.md"),
            "trash"
        );
        assert_eq!(provenance_for("/tmp/x/skills/demo/SKILL.md"), "trash");
        assert_eq!(
            provenance_for("/home/u/.cache/vettd/skills/demo/SKILL.md"),
            "cache"
        );
        assert_eq!(
            provenance_for("/repo/node_modules/pkg/skills/demo/SKILL.md"),
            "vendored"
        );
        assert_eq!(
            provenance_for("/repo/vendor/skills/demo/SKILL.md"),
            "vendored"
        );
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            provenance_for(&format!("{home}/.claude/skills/demo/SKILL.md")),
            "installed"
        );
        assert_eq!(
            provenance_for("/repo/.claude/skills/demo/SKILL.md"),
            "bundled"
        );
    }

    #[test]
    fn lineage_reads_remote_and_commit_from_git_files() {
        let root = fixture_root("lineage");
        fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        fs::write(
            root.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/acme/skills.git\n",
        )
        .unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            root.join(".git/refs/heads/main"),
            "0123456789012345678901234567890123456789\n",
        )
        .unwrap();
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();

        let lineage = lineage_for(&root.join("SKILL.md"));
        assert_eq!(
            lineage.git_remote_url.as_deref(),
            Some("https://github.com/acme/skills.git")
        );
        assert_eq!(
            lineage.git_commit.as_deref(),
            Some("0123456789012345678901234567890123456789")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lineage_falls_back_to_packed_refs() {
        let root = fixture_root("packed");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            root.join(".git/packed-refs"),
            "# pack-refs with: peeled fully-peeled sorted \n\
             0123456789abcdef0123456789abcdef01234567 refs/heads/main\n",
        )
        .unwrap();
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();

        let lineage = lineage_for(&root.join("SKILL.md"));
        assert_eq!(
            lineage.git_commit.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(lineage.git_remote_url, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn digest_is_stable_regardless_of_hashmap_iteration_order() {
        // HashMap iteration order is randomized per process; the digest must
        // not depend on it, or identity would break across runs.
        let mut a = HashMap::new();
        a.insert("b.md".to_string(), "2".to_string());
        a.insert("a.md".to_string(), "1".to_string());
        let mut b = HashMap::new();
        b.insert("a.md".to_string(), "1".to_string());
        b.insert("b.md".to_string(), "2".to_string());
        assert_eq!(canonical_digest(&a), canonical_digest(&b));
    }
}
