//! `vettd directory download <slug>` — resolve a public directory skill, then
//! fetch its exact scanned source from GitHub codeload and extract the scanned
//! subtree locally.
//!
//! Pipeline (server-first, see `docs` / issue #169):
//!   1. Resolve — `POST {directory}/<slug>/download` (unauthenticated) returns
//!      `{slug, name, sourceType, sourceUrl, sourceHash, commitSha}`.
//!   2. Fetch — parse the canonical GitHub tree URL, download
//!      `codeload.github.com/<owner>/<repo>/tar.gz/<commitSha>`, and extract
//!      only the scanned `{path}` subtree.
//!   3. Write — `--out` default `./<slug>`; refuse an existing non-empty
//!      destination before any network call.

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::directory::{self, percent_encode};
use crate::read_client::{self, ReadError};

/// HTTP timeout for the codeload archive download.
const DOWNLOAD_TIMEOUT_SECS: u64 = 300;

/// Canonical GitHub tree URL prefix.
const GITHUB_TREE_PREFIX: &str = "https://github.com/";

/// Prefix of the GitHub codeload tarball host.
const CODELOAD_HOST: &str = "https://codeload.github.com";

// ---------------------------------------------------------------------------
// Allow-list deserialization structs
// ---------------------------------------------------------------------------

/// The six-field download-resolve response from `POST /api/directory/<slug>/download`.
///
/// All six fields are required by the contract; a missing field is treated as a
/// malformed response (a decode error) rather than silently forwarded as `None`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResolveResponse {
    pub slug: String,
    pub name: String,
    pub source_type: String,
    pub source_url: String,
    pub source_hash: String,
    pub commit_sha: String,
}

/// The machine-output shape for `--json`: the resolve metadata plus the written
/// destination path.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadOutput {
    slug: String,
    name: String,
    source_type: String,
    source_url: String,
    source_hash: String,
    commit_sha: String,
    written_path: String,
}

/// The successfully resolved + extracted download.
#[derive(Debug)]
struct DownloadOutcome {
    resolve: DownloadResolveResponse,
    parts: GitHubTreeParts,
    dest: PathBuf,
    /// The commit SHA embedded in the archive's root directory name
    /// (`{repo}-<sha>`), when derivable — used for the provenance drift check.
    archive_sha: Option<String>,
}

/// The parsed components of a canonical GitHub tree URL.
#[derive(Debug, Clone)]
struct GitHubTreeParts {
    owner: String,
    repo: String,
    branch: String,
    /// The scanned subtree path within the repo (everything after
    /// `tree/<branch>/`) — empty for a root-path skill, where the whole repo
    /// IS the skill.
    path: String,
}

// ---------------------------------------------------------------------------
// Destination pre-check
// ---------------------------------------------------------------------------

/// Refuse a destination that already exists and is non-empty.
///
/// Returns `Some(message)` when the destination may not be safely written into
/// (an existing non-empty directory or a pre-existing file), and `None` when it
/// is safe to create it. Called **before** any network request.
fn destination_refusal(dest: &Path) -> Option<String> {
    let dest_disp = dest.display();
    match std::fs::metadata(dest) {
        Ok(meta) if meta.is_dir() => match std::fs::read_dir(dest) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Some(format!(
                        "destination {dest_disp} already exists and is not empty. \
                          Move it aside or pass a different --out."
                    ));
                }
                None
            }
            Err(e) => Some(format!("cannot read destination {dest_disp}: {e}")),
        },
        Ok(_) => Some(format!(
            "destination {dest_disp} already exists and is not a directory."
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(format!("cannot check destination {dest_disp}: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Resolve (step 1)
// ---------------------------------------------------------------------------

/// Resolve a slug via the public download endpoint and decode the response.
///
/// Maps the contract's status codes to clear, user-facing messages:
/// 404 → "skill not found in directory", 422 → "not downloadable", and every
/// other failure to a descriptive message. All map to exit code 1 at the
/// call site.
fn resolve_download(base_url: &str, slug: &str) -> Result<DownloadResolveResponse, String> {
    let url = format!("{base_url}/{}/download", percent_encode(slug));
    // Unauthenticated POST with an empty JSON body and Content-Type:
    // application/json — the server only reads the path segment (the slug).
    match read_client::post_json::<DownloadResolveResponse>(&url, &serde_json::json!({})) {
        Ok(response) => Ok(response),
        Err(ReadError::NotFound) => Err("skill not found in directory".to_string()),
        Err(ReadError::ServerError(422)) => {
            Err("this skill has no downloadable source yet".to_string())
        }
        Err(ReadError::ServerError(code)) => Err(format!("download endpoint returned HTTP {code}")),
        Err(ReadError::Unreachable(msg)) => {
            Err(format!("could not reach the vettd directory: {msg}"))
        }
        Err(ReadError::Decode(msg)) => Err(format!("could not parse the download response: {msg}")),
        Err(ReadError::RateLimited) => {
            Err("the vettd download endpoint is rate limited (HTTP 429)".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Source URL parsing
// ---------------------------------------------------------------------------

/// Parse a canonical GitHub tree URL
/// `https://github.com/{owner}/{repo}/tree/{branch}[/{path}]` into its parts.
///
/// The path segment is optional: a root-path skill is a valid, supported shape
/// (the whole repo IS the skill — `path` comes back empty). Fails loudly on
/// anything that is not this shape — the server only ever emits tree URLs for
/// downloadable skills, so a non-matching URL means the resolved source is not
/// what we expected and must not be silently misparsed.
fn parse_github_tree_url(url: &str) -> Result<GitHubTreeParts, String> {
    let rest = url
        .strip_prefix(GITHUB_TREE_PREFIX)
        .ok_or_else(|| format!("source URL is not a canonical GitHub tree link: {url}"))?;

    let tree_pos = rest
        .find("/tree/")
        .ok_or_else(|| format!("source URL has no /tree/ segment: {url}"))?;

    let repo_path = &rest[..tree_pos]; // {owner}/{repo}
    let branch_path = &rest[tree_pos + "/tree/".len()..]; // {branch}[/{path}]

    let mut repo_parts = repo_path.splitn(2, '/');
    let owner = repo_parts.next().unwrap_or("");
    let repo = repo_parts
        .next()
        .ok_or_else(|| format!("source URL is missing the repo: {url}"))?;
    if owner.is_empty() || repo.is_empty() {
        return Err(format!("source URL has an empty owner or repo: {url}"));
    }

    let mut branch_parts = branch_path.splitn(2, '/');
    let branch = branch_parts.next().unwrap_or("");
    // Empty path is allowed: a root-path skill has nothing after the branch.
    let path = branch_parts.next().unwrap_or("");
    if branch.is_empty() {
        return Err(format!("source URL has an empty git branch: {url}"));
    }

    Ok(GitHubTreeParts {
        owner: owner.to_string(),
        repo: repo.to_string(),
        branch: branch.to_string(),
        path: path.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Subtree filtering + traversal-safe extraction (step 2)
// ---------------------------------------------------------------------------

/// Whether a tar entry (relative to the archive root) lives under `skill_path`.
///
/// An empty `skill_path` is a root-path skill: every entry in the archive
/// belongs to the skill, so everything matches.
fn is_subtree_entry(rel: &str, skill_path: &str) -> bool {
    if skill_path.is_empty() {
        return true;
    }
    rel == skill_path || rel.starts_with(&format!("{skill_path}/"))
}

/// Lexically resolve `dest` joined with the relative `rel`, rejecting any path
/// that would escape `dest` via `..`.
///
/// Works purely on path components (no filesystem access) so it can guard
/// entries before any file is created. The lexical pass neutralizes `..`; a
/// `starts_with` backstop catches anything the lexical pass cannot see.
fn safe_join(dest: &Path, rel: &Path) -> Result<PathBuf, String> {
    let dest_disp = dest.display();
    let base = if dest.is_absolute() {
        dest.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(dest))
            .map_err(|e| format!("cannot resolve destination {dest_disp}: {e}"))?
    };

    let rel_disp = rel.display();
    let mut out: PathBuf = base.components().collect();
    for component in rel.components() {
        match component {
            Component::Normal(c) => out.push(c),
            Component::ParentDir => {
                if !out.pop() {
                    return Err(format!(
                        "path traversal detected in archive entry: {rel_disp}"
                    ));
                }
            }
            Component::CurDir => {}
            // Absolute or prefixed components can never stay within `dest`.
            _ => {
                return Err(format!(
                    "unsafe path component in archive entry: {rel_disp}"
                ))
            }
        }
    }

    if !out.starts_with(&base) {
        return Err(format!(
            "path traversal detected in archive entry: {rel_disp}"
        ));
    }
    Ok(out)
}

/// Extract every entry under `skill_path` from a (decompressed) tar archive into
/// `dest`, stripping the leading `{repo}-{sha}` root prefix that GitHub adds.
///
/// Returns the archive's root directory name (`{repo}-<sha>`) when the archive
/// is non-empty, so the caller can derive the embedded commit SHA. Directory and
/// regular-file entries are written; symlinks and other special entries are
/// skipped. Traversal-safe: a `..` component anywhere in an entry's full path is
/// unconditionally invalid (GitHub-generated tarballs never contain one) and
/// aborts the extraction rather than being lexically relocated by [`safe_join`].
/// If no entry matches the subtree at all, the extraction fails with a
/// "source unavailable" error — an empty output directory must never be
/// reported as success.
fn extract_subtree<R: Read>(
    reader: R,
    skill_path: &str,
    dest: &Path,
) -> Result<Option<String>, String> {
    use flate2::read::GzDecoder;
    use tar::{Archive, EntryType};

    let decoder = GzDecoder::new(reader);
    let mut archive = Archive::new(decoder);

    let mut root_name: Option<String> = None;
    let mut written = 0usize;

    let entries = archive
        .entries()
        .map_err(|e| format!("failed to open tar archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("failed to read tar entry: {e}"))?;
        let entry_type = entry.header().entry_type();

        // Only real filesystem entries are extractable; skip pax global /
        // extended-header metadata and special file types.
        if !matches!(entry_type, EntryType::Regular | EntryType::Directory) {
            continue;
        }

        let path = entry
            .path()
            .map_err(|e| format!("bad path in tar entry: {e}"))?
            .into_owned();

        // Reject any `..` component in the FULL archive-relative path before any
        // subtree matching: a crafted entry like
        // `skills/azure-prepare/../azure-deploy/SKILL.md` passes the string-prefix
        // check for `skills/azure-prepare` while lexically resolving to a sibling
        // directory outside the intended subtree. GitHub codeload archives never
        // contain `..` paths, so any entry that has one is invalid outright.
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(format!(
                "path traversal detected in archive entry: {}",
                path.display()
            ));
        }

        if root_name.is_none() {
            if let Some(first) = path.components().next() {
                root_name = Some(first.as_os_str().to_string_lossy().into_owned());
            }
        }
        let root = root_name.as_deref().ok_or("archive root name is missing")?;

        let rel = path
            .strip_prefix(root)
            .map_err(|e| format!("bad path in tar entry: {e}"))?;
        // The root directory entry itself strips to an empty relative path.
        if rel.as_os_str().is_empty() {
            continue;
        }

        let rel_str = rel.to_string_lossy();
        if !is_subtree_entry(&rel_str, skill_path) {
            continue;
        }

        let target = safe_join(dest, rel)?;
        let target_disp = target.display();
        match entry_type {
            EntryType::Directory => {
                std::fs::create_dir_all(&target)
                    .map_err(|e| format!("failed to create {target_disp}: {e}"))?;
                set_permissions(&target, entry.header().mode().unwrap_or(0o755))?;
                written += 1;
            }
            EntryType::Regular => {
                if let Some(parent) = target.parent() {
                    let parent_disp = parent.display();
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("failed to create {parent_disp}: {e}"))?;
                }
                let mut file = std::fs::File::create(&target)
                    .map_err(|e| format!("failed to write {target_disp}: {e}"))?;
                io::copy(&mut entry, &mut file)
                    .map_err(|e| format!("failed to extract {target_disp}: {e}"))?;
                set_permissions(&target, entry.header().mode().unwrap_or(0o644))?;
                written += 1;
            }
            _ => {}
        }
    }

    // The resolved path matched nothing in the archive — report failure rather
    // than a silent success with an empty output directory (issue #169: "fails
    // clearly when… no downloadable source"). Same "source unavailable" contract
    // as a codeload fetch failure; the caller maps it to exit code 1.
    if written == 0 {
        return Err(format!(
            "source unavailable: the archive contains no files under '{skill_path}' — \
             the scanned path may have moved or been renamed upstream"
        ));
    }

    Ok(root_name)
}

/// Fetch the codeload tarball for `commit_sha` and extract its subtree.
///
/// Any network failure or non-200 response is reported as "source unavailable"
/// per the contract. Streams the archive directly into the decoder — the
/// response body is never fully buffered in memory.
fn fetch_and_extract_subtree(
    owner: &str,
    repo: &str,
    commit_sha: &str,
    skill_path: &str,
    dest: &Path,
) -> Result<Option<String>, String> {
    let url = format!("{CODELOAD_HOST}/{owner}/{repo}/tar.gz/{commit_sha}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS)))
        .http_status_as_error(false)
        .build()
        .into();

    let mut response = agent
        .get(&url)
        .header("User-Agent", &crate::updater::user_agent_string())
        .call()
        .map_err(|e| format!("source unavailable: failed to fetch {url}: {e}"))?;

    let status = response.status();
    if status != 200 {
        return Err(format!(
            "source unavailable: GitHub codeload returned HTTP {status} for {url}"
        ));
    }

    let reader = response.body_mut().as_reader();
    extract_subtree(reader, skill_path, dest)
}

/// Derive the embedded commit SHA from a codeload archive root directory name.
///
/// GitHub names the tarball's top-level directory `{repo}-<commitSha>` (this is
/// the value the archive's pax global header `path` field also carries). Stripping
/// the known `{repo}-` prefix yields the commit SHA. Returns `None` when the
/// root name does not match that shape.
fn extract_archive_sha(root_name: &str, repo: &str) -> Option<String> {
    root_name
        .strip_prefix(format!("{repo}-").as_str())
        .map(|sha| sha.to_string())
}

/// Derive the drift-check comparison value from an archive's root directory name.
///
/// Kept as its own function (rather than inlined at the `run_download` call site)
/// specifically so this composition is unit-testable: `extract_archive_sha` alone
/// was already covered by a passing test, but `run_download` never actually called
/// it — the raw `{repo}-{sha}` root name flowed straight into the drift comparison
/// unstripped, so every download printed a false-positive drift warning (e.g.
/// `aws-agent-skills-4ab904a6...` vs the resolved bare `4ab904a6...`). A test that
/// only exercised `extract_archive_sha` in isolation could not catch that — this
/// function is the seam a regression test can actually reach.
fn resolve_archive_sha(root_name: Option<String>, repo: &str) -> Option<String> {
    root_name.and_then(|name| extract_archive_sha(&name, repo))
}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let path_disp = path.display();
    // Mask to the rwx bits only — git's object model cannot carry setuid/setgid/
    // sticky bits anyway, so retaining `& 0o7777` would only ever re-apply bits
    // the archive could never legitimately claim.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))
        .map_err(|e| format!("failed to set permissions on {path_disp}: {e}"))
}

#[cfg(not(unix))]
fn set_permissions(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Orchestration + output
// ---------------------------------------------------------------------------

/// Resolve, fetch, and extract the scanned subtree for `slug`.
///
/// Returns [`DownloadOutcome`] on success. The destination pre-check runs
/// before any network call, and every failure mode maps to a clear message.
fn run_download(slug: &str, out: Option<PathBuf>) -> Result<DownloadOutcome, String> {
    let dest = out.unwrap_or_else(|| PathBuf::from(format!("./{slug}")));

    // Refuse a pre-existing non-empty destination before touching the network.
    if let Some(message) = destination_refusal(&dest) {
        return Err(message);
    }

    let base_url = directory::directory_base_url();
    let resolve = resolve_download(&base_url, slug)?;

    if resolve.source_type != "github" {
        return Err("this skill has no downloadable source yet".to_string());
    }

    let parts = parse_github_tree_url(&resolve.source_url)?;
    let root_name = fetch_and_extract_subtree(
        &parts.owner,
        &parts.repo,
        &resolve.commit_sha,
        &parts.path,
        &dest,
    )?;
    let archive_sha = resolve_archive_sha(root_name, &parts.repo);

    Ok(DownloadOutcome {
        resolve,
        parts,
        dest,
        archive_sha,
    })
}

/// Human-facing success output: provenance (`fetched …`) on stderr.
fn print_human_outcome(outcome: &DownloadOutcome) {
    let short = outcome
        .resolve
        .commit_sha
        .get(..7)
        .unwrap_or(&outcome.resolve.commit_sha);
    eprintln!(
        "fetched {}/{}@{}",
        outcome.parts.owner, outcome.parts.repo, short
    );
}

/// Machine success output: resolve metadata + written path on stdout.
fn print_json_outcome(outcome: &DownloadOutcome) {
    let output = DownloadOutput {
        slug: outcome.resolve.slug.clone(),
        name: outcome.resolve.name.clone(),
        source_type: outcome.resolve.source_type.clone(),
        source_url: outcome.resolve.source_url.clone(),
        source_hash: outcome.resolve.source_hash.clone(),
        commit_sha: outcome.resolve.commit_sha.clone(),
        written_path: outcome.dest.display().to_string(),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&output).unwrap_or_default()
    );
}

/// Entry point invoked from `cli::run`.
pub fn handle_download(slug: &str, out: Option<PathBuf>, json: bool) {
    let outcome = match run_download(slug, out) {
        Ok(outcome) => outcome,
        Err(message) => {
            eprintln!("Error: {message}");
            std::process::exit(1);
        }
    };

    // Provenance drift check: the archive's embedded commit SHA should equal the
    // resolved one (we fetched by that exact SHA). Never expected to differ — but
    // if it does, say so loudly on stderr regardless of --json.
    if let Some(archive_sha) = &outcome.archive_sha {
        if *archive_sha != outcome.resolve.commit_sha {
            eprintln!(
                "Warning: source archive commit {archive_sha} does not match the \
                 resolved commit {}. The downloaded bytes may not correspond to the \
                 scanned source.",
                outcome.resolve.commit_sha
            );
        }
    }

    if json {
        print_json_outcome(&outcome);
    } else {
        print_human_outcome(&outcome);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;
    use serde_json::json;
    use std::io::Write;

    // Environment mutation is process-global; serialize access across tests and
    // restore the original value in Drop, same convention as network.rs's
    // ScopedEnvVar.
    static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

    struct ScopedEnv(&'static str, Option<String>);
    impl ScopedEnv {
        fn set(name: &'static str, value: &str) -> ScopedEnv {
            let _guard = ENV_LOCK.lock().unwrap();
            let prev = std::env::var(name).ok();
            // SAFETY: environment mutation is serialized by ENV_LOCK, and the
            // original value is restored in Drop.
            unsafe { std::env::set_var(name, value) };
            ScopedEnv(name, prev)
        }
    }
    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: this value was created under ENV_LOCK, so restoration is
            // ordered with respect to other env-mutating tests.
            unsafe {
                match &self.1 {
                    Some(v) => std::env::set_var(self.0, v),
                    None => std::env::remove_var(self.0),
                }
            }
        }
    }

    /// Build a raw (uncompressed) tarball from `(path, bytes)` pairs.
    fn build_tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for &(path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            builder
                .append_data(&mut header, path, data)
                .expect("append tar entry");
        }
        builder.into_inner().expect("finalize tarball")
    }

    /// Gzip a raw tarball, mirroring GitHub codeload's `tar.gz` output.
    fn gzip(raw: &[u8]) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(raw).expect("gzip");
        encoder.finish().expect("flush gzip")
    }

    /// Append a single raw (uncompressed) tar entry to `buf`, bypassing the
    /// builder's write-time `..` guard so a test can exercise the *reader's*
    /// traversal protection against a hand-crafted malicious archive.
    ///
    /// The tar format is a 512-byte header block followed by the data padded to
    /// a 512-byte boundary. The path must fit in the 100-byte header field.
    fn append_raw_entry(buf: &mut Vec<u8>, name: &str, data: &[u8]) {
        assert!(
            name.len() < 100,
            "raw entry name must fit in the 100-byte tar header field"
        );
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());

        // Zero-padded octal of exactly `width` bytes, matching the tar crate's
        // field widths (mode/uid/gid: 8, size/mtime: 12, cksum: 8).
        let octal = |value: u64, width: usize| -> Vec<u8> {
            let text = format!("{value:o}");
            let mut out = vec![b'0'; width];
            let start = width.saturating_sub(text.len());
            out[start..].copy_from_slice(text.as_bytes());
            out
        };
        header[100..108].copy_from_slice(&octal(0o644, 8)); // mode
        header[108..116].copy_from_slice(&octal(0, 8)); // uid
        header[116..124].copy_from_slice(&octal(0, 8)); // gid
        header[124..136].copy_from_slice(&octal(data.len() as u64, 12)); // size (12-byte field)
        header[136..148].copy_from_slice(&octal(0, 12)); // mtime (12-byte field)
                                                         // These fields lie inside the checksum-summed region, so set them before
                                                         // the checksum is computed below.
        header[156] = b'0'; // typeflag: regular file
        header[257..263].copy_from_slice(b"ustar\0"); // magic
        header[263..265].copy_from_slice(b"00"); // version

        // Checksum: sum every byte, treating the 8 checksum-field bytes
        // (148..156) as spaces — this mirrors the tar crate's reader exactly.
        let mut sum: u64 = 0;
        for (i, &byte) in header.iter().enumerate() {
            if (148..156).contains(&i) {
                sum += 0x20;
            } else {
                sum += byte as u64;
            }
        }
        let checksum = format!("{sum:08o}");
        header[148..156].copy_from_slice(checksum.as_bytes());

        buf.extend_from_slice(&header);
        buf.extend_from_slice(data);
        let pad = (512 - (data.len() % 512)) % 512;
        buf.extend(std::iter::repeat(0u8).take(pad));
    }

    const ROOT: &str = "azure-skills-deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

    #[test]
    fn parse_canonical_github_tree_url() {
        let parts = parse_github_tree_url(
            "https://github.com/microsoft/azure-skills/tree/main/skills/azure-prepare",
        )
        .unwrap();
        assert_eq!(parts.owner, "microsoft");
        assert_eq!(parts.repo, "azure-skills");
        assert_eq!(parts.branch, "main");
        assert_eq!(parts.path, "skills/azure-prepare");
    }

    #[test]
    fn parse_rejects_non_github_scheme() {
        let err = parse_github_tree_url("https://example.com/o/r/tree/main/p").unwrap_err();
        assert!(err.contains("not a canonical GitHub tree link"), "{err}");
    }

    #[test]
    fn parse_rejects_missing_tree_segment() {
        let err = parse_github_tree_url("https://github.com/microsoft/azure-skills/main/skills/x")
            .unwrap_err();
        assert!(err.contains("no /tree/ segment"));
    }

    #[test]
    fn parse_accepts_root_path_url_without_a_subtree_path() {
        // A canonical root-path skill URL has nothing after the branch — the
        // whole repo IS the skill (the server, the backfill guard, and #169 all
        // support root-path audits).
        let parts =
            parse_github_tree_url("https://github.com/microsoft/azure-skills/tree/main").unwrap();
        assert_eq!(parts.owner, "microsoft");
        assert_eq!(parts.repo, "azure-skills");
        assert_eq!(parts.branch, "main");
        assert_eq!(parts.path, "");
    }

    #[test]
    fn is_subtree_entry_includes_subtree_and_siblings() {
        assert!(is_subtree_entry(
            "skills/azure-prepare/SKILL.md",
            "skills/azure-prepare"
        ));
        assert!(is_subtree_entry(
            "skills/azure-prepare",
            "skills/azure-prepare"
        ));
        // A sibling in the same repo must NOT match.
        assert!(!is_subtree_entry(
            "skills/azure-deploy/SKILL.md",
            "skills/azure-prepare"
        ));
        assert!(!is_subtree_entry("README.md", "skills/azure-prepare"));
        // A root-path skill (empty path) matches every entry in the archive.
        assert!(is_subtree_entry("README.md", ""));
        assert!(is_subtree_entry("skills/azure-prepare/SKILL.md", ""));
        assert!(is_subtree_entry("skills/azure-prepare/", ""));
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let base = Path::new("/tmp/dest");
        // Three `..` pop past `base` entirely → the `pop` guard rejects it.
        let too_deep = Path::new("a/b/c/../../../../evil");
        assert!(safe_join(base, too_deep).is_err());
        // Two `..` land on a sibling of `base` (/evil instead of /tmp/dest/…)
        // → the `starts_with` backstop rejects it.
        let sibling = Path::new("../../evil");
        assert!(safe_join(base, sibling).is_err());
        // A normal entry stays inside.
        let good = Path::new("skills/azure-prepare/SKILL.md");
        assert!(safe_join(base, good).is_ok());
    }

    #[test]
    fn extract_archive_sha_strips_repo_prefix() {
        assert_eq!(
            extract_archive_sha(ROOT, "azure-skills"),
            Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string())
        );
        // A root name that does not carry the `{repo}-` prefix yields None.
        assert_eq!(extract_archive_sha("unexpected", "azure-skills"), None);
    }

    #[test]
    fn resolve_archive_sha_strips_the_root_prefix_before_the_drift_comparison() {
        // Regression test for a real false-positive drift warning: `run_download`
        // was passing the raw `{repo}-{sha}` root name straight through as
        // `archive_sha` without ever calling `extract_archive_sha`, so the drift
        // check compared "aws-agent-skills-4ab904a6..." against the bare resolved
        // "4ab904a6..." and always reported a mismatch. Exercises the exact
        // composition `run_download` calls — `extract_archive_sha_strips_repo_prefix`
        // above only proved the stripper works in isolation, which is exactly what
        // let this ship: the seam between "fetch returns a root name" and "the
        // drift check gets a bare SHA" was never actually tested.
        let root_name = "aws-agent-skills-4ab904a69cda893b5c98f97966bf9a48311823e9".to_string();
        assert_eq!(
            resolve_archive_sha(Some(root_name), "aws-agent-skills"),
            Some("4ab904a69cda893b5c98f97966bf9a48311823e9".to_string())
        );
        // No archive entries were extracted at all (empty repo/path) — nothing to
        // compare, so no false drift warning either.
        assert_eq!(resolve_archive_sha(None, "aws-agent-skills"), None);
    }

    #[test]
    fn extract_subtree_only_writes_subtree() {
        let raw = build_tarball(&[
            (format!("{ROOT}/README.md").as_str(), b"root readme"),
            (
                format!("{ROOT}/skills/azure-prepare/SKILL.md").as_str(),
                b"# prepare",
            ),
            (
                format!("{ROOT}/skills/azure-prepare/notes.txt").as_str(),
                b"notes",
            ),
            // Sibling in the same repo — must be skipped.
            (
                format!("{ROOT}/skills/azure-deploy/SKILL.md").as_str(),
                b"# deploy",
            ),
        ]);
        let gz = gzip(&raw);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");

        let root = extract_subtree(&gz[..], "skills/azure-prepare", &dest).unwrap();
        assert_eq!(root.as_deref(), Some(ROOT));

        assert!(dest.join("skills/azure-prepare/SKILL.md").exists());
        assert!(dest.join("skills/azure-prepare/notes.txt").exists());
        // Sibling and repo-root files were NOT extracted.
        assert!(!dest.join("README.md").exists());
        assert!(!dest.join("skills/azure-deploy/SKILL.md").exists());
    }

    #[test]
    fn extract_raw_tarball_successfully() {
        // Sanity check for the raw-entry builder: a well-formed archive must
        // extract without error (proves the reader accepts our bytes).
        let mut buf = Vec::new();
        append_raw_entry(
            &mut buf,
            &format!("{ROOT}/skills/azure-prepare/SKILL.md"),
            b"# prepare\n",
        );
        let gz = gzip(&buf);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");

        let root = extract_subtree(&gz[..], "skills/azure-prepare", &dest).unwrap();
        assert_eq!(root.as_deref(), Some(ROOT));
        assert!(dest.join("skills/azure-prepare/SKILL.md").exists());
    }

    #[test]
    fn extract_subtree_rejects_path_traversal() {
        // A hand-crafted entry whose path escapes the root after the
        // `{repo}-{sha}` prefix is stripped. The high-level tar builder refuses
        // to write `..` on disk, so we append the raw bytes directly to exercise
        // the reader's traversal guard. Three `..` defeat the two-component
        // `skills/azure-prepare` prefix and pop past the destination.
        let mut buf = Vec::new();
        append_raw_entry(
            &mut buf,
            &format!("{ROOT}/skills/azure-prepare/../../../etc/passwd"),
            b"evil",
        );
        let gz = gzip(&buf);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");

        let result = extract_subtree(&gz[..], "skills/azure-prepare", &dest);
        // The traversal entry must be rejected, not silently followed.
        assert!(
            result.is_err(),
            "expected a traversal rejection, got {result:?}"
        );
        let err = result.unwrap_err();
        assert!(err.contains("path traversal"), "unexpected error: {err}");
        // The malicious file was never written inside the destination either.
        assert!(!dest.join("etc").exists(), "traversal wrote inside dest");
    }

    #[test]
    fn extract_subtree_rejects_parent_dir_entries_outright() {
        // A crafted entry like `skills/azure-prepare/../azure-deploy/SKILL.md`
        // passes the string-prefix subtree check while lexically resolving
        // (after `..` handling) to a SIBLING directory outside the intended
        // subtree. `..` never appears in a real GitHub codeload archive, so the
        // entry is rejected outright — NOT silently relocated to its lexical
        // destination. The high-level tar builder refuses `..` paths, so we
        // append raw bytes to exercise the reader's guard.
        let mut buf = Vec::new();
        append_raw_entry(
            &mut buf,
            &format!("{ROOT}/skills/azure-prepare/../azure-deploy/SKILL.md"),
            b"# deploy\n",
        );
        let gz = gzip(&buf);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");

        let result = extract_subtree(&gz[..], "skills/azure-prepare", &dest);
        assert!(result.is_err(), "expected rejection, got {result:?}");
        let err = result.unwrap_err();
        assert!(err.contains("path traversal"), "unexpected error: {err}");
        // The entry appears neither in the correct subtree location nor
        // anywhere else under dest — it was rejected, not relocated.
        assert!(!dest.join("skills/azure-prepare/SKILL.md").exists());
        assert!(!dest.join("skills/azure-deploy/SKILL.md").exists());
        assert!(!dest.exists(), "nothing should be written under dest");
    }

    #[test]
    fn extract_subtree_fails_when_no_subtree_entry_matches() {
        // A stale/mismatched resolved path: the archive contains sibling
        // content only, so nothing matches the subtree. The command must fail
        // clearly ("source unavailable") rather than report success with an
        // empty output directory (issue #169: "fails clearly when… no
        // downloadable source").
        let raw = build_tarball(&[
            (format!("{ROOT}/README.md").as_str(), b"root readme"),
            (
                format!("{ROOT}/skills/azure-deploy/SKILL.md").as_str(),
                b"# deploy",
            ),
        ]);
        let gz = gzip(&raw);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");

        let result = extract_subtree(&gz[..], "skills/azure-prepare", &dest);
        assert!(result.is_err(), "expected failure, got {result:?}");
        let err = result.unwrap_err();
        assert!(
            err.contains("source unavailable"),
            "unexpected error: {err}"
        );
        // Nothing was written anywhere.
        assert!(!dest.exists());
    }

    #[test]
    fn root_path_skill_extracts_the_whole_repo() {
        // End to end for a root-path skill: the canonical URL parses with an
        // empty path, and extraction with that empty path treats every archive
        // entry as part of the skill (the whole repo IS the skill).
        let parts =
            parse_github_tree_url("https://github.com/microsoft/azure-skills/tree/main").unwrap();
        assert_eq!(parts.path, "");

        let raw = build_tarball(&[
            (format!("{ROOT}/README.md").as_str(), b"root readme"),
            (
                format!("{ROOT}/skills/azure-prepare/SKILL.md").as_str(),
                b"# prepare",
            ),
        ]);
        let gz = gzip(&raw);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");

        let root = extract_subtree(&gz[..], &parts.path, &dest).unwrap();
        assert_eq!(root.as_deref(), Some(ROOT));
        assert!(dest.join("README.md").exists());
        assert!(dest.join("skills/azure-prepare/SKILL.md").exists());
    }

    #[test]
    fn run_download_refuses_non_empty_destination_before_any_network_call() {
        // If the destination-check ordering ever regressed and a request was
        // made, the env override would point it at the mock server (never
        // production), where the catch-all mock below counts it.
        let server = MockServer::start();
        let _beta = ScopedEnv::set("SEARCH_BETA_TESTING", "1");
        let _endpoint = ScopedEnv::set("VETTD_DIRECTORY_ENDPOINT", &server.base_url());
        // Catch-all POST mock: ANY request reaching the server is a regression.
        let any_request = server.mock(|when, then| {
            when.method(httpmock::Method::POST);
            then.status(200).json_body(json!({
                "slug": "ok", "name": "OK", "sourceType": "github",
                "sourceUrl": "https://github.com/o/r/tree/main/skills/ok",
                "sourceHash": "abc123",
                "commitSha": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
            }));
        });

        // Pre-populate a non-empty destination BEFORE running the orchestration.
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("existing.txt"), b"x").unwrap();

        let result = run_download("ok", Some(dest.clone()));

        let err = result.unwrap_err();
        assert!(err.contains("not empty"), "unexpected error: {err}");
        // The destination check ran first: the server received zero requests.
        assert_eq!(any_request.calls(), 0);
    }

    #[test]
    fn destination_refusal_only_for_non_empty_existing() {
        let tmp = tempfile::tempdir().unwrap();
        // Missing destination — safe.
        assert!(destination_refusal(&tmp.path().join("absent")).is_none());
        // Empty existing directory — safe.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(destination_refusal(&empty).is_none());
        // Non-empty directory — refused.
        std::fs::write(empty.join("file.txt"), b"x").unwrap();
        let msg = destination_refusal(&empty).expect("non-empty dir must be refused");
        assert!(msg.contains("not empty"), "{msg}");
    }

    #[test]
    fn resolve_download_maps_contract_status_codes() {
        let server = MockServer::start();
        let base = format!("{}/api/directory", server.base_url());

        // Positive (200) — also asserts no Authorization header is sent. The
        // first mock would 401 if the client *did* attach an auth header; the
        // successful 200 therefore proves the client left it off.
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .header_exists("authorization");
            then.status(401).json_body(json!({"error": "unauthorized"}));
        });
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/directory/ok/download");
            then.status(200).json_body(json!({
                "slug": "ok",
                "name": "OK",
                "sourceType": "github",
                "sourceUrl": "https://github.com/o/r/tree/main/skills/ok",
                "sourceHash": "abc123",
                "commitSha": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
            }));
        });
        let resp = resolve_download(&base, "ok").unwrap();
        assert_eq!(resp.source_type, "github");
        assert_eq!(resp.commit_sha, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef");

        // 404 → "skill not found in directory".
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/directory/missing/download");
            then.status(404).json_body(json!({"error": "not found"}));
        });
        let err = resolve_download(&base, "missing").unwrap_err();
        assert!(err.contains("skill not found in directory"), "{err}");

        // 422 → "this skill has no downloadable source yet".
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/directory/nodl/download");
            then.status(422)
                .json_body(json!({"error": "not downloadable"}));
        });
        let err = resolve_download(&base, "nodl").unwrap_err();
        assert!(
            err.contains("this skill has no downloadable source yet"),
            "{err}"
        );
    }

    #[test]
    fn json_output_shape_includes_written_path() {
        let outcome = DownloadOutcome {
            resolve: DownloadResolveResponse {
                slug: "ok".into(),
                name: "OK".into(),
                source_type: "github".into(),
                source_url: "https://github.com/o/r/tree/main/skills/ok".into(),
                source_hash: "abc123".into(),
                commit_sha: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into(),
            },
            parts: GitHubTreeParts {
                owner: "o".into(),
                repo: "r".into(),
                branch: "main".into(),
                path: "skills/ok".into(),
            },
            dest: PathBuf::from("./ok"),
            archive_sha: Some("deadbeef".into()),
        };
        let value: serde_json::Value = serde_json::to_value(DownloadOutput {
            slug: outcome.resolve.slug.clone(),
            name: outcome.resolve.name.clone(),
            source_type: outcome.resolve.source_type.clone(),
            source_url: outcome.resolve.source_url.clone(),
            source_hash: outcome.resolve.source_hash.clone(),
            commit_sha: outcome.resolve.commit_sha.clone(),
            written_path: outcome.dest.display().to_string(),
        })
        .unwrap();
        assert_eq!(value["slug"], "ok");
        assert_eq!(value["writtenPath"], "./ok");
        assert_eq!(
            value["commitSha"],
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        );
        // camelCase keys, not snake_case.
        assert!(value.get("writtenPath").is_some());
        assert!(value.get("written_path").is_none());
    }
}
