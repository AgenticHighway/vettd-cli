use clap::{CommandFactory, Parser, Subcommand};
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use crate::contract::{
    build_contract_payload, build_contract_payload_for_disclosure, render_disclosure,
    ContractPayload,
};
use crate::lite_mode::{limit_lite_mode_report, print_locked_summary, LITE_MODE_VISIBLE_RESULTS};
use crate::models::{ArtifactReport, ScanReport};
use crate::output::{do_submit, emit, resolve_submit_auth};
use crate::scan::run_scan_with_cache;
use crate::submit::{save_auth_config, AuthConfig, SaveOutcome, DEFAULT_PRODUCTION_ENDPOINT};

// ---------------------------------------------------------------------------
// CLI argument definitions
// ---------------------------------------------------------------------------

/// Build the two-line version string at runtime.
///
/// `vettd_skill_scanner::VERSION` is a `pub const`, not a string literal, so
/// it cannot be used in `concat!()` inside a `#[command(...)]` attribute (only
/// literals are accepted there). This function constructs the string at runtime
/// and is used by the manual `--version` handler in `main.rs`.
pub fn long_version_string() -> String {
    format!(
        "vettd {}\nskill-scanner {}",
        env!("CARGO_PKG_VERSION"),
        vettd_skill_scanner::VERSION
    )
}

#[derive(Parser)]
#[command(
    name = "vettd",
    about = "AI Execution Inventory — detect, analyze, and report AI execution artifacts.",
    version = env!("CARGO_PKG_VERSION"),
)]
pub struct Cli {
    /// Output machine-readable JSON to stdout
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Scan for AI execution artifacts
    Scan {
        #[command(subcommand)]
        subcommand: Option<ScanSubcommand>,
    },
    /// Configure API credentials for scan submission
    Auth {
        /// API key (e.g. ah_xxxx). If omitted, vettd prompts securely.
        #[arg(long)]
        key: Option<String>,
        /// Ingest endpoint URL (defaults to production)
        #[arg(long)]
        endpoint: Option<String>,
        /// Allow saving a public (non-local/private) endpoint
        #[arg(long)]
        allow_public_endpoint: bool,
        /// Optional auth subcommand (e.g. `status`)
        #[command(subcommand)]
        action: Option<AuthSubcommand>,
    },
    /// Inspect the scanner data contract
    Contract {
        #[command(subcommand)]
        action: ContractSubcommand,
    },
    /// Browse the public vettd directory
    Directory {
        #[command(subcommand)]
        action: DirectorySubcommand,
    },
    /// Browse your own vettd inventory (requires authentication)
    Inventory {
        #[command(subcommand)]
        action: InventorySubcommand,
    },
    /// Check for updates and self-update the scanner binary
    Update {
        /// Only check for updates — don't download or install
        #[arg(long)]
        check: bool,
        /// Skip the confirmation prompt
        #[arg(long)]
        force: bool,
    },
    /// Manage custom detection rules
    Rules {
        #[command(subcommand)]
        action: RuleAction,
    },
}

#[derive(Subcommand)]
pub enum ScanSubcommand {
    /// Default scan — critical host roots plus bounded user-space/project roots
    Default {
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Quick scan — critical OS-aware agent config areas only
    Quick {
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Full scan — entire filesystem from root
    Full {
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Scan a single file
    File {
        path: PathBuf,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Scan a folder
    Folder {
        path: PathBuf,
        /// Walk all subdirectories without a depth limit
        #[arg(long)]
        deep: bool,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Deep-scan a local git repo
    Repo {
        path: PathBuf,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Submit a previously saved report file
    Submit {
        /// Path to the JSON report file
        report: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum RuleAction {
    /// List installed rules
    List,
    /// Install a rule file into ~/.vettd/rules/
    Add {
        /// Path to the .toml rule file
        path: PathBuf,
    },
    /// Remove an installed rule by name (e.g. terraform-ai or terraform-ai.toml)
    Remove {
        /// Rule name or filename
        name: String,
    },
    /// Validate a rule file without installing it
    Validate {
        /// Path to the .toml rule file
        path: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum AuthSubcommand {
    /// Show current auth/identity and reachability status
    Status,
}

#[derive(Subcommand)]
pub enum ContractSubcommand {
    /// Show local vs. server contract version status
    Status,
}

// The `Search` variant carries the full beta search-filter surface
// (language / agent-compat / rankings / source / rank-filter / asset-type /
// mcp-category / deployment / registry-type) inline, which dwarfs the other
// variants. Boxing clap arg fields isn't ergonomic and the enum is parsed
// exactly once per process, so the size asymmetry is deliberate.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum DirectorySubcommand {
    /// Search the directory
    Search {
        /// Search query (use quotes for multi-word queries)
        #[arg(required = true)]
        query: Vec<String>,
        /// Page number to retrieve
        #[arg(long, default_value = "1")]
        page: u32,
        /// Sort order: newest|rating|alpha
        #[arg(long, default_value = "newest")]
        sort: String,
        /// Reverse the sort order
        #[arg(long, short = 'r')]
        reverse: bool,
        /// Filter by implementation language (repeatable). Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "language")]
        languages: Vec<String>,
        /// Filter by agent/runtime compatibility (repeatable). Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "agent-compatibility")]
        agent_compatibility: Vec<String>,
        /// Minimum-threshold ranking filter as a JSON object, e.g.
        /// '{"stars": 50, "officialClaudeMarketplace": true}'. Requires SEARCH_BETA_TESTING=1.
        #[arg(long)]
        rankings: Option<String>,
        /// Which catalog to search: skill (default) or mcp. `mcp` requires SEARCH_BETA_TESTING=1.
        #[arg(long = "asset-type", default_value = "skill", value_parser = ["skill", "mcp"])]
        asset_type: String,
        /// Filter by discovery source, e.g. marketplace|seed|search|manual (repeatable).
        /// Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "source")]
        sources: Vec<String>,
        /// Per-source search-rank ceiling as key=N (repeatable), e.g.
        /// --rank-filter search_rank_skills_sh_rank=100. Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "rank-filter")]
        rank_filters: Vec<String>,
        /// MCP-only: filter by category server|client|framework|tooling (repeatable).
        /// Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "mcp-category")]
        mcp_category: Vec<String>,
        /// MCP-only: filter by deployment local|remote|hybrid (repeatable).
        /// Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "deployment")]
        deployment: Vec<String>,
        /// MCP-only: filter by registry type npm|pypi|oci|… (repeatable).
        /// Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "registry-type")]
        registry_type: Vec<String>,
    },
    /// List directory entries
    List {
        /// Page number to retrieve
        #[arg(long, default_value = "1")]
        page: u32,
        /// Sort order: newest|rating|alpha
        #[arg(long, default_value = "newest")]
        sort: String,
        /// Reverse the sort order
        #[arg(long, short = 'r')]
        reverse: bool,
    },
    /// Show a random entry
    Random,
    /// View a directory entry by slug
    View {
        /// Entry slug
        slug: String,
    },
    /// Show findings for an entry
    Findings {
        /// Entry slug
        slug: String,
        /// Minimum severity: critical|high|medium|low|info
        #[arg(long, default_value = "info")]
        min_severity: String,
    },
    /// Compare two directory entries
    Compare {
        /// First entry slug
        slug_a: String,
        /// Second entry slug
        slug_b: String,
    },
    /// Download a skill's scanned source from GitHub
    ///
    /// Resolves the slug via the vettd directory, fetches the exact commit the
    /// skill was scanned against from GitHub codeload, and writes only the
    /// scanned subtree locally. Public (unauthenticated); works for public
    /// GitHub sources.
    Download {
        /// Entry slug
        slug: String,
        /// Destination directory (defaults to ./<slug>)
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

// See the note on `DirectorySubcommand` — the `Search` variant's inline
// filter surface makes it much larger than the sibling variants by design.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum InventorySubcommand {
    /// Search within your own skills
    Search {
        /// Search query (use quotes for multi-word queries)
        #[arg(required = true)]
        query: Vec<String>,
        /// Page number to retrieve
        #[arg(long, default_value = "1")]
        page: u32,
        /// Sort order: newest|rating|alpha
        #[arg(long, default_value = "newest")]
        sort: String,
        /// Reverse the sort order
        #[arg(long, short = 'r')]
        reverse: bool,
        /// Filter by implementation language (repeatable). Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "language")]
        languages: Vec<String>,
        /// Filter by agent/runtime compatibility (repeatable). Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "agent-compatibility")]
        agent_compatibility: Vec<String>,
        /// Minimum-threshold ranking filter as a JSON object, e.g.
        /// '{"stars": 50, "officialClaudeMarketplace": true}'. Requires SEARCH_BETA_TESTING=1.
        #[arg(long)]
        rankings: Option<String>,
        /// Which catalog to search: skill (default). `mcp` is rejected here —
        /// the MCP catalog is not user-scoped; use `directory search --asset-type mcp`.
        #[arg(long = "asset-type", default_value = "skill", value_parser = ["skill", "mcp"])]
        asset_type: String,
        /// Filter by discovery source, e.g. marketplace|seed|search|manual (repeatable).
        /// Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "source")]
        sources: Vec<String>,
        /// Per-source search-rank ceiling as key=N (repeatable), e.g.
        /// --rank-filter search_rank_skills_sh_rank=100. Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "rank-filter")]
        rank_filters: Vec<String>,
        /// MCP-only: filter by category server|client|framework|tooling (repeatable).
        /// Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "mcp-category")]
        mcp_category: Vec<String>,
        /// MCP-only: filter by deployment local|remote|hybrid (repeatable).
        /// Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "deployment")]
        deployment: Vec<String>,
        /// MCP-only: filter by registry type npm|pypi|oci|… (repeatable).
        /// Requires SEARCH_BETA_TESTING=1.
        #[arg(long = "registry-type")]
        registry_type: Vec<String>,
    },
    /// List the authenticated user's skills (published and unpublished)
    List {
        /// Page number to retrieve
        #[arg(long, default_value = "1")]
        page: u32,
        /// Sort order: newest|rating|alpha
        #[arg(long, default_value = "newest")]
        sort: String,
        /// Reverse the sort order
        #[arg(long, short = 'r')]
        reverse: bool,
    },
    /// View detail for one of your skills
    View {
        /// Entry slug
        slug: String,
    },
    /// Show findings for one of your skills
    Findings {
        /// Entry slug
        slug: String,
        /// Minimum rating grade: A|B|C|F (A = safest/default, F = most severe)
        #[arg(long, default_value = "A")]
        min_rating: String,
    },
    /// Side-by-side comparison of two of your skills
    Compare {
        /// First entry slug
        slug_a: String,
        /// Second entry slug
        slug_b: String,
    },
}

#[derive(clap::Args)]
pub struct OutputArgs {
    /// Full per-artifact detail output
    #[arg(long)]
    pub full: bool,
    /// Output JSON to stdout
    #[arg(long)]
    pub stdout: bool,
    /// Print compact summary only
    #[arg(long)]
    pub summary: bool,
    /// Write JSON report to file
    #[arg(long, value_name = "FILE")]
    pub out: Option<Option<PathBuf>>,
    /// Minimum severity: critical|high|medium|low|info
    #[arg(long, default_value = "info")]
    pub min_severity: String,
    /// Output JSON conforming to the scanner data contract
    #[arg(long)]
    pub contract: bool,
    /// Submit scan results to the given URL (or the configured default)
    #[arg(long, value_name = "URL")]
    pub submit: Option<Option<String>>,
    /// API key for submission (overrides config file; useful for automation)
    #[arg(long, value_name = "KEY")]
    pub api_key: Option<String>,
    /// Allow submission to public (non-local/private) endpoints
    #[arg(long)]
    pub allow_public_endpoint: bool,
    /// Force a fresh scan — do not reuse the scan cache
    #[arg(long)]
    pub no_cache: bool,
}

impl Default for OutputArgs {
    fn default() -> Self {
        Self {
            full: false,
            stdout: false,
            summary: false,
            out: None,
            min_severity: "info".to_string(),
            contract: false,
            submit: None,
            api_key: None,
            allow_public_endpoint: false,
            no_cache: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Access configuration
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct AccessConfig {
    mode: String,
    /// Opt-in to the beta search filters (`--language`, `--agent-compatibility`,
    /// `--rankings`) and the POST request shape. See [`search_beta_testing_enabled`].
    search_beta_testing: bool,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            mode: "licensed".into(),
            search_beta_testing: false,
        }
    }
}

/// Per-user access-tier config path: `~/.vettd/.vettd.toml`.
///
/// The cwd `.vettd.toml` lookup was removed (issue #198) because a scanned
/// repo must never be able to self-gate its own findings. Access-tier config
/// now lives exclusively under the per-user `~/.vettd/` root, consistent
/// with where rules, the scan cache, and other per-user state already live.
const ACCESS_CONFIG_FILE: &str = ".vettd.toml";

fn load_access_config() -> AccessConfig {
    let Some(home) = dirs::home_dir() else {
        return AccessConfig::default();
    };
    let path = home.join(".vettd").join(ACCESS_CONFIG_FILE);
    load_access_config_from(&path)
}

/// Parse a `~/.vettd/.vettd.toml`-style access config from an explicit path.
///
/// Kept separate from [`load_access_config`] so tests can point the loader at
/// an isolated fake home directory and prove that (a) the per-user config is
/// read and (b) any conflicting cwd `.vettd.toml` is never consulted.
fn load_access_config_from(path: &Path) -> AccessConfig {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return AccessConfig::default(),
    };

    let table: toml::Table = match content.parse() {
        Ok(t) => t,
        Err(_) => return AccessConfig::default(),
    };

    let access = match table.get("access") {
        Some(toml::Value::Table(t)) => t,
        _ => return AccessConfig::default(),
    };

    let mut cfg = AccessConfig::default();

    if let Some(toml::Value::String(v)) = access.get("mode") {
        cfg.mode = v.clone();
    }

    // `search_beta_testing` opts into the beta search filters and POST request
    // shape. Env var `SEARCH_BETA_TESTING` takes precedence; this is the
    // per-user config fallback. See `search_beta_testing_enabled`.
    if let Some(toml::Value::Boolean(v)) = access.get("search_beta_testing") {
        cfg.search_beta_testing = *v;
    }

    cfg
}

/// Returns the `search_beta_testing` value from the user's per-user config
/// (`~/.vettd/.vettd.toml`).
///
/// The actual decision used by the network layer is [`search_beta_testing_enabled`],
/// which ORs this value with the `SEARCH_BETA_TESTING` env var.
pub(crate) fn search_beta_testing_from_config() -> bool {
    load_access_config().search_beta_testing
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

fn min_severity_score(level: &str) -> i32 {
    match level {
        "critical" => 90,
        "high" => 70,
        "medium" => 40,
        "low" => 10,
        _ => 0,
    }
}

fn filter_by_severity(report: &mut ScanReport, min_score: i32) {
    report.artifacts.retain(|a| a.risk_score >= min_score);
}

// ---------------------------------------------------------------------------
// Scan dispatch
// ---------------------------------------------------------------------------

struct ScanParams<'a> {
    mode: &'a str,
    workdir: Option<&'a Path>,
    file: Option<&'a Path>,
    deep: bool,
}

fn resolve_scan_params(sub: &ScanSubcommand) -> ScanParams<'_> {
    match sub {
        ScanSubcommand::Default { .. } => ScanParams {
            mode: "scan",
            workdir: None,
            file: None,
            deep: false,
        },
        ScanSubcommand::Quick { .. } => ScanParams {
            mode: "host",
            workdir: None,
            file: None,
            deep: false,
        },
        ScanSubcommand::Full { .. } => ScanParams {
            mode: "root",
            workdir: None,
            file: None,
            deep: false,
        },
        ScanSubcommand::File { path, .. } => ScanParams {
            mode: "file",
            workdir: None,
            file: Some(path.as_path()),
            deep: false,
        },
        ScanSubcommand::Folder { path, deep, .. } => ScanParams {
            mode: "workdir",
            workdir: Some(path.as_path()),
            file: None,
            deep: *deep,
        },
        ScanSubcommand::Repo { path, .. } => ScanParams {
            mode: "workdir",
            workdir: Some(path.as_path()),
            file: None,
            deep: true,
        },
        ScanSubcommand::Submit { .. } => {
            unreachable!("handled before scan dispatch")
        }
    }
}

fn output_args(sub: &ScanSubcommand) -> &OutputArgs {
    match sub {
        ScanSubcommand::Default { output, .. }
        | ScanSubcommand::Quick { output, .. }
        | ScanSubcommand::Full { output, .. }
        | ScanSubcommand::File { output, .. }
        | ScanSubcommand::Folder { output, .. }
        | ScanSubcommand::Repo { output, .. } => output,
        ScanSubcommand::Submit { .. } => {
            unreachable!("handled before output dispatch")
        }
    }
}

fn command_name(sub: &ScanSubcommand) -> &'static str {
    match sub {
        ScanSubcommand::Default { .. } => "scan",
        ScanSubcommand::Quick { .. } => "quick",
        ScanSubcommand::Full { .. } => "full",
        ScanSubcommand::File { .. } => "file",
        ScanSubcommand::Folder { .. } => "folder",
        ScanSubcommand::Repo { .. } => "repo",
        ScanSubcommand::Submit { .. } => {
            unreachable!("handled before command_name")
        }
    }
}

// ---------------------------------------------------------------------------
// Access gate
// ---------------------------------------------------------------------------

/// Compute the display-limited report for human console rendering.
///
/// Returns a tuple of:
/// - The display report (limited to `LITE_MODE_VISIBLE_RESULTS` artifacts in
///   lite mode, otherwise a clone of `report`)
/// - Hidden artifacts (non-empty only in lite mode, for `print_locked_summary`)
///
/// The original `report` is never mutated — machine-output paths (`--out`,
/// `--contract`, `--submit`, `--json`, `--stdout`) always receive the full
/// un-truncated findings.
///
/// When `machine_mode` is `true`, `print_locked_summary` is suppressed so
/// that machine-readable output stays clean (no human-oriented summary on
/// stderr polluting `--stdout | jq` pipelines).
pub(crate) fn display_limited_report(
    report: &ScanReport,
    access: &AccessConfig,
    machine_mode: bool,
) -> (ScanReport, Vec<ArtifactReport>) {
    if access.mode == "lite" {
        let (limited, _hidden_count, hidden_artifacts) =
            limit_lite_mode_report(report, LITE_MODE_VISIBLE_RESULTS);
        if !hidden_artifacts.is_empty() && !machine_mode {
            print_locked_summary(&hidden_artifacts);
        }
        (limited, hidden_artifacts)
    } else {
        (report.clone(), Vec::new())
    }
}

/// Determine whether the output path is machine-readable.
///
/// Machine mode covers `--json`, `--stdout`, `--contract`, `--submit`, and
/// `--out` (file write). In machine mode the lite gate must not limit the
/// report and `print_locked_summary` must be suppressed so that stdout stays
/// clean for pipeline consumption (`--stdout | jq`, file writes, contract
/// payloads, submissions).
fn is_machine_mode(cli_json: bool, out: &OutputArgs, wants_submit: bool) -> bool {
    cli_json || out.stdout || out.contract || wants_submit || out.out.is_some()
}

// ---------------------------------------------------------------------------
// Not-yet-implemented stubs
// ---------------------------------------------------------------------------

/// Print a clear not-implemented notice to stderr and exit non-zero.
///
/// Implement `vettd auth status`.
///
/// Exit codes: 0 = configured and reachable, 3 = not configured, 5 = unreachable.
#[derive(serde::Deserialize)]
struct WhoamiUser {
    name: Option<String>,
    email: Option<String>,
    role: Option<String>,
}

#[derive(serde::Deserialize)]
struct WhoamiApiKeyInfo {
    name: Option<String>,
}

#[derive(serde::Deserialize)]
struct WhoamiResponse {
    user: WhoamiUser,
    #[serde(rename = "apiKey")]
    api_key: WhoamiApiKeyInfo,
}

#[derive(serde::Serialize)]
struct AuthStatusOutput {
    configured: bool,
    endpoint: Option<String>,
    api_key_set: bool,
    scanner_uuid: Option<String>,
    account_uuid: Option<String>,
    reachable: Option<bool>,
    account: Option<AuthAccountInfo>,
}

#[derive(serde::Serialize)]
struct AuthAccountInfo {
    name: Option<String>,
    email: Option<String>,
    role: Option<String>,
    key_name: Option<String>,
}

fn handle_auth_status(json: bool) -> i32 {
    let config = crate::submit::load_auth_config();

    let mut out = AuthStatusOutput {
        configured: config.is_some(),
        endpoint: config.as_ref().map(|c| c.endpoint.clone()),
        api_key_set: config.is_some(),
        scanner_uuid: None,
        account_uuid: None,
        reachable: None,
        account: None,
    };

    if !json {
        match &config {
            None => {
                println!("Not configured. Run `vettd auth` to set up credentials.");
            }
            Some(cfg) => {
                let host = crate::network::endpoint_display_host(&cfg.endpoint);
                println!("{:<13}  {host}", "Endpoint:");
                println!("{:<13}  set", "API key:");
            }
        }
    }

    // Scanner identity files (read-only — do not generate if absent).
    out.scanner_uuid = crate::identity::default_scanner_uuid_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    out.account_uuid = crate::identity::default_scanner_account_uuid_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if !json {
        println!(
            "{:<13}  {}",
            "Scanner UUID:",
            out.scanner_uuid.as_deref().unwrap_or("not set")
        );
        println!(
            "{:<13}  {}",
            "Account UUID:",
            out.account_uuid.as_deref().unwrap_or("not set")
        );
    }

    if config.is_none() {
        if json {
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        }
        return 3;
    }

    let cfg_inner = config.unwrap();
    let endpoint = cfg_inner.endpoint;
    let api_key = cfg_inner.api_key;

    // Reachability probe via the public contract endpoint (no auth header).
    let contract_url = format!(
        "{}?version=true",
        crate::network::derive_api_url(&endpoint, "contract")
    );
    match crate::read_client::fetch_raw(&contract_url) {
        Err(crate::read_client::ReadError::Unreachable(msg)) => {
            out.reachable = Some(false);
            if json {
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            } else {
                println!("{:<13}  unreachable ({msg})", "Reachability:");
            }
            return 5;
        }
        _ => {
            out.reachable = Some(true);
            if !json {
                println!("{:<13}  ok", "Reachability:");
            }
        }
    }

    // Whoami — authenticated GET to confirm the key is valid and fetch identity.
    let whoami_url = crate::network::derive_api_url(&endpoint, "auth/whoami");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    match agent
        .get(&whoami_url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .header("User-Agent", &crate::updater::user_agent_string())
        .call()
    {
        Ok(mut response) => {
            let status = response.status().as_u16();
            if status == 401 || status == 403 {
                if json {
                    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
                } else {
                    println!("{:<13}  API key invalid or revoked", "Identity:");
                }
                return 3;
            }
            if status == 200 {
                if let Ok(whoami) = response.body_mut().read_json::<WhoamiResponse>() {
                    out.account = Some(AuthAccountInfo {
                        name: whoami.user.name.clone(),
                        email: whoami.user.email.clone(),
                        role: whoami.user.role.clone(),
                        key_name: whoami.api_key.name.clone(),
                    });
                    if !json {
                        if let Some(name) = &whoami.user.name {
                            println!("{:<13}  {name}", "Account:");
                        }
                        if let Some(email) = &whoami.user.email {
                            println!("{:<13}  {email}", "Email:");
                        }
                        if let Some(role) = &whoami.user.role {
                            println!("{:<13}  {role}", "Role:");
                        }
                        if let Some(key_name) = &whoami.api_key.name {
                            println!("{:<13}  {key_name}", "Key name:");
                        }
                    }
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            }
            0
        }
        Err(_) => {
            // Server was reachable (confirmed above) but whoami failed at the
            // transport layer — treat as a transient error, don't change exit code.
            if json {
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            }
            0
        }
    }
}

/// Implement `vettd contract status`.
///
/// Exit codes: 0 = match, 3 = behind (server ahead), 4 = ahead (CLI forked),
/// 5 = unreachable or unparseable server version.
fn handle_contract_status(json: bool) -> i32 {
    #[derive(serde::Serialize)]
    struct ContractStatusOutput<'a> {
        local_version: &'a str,
        server_version: Option<String>,
        status: &'a str,
    }

    let endpoint = crate::submit::load_auth_config()
        .map(|c| c.endpoint)
        .unwrap_or_else(|| crate::submit::DEFAULT_PRODUCTION_ENDPOINT.to_string());

    let local = crate::contract_sync::COMPILED_CONTRACT_VERSION;

    let emit_json = |server_version: Option<String>, status: &str| {
        println!(
            "{}",
            serde_json::to_string_pretty(&ContractStatusOutput {
                local_version: local,
                server_version,
                status,
            })
            .unwrap_or_default()
        );
    };

    match crate::contract_sync::fetch_server_contract_version(&endpoint) {
        Ok(server) => match crate::semver::cmp(local, &server) {
            Some(std::cmp::Ordering::Equal) => {
                if json {
                    emit_json(Some(server), "up_to_date");
                } else {
                    println!("Contract: up to date (v{local})");
                }
                0
            }
            Some(std::cmp::Ordering::Less) => {
                if json {
                    emit_json(Some(server.clone()), "behind");
                } else {
                    println!(
                        "Contract: behind — compiled v{local}, server v{server}. \
                         Run `vettd update` to upgrade."
                    );
                }
                3
            }
            Some(std::cmp::Ordering::Greater) => {
                if json {
                    emit_json(Some(server.clone()), "ahead");
                } else {
                    println!(
                        "Contract: ahead — compiled v{local}, server v{server}. \
                         This build produces a newer contract than the server expects."
                    );
                }
                4
            }
            None => {
                if json {
                    emit_json(Some(server.clone()), "error");
                } else {
                    eprintln!(
                        "Error: could not parse server contract version '{server}' as semver."
                    );
                }
                5
            }
        },
        Err(crate::contract_sync::SyncError::Unreachable(msg)) => {
            if json {
                emit_json(None, "error");
            } else {
                eprintln!("Error: could not reach contract endpoint: {msg}");
            }
            5
        }
        Err(crate::contract_sync::SyncError::ServerError(msg)) => {
            if json {
                emit_json(None, "error");
            } else {
                eprintln!("Error: contract endpoint error: {msg}");
            }
            5
        }
    }
}

/// Exit with code 2 for commands that are scaffolded but not yet implemented.
///
/// Exit code 2 distinguishes recognized-but-unimplemented from runtime errors
/// (exit 1) and allows scripts to detect this specific state.
fn not_implemented(command: &str) -> ! {
    eprintln!("Error: `vettd {command}` is not yet implemented.");
    std::process::exit(2);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run() {
    let cli = Cli::parse();
    let json = cli.json;

    let cmd = match cli.command {
        Some(c) => c,
        None => {
            Cli::command().print_help().unwrap();
            eprintln!();
            return;
        }
    };

    // Handle rules subcommand
    if let Commands::Rules { action } = &cmd {
        match action {
            RuleAction::List => crate::rules::cmd_list(json),
            RuleAction::Add { path } => crate::rules::cmd_add(path),
            RuleAction::Remove { name } => crate::rules::cmd_remove(name),
            RuleAction::Validate { path } => crate::rules::cmd_validate(path, json),
        }
        return;
    }

    // Handle update command
    if let Commands::Update { check, force } = &cmd {
        if *check {
            match crate::updater::check_for_update(10) {
                Ok(result) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&result).unwrap_or_default()
                        );
                    } else if result.is_newer {
                        eprintln!(
                            "Update available: {} → {}",
                            result.current_version, result.latest_version
                        );
                        eprintln!("Run `vettd update` to install.");
                    } else {
                        eprintln!(
                            "You are running the latest version ({}).",
                            result.current_version
                        );
                    }
                }
                Err(e) => {
                    if json {
                        println!("{}", serde_json::json!({"error": e.to_string()}));
                    } else {
                        eprintln!("Update check failed: {e}");
                    }
                    std::process::exit(1);
                }
            }
        } else {
            if json {
                println!("{{}}");
            }
            if let Err(e) = crate::updater::perform_update(*force) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // Handle auth command
    if let Commands::Auth {
        key,
        endpoint,
        allow_public_endpoint,
        action,
    } = &cmd
    {
        if let Some(AuthSubcommand::Status) = action {
            std::process::exit(handle_auth_status(json));
        }
        let api_key = match require_auth_key(key.clone(), is_interactive()) {
            Ok(Some(value)) => value,
            Ok(None) => crate::wizard::ask_secret("API key"),
            Err(msg) => {
                eprintln!("{msg}");
                std::process::exit(2);
            }
        };
        if api_key.is_empty() {
            eprintln!("Error: API key cannot be empty.");
            std::process::exit(1);
        }

        let resolved_endpoint = endpoint
            .clone()
            .unwrap_or_else(|| DEFAULT_PRODUCTION_ENDPOINT.to_string());

        // Only enforce the public-endpoint gate when the caller supplied a
        // custom --endpoint.  The built-in production endpoint is always
        // trusted; requiring --allow-public-endpoint for the normal hosted
        // flow would be needlessly hostile.
        let is_custom_endpoint = endpoint.is_some();
        if is_custom_endpoint {
            if let Err(e) =
                crate::network::ensure_endpoint_allowed(&resolved_endpoint, *allow_public_endpoint)
            {
                eprintln!("Error: {e}");
                eprintln!("  Pass --allow-public-endpoint to permit public endpoints.");
                std::process::exit(1);
            }
        } else if let Err(e) = crate::network::ensure_endpoint_allowed(&resolved_endpoint, true) {
            // Default endpoint: still validate scheme/format, but allow public.
            eprintln!("Error: {e}");
            std::process::exit(1);
        }

        let config = AuthConfig {
            endpoint: resolved_endpoint,
            api_key,
        };
        match save_auth_config(&config) {
            Ok(outcome) => {
                eprint!("{}", auth_save_report(&outcome, &config.endpoint));
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // Handle contract command
    if let Commands::Contract { action } = &cmd {
        match action {
            ContractSubcommand::Status => std::process::exit(handle_contract_status(json)),
        }
    }

    // Handle directory commands
    if let Commands::Directory { action } = &cmd {
        match action {
            DirectorySubcommand::Search {
                query,
                page,
                sort,
                reverse,
                languages,
                agent_compatibility,
                rankings,
                asset_type,
                sources,
                rank_filters,
                mcp_category,
                deployment,
                registry_type,
            } => {
                if query.len() > 1 {
                    eprintln!(
                        "Error: use quotes for multi-word queries: vettd directory search '{}'",
                        query.join(" ")
                    );
                    std::process::exit(1);
                }
                let filters = crate::directory::SearchFilters {
                    asset_type: asset_type.clone(),
                    languages: languages.clone(),
                    agent_compatibility: agent_compatibility.clone(),
                    sources: sources.clone(),
                    rank_filters: rank_filters.clone(),
                    mcp_category: mcp_category.clone(),
                    deployment: deployment.clone(),
                    registry_type: registry_type.clone(),
                    rankings: rankings.clone(),
                };
                crate::directory::handle_search(&query[0], *page, sort, *reverse, json, &filters)
            }
            DirectorySubcommand::List {
                page,
                sort,
                reverse,
            } => crate::directory::handle_list(*page, sort, *reverse, json),
            DirectorySubcommand::Random => crate::directory::handle_random(json),
            DirectorySubcommand::View { slug } => crate::directory::handle_view(slug, json),
            DirectorySubcommand::Findings { slug, min_severity } => {
                crate::directory::handle_findings(slug, min_severity, json)
            }
            DirectorySubcommand::Compare { slug_a, slug_b } => {
                crate::directory::handle_compare(slug_a, slug_b, json)
            }
            DirectorySubcommand::Download { slug, out } => {
                crate::directory_download::handle_download(slug, out.clone(), json)
            }
        }
        return;
    }

    // Handle inventory commands
    if let Commands::Inventory { action } = &cmd {
        match action {
            InventorySubcommand::Search {
                query,
                page,
                sort,
                reverse,
                languages,
                agent_compatibility,
                rankings,
                asset_type,
                sources,
                rank_filters,
                mcp_category,
                deployment,
                registry_type,
            } => {
                if query.len() > 1 {
                    eprintln!(
                        "Error: use quotes for multi-word queries: vettd inventory search '{}'",
                        query.join(" ")
                    );
                    std::process::exit(1);
                }
                let filters = crate::directory::SearchFilters {
                    asset_type: asset_type.clone(),
                    languages: languages.clone(),
                    agent_compatibility: agent_compatibility.clone(),
                    sources: sources.clone(),
                    rank_filters: rank_filters.clone(),
                    mcp_category: mcp_category.clone(),
                    deployment: deployment.clone(),
                    registry_type: registry_type.clone(),
                    rankings: rankings.clone(),
                };
                crate::inventory::handle_search(&query[0], *page, sort, *reverse, json, &filters)
            }
            InventorySubcommand::List {
                page,
                sort,
                reverse,
            } => crate::inventory::handle_list(*page, sort, *reverse, json),
            InventorySubcommand::View { slug } => crate::inventory::handle_view(slug, json),
            InventorySubcommand::Findings { slug, min_rating } => {
                crate::inventory::handle_findings(slug, min_rating, json)
            }
            InventorySubcommand::Compare { slug_a, slug_b } => {
                crate::inventory::handle_compare(slug_a, slug_b, json)
            }
        }
        return;
    }

    // Remaining command must be Scan
    let Commands::Scan { subcommand } = cmd else {
        return;
    };

    let sub = match require_scan_subcommand(subcommand, is_interactive()) {
        Ok(Some(s)) => s,
        Ok(None) => crate::wizard::pick_scan(),
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    // Handle submit separately — reads a saved report and submits it
    if let ScanSubcommand::Submit { report } = &sub {
        handle_submit_report(report);
        return;
    }

    // Validate file/folder paths exist before scanning
    match &sub {
        ScanSubcommand::File { path, .. } => {
            if !path.exists() {
                eprintln!("Error: file not found: {}", path.display());
                std::process::exit(1);
            }
        }
        ScanSubcommand::Folder { path, .. } | ScanSubcommand::Repo { path, .. } => {
            if !path.exists() {
                eprintln!("Error: path not found: {}", path.display());
                std::process::exit(1);
            }
        }
        _ => {}
    }

    let access = load_access_config();

    let params = resolve_scan_params(&sub);
    let out = output_args(&sub);
    let min_score = min_severity_score(&out.min_severity);

    let interactive = is_interactive();
    let scan_start = std::time::Instant::now();
    let progress = if interactive {
        Some(crate::progress::ScanProgress::new(false))
    } else {
        None
    };
    // Wrap progress in a cell so the closure can borrow it
    let progress_cell = std::cell::RefCell::new(progress);
    let tick_fn = |detail: &str| {
        if let Some(ref mut p) = *progress_cell.borrow_mut() {
            p.tick(detail);
        }
    };
    if let Some(ref mut p) = *progress_cell.borrow_mut() {
        p.phase("Scanning");
    }
    let mut report = run_scan_with_cache(
        params.mode,
        params.workdir,
        params.file,
        params.deep,
        out.no_cache,
        if interactive { Some(&tick_fn) } else { None },
    );
    let scan_duration_ms = scan_start.elapsed().as_millis() as u64;
    if let Some(ref mut p) = *progress_cell.borrow_mut() {
        p.done(Some(&format!(
            "Found {} artifact(s)",
            report.artifacts.len()
        )));
    }

    filter_by_severity(&mut report, min_score);

    // Machine-output paths (contract, JSON, file, submit) always receive the
    // full un-truncated report. The lite gate is display-only: the
    // display-limited report is used only for human console rendering.
    let wants_submit = out.submit.is_some();
    let machine_mode = is_machine_mode(json, out, wants_submit);
    let (display_report, _hidden_artifacts) =
        display_limited_report(&report, &access, machine_mode);

    if machine_mode {
        // Any machine flag (`--json`, `--stdout`, `--contract`, `--out`,
        // `--submit`) routes the FULL report through the contract payload,
        // not the display-limited report. `--out <file>` must therefore carry
        // all findings (issue #198 AC #2), and `--stdout`/`--json`/`--contract`
        // must emit jq-parseable JSON to stdout.
        //
        // For `--submit`, configured auth is resolved FIRST — that establishes
        // standing consent (issue #196) — before `build_contract_payload` reads
        // logs/host-network state and user files. No logs or user files are
        // read before consent is established.
        let submit_auth = if wants_submit {
            Some(
                match resolve_submit_auth(
                    &out.submit,
                    out.api_key.as_deref(),
                    out.allow_public_endpoint,
                ) {
                    Ok(auth) => auth,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                },
            )
        } else {
            None
        };

        let payload = build_contract_payload(&report, scan_duration_ms);
        let contract_json = match serde_json::to_string_pretty(&payload) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("Error serializing contract payload: {e}");
                std::process::exit(1);
            }
        };
        let plan = plan_machine_output(out, wants_submit, json);

        // Print contract JSON to stdout for --stdout/--json/--contract (never
        // when submitting — stdout stays clean for pipelines).
        if plan.to_stdout {
            println!("{contract_json}");
        }

        // Write to file if --out is specified, or always when submitting.
        if let Some(dest) = plan.write_path {
            if let Err(e) = fs::write(&dest, &contract_json) {
                eprintln!("Error writing contract to {}: {}", dest.display(), e);
            } else {
                eprintln!("Contract written to {}", dest.display());
            }
        }

        if let Some(auth) = submit_auth {
            // Show the disclosure to stderr before submitting — the user
            // needs to see what data is being transmitted regardless of
            // whether the flow is interactive. Consent is implicit (configured
            // auth = standing consent); only the interactive path blocks.
            print_submit_disclosure(&payload);
            if let Err(e) = do_submit(&contract_json, &auth) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    } else {
        // Human console rendering: use the display-limited report so that
        // lite-mode users see only the top N artifacts on the terminal.
        //
        // In this branch `machine_mode` is false, so `json`, `out.stdout`,
        // `out.contract`, `wants_submit`, and `out.out` are all false/None.
        let cmd_name = command_name(&sub);
        emit(
            &display_report,
            scan_duration_ms,
            false,
            &None,
            out.summary,
            out.full,
            cmd_name,
        );
    }

    // Offer interactive follow-up actions for local-only scans.
    if !machine_mode && is_interactive() {
        let cmd_name = command_name(&sub);
        prompt_post_scan_action(&report, scan_duration_ms, cmd_name);
    }
}

/// Where machine-readable scan output should go.
///
/// Machine output is the full contract payload rendered as JSON. Which
/// destinations receive it depends on the flags: `--stdout`, `--json`, and
/// `--contract` print it to stdout; `--out <file>` (or submitting) writes it
/// to a file. Submission transmits the same full payload.
#[derive(Debug)]
struct MachineOutputPlan {
    /// Whether the contract JSON should be printed to stdout.
    to_stdout: bool,
    /// Destination path to write the contract JSON to, if any.
    write_path: Option<PathBuf>,
}

/// Decide stdout/file destinations for the machine-output branch.
///
/// Kept as a pure function so the routing logic is unit-testable: `--stdout`,
/// `--json`, and `--contract` print the full contract JSON to stdout (unless
/// submitting, which keeps stdout clean), and `--out`/submission write it to a
/// file with a default filename when `--out` is bare.
fn plan_machine_output(out: &OutputArgs, wants_submit: bool, cli_json: bool) -> MachineOutputPlan {
    let to_stdout = (out.stdout || out.contract || cli_json) && !wants_submit;
    let write_path = match &out.out {
        Some(maybe) => Some(match maybe {
            Some(p) => p.clone(),
            None => PathBuf::from("vettd-contract.json"),
        }),
        None if wants_submit => Some(PathBuf::from("vettd-contract.json")),
        None => None,
    };
    MachineOutputPlan {
        to_stdout,
        write_path,
    }
}

// ---------------------------------------------------------------------------
// Submit saved report
// ---------------------------------------------------------------------------

fn handle_submit_report(report: &Path) {
    let json = match fs::read_to_string(report) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error reading {}: {e}", report.display());
            std::process::exit(1);
        }
    };

    // Resolve auth FIRST — configured auth is standing consent (issue #196).
    // Only after consent is established may the report be transmitted.
    let auth = match resolve_submit_auth(&Some(None), None, false) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Fail closed (issue #196 finding #6): never transmit raw undisclosed JSON.
    // If the saved report cannot be parsed into the supported contract, refuse
    // to submit rather than silently shipping unmapped data.
    let payload: ContractPayload = match serde_json::from_str(&json) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "Error: {} is not a valid vettd contract report: {e}",
                report.display()
            );
            eprintln!(
                "Refusing to submit undisclosed JSON. Re-scan with --contract/--out and submit the contract output instead."
            );
            std::process::exit(1);
        }
    };

    // Show the disclosure to stderr before submitting — the user must see what
    // data is being transmitted. This runs on every submit-report success path.
    print_submit_disclosure(&payload);

    if let Err(e) = do_submit(&json, &auth) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Interactive post-scan actions
// ---------------------------------------------------------------------------

const VETTD_SETTINGS_URL: &str = "https://vettd.agentichighway.ai/settings";

enum PostScanAction {
    SaveReport,
    SubmitToVettd,
    DoNothing,
}

fn is_interactive() -> bool {
    io::stdin().is_terminal()
}

/// Resolve the scan subcommand, failing fast in non-interactive mode.
///
/// - An explicit subcommand always passes through.
/// - With no subcommand on a TTY, returns `Ok(None)` so the caller can show
///   the interactive scan picker.
/// - With no subcommand and no TTY, returns guidance instead of silently
///   running a default scan, so automation never hangs or guesses (issue #145).
fn require_scan_subcommand(
    subcommand: Option<ScanSubcommand>,
    interactive: bool,
) -> Result<Option<ScanSubcommand>, String> {
    match subcommand {
        Some(sub) => Ok(Some(sub)),
        None if interactive => Ok(None),
        None => Err("Error: no scan subcommand given. In non-interactive mode, \
             run a scan subcommand, e.g. `vettd scan quick` \
             (or full, default, folder <path>, file <path>)."
            .to_string()),
    }
}

/// Resolve the `vettd auth` API key, failing fast in non-interactive mode.
///
/// - An explicit `--key` always passes through.
/// - With no key on a TTY, returns `Ok(None)` so the caller can prompt securely.
/// - With no key and no TTY, returns guidance instead of prompting, so
///   automation never hangs waiting for input (issue #145).
fn require_auth_key(key: Option<String>, interactive: bool) -> Result<Option<String>, String> {
    match key {
        Some(value) => Ok(Some(value)),
        None if interactive => Ok(None),
        None => Err(
            "Error: no API key given. In non-interactive mode, pass it explicitly: \
             `vettd auth --key <key>`."
                .to_string(),
        ),
    }
}

/// Build the stderr report for a `vettd auth` credential save.
///
/// The user needs the on-disk config path to confirm setup completed and to
/// locate the file for debugging; when nothing was written (already up to
/// date) that state is stated explicitly instead of implying a write
/// (issue #126).
fn auth_save_report(outcome: &SaveOutcome, endpoint: &str) -> String {
    let status = if outcome.written {
        "Credentials saved."
    } else {
        "Credentials already up to date — nothing written."
    };
    format!(
        "{status}\n  Config:   {}\n  Endpoint: {endpoint}\n",
        outcome.path.display()
    )
}

fn prompt_post_scan_action(report: &ScanReport, scan_duration_ms: u64, cmd_name: &str) {
    let saved = crate::submit::load_auth_config();
    let endpoint = saved
        .as_ref()
        .map(|a| a.endpoint.as_str())
        .unwrap_or(DEFAULT_PRODUCTION_ENDPOINT);
    let submit_host = crate::network::endpoint_display_host(endpoint);
    let submit_label = format!("Submit results to {submit_host}");

    let options = ["Write report to disk", submit_label.as_str(), "Do nothing"];

    let action = match crate::wizard::pick("Next step", &options, 2) {
        0 => PostScanAction::SaveReport,
        1 => PostScanAction::SubmitToVettd,
        _ => PostScanAction::DoNothing,
    };

    match action {
        PostScanAction::SaveReport => save_report_interactively(report, scan_duration_ms),
        PostScanAction::SubmitToVettd => prompt_submit(report, scan_duration_ms, cmd_name),
        PostScanAction::DoNothing => {}
    }
}

fn save_report_interactively(report: &ScanReport, scan_duration_ms: u64) {
    let path = crate::wizard::ask("Report path", "vettd-report.json");
    let maybe_path = Some(PathBuf::from(path));
    crate::output::write_json_report(report, scan_duration_ms, &maybe_path);
}

/// Count the non-MCP artifact records (prompts, skills, agents, agentic apps)
/// that a payload will actually transmit.
///
/// This counts the real contract records — not `report.artifacts` — so that
/// artifact types dropped during partitioning (e.g. `browser_footprint`) are
/// not disclosed as transmitted. MCP servers are disclosed on their own line.
fn disclosure_artifact_record_count(payload: &ContractPayload) -> usize {
    payload.prompts.len() + payload.skills.len() + payload.agents.len() + payload.agentic_apps.len()
}

/// Print a concise summary of the data categories included in a submission.
///
/// Called in interactive flows immediately before asking for consent. The
/// summary is data-driven — it lists only the categories actually present in
/// the payload, with one line per category. Rendered to stderr.
fn print_submit_disclosure(payload: &ContractPayload) {
    eprint!("{}", render_disclosure(payload));
}

/// After a scan, ask the user if they want to submit results.
fn prompt_submit(report: &ScanReport, scan_duration_ms: u64, cmd_name: &str) {
    // Resolve or collect API key
    let saved = crate::submit::load_auth_config();
    let api_key = match saved.as_ref().filter(|a| !a.api_key.is_empty()) {
        Some(auth) => {
            eprintln!("  Using saved API key.");
            auth.api_key.clone()
        }
        None => collect_api_key(),
    };

    if api_key.is_empty() {
        eprintln!("  No API key provided — submission cancelled.");
        return;
    }

    let endpoint = saved
        .map(|a| a.endpoint)
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| DEFAULT_PRODUCTION_ENDPOINT.to_string());

    // Always show the actual destination before submitting.
    eprintln!(
        "  Destination: {}",
        crate::network::endpoint_display_host(&endpoint)
    );

    // Build the disclosure payload BEFORE consent with NO side effects on user
    // files or host state: `build_contract_payload_for_disclosure` skips
    // reading application logs and running host-network subprocesses (issue
    // #196 AC #3). No log files or other user files may be read before the
    // user consents, so the disclosure is generated from a side-effect-free
    // payload. The disclosure still names the LogDerivedNetworkEvidence
    // category whenever MCP servers are present (disclosure.rs decides that
    // structurally, from `!mcp_servers.is_empty()`, not from having read logs).
    let disclosure_payload = build_contract_payload_for_disclosure(report, scan_duration_ms);

    // Show a concise data-disclosure summary, then ask for consent.
    print_submit_disclosure(&disclosure_payload);
    let confirmed = crate::wizard::confirm("Send this data?", false);
    if !confirmed {
        eprintln!("  Submission cancelled.");
        return;
    }

    // Only after the user consents may we read logs / host network state and
    // build the real payload to transmit.
    let payload = build_contract_payload(report, scan_duration_ms);
    let json = match serde_json::to_string_pretty(&payload) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("  Error serializing payload: {e}");
            return;
        }
    };

    let auth = AuthConfig {
        endpoint: endpoint.clone(),
        api_key: api_key.clone(),
    };

    match do_submit(&json, &auth) {
        Ok(()) => {
            let _ = crate::submit::save_auth_config(&auth);
        }
        Err(e) => {
            eprintln!("  {e}");
            eprintln!("  You can retry later with: \x1b[1mvettd scan {cmd_name} --submit\x1b[0m");
        }
    }
}

/// Guide the user through obtaining and entering an API key.
fn collect_api_key() -> String {
    eprintln!();
    eprintln!("  You can get an API key from \x1b[36m{VETTD_SETTINGS_URL}\x1b[0m");

    // Quick reachability check
    match ureq::get(VETTD_SETTINGS_URL)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build()
        .call()
    {
        Ok(_) => {
            eprintln!("  \x1b[32m✓\x1b[0m Vettd is reachable.");
        }
        Err(_) => {
            eprintln!(
                "  \x1b[33m!\x1b[0m Could not reach Vettd — check your connection and try again later."
            );
            return String::new();
        }
    }

    eprintln!();
    let key = crate::wizard::ask_secret("Paste your API key");
    if key.is_empty() {
        return String::new();
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{build_contract_payload_for_disclosure, disclosure_categories};
    use clap::Parser;

    #[test]
    fn print_submit_disclosure_runs_without_panic() {
        // Smoke-test: disclosure function must not panic for empty or non-empty payloads.
        let empty = build_contract_payload(&ScanReport::new("/test"), 0);
        print_submit_disclosure(&empty);

        let mut with_artifacts = ScanReport::new("/test");
        with_artifacts
            .artifacts
            .push(crate::models::ArtifactReport::new("mcp_config", 0.9));
        with_artifacts
            .artifacts
            .push(crate::models::ArtifactReport::new("prompt_config", 0.5));
        let payload = build_contract_payload(&with_artifacts, 0);
        print_submit_disclosure(&payload);
    }

    #[test]
    fn disclosure_count_excludes_dropped_artifact_types() {
        // Issue #156: the disclosed count must reflect the real payload, not
        // `report.artifacts.len()`. An unrecognized type (browser_footprint) is
        // dropped during partitioning and must not be counted, while a real
        // prompt_config artifact must be.
        let mut report = ScanReport::new("/test");
        report
            .artifacts
            .push(crate::models::ArtifactReport::new("prompt_config", 0.5));
        report
            .artifacts
            .push(crate::models::ArtifactReport::new("browser_footprint", 0.5));

        assert_eq!(report.artifacts.len(), 2, "two raw artifacts scanned");

        let payload = build_contract_payload(&report, 0);
        assert_eq!(
            disclosure_artifact_record_count(&payload),
            1,
            "only the prompt_config maps to a transmitted record"
        );
    }

    #[test]
    fn min_severity_score_critical() {
        assert_eq!(min_severity_score("critical"), 90);
    }

    #[test]
    fn min_severity_score_high() {
        assert_eq!(min_severity_score("high"), 70);
    }

    #[test]
    fn min_severity_score_medium() {
        assert_eq!(min_severity_score("medium"), 40);
    }

    #[test]
    fn min_severity_score_low() {
        assert_eq!(min_severity_score("low"), 10);
    }

    // ── #145: interactive prompts must have a non-interactive fail-fast path ──

    #[test]
    fn require_scan_subcommand_passes_explicit_through() {
        // An explicit subcommand is honored regardless of TTY state.
        let sub = ScanSubcommand::Quick {
            output: OutputArgs::default(),
        };
        let resolved = require_scan_subcommand(Some(sub), false);
        assert!(matches!(resolved, Ok(Some(ScanSubcommand::Quick { .. }))));
    }

    #[test]
    fn require_scan_subcommand_prompts_only_on_a_tty() {
        // On a TTY with no subcommand, the caller should fall back to the picker.
        assert!(matches!(require_scan_subcommand(None, true), Ok(None)));
    }

    #[test]
    fn require_scan_subcommand_non_interactive_errors_with_guidance() {
        // Without a TTY and without a subcommand, automation must get an error
        // (not a silent default scan and not a hang). The message must name a
        // concrete subcommand so the caller knows the flag equivalent.
        // (`ScanSubcommand` isn't `Debug`, so match rather than `unwrap_err`.)
        match require_scan_subcommand(None, false) {
            Err(err) => assert!(err.contains("vettd scan quick"), "guidance was: {err}"),
            Ok(_) => panic!("expected an error with no subcommand and no TTY"),
        }
    }

    #[test]
    fn require_auth_key_passes_explicit_through() {
        let resolved = require_auth_key(Some("secret".to_string()), false);
        assert_eq!(resolved, Ok(Some("secret".to_string())));
    }

    #[test]
    fn require_auth_key_prompts_only_on_a_tty() {
        assert_eq!(require_auth_key(None, true), Ok(None));
    }

    #[test]
    fn require_auth_key_non_interactive_errors_with_guidance() {
        // Without a TTY and without --key, automation must get actionable
        // guidance naming the flag, not a hanging secret prompt.
        let err = require_auth_key(None, false).unwrap_err();
        assert!(err.contains("--key"), "guidance was: {err}");
    }

    // ── #126: auth must tell the user where the config file landed ──

    #[test]
    fn auth_save_report_names_the_config_path() {
        // "Credentials saved." alone doesn't help a user confirm setup or
        // find the file later — the on-disk path is the actionable part.
        let outcome = SaveOutcome {
            path: PathBuf::from("/home/u/.config/vettd/config.json"),
            written: true,
        };
        let report = auth_save_report(&outcome, "https://example.com/api");
        assert!(report.contains("Credentials saved."));
        assert!(
            report.contains("/home/u/.config/vettd/config.json"),
            "report must name the written file: {report}"
        );
        assert!(report.contains("https://example.com/api"));
    }

    #[test]
    fn auth_save_report_unchanged_is_explicit_about_no_write() {
        // When the config was already up to date the user must be told no
        // file was written — while still being pointed at the existing
        // config so they can locate it.
        let outcome = SaveOutcome {
            path: PathBuf::from("/home/u/.config/vettd/config.json"),
            written: false,
        };
        let report = auth_save_report(&outcome, "https://example.com/api");
        assert!(
            report.contains("nothing written"),
            "unchanged state must be explicit: {report}"
        );
        assert!(!report.contains("Credentials saved."));
        assert!(
            report.contains("/home/u/.config/vettd/config.json"),
            "unchanged report must still name the config: {report}"
        );
    }

    #[test]
    fn min_severity_score_info_default() {
        assert_eq!(min_severity_score("info"), 0);
        assert_eq!(min_severity_score("anything"), 0);
    }

    #[test]
    fn filter_by_severity_removes_below_threshold() {
        let mut report = ScanReport::new("/tmp");
        let mut a1 = crate::models::ArtifactReport::new("prompt_config", 0.8);
        a1.risk_score = 80;
        let mut a2 = crate::models::ArtifactReport::new("prompt_config", 0.8);
        a2.risk_score = 30;
        let mut a3 = crate::models::ArtifactReport::new("prompt_config", 0.8);
        a3.risk_score = 50;
        report.artifacts = vec![a1, a2, a3];

        filter_by_severity(&mut report, 40);
        assert_eq!(report.artifacts.len(), 2);
        assert!(report.artifacts.iter().all(|a| a.risk_score >= 40));
    }

    #[test]
    fn filter_by_severity_zero_keeps_all() {
        let mut report = ScanReport::new("/tmp");
        let mut a = crate::models::ArtifactReport::new("prompt_config", 0.8);
        a.risk_score = 5;
        report.artifacts = vec![a];

        filter_by_severity(&mut report, 0);
        assert_eq!(report.artifacts.len(), 1);
    }

    #[test]
    fn parse_cli_scan_no_subcommand() {
        let cli = Cli::parse_from(["vettd", "scan"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Scan { subcommand: None })
        ));
    }

    #[test]
    fn parse_cli_scan_quick() {
        let cli = Cli::parse_from(["vettd", "scan", "quick"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { .. })
            })
        ));
    }

    #[test]
    fn parse_cli_scan_full() {
        let cli = Cli::parse_from(["vettd", "scan", "full"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Full { .. })
            })
        ));
    }

    #[test]
    fn parse_cli_scan_file() {
        let cli = Cli::parse_from(["vettd", "scan", "file", "/tmp/test.md"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::File { path, .. }),
            }) => {
                assert_eq!(path, PathBuf::from("/tmp/test.md"));
            }
            _ => panic!("Expected scan file command"),
        }
    }

    #[test]
    fn parse_cli_scan_folder() {
        let cli = Cli::parse_from(["vettd", "scan", "folder", "/tmp"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Folder { path, .. }),
            }) => {
                assert_eq!(path, PathBuf::from("/tmp"));
            }
            _ => panic!("Expected scan folder command"),
        }
    }

    #[test]
    fn parse_cli_scan_repo() {
        let cli = Cli::parse_from(["vettd", "scan", "repo", "."]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Repo { path, .. }),
            }) => {
                assert_eq!(path, PathBuf::from("."));
            }
            _ => panic!("Expected scan repo command"),
        }
    }

    #[test]
    fn parse_cli_scan_submit() {
        let cli = Cli::parse_from(["vettd", "scan", "submit", "report.json"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Submit { report }),
            }) => {
                assert_eq!(report, PathBuf::from("report.json"));
            }
            _ => panic!("Expected scan submit command"),
        }
    }

    #[test]
    fn parse_cli_auth() {
        let cli = Cli::parse_from(["vettd", "auth", "--key", "ah_test123"]);
        match cli.command {
            Some(Commands::Auth {
                key,
                endpoint,
                allow_public_endpoint,
                action,
            }) => {
                assert_eq!(key.as_deref(), Some("ah_test123"));
                assert!(endpoint.is_none());
                assert!(!allow_public_endpoint);
                // Bare connect flow: no subcommand routes to credential save.
                assert!(action.is_none());
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_cli_auth_with_endpoint() {
        let cli = Cli::parse_from([
            "vettd",
            "auth",
            "--key",
            "ah_test",
            "--endpoint",
            "https://example.com/api",
        ]);
        match cli.command {
            Some(Commands::Auth {
                key,
                endpoint,
                allow_public_endpoint,
                action,
            }) => {
                assert_eq!(key.as_deref(), Some("ah_test"));
                assert_eq!(endpoint.unwrap(), "https://example.com/api");
                assert!(!allow_public_endpoint);
                assert!(action.is_none());
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_cli_auth_with_allow_public_endpoint() {
        let cli = Cli::parse_from([
            "vettd",
            "auth",
            "--key",
            "ah_test",
            "--endpoint",
            "https://example.com/api",
            "--allow-public-endpoint",
        ]);
        match cli.command {
            Some(Commands::Auth {
                key,
                endpoint,
                allow_public_endpoint,
                action,
            }) => {
                assert_eq!(key.as_deref(), Some("ah_test"));
                assert_eq!(endpoint.as_deref(), Some("https://example.com/api"));
                assert!(allow_public_endpoint);
                assert!(action.is_none());
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_cli_auth_without_key() {
        let cli = Cli::parse_from(["vettd", "auth"]);
        match cli.command {
            Some(Commands::Auth {
                key,
                endpoint,
                allow_public_endpoint,
                action,
            }) => {
                assert!(key.is_none());
                assert!(endpoint.is_none());
                assert!(!allow_public_endpoint);
                assert!(action.is_none());
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_cli_auth_status() {
        let cli = Cli::parse_from(["vettd", "auth", "status"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth {
                action: Some(AuthSubcommand::Status),
                ..
            })
        ));
    }

    #[test]
    fn parse_cli_auth_key_and_status() {
        // Parent flags must precede the subcommand token; both must parse.
        let cli = Cli::parse_from(["vettd", "auth", "--key", "K", "status"]);
        match cli.command {
            Some(Commands::Auth { key, action, .. }) => {
                assert_eq!(key.as_deref(), Some("K"));
                assert!(matches!(action, Some(AuthSubcommand::Status)));
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_cli_contract_status() {
        let cli = Cli::parse_from(["vettd", "contract", "status"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Contract {
                action: ContractSubcommand::Status
            })
        ));
    }

    #[test]
    fn parse_cli_directory_search() {
        let cli = Cli::parse_from(["vettd", "directory", "search", "foo"]);
        match cli.command {
            Some(Commands::Directory {
                action:
                    DirectorySubcommand::Search {
                        query, page, sort, ..
                    },
            }) => {
                assert_eq!(query, vec!["foo"]);
                assert_eq!(page, 1);
                assert_eq!(sort, "newest");
            }
            _ => panic!("Expected directory search command"),
        }
    }

    #[test]
    fn parse_cli_directory_search_page() {
        let cli = Cli::parse_from(["vettd", "directory", "search", "foo", "--page", "3"]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::Search { query, page, .. },
            }) => {
                assert_eq!(query, vec!["foo"]);
                assert_eq!(page, 3);
            }
            _ => panic!("Expected directory search command"),
        }
    }

    #[test]
    fn parse_cli_directory_search_sort() {
        let cli = Cli::parse_from(["vettd", "directory", "search", "foo", "--sort", "rating"]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::Search { sort, .. },
            }) => assert_eq!(sort, "rating"),
            _ => panic!("Expected directory search command"),
        }
    }

    #[test]
    fn parse_cli_directory_list() {
        let cli = Cli::parse_from(["vettd", "directory", "list"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Directory {
                action: DirectorySubcommand::List { page: 1, .. }
            })
        ));
    }

    #[test]
    fn parse_cli_directory_list_page() {
        let cli = Cli::parse_from(["vettd", "directory", "list", "--page", "2"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Directory {
                action: DirectorySubcommand::List { page: 2, .. }
            })
        ));
    }

    #[test]
    fn parse_cli_directory_list_sort() {
        let cli = Cli::parse_from(["vettd", "directory", "list", "--sort", "alpha"]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::List { sort, .. },
            }) => assert_eq!(sort, "alpha"),
            _ => panic!("Expected directory list command"),
        }
    }

    #[test]
    fn parse_cli_directory_list_reverse() {
        let cli = Cli::parse_from(["vettd", "directory", "list", "--reverse"]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::List { reverse, .. },
            }) => assert!(reverse),
            _ => panic!("Expected directory list command"),
        }
    }

    #[test]
    fn parse_cli_directory_search_reverse() {
        let cli = Cli::parse_from(["vettd", "directory", "search", "foo", "--reverse"]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::Search { reverse, .. },
            }) => assert!(reverse),
            _ => panic!("Expected directory search command"),
        }
    }

    #[test]
    fn parse_cli_directory_random() {
        let cli = Cli::parse_from(["vettd", "directory", "random"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Directory {
                action: DirectorySubcommand::Random
            })
        ));
    }

    #[test]
    fn parse_cli_directory_view() {
        let cli = Cli::parse_from(["vettd", "directory", "view", "alpha"]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::View { slug },
            }) => assert_eq!(slug, "alpha"),
            _ => panic!("Expected directory view command"),
        }
    }

    #[test]
    fn parse_cli_directory_findings_default_severity() {
        let cli = Cli::parse_from(["vettd", "directory", "findings", "alpha"]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::Findings { slug, min_severity },
            }) => {
                assert_eq!(slug, "alpha");
                assert_eq!(min_severity, "info");
            }
            _ => panic!("Expected directory findings command"),
        }
    }

    #[test]
    fn parse_cli_directory_findings_min_severity() {
        let cli = Cli::parse_from([
            "vettd",
            "directory",
            "findings",
            "alpha",
            "--min-severity",
            "high",
        ]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::Findings { slug, min_severity },
            }) => {
                assert_eq!(slug, "alpha");
                assert_eq!(min_severity, "high");
            }
            _ => panic!("Expected directory findings command"),
        }
    }

    #[test]
    fn parse_cli_directory_compare() {
        let cli = Cli::parse_from(["vettd", "directory", "compare", "a", "b"]);
        match cli.command {
            Some(Commands::Directory {
                action: DirectorySubcommand::Compare { slug_a, slug_b },
            }) => {
                // Positional order: first token -> slug_a, second -> slug_b.
                assert_eq!(slug_a, "a");
                assert_eq!(slug_b, "b");
            }
            _ => panic!("Expected directory compare command"),
        }
    }

    #[test]
    fn parse_cli_directory_search_requires_query() {
        assert!(Cli::try_parse_from(["vettd", "directory", "search"]).is_err());
    }

    #[test]
    fn parse_cli_directory_compare_requires_two_slugs() {
        assert!(Cli::try_parse_from(["vettd", "directory", "compare", "a"]).is_err());
    }

    #[test]
    fn parse_cli_policy_not_registered() {
        // policy is out of scope for #149 (deferred to vettd#631).
        assert!(Cli::try_parse_from(["vettd", "policy"]).is_err());
    }

    #[test]
    fn parse_cli_open_not_registered() {
        // open is out of scope for #149 (deferred).
        assert!(Cli::try_parse_from(["vettd", "open"]).is_err());
    }

    #[test]
    fn parse_cli_allow_public_endpoint_in_scan() {
        let cli = Cli::parse_from([
            "vettd",
            "scan",
            "quick",
            "--submit",
            "--allow-public-endpoint",
        ]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { output, .. }),
            }) => {
                assert!(output.allow_public_endpoint);
            }
            _ => panic!("Expected scan quick command"),
        }
    }

    #[test]
    fn parse_cli_allow_public_endpoint_defaults_false() {
        let cli = Cli::parse_from(["vettd", "scan", "quick"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { output, .. }),
            }) => {
                assert!(!output.allow_public_endpoint);
            }
            _ => panic!("Expected scan quick command"),
        }
    }

    #[test]
    fn parse_cli_update_check() {
        let cli = Cli::parse_from(["vettd", "update", "--check"]);
        match cli.command {
            Some(Commands::Update { check, force }) => {
                assert!(check);
                assert!(!force);
            }
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn parse_cli_rules_list() {
        let cli = Cli::parse_from(["vettd", "rules", "list"]);
        match cli.command {
            Some(Commands::Rules {
                action: RuleAction::List,
            }) => {}
            _ => panic!("Expected Rules List"),
        }
    }

    #[test]
    fn parse_cli_output_args_json() {
        let cli = Cli::parse_from(["vettd", "scan", "quick", "--stdout"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { output, .. }),
            }) => {
                assert!(output.stdout);
                assert!(!output.summary);
                assert!(!output.full);
            }
            _ => panic!("Expected scan quick command"),
        }
    }

    #[test]
    fn parse_cli_output_args_summary() {
        let cli = Cli::parse_from(["vettd", "scan", "quick", "--summary"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { output, .. }),
            }) => {
                assert!(output.summary);
            }
            _ => panic!("Expected scan quick command"),
        }
    }

    #[test]
    fn parse_cli_output_args_min_severity() {
        let cli = Cli::parse_from(["vettd", "scan", "quick", "--min-severity", "high"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { output, .. }),
            }) => {
                assert_eq!(output.min_severity, "high");
            }
            _ => panic!("Expected scan quick command"),
        }
    }

    #[test]
    fn parse_cli_no_cache_flag() {
        // Issue #199: --no-cache must flip the no_cache field on OutputArgs.
        let cli = Cli::parse_from(["vettd", "scan", "quick", "--no-cache"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { output, .. }),
            }) => {
                assert!(output.no_cache, "--no-cache must set no_cache: true");
            }
            _ => panic!("Expected scan quick command"),
        }
    }

    #[test]
    fn parse_cli_no_cache_flag_folder() {
        // Issue #199 clarification: the user runs `scan folder`, which maps to
        // workdir mode — a mode where `cache_enabled_for_mode` returns true (so
        // the vettd cache IS normally on). `--no-cache` must therefore flip
        // `no_cache` on the Folder variant too, else it would be a silent no-op
        // for the exact command people use. Verified at runtime: with
        // `--no-cache` the `cache_hits=` timing line never appears, confirming
        // the cache guard is fully skipped.
        let cli = Cli::parse_from(["vettd", "scan", "folder", ".", "--no-cache"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Folder { output, .. }),
            }) => {
                assert!(
                    output.no_cache,
                    "--no-cache must set no_cache: true on the folder (workdir) variant"
                );
            }
            _ => panic!("Expected scan folder command"),
        }
    }

    #[test]
    fn parse_cli_no_cache_defaults_false() {
        let cli = Cli::parse_from(["vettd", "scan", "quick"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { output, .. }),
            }) => {
                assert!(!output.no_cache, "no-cache defaults to false");
            }
            _ => panic!("Expected scan quick command"),
        }
    }

    #[test]
    fn parse_cli_no_command() {
        let cli = Cli::parse_from(["vettd"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn resolve_scan_params_default() {
        let sub = ScanSubcommand::Default {
            output: OutputArgs::default(),
        };
        let params = resolve_scan_params(&sub);
        assert_eq!(params.mode, "scan");
        assert!(params.workdir.is_none());
        assert!(!params.deep);
    }

    #[test]
    fn resolve_scan_params_quick() {
        let sub = ScanSubcommand::Quick {
            output: OutputArgs::default(),
        };
        let params = resolve_scan_params(&sub);
        assert_eq!(params.mode, "host");
    }

    #[test]
    fn resolve_scan_params_repo_deep() {
        let sub = ScanSubcommand::Repo {
            path: PathBuf::from("/tmp/repo"),
            output: OutputArgs::default(),
        };
        let params = resolve_scan_params(&sub);
        assert_eq!(params.mode, "workdir");
        assert!(params.deep);
        assert_eq!(params.workdir.unwrap(), Path::new("/tmp/repo"));
    }

    #[test]
    fn resolve_scan_params_file() {
        let sub = ScanSubcommand::File {
            path: PathBuf::from("/tmp/test.md"),
            output: OutputArgs::default(),
        };
        let params = resolve_scan_params(&sub);
        assert_eq!(params.mode, "file");
        assert_eq!(params.file.unwrap(), Path::new("/tmp/test.md"));
    }

    #[test]
    fn load_access_config_defaults_when_no_file() {
        // When no `~/.vettd/.vettd.toml` exists, we fall back to the
        // default (licensed) access tier. License-related fields were
        // removed in #198 — only `mode` remains.
        // Loaded via `load_access_config_from` against a nonexistent path so
        // the test is hermetic — calling the real `load_access_config()`
        // would read whatever `~/.vettd/.vettd.toml` exists on the dev box
        // and fail in non-default environments.
        let missing = tempfile::tempdir()
            .unwrap()
            .path()
            .join(".vettd/.vettd.toml");
        let cfg = load_access_config_from(&missing);
        assert_eq!(cfg.mode, "licensed");
    }

    // ── #198: lite-mode access gate (display-only, per-user config) ──

    #[test]
    fn display_limited_report_in_lite_mode_limits_display_but_preserves_full() {
        // AC #1: machine-output paths (contract, --out, --submit, --json,
        // --stdout) must receive ALL findings, not just the top 3. The
        // lite gate is display-only.
        let mut report = ScanReport::new("/test");
        for i in 0..5 {
            let mut artifact = crate::models::ArtifactReport::new("test_artifact", 0.5);
            artifact.risk_score = i * 20 + 10;
            report.artifacts.push(artifact);
        }

        let access = AccessConfig {
            mode: "lite".to_string(),
            search_beta_testing: false,
        };

        // Machine mode: hidden summary suppressed, but the full report is
        // unchanged (caller decides what to do with it).
        let (display_report, hidden) = display_limited_report(&report, &access, true);
        assert_eq!(
            display_report.artifacts.len(),
            LITE_MODE_VISIBLE_RESULTS,
            "display report must be limited to top 3 in lite mode"
        );
        assert_eq!(hidden.len(), 2, "two artifacts must be hidden");

        // Non-machine mode: same limiting, but `print_locked_summary` is
        // called internally (we can't easily assert stderr here, but we
        // verify the hidden artifacts are returned for the caller to use).
        let (display_report2, hidden2) = display_limited_report(&report, &access, false);
        assert_eq!(display_report2.artifacts.len(), LITE_MODE_VISIBLE_RESULTS);
        assert_eq!(hidden2.len(), 2);

        // Licensed mode: no limiting.
        let licensed = AccessConfig {
            mode: "licensed".to_string(),
            search_beta_testing: false,
        };
        let (display_report3, hidden3) = display_limited_report(&report, &licensed, false);
        assert_eq!(
            display_report3.artifacts.len(),
            5,
            "licensed mode must not limit the report"
        );
        assert!(hidden3.is_empty(), "no hidden artifacts in licensed mode");
    }

    #[test]
    fn display_limited_report_does_not_mutate_input_report() {
        // The gate must never shrink a saved report, contract payload, or
        // submission. Verify the original report is untouched.
        let mut report = ScanReport::new("/test");
        for i in 0..4 {
            let mut artifact = crate::models::ArtifactReport::new("test_artifact", 0.5);
            artifact.risk_score = i * 20;
            report.artifacts.push(artifact);
        }
        let original_len = report.artifacts.len();

        let access = AccessConfig {
            mode: "lite".to_string(),
            search_beta_testing: false,
        };
        let (_display, _hidden) = display_limited_report(&report, &access, true);

        assert_eq!(
            report.artifacts.len(),
            original_len,
            "input report must not be mutated by display_limited_report"
        );
    }

    #[test]
    fn is_machine_mode_detects_all_machine_output_paths() {
        // Every machine-output flag should produce `true`.
        let empty = OutputArgs::default();

        assert!(
            !is_machine_mode(false, &empty, false),
            "default (no flags) is human console"
        );
        assert!(is_machine_mode(true, &empty, false), "--json is machine");
        assert!(is_machine_mode(false, &empty, true), "--submit is machine");
        assert!(
            is_machine_mode(
                false,
                &OutputArgs {
                    stdout: true,
                    ..OutputArgs::default()
                },
                false
            ),
            "--stdout is machine"
        );
        assert!(
            is_machine_mode(
                false,
                &OutputArgs {
                    contract: true,
                    ..OutputArgs::default()
                },
                false
            ),
            "--contract is machine"
        );
        assert!(
            is_machine_mode(
                false,
                &OutputArgs {
                    out: Some(Some(PathBuf::from("r.json"))),
                    ..OutputArgs::default()
                },
                false
            ),
            "--out is machine"
        );
    }

    #[test]
    fn plan_machine_output_routes_stdout_json_contract_and_submit() {
        // --stdout → contract JSON to stdout, no file.
        let plan = plan_machine_output(
            &OutputArgs {
                stdout: true,
                ..OutputArgs::default()
            },
            false,
            false,
        );
        assert!(
            plan.to_stdout,
            "--stdout must print contract JSON to stdout"
        );
        assert!(plan.write_path.is_none(), "--stdout alone writes no file");

        // global --json → contract JSON to stdout.
        let plan = plan_machine_output(&OutputArgs::default(), false, true);
        assert!(plan.to_stdout, "--json must print contract JSON to stdout");

        // --contract → contract JSON to stdout.
        let plan = plan_machine_output(
            &OutputArgs {
                contract: true,
                ..OutputArgs::default()
            },
            false,
            false,
        );
        assert!(
            plan.to_stdout,
            "--contract must print contract JSON to stdout"
        );

        // --stdout + --out <file> → stdout AND file write.
        let plan = plan_machine_output(
            &OutputArgs {
                stdout: true,
                out: Some(Some(PathBuf::from("r.json"))),
                ..OutputArgs::default()
            },
            false,
            false,
        );
        assert!(plan.to_stdout);
        assert_eq!(plan.write_path, Some(PathBuf::from("r.json")));

        // bare --out → default file, no stdout.
        let plan = plan_machine_output(
            &OutputArgs {
                out: Some(None),
                ..OutputArgs::default()
            },
            false,
            false,
        );
        assert!(!plan.to_stdout, "plain --out should not print to stdout");
        assert_eq!(
            plan.write_path,
            Some(PathBuf::from("vettd-contract.json")),
            "bare --out must write to the default contract file"
        );

        // --submit → full payload written to file + submitted, stdout clean.
        let plan = plan_machine_output(
            &OutputArgs {
                stdout: true,
                ..OutputArgs::default()
            },
            true,
            false,
        );
        assert!(
            !plan.to_stdout,
            "submitting must not pollute stdout even with --stdout"
        );
        assert_eq!(
            plan.write_path,
            Some(PathBuf::from("vettd-contract.json")),
            "--submit writes the full contract payload to the default file"
        );
    }

    #[test]
    fn machine_output_contract_json_is_jq_parseable_and_full() {
        // Issue #198 AC #2 (pipe-to-jq / --out): the machine-output JSON must
        // parse cleanly and carry the FULL report — more findings than the
        // lite-mode display cap (3), so `--out` can never silently truncate.
        let mut report = ScanReport::new("/test");
        for _i in 0..(LITE_MODE_VISIBLE_RESULTS + 2) {
            report
                .artifacts
                .push(crate::models::ArtifactReport::new("prompt_config", 0.8));
        }
        let payload = build_contract_payload(&report, 0);

        // The full payload must surface every prompt_config artifact: the
        // machine path is built from `&report`, not `display_report`, so the
        // count exceeds the lite display cap.
        assert!(
            payload.prompts.len() >= LITE_MODE_VISIBLE_RESULTS,
            "machine payload must not be lite-limited: {}",
            payload.prompts.len()
        );

        // And the serialized JSON must be jq-parseable (valid JSON object).
        let json = serde_json::to_string_pretty(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .expect("machine-output contract JSON must parse as valid JSON");
        assert!(parsed.is_object(), "contract JSON must be a JSON object");
    }

    #[test]
    fn load_access_config_uses_per_user_path_not_cwd() {
        // AC #1: a `.vettd.toml` in the current working directory must NOT
        // affect the report. Only `~/.vettd/.vettd.toml` is consulted, and
        // `[access] mode` alone governs the tier (no endpoint/license fields).
        let tmp = tempfile::tempdir().unwrap();
        let fake_home = tmp.path().join("home");
        std::fs::create_dir_all(fake_home.join(".vettd")).unwrap();
        std::fs::write(
            fake_home.join(".vettd/.vettd.toml"),
            "[access]\nmode = \"lite\"\n",
        )
        .unwrap();

        // A conflicting CWD file that claims licensed must be ignored — it must
        // never self-gate a scanned repo's findings.
        let cwd_conflict = tmp.path().join("cwd-proj");
        std::fs::create_dir_all(&cwd_conflict).unwrap();
        let mut cwd_guard = cwd_conflict.clone();
        cwd_guard.push(".vettd.toml");
        std::fs::write(&cwd_guard, "[access]\nmode = \"licensed\"\n").unwrap();

        let cfg = load_access_config_from(fake_home.join(".vettd").join(".vettd.toml").as_path());
        assert_eq!(cfg.mode, "lite", "home config must win over any cwd config",);
        drop(cwd_guard);
    }

    #[test]
    fn access_config_has_no_license_fields() {
        // AC #3: license_key, endpoint, and license_timeout_seconds were
        // removed as dead fields. The struct must only have `mode`.
        let cfg = AccessConfig::default();
        assert_eq!(cfg.mode, "licensed");
        // If any of these fields were added back, this test would fail to
        // compile — that's the point.
    }

    #[test]
    fn access_config_parses_search_beta_testing_true() {
        // search_beta_testing = true in the [access] table must be parsed as true.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".vettd")).unwrap();
        std::fs::write(
            tmp.path().join(".vettd/.vettd.toml"),
            "[access]\nsearch_beta_testing = true\nmode = \"lite\"\n",
        )
        .unwrap();

        let cfg = load_access_config_from(&tmp.path().join(".vettd/.vettd.toml"));
        assert!(
            cfg.search_beta_testing,
            "search_beta_testing must be true when set in config"
        );
        assert_eq!(cfg.mode, "lite", "mode must still be parsed alongside it");
    }

    #[test]
    fn access_config_search_beta_testing_defaults_to_false() {
        // When search_beta_testing is absent (or the config only has `mode`),
        // it must default to false.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".vettd")).unwrap();
        std::fs::write(
            tmp.path().join(".vettd/.vettd.toml"),
            "[access]\nmode = \"lite\"\n",
        )
        .unwrap();

        let cfg = load_access_config_from(&tmp.path().join(".vettd/.vettd.toml"));
        assert!(
            !cfg.search_beta_testing,
            "search_beta_testing must default to false when missing from config"
        );
    }

    #[test]
    fn access_config_parses_search_beta_testing_explicit_false() {
        // Explicit `search_beta_testing = false` must be parsed as false.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".vettd")).unwrap();
        std::fs::write(
            tmp.path().join(".vettd/.vettd.toml"),
            "[access]\nsearch_beta_testing = false\n",
        )
        .unwrap();

        let cfg = load_access_config_from(&tmp.path().join(".vettd/.vettd.toml"));
        assert!(
            !cfg.search_beta_testing,
            "search_beta_testing = false must be parsed as false"
        );
    }

    // ── #196: disclosure reflects the actual payload ──

    #[test]
    fn disclosure_includes_mcp_categories_when_servers_present() {
        // The disclosure must list MCP-specific categories (command, tool
        // names, etc.) only when there are MCP servers with data.
        let payload = build_contract_payload_for_disclosure(&ScanReport::new("/test"), 0);
        let cats = disclosure_categories(&payload);
        // With no MCP servers, no MCP categories should appear.
        for cat in &cats {
            assert!(
                !matches!(
                    cat,
                    crate::contract::DisclosureCategory::McpServerCommand
                        | crate::contract::DisclosureCategory::McpToolNames
                        | crate::contract::DisclosureCategory::LogDerivedNetworkEvidence
                        | crate::contract::DisclosureCategory::EnvVarNames
                ),
                "unexpected MCP category present: {:?}",
                cat
            );
        }
    }

    #[test]
    fn disclosure_rendering_does_not_write_to_stdout() {
        // The disclosure must never pollute stdout — it goes to stderr so
        // `--stdout | jq` pipelines stay clean.
        let payload = build_contract_payload(&ScanReport::new("/test"), 0);
        // We can't easily capture stderr in a test, but we can verify the
        // render function returns a string (not writes). The string is
        // non-empty and contains the header.
        let rendered = render_disclosure(&payload);
        assert!(
            rendered.contains("This submission will include"),
            "rendered disclosure must contain header: {rendered}"
        );
    }

    #[test]
    fn disclosure_mentions_scan_root_paths_always() {
        // ScanRootPaths is always present (scan_roots is never empty).
        let payload = build_contract_payload_for_disclosure(&ScanReport::new("/test"), 0);
        let cats = disclosure_categories(&payload);
        assert!(
            cats.contains(&crate::contract::DisclosureCategory::ScanRootPaths),
            "ScanRootPaths must always be present"
        );
    }

    #[test]
    fn disclosure_mentions_hostname_always() {
        // Hostname is always present (endpoint_hostname is never empty).
        let payload = build_contract_payload_for_disclosure(&ScanReport::new("/test"), 0);
        let cats = disclosure_categories(&payload);
        assert!(
            cats.contains(&crate::contract::DisclosureCategory::Hostname),
            "Hostname must always be present"
        );
    }

    #[test]
    fn disclosure_mentions_scanner_version_always() {
        // ScannerVersion is always present.
        let payload = build_contract_payload_for_disclosure(&ScanReport::new("/test"), 0);
        let cats = disclosure_categories(&payload);
        assert!(
            cats.contains(&crate::contract::DisclosureCategory::ScannerVersion),
            "ScannerVersion must always be present"
        );
    }
}
