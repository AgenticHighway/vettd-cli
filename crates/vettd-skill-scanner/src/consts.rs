//! Shared constants for the vettd skill scanner.
//!
//! Any constant used in more than one module, or that may need to be shared
//! with callers, lives here instead of in the file that first needs it.

/// Scanner version. Must stay in sync with `CURRENT_SCANNER_VERSION` in the
/// vettd web app's `skill-analyzer.ts`. The server uses this value to detect
/// stale scan results and trigger re-scans.
pub const CURRENT_SCANNER_VERSION: u32 = 9;

/// Default `source` value for findings produced by this scanner.
pub const DEFAULT_SOURCE: &str = "vettd";

/// Required prefix for the `detail` field on file-specific findings.
///
/// Chain detection in the vettd web scanner parses the filepath out of this
/// string with the regex `Detected in ([^:]+):`. The full format is:
///
/// ```text
/// Detected in <filepath>:<linenum> — `snippet`
/// ```
///
/// The Rust scanner must emit `detail` strings in this format for any finding
/// that has an associated file and line number so that chain detection can
/// co-locate credential-source and network-sink findings.
pub const DETAIL_DETECTED_IN_PREFIX: &str = "Detected in ";

// ── Validation thresholds (must match skill-analyzer.ts) ─────────────────────

pub const DESCRIPTION_MAX_LENGTH: usize = 1024;
pub const EVALS_MIN_TEST_CASES: usize = 3;
pub const SKILL_NAME_MAX_LENGTH: usize = 64;
pub const SKILL_MD_BODY_MAX_LINES: usize = 500;
pub const NEGATION_LOOKBACK_CHARS: usize = 40;

// ── Eval file candidates ──────────────────────────────────────────────────────

/// Paths checked in order before falling back to a full evals/ directory scan.
pub const EVAL_JSON_CANDIDATES: &[&str] = &[
    "evals/evals.json",
    "evals.json",
    "tests/tests.json",
    "tests/evals.json",
    "test/tests.json",
    "test/evals.json",
    "evals/tests.json",
];

// ── Chain classification fragments ───────────────────────────────────────────
// Used by detect_malicious_activity_chains to bucket security findings by type.

pub const EVASION_FRAGS: &[&str] = &[
    "Shell history",
    "Audit daemon",
    "Windows event log clearing",
    "Script self-deletion",
    "System log truncation",
    "Shell history file wipe",
    "Journal log vacuum",
    "Forced log rotation",
];

pub const PERSISTENCE_FRAGS: &[&str] = &[
    "Cron persistence",
    "Systemd user service",
    "Shell rc file write",
    "Time-delayed execution via at",
    "Git hook injection",
    "LD_PRELOAD environment injection",
];

pub const FETCH_FRAGS: &[&str] = &[
    "Remote content fetched into variable",
    "Remote content fetched into variable for execution (Python)",
    "Base64-decoded content stored in variable",
];

pub const EXECUTION_FRAGS: &[&str] = &[
    "Remote code execution via command substitution",
    "Shell variable execution",
    "Remote code execution via pipe to shell",
    "PowerShell encoded command",
    "PowerShell IEX download cradle",
    "Python exec/eval of variable content",
];

pub const COVERT_CHANNEL_FRAGS: &[&str] = &[
    "DNS query with variable-constructed hostname",
    "DNS TXT record lookup",
    "Outbound POST with application/octet-stream",
];

pub const CRED_SOURCE_FRAGS: &[&str] = &[
    "credential file access",
    "private key file access",
    "Keychain file access",
    "hardcoded secret",
    "API key",
    ".env",
    "High-entropy value",
    "OIDC token environment variable",
];

// ── Typosquatting reference list ──────────────────────────────────────────────

pub const POPULAR_SKILL_NAMES: &[&str] = &[
    "github-pr-review",
    "github-issue-triage",
    "git-commit-helper",
    "github-actions-helper",
    "github-actions-debug",
    "github-release-notes",
    "aws-cost-explorer",
    "aws-s3-manager",
    "gcp-resource-audit",
    "azure-devops-helper",
    "terraform-plan-review",
    "kubernetes-debug",
    "solana-wallet",
    "phantom-wallet",
    "metamask-helper",
    "ethereum-signer",
    "bitcoin-address",
    "binance-api",
    "coinbase-trader",
    "uniswap-helper",
    "ledger-connect",
    "trezor-verify",
    "code-review",
    "test-generator",
    "doc-writer",
    "api-mocker",
    "sql-query-builder",
    "regex-builder",
    "json-formatter",
    "openai-validator",
    "lint-fixer",
    "dependency-updater",
    "prompt-optimizer",
    "context-summarizer",
    "claude-helper",
    "openai-wrapper",
    "llm-evaluator",
    "slack-notifier",
    "jira-issue-creator",
    "linear-ticket",
    "notion-page-writer",
    "calendar-scheduler",
    "email-drafter",
    "secret-scanner",
    "vulnerability-scanner",
    "cve-lookup",
    "permissions-auditor",
    "sast-runner",
];

// ── Description mismatch keywords ────────────────────────────────────────────

pub const BENIGN_DESCRIPTION_KEYWORDS: &[&str] = &[
    "format",
    "parse",
    "convert",
    "search",
    "summarize",
    "summarization",
    "translate",
    "lint",
    "validate",
    "sort",
    "filter",
    "render",
    "pretty",
    "prettify",
    "diff",
    "compare",
    "clean",
    "normalize",
    "transform",
    "extract",
    "template",
    "generate",
    "scaffold",
    "snippet",
    "helper",
    "utility",
    "wrapper",
    "markdown",
    "json",
    "yaml",
    "csv",
    "html",
    "css",
];
