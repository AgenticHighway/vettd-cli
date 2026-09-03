//! The on-disk asset index: local skills, agents and MCP server descriptors.
//!
//! Ported from the `FsIndex` half of `spikes/828-passive-observer/prototype/attribute.py`. It
//! answers one question for the attributor: *does this machine hold a copy of the asset the
//! harness named, and what does it hash to?* A hit produces a `content_hash` or `descriptor_hash`
//! key; a miss degrades that one asset to a keyed `name_hash`.
//!
//! **Fail-open is the contract, not a convenience.** Every filesystem error here means "this asset
//! is absent", never "this run fails": a permission-denied skill file, a truncated `.claude.json`
//! or a vanished directory costs the user one hashed asset, not the observation. Nothing returns
//! `Err`.
//!
//! **Privacy.** The keys of [`FsIndex`]'s maps are local names (skill names, agent types, MCP
//! server names) read off this machine's disk, as are the paths walked to build them. They are
//! local-only: the attributor pairs a name with a hash and puts only that hash, and the
//! closed-enum asset type, on the wire.
//!
//! **Divergence from the prototype (deliberate).** The prototype looked for MCP descriptors under
//! the harness root only and therefore never found one on a real machine, because Claude Code
//! writes `mcpServers` to `~/.claude.json` — a sibling of `~/.claude/`, not a child.
//! [`FsIndex::new`] reads, first-wins: `<root>/.claude.json`, `<home>/.claude.json`,
//! `<root>/settings.json` — fixture source, real source, prototype fallback, the fix
//! `docs/vettd-observe-port-plan.md` ("`FsIndex` descriptor sources") calls for.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde_json::{json, Map, Value};
use walkdir::WalkDir;

use crate::observe::canonical::{canonical_json, hex_sha256};
use crate::observe::types::{ASSET_AGENT, ASSET_MCP_SERVER, ASSET_SKILL};

/// Flags whose *value* is dropped from a descriptor's argv (`attribute.py:64`). The flag itself
/// stays: it is part of how the server is invoked (identity); the value after it is a credential.
pub(super) const SECRET_FLAGS: [&str; 6] = [
    "--api-key",
    "--token",
    "--password",
    "--secret",
    "-k",
    "--bearer",
];

/// Minimum length for the opaque-token rule — see [`is_opaque_token`].
const OPAQUE_MIN_LEN: usize = 32;

/// Vendor-prefixed bearer tokens (`attribute.py:65`), searched anywhere in a token.
static BEARER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[^A-Za-z0-9])(?:sk|ghp|gho|ghu|ghs|xox[abp]|AKIA|ah|ntn)[_-][A-Za-z0-9_-]{8,}",
    )
    .expect("BEARER_RE is valid")
});

/// A JWT's header segment (`attribute.py:66`), searched anywhere in a token.
static JWT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"eyJ[A-Za-z0-9_-]{10,}\.").expect("JWT_RE is valid"));

/// One local asset's identity: the hash that may egress, and the newest mtime behind it — which
/// the attributor's mtime rule compares against the harness's listing timestamp, and which never
/// egresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LocalAsset {
    /// Lowercase hex SHA-256 — a tree hash for skills, a file hash for agents.
    pub(super) content_hash: String,
    /// Newest mtime in milliseconds over every file *and* directory behind `content_hash`.
    pub(super) max_mtime_ms: i64,
}

/// Read-only view of the local asset files of one Claude Code home. Built eagerly: the prototype
/// hashed the whole index on first access, so the constructor does the same work without needing
/// interior mutability. Every map is keyed by a **local-only** name (see the module docs).
///
/// # This index is Claude Code's, and nothing in its type says so
///
/// The prototype keys `_mcp` by harness and returns skills and agents from `listed()` only for
/// `claude_code`; this index drops that argument because v1 has one harness (plan ruling 2). A
/// second `Source` must therefore not reuse it as-is: a Codex run passed this index would key its
/// MCP servers off Claude's `.claude.json` descriptors and inherit Claude's skills and agents,
/// silently emitting hashes for assets that run never loaded. The failure is wrong data on the
/// wire, not an error, so the Codex port has to restore the harness dimension here — and add a
/// `~/.codex/config.toml` descriptor reader, which this has no equivalent of.
#[derive(Debug, Default)]
pub(crate) struct FsIndex {
    skills: BTreeMap<String, LocalAsset>,
    agents: BTreeMap<String, LocalAsset>,
    mcp: BTreeMap<String, String>,
}

impl FsIndex {
    /// Indexes `root` (typically `~/.claude`), taking the second descriptor source from
    /// `dirs::home_dir()`; `None` yields an empty index rather than an error.
    pub(crate) fn new(root: Option<&Path>) -> FsIndex {
        FsIndex::with_home(root, dirs::home_dir().as_deref())
    }
    /// [`FsIndex::new`] with the home directory injected, so tests never read the real `$HOME`.
    pub(crate) fn with_home(root: Option<&Path>, home: Option<&Path>) -> FsIndex {
        let Some(root) = root else {
            return FsIndex::default();
        };
        let mut mcp = BTreeMap::new();
        let sources = [
            Some(root.join(".claude.json")),
            home.map(|h| h.join(".claude.json")),
            Some(root.join("settings.json")),
        ];
        for path in sources.into_iter().flatten() {
            for (name, raw) in json_servers(&path) {
                // First source wins: the prototype's `setdefault`, over a longer source list.
                mcp.entry(name).or_insert_with(|| descriptor_hash(&raw));
            }
        }
        FsIndex {
            skills: index_skills(&root.join("skills")),
            agents: index_agents(&root.join("agents")),
            mcp,
        }
    }

    /// The local tree behind skill `name`, if this machine holds one.
    pub(super) fn skill(&self, name: &str) -> Option<&LocalAsset> {
        self.skills.get(name)
    }
    /// The local `agents/<type>.md` behind agent type `name`, if this machine holds one.
    pub(super) fn agent(&self, name: &str) -> Option<&LocalAsset> {
        self.agents.get(name)
    }
    /// The descriptor hash configured for MCP server `name`, if any source declares it.
    pub(super) fn mcp_descriptor(&self, name: &str) -> Option<&str> {
        self.mcp.get(name).map(String::as_str)
    }
    /// Every asset name the filesystem knows, by asset type — the filesystem-basis loaded set
    /// (`attribute.py::FsIndex.listed`). Names are local-only; only the attributor's hashes egress.
    pub(super) fn listed(&self) -> BTreeMap<&'static str, BTreeSet<String>> {
        BTreeMap::from([
            (ASSET_SKILL, self.skills.keys().cloned().collect()),
            (ASSET_AGENT, self.agents.keys().cloned().collect()),
            (ASSET_MCP_SERVER, self.mcp.keys().cloned().collect()),
        ])
    }
}

// -- skills and agents ---------------------------------------------------------------------------

/// `st_mtime_ns // 1_000_000`, floored so a pre-1970 mtime rounds the way Python's floor division
/// does; `None` when the time is unrepresentable.
fn mtime_ms(meta: &std::fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    let ns: i128 = match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => i128::try_from(d.as_nanos()).ok()?,
        Err(e) => -i128::try_from(e.duration().as_nanos()).ok()?,
    };
    i64::try_from(ns.div_euclid(1_000_000)).ok()
}

/// SHA-256 over the sorted `[relative posix path, sha256(file)]` pairs of every regular file under
/// `root`, plus the newest mtime over those files *and* the directories walked. Relative paths are
/// joined with `/` on every platform: a Windows `\` would change the preimage and split an asset's
/// identity across operating systems. `None` if a file that is there cannot be read — a partially
/// hashed tree is a *wrong* identity, not a missing one.
fn tree_asset(root: &Path) -> Option<LocalAsset> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut max_mtime: Option<i64> = None;
    for entry in WalkDir::new(root).sort_by_file_name() {
        // An unreadable subdirectory yields an error and nothing else, as `os.walk` swallows the
        // same condition: the tree is hashed from what is reachable. `file_type()` does not follow
        // links, so only real directories count — the set `os.walk(followlinks=False)` stats.
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if entry.file_type().is_dir() {
            max_mtime = max_mtime.max(mtime_ms(&std::fs::metadata(path).ok()?));
            continue;
        }
        // `metadata` follows links, like `os.path.isfile`: a link to a regular file is hashed.
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let bytes = std::fs::read(path).ok()?;
        pairs.push((relative_posix(root, path)?, hex_sha256(&bytes)));
        max_mtime = max_mtime.max(mtime_ms(&meta));
    }
    pairs.sort();
    let rows: Vec<Value> = pairs
        .into_iter()
        .map(|(rel, sha)| json!([rel, sha]))
        .collect();
    let preimage = canonical_json(&Value::Array(rows)).ok()?;
    Some(LocalAsset {
        content_hash: hex_sha256(preimage.as_bytes()),
        max_mtime_ms: max_mtime?,
    })
}

/// `path` relative to `root` with `/` separators; `None` if `path` is not under `root`. A non-UTF-8
/// component becomes U+FFFD, a divergence from CPython's surrogate-escaped names: such a file
/// would hash differently here than in the prototype.
fn relative_posix(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts = rel.components().map(|c| c.as_os_str().to_string_lossy());
    Some(parts.collect::<Vec<_>>().join("/"))
}

/// Every directory under `root` that lists a `SKILL.md`, keyed by its own directory name. First hit
/// wins, in sorted walk order, as the prototype's `setdefault` does; a nested skill therefore gets
/// its own entry *and* contributes to its parent's tree hash.
fn index_skills(root: &Path) -> BTreeMap<String, LocalAsset> {
    let mut out = BTreeMap::new();
    if !root.is_dir() {
        return out;
    }
    for entry in WalkDir::new(root).sort_by_file_name() {
        let Ok(entry) = entry else { continue };
        // Membership by name in the directory listing, like `"SKILL.md" in filenames`.
        //
        // `os.walk` splits each directory's entries into `dirnames` and `filenames`, so a `SKILL.md`
        // that is itself a directory — or a symlink to one — is never in `filenames` and its parent
        // is not a skill. `symlink_metadata(..).is_ok()` alone accepts both and mints a phantom
        // skill, which moves `bom_version`. A broken symlink and a symlink to a file DO appear in
        // `filenames`, so the test cannot simply reject symlinks: stat through the link and reject
        // only when it resolves to a directory.
        if !entry.file_type().is_dir() || !lists_skill_file(entry.path()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let (false, Some(asset)) = (out.contains_key(&name), tree_asset(entry.path())) {
            out.insert(name, asset);
        }
    }
    out
}

/// Whether `dir` lists a `SKILL.md` the way `os.walk` would put it in `filenames`.
fn lists_skill_file(dir: &Path) -> bool {
    let candidate = dir.join("SKILL.md");
    std::fs::symlink_metadata(&candidate).is_ok()
        && !std::fs::metadata(&candidate).is_ok_and(|meta| meta.is_dir())
}

/// Every `<root>/<type>.md` regular file, keyed by `<type>`, hashed as raw bytes.
fn index_agents(root: &Path) -> BTreeMap<String, LocalAsset> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let Some(stem) = name.strip_suffix(".md").map(str::to_string) else {
            continue;
        };
        if let Some(asset) = file_asset(&path) {
            out.insert(stem, asset);
        }
    }
    out
}

/// One regular file hashed as its raw bytes — the agent preimage. `None` for anything that is not
/// a readable regular file, which is how an agent degrades to a `name_hash`.
fn file_asset(path: &Path) -> Option<LocalAsset> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(LocalAsset {
        content_hash: hex_sha256(&std::fs::read(path).ok()?),
        max_mtime_ms: mtime_ms(&meta)?,
    })
}

/// `mcpServers` from a JSON config file, keeping only object-valued entries. Unreadable or
/// malformed input is an empty map: a hand-edited `.claude.json` with a trailing comma must cost
/// the descriptors, not the run.
fn json_servers(path: &Path) -> BTreeMap<String, Map<String, Value>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&text) else {
        return BTreeMap::new();
    };
    let Some(Value::Object(servers)) = data.get("mcpServers") else {
        return BTreeMap::new();
    };
    servers
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), v.as_object()?.clone())))
        .collect()
}

/// Python's `str()` for a JSON value, which the prototype applies to every argv token. Exact for
/// the shapes that occur (strings, and the scalars a hand-edited config can hold); arrays and
/// objects fall back to JSON rendering, *not* Python's `repr`, because an MCP argv holding a
/// nested container is outside the observed shape.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

/// `str(value or "")`: Python's falsy values (absent, null, `false`, `0`, `""`, `[]`, `{}`) → `""`.
fn python_str_or_empty(value: Option<&Value>) -> String {
    let falsy = match value {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => !b,
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
    };
    if falsy {
        String::new()
    } else {
        python_str(value.unwrap_or(&Value::Null))
    }
}

/// A token that is opaque enough to be a credential: at least [`OPAQUE_MIN_LEN`] characters, all
/// from `[A-Za-z0-9_+/=-]`, with at least one letter and at least one digit. This replaces the
/// prototype's `_OPAQUE_RE` (`attribute.py:69`), whose two lookaheads the `regex` crate cannot
/// compile.
///
/// Python's `$` is not end-of-string: outside `MULTILINE` it also matches immediately before a
/// single trailing newline, so `"<31 opaque chars>\n"` is opaque to the prototype. One trailing
/// newline is therefore stripped before the test. The direction matters — judging such a token
/// *not* opaque leaves it in the descriptor argv, so a credential-shaped value would enter the
/// `descriptor_hash` preimage that this rule exists to keep out.
pub(super) fn is_opaque_token(token: &str) -> bool {
    let token = token.strip_suffix('\n').unwrap_or(token);
    let mut len = 0usize;
    let mut alpha = false;
    let mut digit = false;
    for c in token.chars() {
        if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '/' | '=' | '-')) {
            return false;
        }
        alpha |= c.is_ascii_alphabetic();
        digit |= c.is_ascii_digit();
        len += 1;
    }
    len >= OPAQUE_MIN_LEN && alpha && digit
}

/// Whether `token` looks like a credential by any of the three rules (`attribute.py:104`).
pub(super) fn is_secret_shaped(token: &str) -> bool {
    BEARER_RE.is_match(token) || JWT_RE.is_match(token) || is_opaque_token(token)
}

/// Path-shaped tokens carry directory names, so they never enter the preimage.
fn looks_like_path(token: &str) -> bool {
    token.contains(['/', '\\'])
}

/// Argv minus path-shaped tokens, secret-shaped tokens, and the value after a secret flag.
/// A `--api-key=<value>` keeps `--api-key` and drops the glued value; a bare `--api-key` keeps the
/// flag and drops the *next* token. A non-list `args` is an empty argv.
pub(super) fn strip_args(args: Option<&Value>) -> Vec<String> {
    let empty: Vec<Value> = Vec::new();
    let items = args.and_then(Value::as_array).unwrap_or(&empty);
    let mut out = Vec::new();
    let mut drop_next = false;
    for raw in items {
        let token = python_str(raw);
        if drop_next {
            drop_next = false;
            continue;
        }
        let split = token.find('=');
        let flag = &token[..split.unwrap_or(token.len())];
        if SECRET_FLAGS.contains(&flag) {
            out.push(flag.to_string());
            drop_next = split.is_none();
            continue;
        }
        if looks_like_path(&token) || is_secret_shaped(&token) {
            continue;
        }
        out.push(token);
    }
    out
}

/// The stripped descriptor whose canonical JSON is the hash preimage. `command` is the basename
/// (never the directory it sits in) or, for a url server, the scheme class — so the hostname never
/// enters the preimage. `env` contributes names only, never values.
pub(super) fn canonical_descriptor(raw: &Map<String, Value>) -> Value {
    let url = raw.get("url").and_then(Value::as_str).unwrap_or("");
    let (transport, command) = if url.trim().is_empty() {
        let command = python_str_or_empty(raw.get("command"));
        let base = command.rsplit(['\\', '/']).next().unwrap_or_default();
        ("stdio", base.to_string())
    } else if url.trim().to_lowercase().starts_with("https://") {
        ("http", "https".to_string())
    } else {
        ("http", "http".to_string())
    };
    let env_names: Vec<Value> = match raw.get("env") {
        Some(Value::Object(env)) => env.keys().map(|k| Value::String(k.clone())).collect(),
        _ => Vec::new(),
    };
    let args = strip_args(raw.get("args"));
    json!({"transport": transport, "command": command, "args": args, "env_names": env_names})
}

/// SHA-256 over the canonical JSON of [`canonical_descriptor`] — the `descriptor_hash` key basis.
pub(super) fn descriptor_hash(raw: &Map<String, Value>) -> String {
    let descriptor = canonical_descriptor(raw);
    let preimage = canonical_json(&descriptor)
        .expect("a canonical descriptor holds only strings and arrays of strings");
    hex_sha256(preimage.as_bytes())
}

#[cfg(test)]
#[path = "fs_index_tests.rs"]
mod tests;
