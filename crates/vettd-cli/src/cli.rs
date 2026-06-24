use clap::{CommandFactory, Parser, Subcommand};
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use crate::contract::build_contract_payload;
use crate::lite_mode::{limit_lite_mode_report, print_locked_summary, LITE_MODE_VISIBLE_RESULTS};
use crate::models::ScanReport;
use crate::output::{do_submit, emit, resolve_submit_auth};
use crate::scan::run_scan;
use crate::submit::{save_auth_config, AuthConfig, DEFAULT_PRODUCTION_ENDPOINT};

// ---------------------------------------------------------------------------
// CLI argument definitions
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "vettd",
    about = "AI Execution Inventory — detect, analyze, and report AI execution artifacts.",
    version = env!("CARGO_PKG_VERSION"),
)]
pub struct Cli {
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
    #[command(hide = true)]
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
    /// Show current auth/identity status (not yet implemented)
    Status,
}

#[derive(Subcommand)]
pub enum ContractSubcommand {
    /// Show local vs. server contract status (not yet implemented)
    Status,
}

#[derive(Subcommand)]
pub enum DirectorySubcommand {
    /// Search the directory (not yet implemented)
    Search {
        /// Search query
        query: String,
    },
    /// List directory entries (not yet implemented)
    List,
    /// Show trending entries (not yet implemented)
    Trending,
    /// Show a random entry (not yet implemented)
    Random,
    /// View a directory entry by slug (not yet implemented)
    View {
        /// Entry slug
        slug: String,
    },
    /// Show findings for an entry (not yet implemented)
    Findings {
        /// Entry slug
        slug: String,
        /// Minimum severity: critical|high|medium|low|info
        #[arg(long, default_value = "info")]
        min_severity: String,
    },
    /// Compare two directory entries (not yet implemented)
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
    pub json: bool,
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
}

impl Default for OutputArgs {
    fn default() -> Self {
        Self {
            full: false,
            json: false,
            summary: false,
            out: None,
            min_severity: "info".to_string(),
            contract: false,
            submit: None,
            api_key: None,
            allow_public_endpoint: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Access configuration
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct AccessConfig {
    mode: String,
    license_key: Option<String>,
    endpoint: Option<String>,
    license_timeout_seconds: f64,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            mode: "licensed".into(),
            license_key: None,
            endpoint: None,
            license_timeout_seconds: 5.0,
        }
    }
}

fn load_access_config() -> AccessConfig {
    let path = Path::new(".vettd.toml");
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
    if let Some(toml::Value::String(v)) = access.get("license_key") {
        cfg.license_key = Some(v.clone());
    }
    if let Some(toml::Value::String(v)) = access.get("endpoint") {
        cfg.endpoint = Some(v.clone());
    }
    if let Some(toml::Value::Float(v)) = access.get("license_timeout_seconds") {
        cfg.license_timeout_seconds = *v;
    }

    cfg
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
        ScanSubcommand::Folder { path, .. } => ScanParams {
            mode: "workdir",
            workdir: Some(path.as_path()),
            file: None,
            deep: false,
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

fn apply_access_gate(report: ScanReport, access: &AccessConfig) -> ScanReport {
    if access.mode == "lite" {
        let (limited, _hidden_count, hidden_artifacts) =
            limit_lite_mode_report(&report, LITE_MODE_VISIBLE_RESULTS);
        if !hidden_artifacts.is_empty() {
            print_locked_summary(&hidden_artifacts);
        }
        limited
    } else {
        report
    }
}

// ---------------------------------------------------------------------------
// Not-yet-implemented stubs
// ---------------------------------------------------------------------------

/// Print a clear not-implemented notice to stderr and exit non-zero.
///
/// Used by web-facing command stubs (auth status, contract status, directory)
/// scaffolded ahead of their backend logic. Exit code 2 marks the
/// recognized-but-unimplemented class distinctly from runtime errors (exit 1).
fn not_implemented(command: &str) -> ! {
    eprintln!("Error: `vettd {command}` is not yet implemented.");
    std::process::exit(2);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run() {
    let cli = Cli::parse();

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
            RuleAction::List => crate::rules::cmd_list(),
            RuleAction::Add { path } => crate::rules::cmd_add(path),
            RuleAction::Remove { name } => crate::rules::cmd_remove(name),
            RuleAction::Validate { path } => crate::rules::cmd_validate(path),
        }
        return;
    }

    // Handle update command
    if let Commands::Update { check, force } = &cmd {
        if *check {
            match crate::updater::check_for_update(10) {
                Ok(result) => {
                    if result.is_newer {
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
                    eprintln!("Update check failed: {e}");
                    std::process::exit(1);
                }
            }
        } else if let Err(e) = crate::updater::perform_update(*force) {
            eprintln!("Error: {e}");
            std::process::exit(1);
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
            not_implemented("auth status");
        }
        let api_key = match key {
            Some(value) => value.clone(),
            None => crate::wizard::ask_secret("API key"),
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
            Ok(()) => {
                eprintln!("Credentials saved.");
                eprintln!("  Endpoint: {}", config.endpoint);
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // Handle contract command (stub — vettd#631 owns real logic)
    if let Commands::Contract { action } = &cmd {
        match action {
            ContractSubcommand::Status => not_implemented("contract status"),
        }
        // Unreachable while all arms diverge; guards fallthrough when real arms land.
        #[allow(unreachable_code)]
        return;
    }

    // Handle directory command (stub — vettd#631 owns real logic)
    if let Commands::Directory { action } = &cmd {
        match action {
            DirectorySubcommand::Search { .. } => not_implemented("directory search"),
            DirectorySubcommand::List => not_implemented("directory list"),
            DirectorySubcommand::Trending => not_implemented("directory trending"),
            DirectorySubcommand::Random => not_implemented("directory random"),
            DirectorySubcommand::View { .. } => not_implemented("directory view"),
            DirectorySubcommand::Findings { .. } => not_implemented("directory findings"),
            DirectorySubcommand::Compare { .. } => not_implemented("directory compare"),
        }
        // Unreachable while all arms diverge; guards fallthrough when real arms land.
        #[allow(unreachable_code)]
        return;
    }

    // Remaining command must be Scan
    let Commands::Scan { subcommand } = cmd else {
        return;
    };

    let sub = match subcommand {
        None => crate::wizard::pick_scan(),
        Some(s) => s,
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
    let mut report = run_scan(
        params.mode,
        params.workdir,
        params.file,
        params.deep,
        if interactive { Some(&tick_fn) } else { None },
    );
    let scan_duration_ms = scan_start.elapsed().as_millis() as u64;
    if let Some(ref mut p) = *progress_cell.borrow_mut() {
        p.done(Some(&format!(
            "Found {} artifact(s)",
            report.artifacts.len()
        )));
    }

    report = apply_access_gate(report, &access);
    filter_by_severity(&mut report, min_score);

    let wants_submit = out.submit.is_some();

    if out.contract || wants_submit {
        let payload = build_contract_payload(&report, scan_duration_ms);
        let json = match serde_json::to_string_pretty(&payload) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("Error serializing contract payload: {e}");
                std::process::exit(1);
            }
        };

        if out.contract && !wants_submit {
            println!("{json}");
        }

        // Write to file if --out is specified, or always when submitting
        let write_dest = if let Some(maybe_path) = &out.out {
            Some(match maybe_path {
                Some(p) => p.clone(),
                None => PathBuf::from("vettd-contract.json"),
            })
        } else if wants_submit {
            Some(PathBuf::from("vettd-contract.json"))
        } else {
            None
        };

        if let Some(dest) = write_dest {
            if let Err(e) = fs::write(&dest, &json) {
                eprintln!("Error writing contract to {}: {}", dest.display(), e);
            } else {
                eprintln!("Contract written to {}", dest.display());
            }
        }

        if wants_submit {
            let auth = match resolve_submit_auth(
                &out.submit,
                out.api_key.as_deref(),
                out.allow_public_endpoint,
            ) {
                Ok(auth) => auth,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            };
            if let Err(e) = do_submit(&json, &auth) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    } else {
        let cmd_name = command_name(&sub);
        emit(
            &report,
            scan_duration_ms,
            out.json,
            &out.out,
            out.summary,
            out.full,
            cmd_name,
        );
    }

    // Offer interactive follow-up actions for local-only scans.
    if !wants_submit && !out.json && !out.contract && is_interactive() {
        prompt_post_scan_action(&report, scan_duration_ms);
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
    let auth = match resolve_submit_auth(&Some(None), None, false) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
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

fn prompt_post_scan_action(report: &ScanReport, scan_duration_ms: u64) {
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
        PostScanAction::SubmitToVettd => prompt_submit(report, scan_duration_ms),
        PostScanAction::DoNothing => {}
    }
}

fn save_report_interactively(report: &ScanReport, scan_duration_ms: u64) {
    let path = crate::wizard::ask("Report path", "vettd-report.json");
    let maybe_path = Some(PathBuf::from(path));
    crate::output::write_json_report(report, scan_duration_ms, &maybe_path);
}

/// Print a concise summary of the data categories included in a submission.
///
/// Called in interactive flows immediately before asking for consent.  The
/// summary is intentionally short — it names the data categories without
/// reproducing actual values.
fn print_submit_disclosure(report: &ScanReport) {
    let artifact_count = report.artifacts.len();
    eprintln!("  This submission will include:");
    eprintln!("    • Scan root paths and machine hostname");
    eprintln!(
        "    • {} AI artifact record(s): file paths, content hashes, capability signals, risk scores",
        artifact_count
    );
    eprintln!("    • MCP server config metadata: commands, tool names, env-var names (not values)");
    eprintln!("    • Host security context (macOS firewall state on macOS; empty elsewhere)");
    eprintln!("    • Scanner version, OS, and architecture");
    eprintln!("  No file contents, secret values, or credential material are transmitted.");
}

/// After a scan, ask the user if they want to submit results.
fn prompt_submit(report: &ScanReport, scan_duration_ms: u64) {
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

    // Show a concise data-disclosure summary, then ask for consent.
    print_submit_disclosure(report);
    let confirmed = crate::wizard::confirm("Send this data?", false);
    if !confirmed {
        eprintln!("  Submission cancelled.");
        return;
    }

    // Build and submit
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
            eprintln!("  You can retry later with: \x1b[1mvettd scan --submit\x1b[0m");
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
    use clap::Parser;

    #[test]
    fn print_submit_disclosure_runs_without_panic() {
        // Smoke-test: disclosure function must not panic for empty or non-empty reports.
        let empty = ScanReport::new("/test");
        print_submit_disclosure(&empty);

        let mut with_artifacts = ScanReport::new("/test");
        with_artifacts
            .artifacts
            .push(crate::models::ArtifactReport::new("mcp_config", 0.9));
        with_artifacts
            .artifacts
            .push(crate::models::ArtifactReport::new("prompt_config", 0.5));
        print_submit_disclosure(&with_artifacts);
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
                action: DirectorySubcommand::Search { query },
            }) => assert_eq!(query, "foo"),
            _ => panic!("Expected directory search command"),
        }
    }

    #[test]
    fn parse_cli_directory_list() {
        let cli = Cli::parse_from(["vettd", "directory", "list"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Directory {
                action: DirectorySubcommand::List
            })
        ));
    }

    #[test]
    fn parse_cli_directory_trending() {
        let cli = Cli::parse_from(["vettd", "directory", "trending"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Directory {
                action: DirectorySubcommand::Trending
            })
        ));
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
        let cli = Cli::parse_from(["vettd", "scan", "quick", "--json"]);
        match cli.command {
            Some(Commands::Scan {
                subcommand: Some(ScanSubcommand::Quick { output, .. }),
            }) => {
                assert!(output.json);
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
        let cfg = load_access_config();
        assert_eq!(cfg.mode, "licensed");
        assert!(cfg.license_key.is_none());
    }
}
