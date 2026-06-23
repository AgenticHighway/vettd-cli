//! Core scan engine — takes a skill file map and produces findings.
//!
//! Rule implementations mirror the vettd web scanner's `analyzeSkillFiles` in
//! `packages/api/src/skills/skill-analyzer.ts`. Where vettd has a bug that is
//! visible in the wire format (e.g. VTD-0123 detail says "non-JSON format" even
//! for .json files), the Rust engine reproduces the same output unaltered so that
//! the parity test can reach a clean pass.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::consts::DEFAULT_SOURCE;
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::result::SkillScanResult;

// ── Rule IDs (must match skill-rule-registry.ts) ─────────────────────────────

// Security — credential & secret patterns
const RULE_EMBEDDED_PRIVATE_KEY: &str = "VTD-0001";
const RULE_POTENTIAL_API_TOKEN: &str = "VTD-0002";
const RULE_HIGH_ENTROPY_SECRET: &str = "VTD-0003";
const RULE_ENV_FILE_IN_PACKAGE: &str = "VTD-0004";
const RULE_CLOUD_CREDENTIAL_FILE: &str = "VTD-0005";
const RULE_SSH_KEY_FILE: &str = "VTD-0006";
const RULE_NPM_CREDENTIAL_FILE: &str = "VTD-0007";
const RULE_PYPI_CREDENTIAL_FILE: &str = "VTD-0008";
const RULE_DOCKER_CREDENTIAL_FILE: &str = "VTD-0009";
const RULE_KUBERNETES_CREDENTIAL_FILE: &str = "VTD-0010";
const RULE_GITHUB_CLI_CREDENTIAL_FILE: &str = "VTD-0011";
const RULE_NETRC_CREDENTIAL_FILE: &str = "VTD-0012";
const RULE_MACOS_KEYCHAIN_ACCESS: &str = "VTD-0013";
const RULE_WINDOWS_CREDENTIAL_STORE: &str = "VTD-0014";
const RULE_WINDOWS_CREDENTIAL_DATABASE: &str = "VTD-0015";
const RULE_AD_CREDENTIAL_DATABASE: &str = "VTD-0016";
// Security — code injection / exec
const RULE_EVAL_CODE_INJECTION: &str = "VTD-0017";
const RULE_SHELL_EXEC_UNSANDBOXED: &str = "VTD-0018";
const RULE_DESTRUCTIVE_FILESYSTEM_OP: &str = "VTD-0019";
const RULE_RCE_PIPE_TO_SHELL: &str = "VTD-0020";
const RULE_RCE_COMMAND_SUBSTITUTION: &str = "VTD-0021";
const RULE_REMOTE_FETCH_TO_VARIABLE: &str = "VTD-0022";
const RULE_SHELL_VARIABLE_EXECUTION: &str = "VTD-0023";
const RULE_PYTHON_REMOTE_FETCH: &str = "VTD-0024";
const RULE_PYTHON_BASE64_DECODE_VARIABLE: &str = "VTD-0025";
const RULE_PYTHON_EXEC_VARIABLE: &str = "VTD-0026";
const RULE_SHELL_BASE64_LITERAL: &str = "VTD-0027";
const RULE_SAFETY_BYPASS_FLAG: &str = "VTD-0028";
// Security — cloud metadata / network / malicious
const RULE_CLOUD_METADATA_PROBE_AWS: &str = "VTD-0029";
const RULE_CLOUD_METADATA_PROBE_GCP: &str = "VTD-0030";
const RULE_CLOUD_METADATA_PROBE_AZURE: &str = "VTD-0031";
const RULE_CLOUD_METADATA_PROBE_ALIBABA: &str = "VTD-0032";
const RULE_CREDENTIAL_DUMPING_TOOL: &str = "VTD-0033";
const RULE_LSASS_MEMORY_ACCESS: &str = "VTD-0034";
const RULE_GITHUB_OIDC_TOKEN_READ: &str = "VTD-0035";
const RULE_SCRIPT_SELF_DELETION_RM: &str = "VTD-0036";
const RULE_SCRIPT_SELF_DELETION_PYTHON: &str = "VTD-0037";
const RULE_SCRIPT_SELF_DELETION_NODE: &str = "VTD-0038";
const RULE_SHELL_HISTORY_SUPPRESSION: &str = "VTD-0039";
const RULE_SHELL_HISTORY_CLEARING: &str = "VTD-0040";
const RULE_SHELL_HISTORY_FILE_WIPE: &str = "VTD-0041";
const RULE_AUDIT_DAEMON_DISABLE: &str = "VTD-0042";
const RULE_AUDIT_DAEMON_STOP: &str = "VTD-0043";
const RULE_WINDOWS_EVENTLOG_CLEARING: &str = "VTD-0044";
const RULE_SYSTEM_LOG_TRUNCATION: &str = "VTD-0045";
const RULE_JOURNAL_LOG_VACUUM: &str = "VTD-0046";
const RULE_FORCED_LOG_ROTATION: &str = "VTD-0047";
const RULE_CRON_PERSISTENCE: &str = "VTD-0048";
const RULE_SYSTEMD_SERVICE_PERSISTENCE: &str = "VTD-0049";
const RULE_SYSTEMD_SERVICE_FILE_WRITE: &str = "VTD-0050";
const RULE_SHELL_RC_PERSISTENCE: &str = "VTD-0051";
const RULE_GIT_HOOK_INJECTION: &str = "VTD-0052";
const RULE_LD_PRELOAD_INJECTION: &str = "VTD-0053";
const RULE_TIME_DELAYED_EXECUTION: &str = "VTD-0054";
const RULE_DESTRUCTIVE_RECURSIVE_DELETE_SYSTEM: &str = "VTD-0055";
const RULE_DESTRUCTIVE_RECURSIVE_DELETE_FIND: &str = "VTD-0056";
const RULE_DNS_COVERT_CHANNEL: &str = "VTD-0057";
const RULE_DNS_TXT_LOOKUP: &str = "VTD-0058";
const RULE_OCTET_STREAM_POST: &str = "VTD-0059";
const RULE_POWERSHELL_ENCODED_COMMAND: &str = "VTD-0060";
const RULE_POWERSHELL_IEX_CRADLE: &str = "VTD-0061";
const RULE_POWERSHELL_EXECUTION_POLICY_BYPASS: &str = "VTD-0062";
const RULE_POWERSHELL_HIDDEN_WINDOW: &str = "VTD-0063";
// Security — behavioral patterns
const RULE_PROMPT_INSTRUCTION_OVERRIDE: &str = "VTD-0064";
const RULE_SYSTEM_PROMPT_REPLACEMENT: &str = "VTD-0065";
const RULE_SYSTEM_PROMPT_OVERRIDE: &str = "VTD-0066";
const RULE_CONTEXT_INVALIDATION: &str = "VTD-0067";
const RULE_JAILBREAK_PERSONA: &str = "VTD-0068";
const RULE_SAFETY_SYSTEM_BYPASS: &str = "VTD-0069";
const RULE_UNRESTRICTED_OPERATION_FRAMING: &str = "VTD-0070";
const RULE_ETHICAL_BYPASS_FRAMING: &str = "VTD-0071";
const RULE_ROLEPLAY_BYPASS_FRAMING: &str = "VTD-0072";
const RULE_CREDENTIAL_SOLICITATION: &str = "VTD-0073";
const RULE_DECEPTIVE_CREDENTIAL_EXTRACTION: &str = "VTD-0074";
const RULE_PROMPT_TEMPLATE_MARKER: &str = "VTD-0075";
const RULE_CHAT_TEMPLATE_SPECIAL_TOKEN: &str = "VTD-0076";
// Security — obfuscation / base64 / typosquat / chain
const RULE_OBFUSCATED_DANGEROUS_CODE: &str = "VTD-0077";
const RULE_HIDDEN_UNICODE_CHARACTER: &str = "VTD-0081";
const RULE_OBFUSCATED_NETWORK_CALL: &str = "VTD-0078";
const RULE_OBFUSCATED_EXTERNAL_URL: &str = "VTD-0079";
const RULE_BASE64_IN_MARKDOWN: &str = "VTD-0080";
const RULE_POSSIBLE_TYPOSQUATTING: &str = "VTD-0082";
const RULE_NO_REPOSITORY_LINK: &str = "VTD-0083";
const RULE_SYSTEM_PROMPT_LEAKAGE: &str = "VTD-0085";
const RULE_DESCRIPTION_BEHAVIOR_MISMATCH: &str = "VTD-0087";
const RULE_EXTERNAL_URL_REFERENCE: &str = "VTD-0088";
const RULE_CREDENTIAL_EXFILTRATION_CHAIN: &str = "VTD-0089";
const RULE_MALICIOUS_ACTIVITY_CHAIN: &str = "VTD-0090";
const RULE_NO_SECRETS_DETECTED: &str = "VTD-0091";
const RULE_NO_BEHAVIORAL_SIGNALS: &str = "VTD-0092";
const RULE_NO_EXTERNAL_URLS: &str = "VTD-0093";
const RULE_BEHAVIORAL_SCAN_TRUNCATED: &str = "VTD-0094";

// Structure
const RULE_SKILL_MD: &str = "VTD-0095";
const RULE_SCRIPTS_DIRECTORY: &str = "VTD-0096";
const RULE_REFERENCES_DIRECTORY: &str = "VTD-0097";
const RULE_ASSETS_DIRECTORY: &str = "VTD-0098";
const RULE_SKILL_NAME_VALIDITY: &str = "VTD-0099";
const RULE_SKILL_NAME_COLLISION: &str = "VTD-0100";

// Best practices
const RULE_SKILL_MD_BODY_LENGTH: &str = "VTD-0101";
const RULE_GOTCHAS_SECTION: &str = "VTD-0102";
const RULE_EXAMPLES_PRESENT: &str = "VTD-0103";
const RULE_CHECKLIST_PRESENT: &str = "VTD-0104";
const RULE_VALIDATION_LOOP: &str = "VTD-0105";
const RULE_WORKFLOW_STRUCTURE: &str = "VTD-0106";
const RULE_PROGRESSIVE_DISCLOSURE: &str = "VTD-0107";
const RULE_GENERIC_INSTRUCTION: &str = "VTD-0108";

// Description
const RULE_DESCRIPTION_PRESENT: &str = "VTD-0109";
const RULE_DESCRIPTION_LENGTH: &str = "VTD-0110";
const RULE_DESCRIPTION_CONTEXT: &str = "VTD-0111";
const RULE_DESCRIPTION_BREVITY: &str = "VTD-0112";
const RULE_DESCRIPTION_SCOPE: &str = "VTD-0113";

// Scripts
const RULE_SCRIPT_CLI_HELP: &str = "VTD-0114";
const RULE_SCRIPT_INTERACTIVE_PROMPTS: &str = "VTD-0115";
const RULE_SCRIPT_STRUCTURED_OUTPUT: &str = "VTD-0116";
const RULE_SCRIPT_DEPENDENCY_PINNING: &str = "VTD-0117";

// Evals
const RULE_EVALS_PRESENT: &str = "VTD-0118";
const RULE_EVAL_FILES_FOUND: &str = "VTD-0123";

// ── Constants (must match skill-analyzer.ts) ──────────────────────────────────

const DESCRIPTION_MAX_LENGTH: usize = 1024;
const SKILL_NAME_MAX_LENGTH: usize = 64;
const SKILL_MD_BODY_MAX_LINES: usize = 500;

// eval JSON candidates checked before falling back to non-trivial file scan
const EVAL_JSON_CANDIDATES: &[&str] = &[
    "evals/evals.json",
    "evals.json",
    "tests/tests.json",
    "tests/evals.json",
    "test/tests.json",
    "test/evals.json",
    "evals/tests.json",
];

// ── Sensitive pattern detection ───────────────────────────────────────────────
// Partial implementation of vettd's SENSITIVE_PATTERNS from checkSecurity().
// Only patterns NOT in CODE_ONLY_LABELS are included here (they fire on all
// file types including .md). Each pattern fires once per file at the first
// matching line.

struct SensitivePattern {
    rule_id: &'static str,
    label: &'static str,
    severity: Severity,
    intent: Intent,
    /// Skip this pattern for .md files (mirrors vettd's CODE_ONLY_LABELS).
    code_only: bool,
    /// When scanning a .md file, use this severity instead of `severity`.
    /// `None` means the pattern fires at its normal severity on .md files too.
    doc_severity: Option<Severity>,
}

// Array order mirrors vettd's SENSITIVE_PATTERNS definition order.
static SENSITIVE_PATTERNS: &[SensitivePattern] = &[
    SensitivePattern {
        rule_id: RULE_EMBEDDED_PRIVATE_KEY,
        label: "Embedded private key",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_POTENTIAL_API_TOKEN,
        label: "Potential API token detected",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_EVAL_CODE_INJECTION,
        label: "Use of eval() — potential code injection risk",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SHELL_EXEC_UNSANDBOXED,
        label: "Shell execution without sandboxing",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_DESTRUCTIVE_FILESYSTEM_OP,
        label: "Destructive file system operation",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_RCE_PIPE_TO_SHELL,
        label: "Remote code execution via pipe to shell",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_RCE_PIPE_TO_SHELL,
        label: "Remote code execution via pipe to shell",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SAFETY_BYPASS_FLAG,
        label: "Safety bypass flag detected",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SAFETY_BYPASS_FLAG,
        label: "Safety bypass flag detected",
        severity: Severity::Low,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_CLOUD_CREDENTIAL_FILE,
        label: "Cloud credential file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_CLOUD_CREDENTIAL_FILE,
        label: "Cloud credential file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_SSH_KEY_FILE,
        label: "SSH private key file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_NPM_CREDENTIAL_FILE,
        label: "npm credential file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_PYPI_CREDENTIAL_FILE,
        label: "PyPI credential file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_DOCKER_CREDENTIAL_FILE,
        label: "Docker credential file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_KUBERNETES_CREDENTIAL_FILE,
        label: "Kubernetes credential file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_GITHUB_CLI_CREDENTIAL_FILE,
        label: "GitHub CLI credential file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_NETRC_CREDENTIAL_FILE,
        label: "netrc credential file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_MACOS_KEYCHAIN_ACCESS,
        label: "macOS Keychain file access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_WINDOWS_CREDENTIAL_STORE,
        label: "Windows credential store access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_WINDOWS_CREDENTIAL_DATABASE,
        label: "Windows credential database access",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: false,
        doc_severity: Some(Severity::Medium),
    },
    SensitivePattern {
        rule_id: RULE_AD_CREDENTIAL_DATABASE,
        label: "Active Directory credential database access (NTDS.dit)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_CLOUD_METADATA_PROBE_AWS,
        label: "Cloud metadata service probe (AWS/standard IMDS)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_CLOUD_METADATA_PROBE_GCP,
        label: "Cloud metadata service probe (GCP)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_CLOUD_METADATA_PROBE_AZURE,
        label: "Cloud metadata service probe (Azure)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_CLOUD_METADATA_PROBE_ALIBABA,
        label: "Cloud metadata service probe (Alibaba Cloud)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SCRIPT_SELF_DELETION_RM,
        label: "Script self-deletion (rm -- $0)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SCRIPT_SELF_DELETION_PYTHON,
        label: "Script self-deletion (os.remove(__file__))",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SCRIPT_SELF_DELETION_NODE,
        label: "Script self-deletion (fs.unlinkSync(__filename))",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SHELL_HISTORY_SUPPRESSION,
        label: "Shell history suppression (unset HISTFILE)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SHELL_HISTORY_CLEARING,
        label: "Shell history clearing (history -c/-w/-d/-a)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_AUDIT_DAEMON_DISABLE,
        label: "Audit daemon disable (auditctl -e 0)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_WINDOWS_EVENTLOG_CLEARING,
        label: "Windows event log clearing (wevtutil cl)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_AUDIT_DAEMON_STOP,
        label: "Audit daemon stop (systemctl stop auditd)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_CREDENTIAL_DUMPING_TOOL,
        label: "Credential dumping tool reference (mimikatz/sekurlsa/lsadump)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_LSASS_MEMORY_ACCESS,
        label: "LSASS process memory access (credential dumping)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_LD_PRELOAD_INJECTION,
        label: "LD_PRELOAD environment injection",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_GIT_HOOK_INJECTION,
        label: "Git hook injection (.git/hooks/ write)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_TIME_DELAYED_EXECUTION,
        label: "Time-delayed execution via at command",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_RCE_COMMAND_SUBSTITUTION,
        label: "Remote code execution via command substitution",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_REMOTE_FETCH_TO_VARIABLE,
        label: "Remote content fetched into variable for execution",
        severity: Severity::High,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SHELL_VARIABLE_EXECUTION,
        label: "Shell variable execution (eval/bash -c with variable)",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_DESTRUCTIVE_RECURSIVE_DELETE_SYSTEM,
        label: "Destructive recursive delete of system or home root",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_DESTRUCTIVE_RECURSIVE_DELETE_FIND,
        label: "Destructive recursive delete via find -exec rm",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SYSTEM_LOG_TRUNCATION,
        label: "System log truncation (forensic evasion)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SHELL_HISTORY_FILE_WIPE,
        label: "Shell history file wipe (forensic evasion)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_JOURNAL_LOG_VACUUM,
        label: "Journal log vacuum (forensic evasion)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_FORCED_LOG_ROTATION,
        label: "Forced log rotation (forensic evasion)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_CRON_PERSISTENCE,
        label: "Cron persistence (writing cron entry)",
        severity: Severity::Medium,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SYSTEMD_SERVICE_PERSISTENCE,
        label: "Systemd user service persistence (systemctl --user enable)",
        severity: Severity::Medium,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SYSTEMD_SERVICE_FILE_WRITE,
        label: "Systemd user service file write",
        severity: Severity::Medium,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SHELL_RC_PERSISTENCE,
        label: "Shell rc file write (persistence via alias/source injection)",
        severity: Severity::Medium,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_DNS_COVERT_CHANNEL,
        label: "DNS query with variable-constructed hostname (possible covert channel)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_DNS_TXT_LOOKUP,
        label: "DNS TXT record lookup (C2 indicator)",
        severity: Severity::Medium,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_POWERSHELL_ENCODED_COMMAND,
        label: "PowerShell encoded command (-enc/-EncodedCommand)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_POWERSHELL_IEX_CRADLE,
        label: "PowerShell IEX download cradle",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_POWERSHELL_EXECUTION_POLICY_BYPASS,
        label: "PowerShell ExecutionPolicy Bypass",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_POWERSHELL_HIDDEN_WINDOW,
        label: "PowerShell hidden window flag",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_PYTHON_REMOTE_FETCH,
        label: "Remote content fetched into variable for execution (Python)",
        severity: Severity::High,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_PYTHON_BASE64_DECODE_VARIABLE,
        label: "Base64-decoded content stored in variable",
        severity: Severity::High,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_PYTHON_EXEC_VARIABLE,
        label: "Python exec/eval of variable content",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_GITHUB_OIDC_TOKEN_READ,
        label: "GitHub Actions OIDC token environment variable read",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_OCTET_STREAM_POST,
        label: "Outbound POST with application/octet-stream",
        severity: Severity::Critical,
        intent: Intent::Negligent,
        code_only: true,
        doc_severity: None,
    },
    SensitivePattern {
        rule_id: RULE_SHELL_BASE64_LITERAL,
        label: "Shell variable assigned long base64 literal (obfuscation)",
        severity: Severity::Critical,
        intent: Intent::Malicious,
        code_only: false,
        doc_severity: None,
    },
];

// Regex strings indexed parallel to SENSITIVE_PATTERNS (same order, same count).
static SENSITIVE_PATTERN_STRS: &[&str] = &[
    // VTD-0001 — embedded private key header
    r"(?i)(?:BEGIN\s+(?:RSA|DSA|EC|OPENSSH)\s+PRIVATE\s+KEY)",
    // VTD-0002 — GitHub/OpenAI API tokens
    r"(?:ghp_[a-zA-Z0-9]{36}|github_pat_[a-zA-Z0-9_]{22,}|sk-[a-zA-Z0-9]{20,})",
    // VTD-0017 — eval() code injection
    r"(?i)\beval\s*\(",
    // VTD-0018 — shell exec without sandboxing
    r"(?i)(?:\bchild_process\b|(?:^|[^.\w])(?:exec|spawn|execSync|spawnSync|execFile|execFileSync)\s*\()",
    // VTD-0019 — destructive filesystem op
    r"(?im)(?:^|[\s;&|])(?:rm\s+-rf|rmdir/s|del\s+/f)\s+(?:/|~|\.{1,2}(?:/|\s|$)|\*|\$[{(]?[A-Za-z_])",
    // VTD-0020 — pipe to shell (curl/wget | bash)
    r"(?i)(?:curl|wget)\s+\S.*\|\s*(?:bash|sh|zsh|dash|ksh|python3?|node|perl|ruby)(?:\s|;|$)",
    // VTD-0020 — process substitution <(curl/wget)
    r"(?i)(?:bash|sh|zsh|dash)\s+<\(\s*(?:curl|wget)\s",
    // VTD-0028 — --no-verify safety bypass
    r"(?i)--no-verify",
    // VTD-0028 — --force safety bypass (low severity)
    r"(?i)--force",
    // VTD-0005 — .aws credentials/config
    r"(?i)\.aws[/\\](?:credentials|config)\b",
    // VTD-0005 — .config/gcloud credentials
    r"(?i)\.config[/\\]gcloud[/\\](?:application_default_credentials|credentials\.db|access_tokens\.db|legacy_credentials)",
    // VTD-0006 — SSH private key files
    r"(?i)\.ssh[/\\]id_(?:rsa|ed25519|ecdsa|dsa|ecdsa-sk|ed25519-sk)\b",
    // VTD-0007 — npm credential file
    r"(?i)~[/\\]\.npmrc\b",
    // VTD-0008 — PyPI credential file
    r"(?i)\.pypirc\b",
    // VTD-0009 — Docker credential file
    r"(?i)\.docker[/\\]config\.json\b",
    // VTD-0010 — Kubernetes credential file
    r"(?i)\.kube[/\\]config\b",
    // VTD-0011 — GitHub CLI credential file
    r"(?i)\.config[/\\]gh[/\\]hosts\.yml\b",
    // VTD-0012 — netrc credential file
    r"(?i)(?:~[/\\])?\.netrc\b",
    // VTD-0013 — macOS Keychain
    r"(?i)Library[/\\]Keychains\b",
    // VTD-0014 — Windows credential store
    r"(?i)(?:%APPDATA%|%LOCALAPPDATA%)[/\\]Microsoft[/\\](?:Credentials|Protect|Vault)\b",
    // VTD-0015 — Windows credential database
    r"(?i)[/\\]Windows[/\\]System32[/\\]config[/\\](?:SAM|SYSTEM|SECURITY)\b",
    // VTD-0016 — Active Directory NTDS.dit
    r"(?i)\bNTDS\.dit\b",
    // VTD-0029 — AWS IMDS (169.254.169.254 or hex/decimal equivalents)
    r"169\.254\.169\.254|2852039166|0[xX][aA]9[fF][eE][aA]9[fF][eE]",
    // VTD-0030 — GCP metadata endpoint
    r"(?i)metadata\.google\.internal",
    // VTD-0031 — Azure metadata endpoint
    r"(?i)metadata\.azure\.com",
    // VTD-0032 — Alibaba Cloud metadata endpoint
    r"100\.100\.100\.200",
    // VTD-0036 — script self-deletion via rm -- $0
    r#"rm\s+--\s+["']?\$0["']?"#,
    // VTD-0037 — script self-deletion via os.remove(__file__)
    r"os\.remove\s*\(\s*__file__\s*\)",
    // VTD-0038 — script self-deletion via fs.unlinkSync(__filename)
    r"fs\.unlinkSync\s*\(\s*__filename\s*\)",
    // VTD-0039 — shell history suppression
    r"unset\s+HISTFILE",
    // VTD-0040 — shell history clearing
    r"history\s+-[cwda]",
    // VTD-0042 — audit daemon disable
    r"auditctl\s+-e\s+0",
    // VTD-0044 — Windows event log clearing
    r"(?i)wevtutil\s+cl\b",
    // VTD-0043 — audit daemon stop
    r"systemctl\s+stop\s+auditd",
    // VTD-0033 — credential dumping tools
    r"(?i)\b(?:mimikatz|sekurlsa|lsadump|kerberoast)\b",
    // VTD-0034 — LSASS memory access
    r"(?i)\blsass\.(?:exe|dmp)\b",
    // VTD-0053 — LD_PRELOAD injection
    r"\bLD_PRELOAD\s*=",
    // VTD-0052 — git hook injection
    r"\.git[/\\]hooks[/\\]",
    // VTD-0054 — time-delayed execution via at(1)
    r"\|\s*at\s+(?:now\b|\d{1,2}:\d{2}|tomorrow\b|midnight\b|noon\b)",
    // VTD-0021 — RCE via command substitution (bash -c "$(curl ...)")
    r#"(?i)(?:bash|sh|zsh|dash|ksh)\s+-c\s+["']?\$\(\s*(?:curl|wget)\s"#,
    // VTD-0022 — remote content fetched into variable (VAR=$(curl ...))
    r"(?i)\b[A-Za-z_]\w*\s*=\s*\$\(\s*(?:curl|wget)\s+[^)]+\)",
    // VTD-0023 — shell variable execution (eval "$VAR" or bash -c "$VAR")
    r#"(?i)(?:\beval\s+["']?\$[A-Za-z_]|(?:bash|sh|zsh)\s+-c\s+["']?\$[A-Za-z_])"#,
    // VTD-0055 — destructive recursive delete of system/home root
    r"(?im)(?:^|[\s;&|])rm\s+-[rf]{1,2}\s+(?:/(?:var|etc|usr|bin|sbin|lib|boot|sys|proc|home|root|tmp)(?:[/\s;]|$)|~/?(?:\s|;|&|$)|\$(?:HOME|\{HOME\})/?(?:\s|;|&|$))",
    // VTD-0056 — destructive recursive delete via find -exec rm
    r"(?i)\bfind\s+(?:/|~|\$(?:HOME|\{HOME\}))\s+[^\n;]+?-exec\s+rm\b",
    // VTD-0045 — system log truncation
    r#"(?im)(?:truncate\s+-s\s+0\s+["']?|(?:^|[\s;&|])>\s*["']?)(?:~|(?:/var/log))/(?:auth\.log|syslog|audit/audit\.log|kern\.log|dpkg\.log|messages|secure)\b"#,
    // VTD-0041 — shell history file wipe
    r#"(?im)(?:truncate\s+-s\s+0\s+["']?|(?:^|[\s;&|])>\s*["']?)(?:~|/root|\$(?:HOME|\{HOME\}))/\.(?:bash_history|zsh_history|history|python_history)\b"#,
    // VTD-0046 — journal log vacuum
    r"\bjournalctl\s+--vacuum-(?:time|size)\b",
    // VTD-0047 — forced log rotation
    r"\blogrotate\s+-f\b",
    // VTD-0048 — cron persistence
    r"(?i)(?:echo\s+[^|;\n]{1,300}\|\s*crontab\s+-|\(?crontab\s+-l[^)]*\)?[^|]*\|\s*crontab|(?:tee\s+|>>?\s*)[^;&\n]*/(?:etc/cron\.(?:d|daily|hourly|weekly|monthly)|var/spool/cron)/)",
    // VTD-0049 — systemd service persistence
    r"\bsystemctl\s+--user\s+enable\b",
    // VTD-0050 — systemd service file write
    r"~/\.config/systemd/user/[^/\s]+\.service\b",
    // VTD-0051 — shell rc persistence
    r"(?i)(?:>>|tee\s+-a)\s+[^;&\n]*~/\.(?:bashrc|zshrc|profile|bash_profile)\b",
    // VTD-0057 — DNS covert channel (variable-constructed hostname)
    r"(?i)(?:dig|nslookup|host)\s+[^;&\n]*\$(?:[{(]?[A-Za-z_]\w*[})]?)\s*\.[a-zA-Z]",
    // VTD-0058 — DNS TXT record lookup
    r"(?i)(?:dig\s+(?:[^;&\n]*\s+)?TXT\b|dig\s+TXT\b|nslookup\s+-(?:type|querytype)=txt)",
    // VTD-0060 — PowerShell encoded command (-enc not -encoding)
    r#"(?i)(?:^|[\s;|"'])-(?:enc\b|EncodedCommand)\b\s+[A-Za-z0-9+/=]{16,}"#,
    // VTD-0061 — PowerShell IEX download cradle
    r"(?i)(?:IEX|Invoke-Expression)\b[\s\S]{0,200}?(?:DownloadString|DownloadFile|WebClient|Invoke-WebRequest|\biwr\b)",
    // VTD-0062 — PowerShell ExecutionPolicy bypass
    r"(?i)-ExecutionPolicy\s+(?:Bypass|Unrestricted)\b",
    // VTD-0063 — PowerShell hidden window
    r"(?i)(?:-WindowStyle\s+Hidden|(?:^|[\s;|])-w\s+hidden)\b",
    // VTD-0024 — Python remote fetch to variable
    r"(?i)\b\w+\s*=\s*(?:urllib\.request\.urlopen|urlopen|requests\.get|requests\.post)\s*\([^)]+\)\s*\.\s*(?:read|text|content)\b",
    // VTD-0025 — Python base64 decode to variable
    r"(?i)\b\w+\s*=\s*base64\.b64decode\s*\(",
    // VTD-0026 — Python exec/eval of variable
    r"(?i)(?:^|[^.\w])(?:exec|eval)\s*\(\s*\w+\s*[,)]",
    // VTD-0035 — GitHub Actions OIDC token
    r"\bACTIONS_ID_TOKEN_REQUEST_(?:TOKEN|URL)\b",
    // VTD-0059 — outbound POST with application/octet-stream
    r#"(?i)(?:Content-Type|content[_-]?type)["']?\s*[:=]\s*["']?application/octet-stream"#,
    // VTD-0027 — shell variable assigned long base64 literal
    r#"\b[A-Za-z_]\w*=["'][A-Za-z0-9+/]{40,}={0,2}["']"#,
];

static SENSITIVE_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();

fn get_sensitive_regexes() -> &'static [Regex] {
    SENSITIVE_REGEXES.get_or_init(|| {
        SENSITIVE_PATTERN_STRS
            .iter()
            .map(|s| Regex::new(s).expect("invalid sensitive pattern"))
            .collect()
    })
}

/// Scan all text files for SENSITIVE_PATTERNS. Returns findings and whether
/// any critical/high security finding was found (used to suppress VTD-0091).
fn scan_sensitive_patterns(text_files: &HashMap<String, String>) -> (Vec<Finding>, bool) {
    let mut findings: Vec<Finding> = Vec::new();
    let regexes = get_sensitive_regexes();

    let mut sorted_files: Vec<(&String, &String)> = text_files.iter().collect();
    sorted_files.sort_by_key(|(p, _)| p.as_str());
    for (path, content) in sorted_files {
        let is_doc = path.to_lowercase().ends_with(".md");
        let lines: Vec<&str> = content.split('\n').collect();

        for (i_pat, pat) in SENSITIVE_PATTERNS.iter().enumerate() {
            if pat.code_only && is_doc {
                continue;
            }
            let effective_severity = if is_doc {
                pat.doc_severity
                    .clone()
                    .unwrap_or_else(|| pat.severity.clone())
            } else {
                pat.severity.clone()
            };
            let re = &regexes[i_pat];
            for (i_line, line) in lines.iter().enumerate() {
                if re.is_match(line) {
                    let snippet = line.trim();
                    let snippet = &snippet[..snippet.len().min(120)];
                    let detail = format!("Detected in {path}:{} — `{snippet}`", i_line + 1);
                    findings.push(Finding {
                        rule_id: pat.rule_id.to_string(),
                        category: FindingCategory::Security,
                        severity: effective_severity,
                        label: pat.label.to_string(),
                        detail,
                        filepath: Some(path.clone()),
                        owasp_llm_category: None,
                        chain_id: None,
                        intent: Some(pat.intent.clone()),
                        source: DEFAULT_SOURCE.to_string(),
                    });
                    break; // first match per pattern per file only
                }
            }
        }
    }

    let secrets_check_failed = findings
        .iter()
        .any(|f| matches!(f.severity, Severity::Critical | Severity::High));

    (findings, secrets_check_failed)
}

// ── Entropy scan ──────────────────────────────────────────────────────────────

fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut freq = [0u32; 256];
    for b in s.bytes() {
        freq[b as usize] += 1;
    }
    let len = s.len() as f64;
    freq.iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

static ASSIGNMENT_QUOTED_VALUE_STR: &str =
    r#"(?:["']?([A-Za-z_][A-Za-z0-9_.:-]*)["']?\s*[:=]\s*["']([^"'\r\n]{20,})["'])"#;
static SUSPICIOUS_SECRET_KEY_STR: &str = r"(?i)(?:^|[-_.])(?:api[-_.]?key|access[-_.]?token|auth[-_.]?token|refresh[-_.]?token|token|secret|password|passwd|pwd|private[-_.]?key|client[-_.]?secret|credential|credentials|bearer)(?:$|[-_.])";

static ASSIGNMENT_QUOTED_VALUE_RE: OnceLock<Regex> = OnceLock::new();
static SUSPICIOUS_SECRET_KEY_RE: OnceLock<Regex> = OnceLock::new();

// ── Behavioral scan ───────────────────────────────────────────────────────────

const NEGATION_LOOKBACK_CHARS: usize = 40;

struct BehavioralPatternRaw {
    rule_id: &'static str,
    pattern_str: &'static str,
    label: &'static str,
    severity: &'static str,
    respect_negation: bool,
}

static BEHAVIORAL_PATTERN_DEFS: &[BehavioralPatternRaw] = &[
    // PROMPT_INJECTION_PATTERNS
    BehavioralPatternRaw {
        rule_id: RULE_PROMPT_INSTRUCTION_OVERRIDE,
        pattern_str: r"(?i)\b(?:ignore|disregard|forget|discard|skip)\s+(?:all\s+|every\s+|the\s+|any\s+|your\s+|my\s+|these\s+|those\s+)*(?:previous|prior|above|earlier|preceding|original|initial|former)\s+(?:instructions?|rules?|directives?|commands?|guidelines?|prompts?|messages?|context|system\s+prompts?|system\s+messages?)\b",
        label: "Instruction override language detected",
        severity: "critical",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_PROMPT_INSTRUCTION_OVERRIDE,
        pattern_str: r"(?i)\bignore\s+everything\s+(?:above|before|prior|earlier|written\s+above|that\s+(?:came|was)\s+(?:before|earlier|prior))\b",
        label: "Instruction override language detected",
        severity: "critical",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_SYSTEM_PROMPT_REPLACEMENT,
        pattern_str: r"(?i)\byour\s+(?:new|real|actual|true|secret|hidden|primary|updated)\s+(?:instructions?|task|job|purpose|mission|directive|goal|objective|role)\s+(?:is|are|will\s+be|shall\s+be)\b",
        label: "System prompt replacement attempt",
        severity: "critical",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_SYSTEM_PROMPT_OVERRIDE,
        pattern_str: r"(?i)\b(?:override|replace|substitute|supersede|overwrite)\s+(?:your|the\s+)?(?:system\s+)?(?:prompt|instructions?|programming|training)\b",
        label: "System prompt override attempt",
        severity: "critical",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_CONTEXT_INVALIDATION,
        pattern_str: r"(?i)\b(?:the\s+(?:above|previous|prior)|previous\s+(?:messages?|instructions?|context)|prior\s+context)\s+(?:is|are|was|were)\s+(?:fake|false|a\s+test|just\s+a\s+test|incorrect|wrong|outdated|invalid)\b",
        label: "Context invalidation attempt",
        severity: "critical",
        respect_negation: false,
    },
    // JAILBREAK_FRAMING_PATTERNS
    BehavioralPatternRaw {
        rule_id: RULE_JAILBREAK_PERSONA,
        pattern_str: r"(?i)\b(?:you\s+are|act\s+as|pretend\s+(?:to\s+be|you(?:'re|\s+are))|roleplay\s+as)\s+(?:now\s+)?(?:DAN|STAN|DUDE|AIM|do\s+anything\s+now)\b",
        label: "Named jailbreak persona invocation",
        severity: "critical",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_SAFETY_SYSTEM_BYPASS,
        pattern_str: r"(?i)\b(?:disable|deactivate|turn\s+off|switch\s+off)\s+(?:your|the|all)?\s*(?:safety|security|ethical|moral|content)\s+(?:filters?|guidelines?|guards?|protocols?|checks?)\b",
        label: "Safety system bypass instruction",
        severity: "critical",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_UNRESTRICTED_OPERATION_FRAMING,
        pattern_str: r"(?i)\b(?:have|with|under|operating\s+with|acting\s+with|free\s+from|without)\s+(?:absolutely\s+)?no\s+(?:restrictions?|limitations?|constraints?|safeguards?|boundaries|inhibitions)\b",
        label: "Unrestricted operation framing",
        severity: "medium",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_ETHICAL_BYPASS_FRAMING,
        pattern_str: r"(?i)\b(?:without|with\s+no|free\s+from|unbound\s+by|not\s+bound\s+by)\s+(?:any\s+)?(?:ethical|safety|moral|legal|content)\s+(?:restrictions?|guidelines?|constraints?|considerations?|filters?|principles?|policies?)\b",
        label: "Ethical-bypass framing",
        severity: "medium",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_ETHICAL_BYPASS_FRAMING,
        pattern_str: r"(?i)\bno\s+(?:ethical|moral|safety|legal)\s+(?:considerations?|guidelines?|constraints?|principles?|concerns?|limitations?|rules?)\s+(?:apply|exist|are\s+(?:needed|required|necessary)|matter)\b",
        label: "Ethical-bypass framing",
        severity: "medium",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_ROLEPLAY_BYPASS_FRAMING,
        pattern_str: r"(?i)\b(?:in\s+this\s+(?:roleplay|scenario|game|simulation)|for\s+the\s+purposes?\s+of\s+this\s+(?:roleplay|scenario|game|simulation))\b[^.!?]{0,60}?\b(?:can|may|will|must|should|are\s+allowed\s+to)\s+(?:ignore|bypass|disregard|skip|forget|disable)\b",
        label: "Roleplay-scoped bypass framing",
        severity: "medium",
        respect_negation: false,
    },
    // CREDENTIAL_SOLICITATION_PATTERNS
    BehavioralPatternRaw {
        rule_id: RULE_CREDENTIAL_SOLICITATION,
        pattern_str: r"(?i)\b(?:ask|request|prompt|query|have|get|obtain|collect|gather|solicit|elicit|tell|instruct|direct|require)\s+(?:the\s+|each\s+|every\s+)?users?\s+(?:to\s+(?:provide|give|enter|share|input|reveal|disclose|type|paste|submit)|for(?:\s+(?:their|a|an|the))?)\s*(?:their\s+|the\s+|a\s+|an\s+)?(?:passwords?|api[-\s_]?keys?|access[-\s_]?tokens?|secret[-\s_]?keys?|secrets?|credentials?|private[-\s_]?keys?|auth(?:entication)?[-\s_]?tokens?|session[-\s_]?tokens?|bearer[-\s_]?tokens?|2fa[-\s_]?(?:codes?|tokens?)?|otps?|pins?|ssns?|seed[-\s_]?phrases?|recovery[-\s_]?(?:keys?|phrases?))\b",
        label: "Instruction to solicit user credentials",
        severity: "high",
        respect_negation: true,
    },
    BehavioralPatternRaw {
        rule_id: RULE_DECEPTIVE_CREDENTIAL_EXTRACTION,
        pattern_str: r"(?i)\b(?:convince|persuade|trick|manipulate|coerce|deceive|fool)\s+(?:the\s+|each\s+)?users?\s+(?:into\s+|to\s+)(?:provide|give|enter|share|reveal|disclose|hand\s+over)[^.!?]{0,60}(?:passwords?|api[-\s_]?keys?|tokens?|secrets?|credentials?|private[-\s_]?keys?|pins?)\b",
        label: "Deceptive credential extraction",
        severity: "critical",
        respect_negation: false,
    },
    // INJECTION_MARKER_PATTERNS
    BehavioralPatternRaw {
        rule_id: RULE_PROMPT_TEMPLATE_MARKER,
        pattern_str: r"(?i)\[(?:SYSTEM|SYS|SYSTEM[\s_-]+(?:PROMPT|MESSAGE|MSG|INSTRUCTION|INST)|INST|/INST|INSTRUCTION|HUMAN|ASSISTANT)\]",
        label: "Embedded prompt-template marker",
        severity: "medium",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_PROMPT_TEMPLATE_MARKER,
        pattern_str: r"(?i)</?(?:system|system_prompt|system_message|instruction|inst|sys|im_start|im_end)(?:\s[^>]*)?>",
        label: "Embedded prompt-template marker",
        severity: "medium",
        respect_negation: false,
    },
    BehavioralPatternRaw {
        rule_id: RULE_CHAT_TEMPLATE_SPECIAL_TOKEN,
        pattern_str: r"(?i)<\|(?:system|user|assistant|im_start|im_end|endoftext|end_of_text|begin_of_text|eot_id|start_header_id|end_header_id)\|>",
        label: "Embedded chat-template special token",
        severity: "medium",
        respect_negation: false,
    },
];

struct CompiledBehavioralPattern {
    rule_id: &'static str,
    regex: Regex,
    label: &'static str,
    severity: &'static str,
    respect_negation: bool,
}

static BEHAVIORAL_REGEXES: OnceLock<Vec<CompiledBehavioralPattern>> = OnceLock::new();
static NEGATION_PRECEDENTS_RE: OnceLock<Regex> = OnceLock::new();

fn get_behavioral_patterns() -> &'static Vec<CompiledBehavioralPattern> {
    BEHAVIORAL_REGEXES.get_or_init(|| {
        BEHAVIORAL_PATTERN_DEFS
            .iter()
            .map(|def| CompiledBehavioralPattern {
                rule_id: def.rule_id,
                regex: Regex::new(def.pattern_str).expect("invalid behavioral pattern"),
                label: def.label,
                severity: def.severity,
                respect_negation: def.respect_negation,
            })
            .collect()
    })
}

fn normalize_for_behavioral_scan(content: &str) -> String {
    static HWS_RE: OnceLock<Regex> = OnceLock::new();
    let hws_re = HWS_RE.get_or_init(|| Regex::new(r"[ \t]+").expect("bad hws re"));
    let lower = content.to_lowercase();
    lower
        .split('\n')
        .map(|line| hws_re.replace_all(line, " ").trim().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_markdown_example_content(content: &str) -> String {
    static FENCE_RE: OnceLock<Regex> = OnceLock::new();
    static HEADING_RE: OnceLock<Regex> = OnceLock::new();
    static EXAMPLE_HEADING_RE: OnceLock<Regex> = OnceLock::new();
    static BLOCKQUOTE_RE: OnceLock<Regex> = OnceLock::new();
    let fence_re = FENCE_RE.get_or_init(|| Regex::new(r"^(\s*)(```|~~~)").expect("bad fence re"));
    let heading_re = HEADING_RE.get_or_init(|| Regex::new(r"^(#{1,6})\s").expect("bad heading re"));
    let example_heading_re = EXAMPLE_HEADING_RE.get_or_init(|| {
        Regex::new(r"(?i)^#{1,4}\s+(?:examples?|test\s+cases?|negative\s+examples?|sample\s+(?:attacks?|injections?|payloads?)|what\s+(?:not\s+to\s+do|to\s+look\s+for|to\s+watch\s+for)|detection\s+(?:examples?|patterns?|rules?)|known\s+(?:attacks?|patterns?|techniques?)|red[\s-]team)").expect("bad example heading re")
    });
    let blockquote_re =
        BLOCKQUOTE_RE.get_or_init(|| Regex::new(r"^\s*>").expect("bad blockquote re"));

    let mut output: Vec<&str> = Vec::new();
    let mut in_fenced_block = false;
    let mut fence_is_backtick = false;
    let mut in_example_section = false;
    let mut example_section_level = 0usize;

    for line in content.split('\n') {
        if let Some(cap) = fence_re.captures(line) {
            let marker = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            if !in_fenced_block {
                in_fenced_block = true;
                fence_is_backtick = marker.starts_with('`');
                output.push("");
                continue;
            } else {
                let expected = if fence_is_backtick { "```" } else { "~~~" };
                if line.trim().starts_with(expected) {
                    in_fenced_block = false;
                    output.push("");
                    continue;
                }
            }
        }
        if in_fenced_block {
            output.push("");
            continue;
        }
        if blockquote_re.is_match(line) {
            output.push("");
            continue;
        }
        if let Some(cap) = heading_re.captures(line) {
            let level = cap.get(1).map(|m| m.len()).unwrap_or(0);
            if in_example_section && level <= example_section_level {
                in_example_section = false;
            }
            if !in_example_section && example_heading_re.is_match(line) {
                in_example_section = true;
                example_section_level = level;
                output.push("");
                continue;
            }
        }
        if in_example_section {
            output.push("");
            continue;
        }
        output.push(line);
    }
    output.join("\n")
}

/// Scan all text files for behavioral injection patterns (BEHAVIORAL_PATTERNS).
/// Returns (findings, behavioral_check_failed) — mirrors vettd's inline behavioral scan.
fn scan_behavioral_patterns(text_files: &HashMap<String, String>) -> (Vec<Finding>, bool) {
    let patterns = get_behavioral_patterns();
    let negation_re = NEGATION_PRECEDENTS_RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:never|don'?t|do\s+not|avoid|prevent|stop|warn|forbid|disallow|refuse|cannot|can'?t|won'?t|would\s+not|should\s+not|shouldn'?t|must\s+not|mustn'?t)\b[^.!?]{0,30}$",
        )
        .expect("bad negation precedents re")
    });

    let mut findings: Vec<Finding> = Vec::new();
    let mut behavioral_check_failed = false;
    const BEHAVIORAL_SCAN_MAX_BYTES: usize = 100 * 1024;

    let mut sorted_files: Vec<(&String, &String)> = text_files.iter().collect();
    sorted_files.sort_by_key(|(p, _)| p.as_str());
    for (path, content) in sorted_files {
        let is_oversized = content.len() > BEHAVIORAL_SCAN_MAX_BYTES;
        let scan_content: &str = if is_oversized {
            let mut end = BEHAVIORAL_SCAN_MAX_BYTES;
            while end > 0 && !content.is_char_boundary(end) {
                end -= 1;
            }
            &content[..end]
        } else {
            content.as_str()
        };
        if is_oversized {
            findings.push(Finding {
                rule_id: RULE_BEHAVIORAL_SCAN_TRUNCATED.to_string(),
                category: FindingCategory::Security,
                severity: Severity::Info,
                label: "Behavioral scan truncated".to_string(),
                detail: format!(
                    "{path} exceeds {}KB — content past that limit was not scanned for behavioral signals",
                    BEHAVIORAL_SCAN_MAX_BYTES / 1024
                ),
                filepath: Some(path.clone()),
                owasp_llm_category: None,
                chain_id: None,
                intent: None,
                source: DEFAULT_SOURCE.to_string(),
            });
        }

        let is_markdown = path.to_lowercase().ends_with(".md");
        let stripped: String;
        let normalized = if is_markdown {
            stripped = strip_markdown_example_content(scan_content);
            normalize_for_behavioral_scan(&stripped)
        } else {
            normalize_for_behavioral_scan(scan_content)
        };
        let normalized_lines: Vec<&str> = normalized.split('\n').collect();

        for bp in patterns {
            let mut match_count = 0usize;
            let mut first_match_line: Option<usize> = None;
            let mut first_match_snippet = String::new();

            for (i, line) in normalized_lines.iter().enumerate() {
                for m in bp.regex.find_iter(line) {
                    if bp.respect_negation {
                        let pre_start = m.start().saturating_sub(NEGATION_LOOKBACK_CHARS);
                        let pre = &line[pre_start..m.start()];
                        if negation_re.is_match(pre) {
                            continue;
                        }
                    }
                    match_count += 1;
                    if first_match_line.is_none() {
                        first_match_line = Some(i + 1);
                        first_match_snippet = line.trim().chars().take(120).collect();
                    }
                }
            }

            if match_count > 0 {
                if let Some(line_num) = first_match_line {
                    let count_note = if match_count > 1 {
                        format!(" ({match_count} matches)")
                    } else {
                        String::new()
                    };
                    let snippet = if !first_match_snippet.is_empty() {
                        format!(" — `{first_match_snippet}`")
                    } else {
                        String::new()
                    };
                    let severity = match bp.severity {
                        "critical" => Severity::Critical,
                        "high" => Severity::High,
                        "medium" => Severity::Medium,
                        "low" => Severity::Low,
                        _ => Severity::Info,
                    };
                    if matches!(severity, Severity::Critical | Severity::High) {
                        behavioral_check_failed = true;
                    }
                    findings.push(Finding {
                        rule_id: bp.rule_id.to_string(),
                        category: FindingCategory::Security,
                        severity,
                        label: bp.label.to_string(),
                        detail: format!("Detected in {path}:{line_num}{count_note}{snippet}"),
                        filepath: Some(path.clone()),
                        owasp_llm_category: None,
                        chain_id: None,
                        intent: None,
                        source: DEFAULT_SOURCE.to_string(),
                    });
                }
            }
        }
    }

    (findings, behavioral_check_failed)
}

/// Scan for hidden Unicode / invisible characters (VTD-0081) — mirrors vettd's inline scan.
/// When invisible chars are present but conceal no dangerous payload, emits VTD-0081.
/// Dangerous-payload cases (obfuscated code/behavioral/network) are already covered by
/// check_base64_payloads and scan_sensitive_patterns; this function handles the benign case.
fn scan_hidden_unicode(text_files: &HashMap<String, String>, findings: &mut Vec<Finding>) {
    // Unicode invisible/control character ranges (same as vettd's INVISIBLE_CHAR_TEST /u regex):
    // U+200B–U+200F, U+202A–U+202E, U+2060–U+206F, U+FEFF, U+E0000–U+E007F
    fn has_invisible(s: &str) -> bool {
        s.chars().any(|c| {
            matches!(c,
                '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{E0000}'..='\u{E007F}'
            )
        })
    }

    let sensitive_regexes = get_sensitive_regexes();
    let behavioral_patterns = get_behavioral_patterns();

    static OBFUSC_URL_RE: OnceLock<Regex> = OnceLock::new();
    let obfusc_url_re =
        OBFUSC_URL_RE.get_or_init(|| Regex::new(r#"(?i)https?://[^\s)>\]"']+"#).expect("bad url re"));

    let mut sorted_files: Vec<(&String, &String)> = text_files.iter().collect();
    sorted_files.sort_by_key(|(p, _)| p.as_str());

    let mut obfusc_chain_count: u32 = 0;

    for (path, content) in sorted_files {
        let lines: Vec<&str> = content.split('\n').collect();
        let mut found_dangerous = false;
        let mut first_invisible_line: Option<usize> = None;

        'lines: for (i, line) in lines.iter().enumerate() {
            if !has_invisible(line) {
                continue;
            }
            if first_invisible_line.is_none() {
                first_invisible_line = Some(i);
            }

            // Strip invisible chars and check for dangerous patterns
            let cleaned: String = line
                .chars()
                .filter(|&c| !matches!(c,
                    '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2060}'..='\u{206F}'
                    | '\u{FEFF}'
                    | '\u{E0000}'..='\u{E007F}'
                ))
                .collect();

            // Check sensitive patterns
            for (i_pat, pat) in SENSITIVE_PATTERNS.iter().enumerate() {
                if sensitive_regexes[i_pat].is_match(&cleaned) {
                    findings.push(Finding {
                        rule_id: RULE_OBFUSCATED_DANGEROUS_CODE.to_string(),
                        category: FindingCategory::Security,
                        severity: Severity::Critical,
                        label: "Obfuscated dangerous code".to_string(),
                        detail: format!(
                            "Hidden Unicode in {path}:{} concealed a dangerous pattern: {}",
                            i + 1,
                            pat.label
                        ),
                        filepath: Some(path.clone()),
                        owasp_llm_category: None,
                        chain_id: None,
                        intent: Some(Intent::Malicious),
                        source: DEFAULT_SOURCE.to_string(),
                    });
                    found_dangerous = true;
                    break 'lines;
                }
            }

            // Check behavioral patterns
            {
                let normalized = normalize_for_behavioral_scan(&cleaned);
                for bp in behavioral_patterns {
                    if bp.regex.is_match(&normalized) {
                        findings.push(Finding {
                            rule_id: RULE_OBFUSCATED_DANGEROUS_CODE.to_string(),
                            category: FindingCategory::Security,
                            severity: Severity::Critical,
                            label: "Obfuscated dangerous code".to_string(),
                            detail: format!(
                                "Hidden Unicode in {path}:{} concealed a behavioral signal: {}",
                                i + 1,
                                bp.label
                            ),
                            filepath: Some(path.clone()),
                            owasp_llm_category: None,
                            chain_id: None,
                            intent: Some(Intent::Malicious),
                            source: DEFAULT_SOURCE.to_string(),
                        });
                        found_dangerous = true;
                        break 'lines;
                    }
                }
            }

            // Check for obfuscated external URL (dead-drop)
            if !found_dangerous && obfusc_url_re.is_match(&cleaned) {
                findings.push(Finding {
                    rule_id: RULE_OBFUSCATED_EXTERNAL_URL.to_string(),
                    category: FindingCategory::Security,
                    severity: Severity::Critical,
                    label: "Obfuscated external URL (dead-drop)".to_string(),
                    detail: format!(
                        "Hidden Unicode in {path}:{} concealed an external URL",
                        i + 1
                    ),
                    filepath: Some(path.clone()),
                    owasp_llm_category: None,
                    chain_id: Some(format!("obfusc-uni-{obfusc_chain_count}")),
                    intent: Some(Intent::Malicious),
                    source: DEFAULT_SOURCE.to_string(),
                });
                obfusc_chain_count += 1;
                found_dangerous = true;
            }

            if found_dangerous {
                break 'lines;
            }
        }

        // Invisible chars present but no dangerous payload — emit presence warning
        if let Some(line_idx) = first_invisible_line {
            if !found_dangerous {
                findings.push(Finding {
                    rule_id: RULE_HIDDEN_UNICODE_CHARACTER.to_string(),
                    category: FindingCategory::Security,
                    severity: Severity::Medium,
                    label: "Hidden Unicode character detected".to_string(),
                    detail: format!(
                        "Invisible formatting/control character in {path}:{}. \
                        May conceal prompt injection content.",
                        line_idx + 1
                    ),
                    filepath: Some(path.clone()),
                    owasp_llm_category: None,
                    chain_id: None,
                    intent: None,
                    source: DEFAULT_SOURCE.to_string(),
                });
            }
        }
    }
}

fn scan_entropy(text_files: &HashMap<String, String>, findings: &mut Vec<Finding>) {
    let assign_re = ASSIGNMENT_QUOTED_VALUE_RE
        .get_or_init(|| Regex::new(ASSIGNMENT_QUOTED_VALUE_STR).expect("bad entropy regex"));
    let key_re = SUSPICIOUS_SECRET_KEY_RE
        .get_or_init(|| Regex::new(SUSPICIOUS_SECRET_KEY_STR).expect("bad key regex"));

    let mut sorted_files: Vec<(&String, &String)> = text_files.iter().collect();
    sorted_files.sort_by_key(|(p, _)| p.as_str());
    for (path, content) in sorted_files {
        if path.to_lowercase().ends_with(".md") {
            continue;
        }
        for (i_line, line) in content.split('\n').enumerate() {
            for cap in assign_re.captures_iter(line) {
                let key = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let value = cap.get(2).map(|m| m.as_str()).unwrap_or("");
                if value.len() < 20 {
                    continue;
                }
                if !key_re.is_match(key) {
                    continue;
                }
                if shannon_entropy(value) >= 3.5 {
                    let snippet = line.trim();
                    let snippet = &snippet[..snippet.len().min(120)];
                    findings.push(Finding {
                        rule_id: RULE_HIGH_ENTROPY_SECRET.to_string(),
                        category: FindingCategory::Security,
                        severity: Severity::Critical,
                        label: "High-entropy value — potential hardcoded secret".to_string(),
                        detail: format!("Detected in {path}:{} — `{snippet}`", i_line + 1),
                        filepath: Some(path.clone()),
                        owasp_llm_category: None,
                        chain_id: None,
                        intent: None,
                        source: DEFAULT_SOURCE.to_string(),
                    });
                    break; // one finding per line
                }
            }
        }
    }
}

fn scan_env_files(text_files: &HashMap<String, String>, findings: &mut Vec<Finding>) {
    static ENV_FILE_RE: OnceLock<Regex> = OnceLock::new();
    let re =
        ENV_FILE_RE.get_or_init(|| Regex::new(r"(?:^|/)\.env($|\.)").expect("bad env file regex"));
    let mut sorted_paths: Vec<&String> = text_files.keys().collect();
    sorted_paths.sort();
    for path in sorted_paths {
        if re.is_match(path) {
            findings.push(Finding {
                rule_id: RULE_ENV_FILE_IN_PACKAGE.to_string(),
                category: FindingCategory::Security,
                severity: Severity::Critical,
                label: "Environment file included in skill package".to_string(),
                detail: format!("Found {path} — should be excluded from distribution"),
                filepath: Some(path.clone()),
                owasp_llm_category: None,
                chain_id: None,
                intent: None,
                source: DEFAULT_SOURCE.to_string(),
            });
        }
    }
}

// ── Malicious activity chain detection ────────────────────────────────────────
// Mirrors vettd's detectMaliciousActivityChains().
// Groups security findings by file, classifies into EVASION/PERSISTENCE/etc.
// buckets, and emits a chain finding when 2+ distinct buckets co-occur.

const EVASION_FRAGS: &[&str] = &[
    "Shell history",
    "Audit daemon",
    "Windows event log clearing",
    "Script self-deletion",
    "System log truncation",
    "Shell history file wipe",
    "Journal log vacuum",
    "Forced log rotation",
];

const PERSISTENCE_FRAGS: &[&str] = &[
    "Cron persistence",
    "Systemd user service",
    "Shell rc file write",
    "Time-delayed execution via at",
    "Git hook injection",
    "LD_PRELOAD environment injection",
];

const FETCH_FRAGS: &[&str] = &[
    "Remote content fetched into variable",
    "Remote content fetched into variable for execution (Python)",
    "Base64-decoded content stored in variable",
];

const EXECUTION_FRAGS: &[&str] = &[
    "Remote code execution via command substitution",
    "Shell variable execution",
    "Remote code execution via pipe to shell",
    "PowerShell encoded command",
    "PowerShell IEX download cradle",
    "Python exec/eval of variable content",
];

const COVERT_CHANNEL_FRAGS: &[&str] = &[
    "DNS query with variable-constructed hostname",
    "DNS TXT record lookup",
    "Outbound POST with application/octet-stream",
];

fn classify_malicious_bucket(label: &str) -> Option<&'static str> {
    if EVASION_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("EVASION");
    }
    if PERSISTENCE_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("PERSISTENCE");
    }
    if FETCH_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("FETCH");
    }
    if EXECUTION_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("EXECUTION");
    }
    if COVERT_CHANNEL_FRAGS.iter().any(|f| label.contains(f)) {
        return Some("COVERT_CHANNEL");
    }
    None
}

fn extract_filepath_from_detail(detail: &str) -> Option<&str> {
    let rest = detail.strip_prefix("Detected in ")?;
    let colon = rest.find(':')?;
    Some(&rest[..colon])
}

fn detect_malicious_activity_chains(findings: &mut Vec<Finding>) {
    // Group bucket-classified findings by file path (extracted from detail).
    let mut buckets_by_file: HashMap<String, Vec<&'static str>> = HashMap::new();
    // Track which finding indices belong to each file.
    let mut indices_by_file: HashMap<String, Vec<usize>> = HashMap::new();

    for (idx, finding) in findings.iter().enumerate() {
        if finding.category != FindingCategory::Security {
            continue;
        }
        let Some(file_path) = extract_filepath_from_detail(&finding.detail) else {
            continue;
        };
        let file_path = file_path.to_string();

        indices_by_file
            .entry(file_path.clone())
            .or_default()
            .push(idx);

        if let Some(bucket) = classify_malicious_bucket(&finding.label) {
            let buckets = buckets_by_file.entry(file_path).or_default();
            // Maintain insertion order with dedup (mirrors JS Set).
            if !buckets.contains(&bucket) {
                buckets.push(bucket);
            }
        }
    }

    let mut chain_index: u32 = 0;
    let mut new_findings: Vec<Finding> = Vec::new();

    for (file_path, buckets) in &buckets_by_file {
        let file_indices = indices_by_file.get(file_path).cloned().unwrap_or_default();

        // Condition A: 2+ distinct buckets.
        // Condition B: 1 bucket + external malicious finding not in any bucket.
        let has_external_malicious = file_indices.iter().any(|&idx| {
            let f = &findings[idx];
            matches!(f.severity, Severity::Critical | Severity::High)
                && f.intent == Some(Intent::Malicious)
                && f.chain_id.is_none()
                && classify_malicious_bucket(&f.label).is_none()
        });

        if buckets.len() < 2 && !has_external_malicious {
            continue;
        }

        let chain_id = format!("mal-activity-{chain_index}");
        chain_index += 1;

        // Mutate component findings: assign chainId, escalate intent/severity.
        for &idx in &file_indices {
            let f = &mut findings[idx];
            if f.chain_id.is_some() {
                continue;
            }
            if classify_malicious_bucket(&f.label).is_none() {
                continue;
            }
            f.chain_id = Some(chain_id.clone());
            f.intent = Some(Intent::Malicious);
            if f.severity != Severity::Critical {
                f.severity = Severity::Critical;
            }
        }

        let bucket_list = buckets.join(" + ");
        new_findings.push(Finding {
            rule_id: RULE_MALICIOUS_ACTIVITY_CHAIN.to_string(),
            category: FindingCategory::Security,
            severity: Severity::Critical,
            label: "Multiple malicious-activity indicators in same file".to_string(),
            detail: format!(
                "{file_path} contains {bucket_list} indicators that co-occur in a malicious pattern."
            ),
            filepath: Some(file_path.clone()),
            owasp_llm_category: None,
            chain_id: Some(chain_id),
            intent: Some(Intent::Malicious),
            source: DEFAULT_SOURCE.to_string(),
        });
    }

    findings.extend(new_findings);
}

// ── Credential-exfiltration chain detection ───────────────────────────────────

const CRED_SOURCE_FRAGS: &[&str] = &[
    "credential file access",
    "private key file access",
    "Keychain file access",
    "hardcoded secret",
    "API key",
    ".env",
    "High-entropy value",
    "OIDC token environment variable",
];

static NETWORK_SINK_STRS: &[&str] = &[
    r"(?i)(?:fetch|axios|requests)\s*\.\s*(?:post|put|patch)\s*\(",
    r#"(?i)fetch\s*\(\s*['"`]https?:"#,
    r"(?i)new\s+XMLHttpRequest\s*\(\s*\)",
    r"(?i)(?:curl|wget)\s+.*-[Xd]",
    r"(?i)(?:socket|sock)\s*\.\s*(?:send|write|connect)\s*\(",
    r"(?i)smtplib|nodemailer|sendgrid",
    r"(?i)requests\.post\s*\(",
    r"(?i)http\.request\s*\(",
];

static NETWORK_SINK_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();

fn get_network_sink_regexes() -> &'static [Regex] {
    NETWORK_SINK_REGEXES.get_or_init(|| {
        NETWORK_SINK_STRS
            .iter()
            .map(|s| Regex::new(s).expect("invalid network sink pattern"))
            .collect()
    })
}

fn detect_exfiltration_chains(findings: &mut Vec<Finding>, text_files: &HashMap<String, String>) {
    // Group credential-source finding indices by file path.
    let mut sources_by_file: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, finding) in findings.iter().enumerate() {
        if finding.category != FindingCategory::Security {
            continue;
        }
        if !CRED_SOURCE_FRAGS
            .iter()
            .any(|&frag| finding.label.contains(frag))
        {
            continue;
        }
        if matches!(finding.severity, Severity::Info) {
            continue;
        }
        if let Some(fp) = extract_filepath_from_detail(&finding.detail) {
            sources_by_file.entry(fp.to_string()).or_default().push(idx);
        }
    }

    let sinks = get_network_sink_regexes();
    let mut chain_index: u32 = 0;

    // Collect (filepath, source_indices, chain_id) for files that have both sources and sinks.
    // Sort by file path to match vettd's deterministic (Map insertion / alphabetical) order.
    let mut sorted_sources: Vec<(&String, &Vec<usize>)> = sources_by_file.iter().collect();
    sorted_sources.sort_by_key(|(p, _)| p.as_str());

    let mut chains: Vec<(String, Vec<usize>, String)> = Vec::new();
    for (file_path, source_indices) in sorted_sources {
        if let Some(content) = text_files.get(file_path.as_str()) {
            if sinks.iter().any(|re| re.is_match(content)) {
                chains.push((
                    file_path.clone(),
                    source_indices.clone(),
                    format!("cred-exfil-{chain_index}"),
                ));
                chain_index += 1;
            }
        }
    }

    let mut new_findings: Vec<Finding> = Vec::new();
    for (file_path, source_indices, chain_id) in &chains {
        // Mutate source findings.
        for &idx in source_indices {
            findings[idx].chain_id = Some(chain_id.clone());
            findings[idx].intent = Some(Intent::Malicious);
            if !matches!(findings[idx].severity, Severity::Critical) {
                findings[idx].severity = Severity::Critical;
            }
        }
        // Tag any network-related findings in the same file.
        let indices_to_tag: Vec<usize> = findings
            .iter()
            .enumerate()
            .filter(|(i, f)| {
                !source_indices.contains(i)
                    && f.chain_id.is_none()
                    && extract_filepath_from_detail(&f.detail)
                        .map(|p| p == file_path.as_str())
                        .unwrap_or(false)
                    && {
                        let lbl = f.label.to_lowercase();
                        lbl.contains("remote code")
                            || lbl.contains("dead-drop")
                            || lbl.contains("network")
                            || lbl.contains("exfil")
                    }
            })
            .map(|(i, _)| i)
            .collect();
        for i in indices_to_tag {
            findings[i].chain_id = Some(chain_id.clone());
            findings[i].intent = Some(Intent::Malicious);
            if !matches!(findings[i].severity, Severity::Critical) {
                findings[i].severity = Severity::Critical;
            }
        }
        new_findings.push(Finding {
            rule_id: RULE_CREDENTIAL_EXFILTRATION_CHAIN.to_string(),
            category: FindingCategory::Security,
            severity: Severity::Critical,
            label: "Credential access followed by network transmission".to_string(),
            detail: format!(
                "{file_path} reads a credential source and transmits data over the network. \
                 Common exfiltration pattern."
            ),
            filepath: Some(file_path.clone()),
            owasp_llm_category: None,
            chain_id: Some(chain_id.clone()),
            intent: Some(Intent::Malicious),
            source: DEFAULT_SOURCE.to_string(),
        });
    }
    findings.extend(new_findings);
}

// ── Base64 obfuscation scan ───────────────────────────────────────────────────

fn decode_base64_lenient(s: &str) -> Option<String> {
    use base64::{engine::general_purpose, Engine as _};
    // Standard decode. Use from_utf8_lossy to mirror atob() which is binary-safe.
    if let Ok(bytes) = general_purpose::STANDARD.decode(s) {
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    // Auto-pad then decode.
    let pad = match s.len() % 4 {
        2 => "==",
        3 => "=",
        _ => "",
    };
    if !pad.is_empty() {
        let padded = format!("{s}{pad}");
        if let Ok(bytes) = general_purpose::STANDARD.decode(&padded) {
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    // Base64url: swap - → + and _ → /
    let swapped: String = s
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect();
    let pad2 = match swapped.len() % 4 {
        2 => "==",
        3 => "=",
        _ => "",
    };
    let padded2 = format!("{swapped}{pad2}");
    if let Ok(bytes) = general_purpose::STANDARD.decode(&padded2) {
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    None
}

fn join_concatenated_strings(content: &str) -> Vec<String> {
    // Find quoted segments with 4+ base64-like chars, join adjacent ones.
    let mut results = Vec::new();
    let mut group = String::new();
    let mut seg_count: usize = 0;
    let mut prev_end: Option<usize> = None;
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let quote = bytes[i];
        if quote != b'\'' && quote != b'"' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != quote {
            end += 1;
        }
        if end >= bytes.len() {
            i = end + 1;
            continue;
        }
        let inner = &content[start..end];
        if inner.len() >= 4
            && inner.chars().all(
                |c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' | '=' | '_' | '-'),
            )
        {
            let match_start = i;
            let match_end = end + 1;
            let is_joining = prev_end
                .map(|pe| {
                    let gap = &content[pe..match_start];
                    gap.len() <= 10
                        && gap
                            .trim_matches(|c: char| c == '+' || c.is_whitespace())
                            .is_empty()
                })
                .unwrap_or(false);
            if is_joining {
                group.push_str(inner);
                seg_count += 1;
            } else {
                if seg_count > 1 && group.len() >= 40 {
                    results.push(group.clone());
                }
                group = inner.to_string();
                seg_count = 1;
            }
            prev_end = Some(match_end);
        }
        i = end + 1;
    }
    if seg_count > 1 && group.len() >= 40 {
        results.push(group);
    }
    results
}

/// Returns (secrets_failed, behavioral_failed) — mirrors vettd's checkBase64Payloads.
fn check_base64_payloads(
    text_files: &HashMap<String, String>,
    findings: &mut Vec<Finding>,
) -> (bool, bool) {
    static BASE64_CHUNK_RE: OnceLock<Regex> = OnceLock::new();
    static SHELL_VAR_ASSIGN_RE: OnceLock<Regex> = OnceLock::new();
    static OBFUSC_URL_RE: OnceLock<Regex> = OnceLock::new();
    static SHELL_VAR_VALID_RE: OnceLock<Regex> = OnceLock::new();
    let chunk_re = BASE64_CHUNK_RE
        .get_or_init(|| Regex::new(r"[A-Za-z0-9+/_-]{32,}={0,2}").expect("bad b64 chunk re"));
    let assign_re = SHELL_VAR_ASSIGN_RE.get_or_init(|| {
        Regex::new(r#"[A-Z_][A-Z0-9_]*=["']([^"'\r\n]{32,})["']"#).expect("bad shell var re")
    });
    let url_re =
        OBFUSC_URL_RE.get_or_init(|| Regex::new(r#"https?://[^\s)>\]"']+"#).expect("bad url re"));
    let valid_b64_re = SHELL_VAR_VALID_RE
        .get_or_init(|| Regex::new(r"^[A-Za-z0-9+/=_-]+$").expect("bad valid b64 re"));
    let sinks = get_network_sink_regexes();
    let sensitive_regexes = get_sensitive_regexes();

    let mut secrets_failed = false;
    let behavioral_failed = false;
    let mut obfusc_count: u32 = 0;

    let mut sorted_files: Vec<(&String, &String)> = text_files.iter().collect();
    sorted_files.sort_by_key(|(p, _)| p.as_str());
    for (path, content) in sorted_files {
        // Skip reference/eval directories.
        if path.starts_with("evals/") || path.starts_with("references/") {
            continue;
        }
        let is_doc = path.to_lowercase().ends_with(".md");
        let mut warn_emitted = false;

        // Collect candidates: chunk matches, joined strings, shell var assignments.
        let mut candidates: Vec<(String, Option<usize>)> = Vec::new();
        for m in chunk_re.find_iter(content) {
            candidates.push((m.as_str().to_string(), Some(m.start())));
        }
        for joined in join_concatenated_strings(content) {
            candidates.push((joined, None));
        }
        for cap in assign_re.captures_iter(content) {
            if let Some(val) = cap.get(1) {
                let stripped: String = val
                    .as_str()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                if stripped.len() >= 32 && valid_b64_re.is_match(&stripped) {
                    candidates.push((stripped, None));
                }
            }
        }

        let mut matched_dangerous = false;
        for (b64, byte_index) in &candidates {
            let Some(decoded) = decode_base64_lenient(b64) else {
                continue;
            };

            // 1. Sensitive patterns.
            for (i_pat, pat) in SENSITIVE_PATTERNS.iter().enumerate() {
                if pat.code_only && is_doc {
                    continue;
                }
                let re = &sensitive_regexes[i_pat];
                if re.is_match(&decoded) {
                    findings.push(Finding {
                        rule_id: RULE_OBFUSCATED_DANGEROUS_CODE.to_string(),
                        category: FindingCategory::Security,
                        severity: Severity::Critical,
                        label: "Obfuscated dangerous code".to_string(),
                        detail: format!("Decoded base64 in {path} matched: {}", pat.label),
                        filepath: Some(path.clone()),
                        owasp_llm_category: None,
                        chain_id: Some(format!("obfusc-code-{obfusc_count}")),
                        intent: Some(Intent::Malicious),
                        source: DEFAULT_SOURCE.to_string(),
                    });
                    obfusc_count += 1;
                    matched_dangerous = true;
                    secrets_failed = true;
                    break;
                }
            }

            // 2. Network sink patterns.
            if !matched_dangerous {
                for re in sinks {
                    if re.is_match(&decoded) {
                        findings.push(Finding {
                            rule_id: RULE_OBFUSCATED_NETWORK_CALL.to_string(),
                            category: FindingCategory::Security,
                            severity: Severity::Critical,
                            label: "Obfuscated network call".to_string(),
                            detail: format!(
                                "Decoded base64 in {path} contained a network transmission call"
                            ),
                            filepath: Some(path.clone()),
                            owasp_llm_category: None,
                            chain_id: Some(format!("obfusc-net-{obfusc_count}")),
                            intent: Some(Intent::Malicious),
                            source: DEFAULT_SOURCE.to_string(),
                        });
                        obfusc_count += 1;
                        matched_dangerous = true;
                        break;
                    }
                }
            }

            // 3. External URL.
            if !matched_dangerous && url_re.is_match(&decoded) {
                findings.push(Finding {
                    rule_id: RULE_OBFUSCATED_EXTERNAL_URL.to_string(),
                    category: FindingCategory::Security,
                    severity: Severity::Critical,
                    label: "Obfuscated external URL (dead-drop)".to_string(),
                    detail: format!(
                        "Decoded base64 in {path} contained an external URL. \
                         Possible dead-drop or remote instruction source."
                    ),
                    filepath: Some(path.clone()),
                    owasp_llm_category: None,
                    chain_id: Some(format!("obfusc-url-{obfusc_count}")),
                    intent: Some(Intent::Malicious),
                    source: DEFAULT_SOURCE.to_string(),
                });
                obfusc_count += 1;
                matched_dangerous = true;
            }

            // 4. Markdown printable-ratio warn fallback.
            if !matched_dangerous && is_doc && !warn_emitted {
                if let Some(byte_idx) = byte_index {
                    let printable = decoded
                        .chars()
                        .filter(|&c| {
                            let n = c as u32;
                            (32u32..=126).contains(&n) || matches!(n, 9 | 10 | 13)
                        })
                        .count();
                    if !decoded.is_empty() && printable as f64 / decoded.len() as f64 >= 0.75 {
                        let line_num = content[..*byte_idx].split('\n').count();
                        findings.push(Finding {
                            rule_id: RULE_BASE64_IN_MARKDOWN.to_string(),
                            category: FindingCategory::Security,
                            severity: Severity::Medium,
                            label: "Base64-encoded content in markdown file".to_string(),
                            detail: format!(
                                "Detected in {path}:{line_num} — base64 content is \
                                 rarely expected in skill documentation"
                            ),
                            filepath: Some(path.clone()),
                            owasp_llm_category: None,
                            chain_id: None,
                            intent: None,
                            source: DEFAULT_SOURCE.to_string(),
                        });
                        warn_emitted = true;
                    }
                }
            }

            if matched_dangerous {
                break;
            }
        }
    }

    (secrets_failed, behavioral_failed)
}

// ── Description-behavior mismatch ────────────────────────────────────────────

const BENIGN_DESCRIPTION_KEYWORDS: &[&str] = &[
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

fn check_description_behavior_mismatch(description: &str, findings: &mut Vec<Finding>) {
    let has_malicious = findings
        .iter()
        .any(|f| f.category == FindingCategory::Security && f.intent == Some(Intent::Malicious));
    if !has_malicious {
        return;
    }
    let desc_lower = description.to_lowercase();
    let matched: Vec<&str> = BENIGN_DESCRIPTION_KEYWORDS
        .iter()
        .copied()
        .filter(|&kw| desc_lower.contains(kw))
        .collect();
    if matched.is_empty() {
        return;
    }
    let keywords = matched[..matched.len().min(3)].join(", ");
    findings.push(Finding {
        rule_id: RULE_DESCRIPTION_BEHAVIOR_MISMATCH.to_string(),
        category: FindingCategory::Security,
        severity: Severity::Medium,
        label: "Description suggests benign skill but code contains malicious security patterns"
            .to_string(),
        detail: format!(
            "Description uses benign-sounding terms ({keywords}) but the package contains \
             malicious security findings. Review carefully."
        ),
        filepath: None,
        owasp_llm_category: None,
        chain_id: None,
        intent: None,
        source: DEFAULT_SOURCE.to_string(),
    });
}

// ── Typosquatting check ───────────────────────────────────────────────────────

const POPULAR_SKILL_NAMES: &[&str] = &[
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

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (la, lb) = (a.len(), b.len());
    let mut dp = vec![0usize; lb + 1];
    dp.iter_mut().enumerate().for_each(|(j, v)| *v = j);
    for i in 1..=la {
        let mut prev = dp[0];
        dp[0] = i;
        for j in 1..=lb {
            let temp = dp[j];
            dp[j] = if a[i - 1] == b[j - 1] {
                prev
            } else {
                1 + prev.min(dp[j]).min(dp[j - 1])
            };
            prev = temp;
        }
    }
    dp[lb]
}

fn check_typosquat(name: &str, findings: &mut Vec<Finding>) {
    if name == "unknown" || name.is_empty() {
        return;
    }
    let matches: Vec<&str> = POPULAR_SKILL_NAMES
        .iter()
        .copied()
        .filter(|&popular| name != popular && levenshtein(name, popular) <= 2)
        .collect();
    if matches.is_empty() {
        return;
    }
    let (severity, detail) = if matches.len() >= 2 {
        let list = matches[..matches.len().min(3)].join(", ");
        let extra = if matches.len() > 3 {
            format!(" and {} more", matches.len() - 3)
        } else {
            String::new()
        };
        (
            Severity::Critical,
            format!(
                "Skill name \"{name}\" is within Levenshtein distance 2 of {} popular skills: {list}{extra}",
                matches.len()
            ),
        )
    } else {
        (
            Severity::Medium,
            format!(
                "Skill name \"{name}\" is within Levenshtein distance 2 of popular skill \"{}\"",
                matches[0]
            ),
        )
    };
    findings.push(Finding {
        rule_id: RULE_POSSIBLE_TYPOSQUATTING.to_string(),
        category: FindingCategory::Security,
        severity,
        label: "Possible typosquatting".to_string(),
        detail,
        filepath: None,
        owasp_llm_category: None,
        chain_id: None,
        intent: Some(Intent::Negligent),
        source: DEFAULT_SOURCE.to_string(),
    });
}

// ── Frontmatter parser ────────────────────────────────────────────────────────

struct ParsedSkillMd {
    name: String,
    description: String,
    repository: String,
    body: String,
}

/// Parse a SKILL.md string into its frontmatter fields and body.
///
/// Mirrors vettd's `parseFrontmatter` function in skill-analyzer.ts.
/// Handles simple scalar `key: value` frontmatter; nested objects and
/// list values are skipped (indented lines are ignored).
fn parse_skill_md(content: &str) -> ParsedSkillMd {
    let empty = ParsedSkillMd {
        name: "unknown".to_string(),
        description: String::new(),
        repository: String::new(),
        body: content.to_string(),
    };

    if !content.starts_with("---\n") {
        return empty;
    }
    let rest = &content[4..]; // skip opening "---\n"

    // Find closing "---" on its own line
    let close_seq = "\n---";
    let Some(close_pos) = rest.find(close_seq) else {
        return empty;
    };

    // Closing "---" must be followed by end-of-string, whitespace, or newline.
    let after_dashes = &rest[close_pos + close_seq.len()..];
    let trimmed_after = after_dashes.trim_start_matches([' ', '\t']);
    if !trimmed_after.is_empty()
        && !trimmed_after.starts_with('\n')
        && !trimmed_after.starts_with('\r')
    {
        return empty;
    }

    let raw = &rest[..close_pos];
    let body = if let Some(stripped) = trimmed_after.strip_prefix('\n') {
        stripped.trim_start_matches('\n').to_string()
    } else {
        String::new()
    };

    let mut name = "unknown".to_string();
    let mut description = String::new();
    let mut repository = String::new();

    for line in raw.lines() {
        // Skip indented lines (nested objects — not needed for scalar fields)
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(colon_pos) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon_pos].trim();
        let value = strip_quotes(trimmed[colon_pos + 1..].trim());
        match key {
            "name" => name = value.to_string(),
            "description" => description = value.to_string(),
            "repository" => repository = value.to_string(),
            _ => {}
        }
    }

    ParsedSkillMd {
        name,
        description,
        repository,
        body,
    }
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

// ── Name validation ───────────────────────────────────────────────────────────

/// Returns an error message if the name is invalid, or `None` if valid.
/// Mirrors `validateName` in skill-analyzer.ts.
fn validate_name(name: &str) -> Option<&'static str> {
    if name.is_empty() || name == "unknown" {
        return Some("name field is missing");
    }
    if name.len() > SKILL_NAME_MAX_LENGTH {
        return Some("name exceeds 64-character limit");
    }
    let chars: Vec<char> = name.chars().collect();
    if chars.is_empty() {
        return Some("name field is missing");
    }
    let first = chars[0];
    let last = *chars.last().unwrap();
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        if first == '-' || last == '-' {
            return Some("name must not start or end with a hyphen");
        }
        return Some("name contains invalid characters (only alphanumeric and hyphens allowed)");
    }
    for &c in &chars {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Some(
                "name contains invalid characters (only alphanumeric and hyphens allowed)",
            );
        }
    }
    if name.contains("--") {
        return Some("name must not contain consecutive hyphens");
    }
    None
}

// ── Body pattern helpers ──────────────────────────────────────────────────────

fn has_examples(body: &str) -> bool {
    let lower = body.to_lowercase();
    body.contains("```")
        || lower.contains("# example")
        || lower.contains("## example")
        || lower.contains("# sample")
        || lower.contains("## sample")
        || lower.contains("# demo")
        || lower.contains("## demo")
        || lower.contains("**input**")
        || lower.contains("**output**")
        || lower.contains("**before**")
        || lower.contains("**after**")
        || lower.contains("**example**")
        || lower.contains("**good**")
        || lower.contains("**bad**")
}

// Mirrors vettd's hasGotchas: /##?\s*gotcha/i || /##?\s*common\s+mistakes/i
fn has_gotchas(body: &str) -> bool {
    static GOTCHAS_RE: OnceLock<Regex> = OnceLock::new();
    let re = GOTCHAS_RE.get_or_init(|| {
        Regex::new(r"(?i)##?\s*(?:gotcha|common\s+mistakes)").expect("bad gotchas re")
    });
    re.is_match(body)
}

// Mirrors vettd's hasChecklist: /- \[ \]/ || /##?\s*checklist/i
fn has_checklist(body: &str) -> bool {
    if body.contains("- [ ]") {
        return true;
    }
    static CHECKLIST_RE: OnceLock<Regex> = OnceLock::new();
    let re = CHECKLIST_RE
        .get_or_init(|| Regex::new(r"(?im)^##?\s*checklist").expect("bad checklist re"));
    re.is_match(body)
}

// Mirrors vettd's hasValidation: /validat/i.test(body) || /##?\s*verification/i.
// Note: matches "invalidation" and similar — this is a vettd bug reproduced as-is.
fn has_validation(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("validat")
        || lower.lines().any(|l| {
            l.trim_start_matches('#')
                .trim()
                .to_lowercase()
                .starts_with("verification")
                && l.trim_start().starts_with('#')
        })
}

fn has_workflow(body: &str) -> bool {
    let lower = body.to_lowercase();
    // Heading-based patterns
    for heading in &[
        "# workflow",
        "## workflow",
        "# steps",
        "## steps",
        "# instructions",
        "## instructions",
        "# procedure",
        "## procedure",
        "# process",
        "## process",
        "# how to",
        "## how to",
        "# usage",
        "## usage",
        "# guidelines",
        "## guidelines",
    ] {
        if lower.contains(heading) {
            return true;
        }
    }
    // step\s*\d — "step" followed by optional space then digit
    if let Some(pos) = lower.find("step") {
        let after = lower[pos + 4..].trim_start_matches(' ');
        if after.starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
    }
    // Numbered list: line starting with digit at column 0 (mirrors vettd's /^\d+\.\s/m).
    // Do NOT trim — indented numbers (e.g. inside code blocks) must not match.
    for line in body.lines() {
        if line.starts_with(|c: char| c.is_ascii_digit()) {
            let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());
            if rest.starts_with(". ") {
                return true;
            }
        }
        // **Step bullet: vettd allows leading whitespace (^\s*[-*]\s+\*\*Step\b)
        let t = line.trim_start();
        if (t.starts_with("- ") || t.starts_with("* ")) && t.to_lowercase().contains("**step") {
            return true;
        }
    }
    false
}

fn has_usage_context(description: &str) -> bool {
    let lower = description.to_lowercase();
    // "use this ", "use when", "use for"
    if lower.contains("use this ") || lower.contains("use when") || lower.contains("use for") {
        return true;
    }
    // "when ... need/want/ask/mention"
    if let Some(pos) = lower.find("when ") {
        let rest = &lower[pos..];
        if rest.contains("need")
            || rest.contains("want")
            || rest.contains("ask")
            || rest.contains("mention")
        {
            return true;
        }
    }
    false
}

fn has_external_url(content: &str) -> bool {
    content.contains("http://") || content.contains("https://")
}

// ── Script helpers ────────────────────────────────────────────────────────────

fn has_cli_hint(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("argparse")
        || content.contains("--help")
        || lower.contains("argumentparser")
        || lower.contains(".option(")
        || lower.contains("yargs")
        || lower.contains("commander")
        || lower.contains("process.argv")
        || lower.contains("deno.args")
        || lower.contains("sys.argv")
        || lower.contains("click.command")
        || lower.contains("click.group")
        || lower.contains("typer.")
        || content.contains("if __name__ == '__main__'")
        || content.contains("if __name__ == \"__main__\"")
}

fn is_likely_cli_script(path: &str, content: &str) -> bool {
    if !path.starts_with("scripts/") {
        return false;
    }
    let lower = path.to_lowercase();
    let basename = lower.rsplit('/').next().unwrap_or("");
    const NON_CLI_BASENAMES: &[&str] = &[
        "__init__.py", "utils.py", "helper.py", "helpers.py", "base.py", "constants.py",
    ];
    if NON_CLI_BASENAMES.contains(&basename) {
        return false;
    }
    let ext = lower.rsplit('.').next().unwrap_or("");

    if matches!(ext, "sh" | "bash" | "zsh") {
        return true;
    }

    const NON_CLI_EXTS: &[&str] = &[
        "json", "xml", "xsd", "yaml", "yml", "toml", "txt", "md", "csv", "tsv",
    ];
    if NON_CLI_EXTS.contains(&ext) {
        return false;
    }

    // helpers/lib/validators subdirs: only if CLI hint present
    if lower.contains("/helpers/") || lower.contains("/lib/") || lower.contains("/validators/") {
        return has_cli_hint(content);
    }
    // schemas/templates/fixtures/examples/testdata: skip
    for skip in &[
        "/schemas/",
        "/templates/",
        "/fixtures/",
        "/examples/",
        "/testdata/",
    ] {
        if lower.contains(skip) {
            return false;
        }
    }
    // depth ≤ 2 (e.g. scripts/run.sh) → CLI
    let depth = path.split('/').count();
    depth <= 2 || has_cli_hint(content)
}

// ── Main scan function ────────────────────────────────────────────────────────

/// Scan a single skill package and return findings.
///
/// # Arguments
///
/// - `text_files` — map of normalized relative paths to decoded UTF-8 content.
///   Binary files must be excluded by the caller. Keyed by the same paths that
///   appear in `all_paths`.
/// - `all_paths` — complete list of normalized relative paths in the package,
///   including binary files. Used for structural presence checks.
///
/// This function performs no filesystem I/O. The caller is responsible for
/// loading files from disk (or a zip, or a network source) and building the
/// input maps.
///
/// # Ordering guarantee
///
/// Chain detection runs as the final internal step and may mutate `severity` on
/// existing findings. The returned `SkillScanResult.findings` slice already
/// reflects any chain-detection mutations; callers must not reorder this step.
pub fn scan_skill(text_files: &HashMap<String, String>, all_paths: &[String]) -> SkillScanResult {
    let mut findings: Vec<Finding> = Vec::new();

    // ── Structural presence flags ────────────────────────────────────────────

    let has_skill_md = text_files.contains_key("SKILL.md")
        || text_files.contains_key("skill.md")
        || all_paths.iter().any(|p| p == "SKILL.md" || p == "skill.md");

    let has_scripts = all_paths.iter().any(|p| p.starts_with("scripts/"));
    let has_references = all_paths.iter().any(|p| p.starts_with("references/"));
    let has_evals = all_paths.iter().any(|p| {
        p.starts_with("evals/")
            || p.starts_with("tests/")
            || p.starts_with("test/")
            || matches!(p.as_str(), "evals.json" | "evals.yaml" | "evals.yml")
    });
    let has_assets = all_paths.iter().any(|p| p.starts_with("assets/"));

    // Helper: build a Finding with all optional fields set to None/default.
    macro_rules! f {
        ($rule:expr, $cat:expr, $sev:expr, $label:expr, $detail:expr) => {
            Finding {
                rule_id: $rule.to_string(),
                category: $cat,
                severity: $sev,
                label: $label.to_string(),
                detail: $detail,
                filepath: None,
                owasp_llm_category: None,
                chain_id: None,
                intent: None,
                source: DEFAULT_SOURCE.to_string(),
            }
        };
    }

    // ── Structure checks ─────────────────────────────────────────────────────
    // Mirror vettd's structure pass in `analyzeSkillFiles`.

    findings.push(if has_skill_md {
        f!(
            RULE_SKILL_MD,
            FindingCategory::Structure,
            Severity::Info,
            "SKILL.md present",
            "Required skill definition file found".to_string()
        )
    } else {
        f!(
            RULE_SKILL_MD,
            FindingCategory::Structure,
            Severity::Critical,
            "SKILL.md missing",
            "Every skill must contain a SKILL.md file with YAML frontmatter and instructions"
                .to_string()
        )
    });

    findings.push(if has_scripts {
        f!(
            RULE_SCRIPTS_DIRECTORY,
            FindingCategory::Structure,
            Severity::Info,
            "scripts/ directory present",
            "Bundled executable scripts found".to_string()
        )
    } else {
        f!(
            RULE_SCRIPTS_DIRECTORY,
            FindingCategory::Structure,
            Severity::Info,
            "No scripts/ directory",
            "Consider bundling reusable scripts for validation and automation".to_string()
        )
    });

    // references/ and assets/ only emit when present (no false clause — vettd comment:
    // "they are optional extras that earn a pass finding when present but are not expected
    // components — unlike evals which warrants a warn")
    if has_references {
        findings.push(f!(
            RULE_REFERENCES_DIRECTORY,
            FindingCategory::Structure,
            Severity::Info,
            "references/ directory present",
            "Additional documentation files available for progressive disclosure".to_string()
        ));
    }

    if has_assets {
        findings.push(f!(
            RULE_ASSETS_DIRECTORY,
            FindingCategory::Structure,
            Severity::Info,
            "assets/ directory present",
            "Static resources (templates, schemas, etc.) found".to_string()
        ));
    }

    // ── Evals structural flag ────────────────────────────────────────────────
    // VTD-0118 always fires (present or absent) — unlike references/assets.

    findings.push(if has_evals {
        f!(
            RULE_EVALS_PRESENT,
            FindingCategory::Evals,
            Severity::Info,
            "Evaluation test cases found",
            "evals/ directory or evals.json present for testing skill quality".to_string()
        )
    } else {
        f!(RULE_EVALS_PRESENT, FindingCategory::Evals, Severity::Info,
           "No evaluation test cases",
           "Add an evals/ directory with test prompts and expected outputs to measure skill quality"
               .to_string())
    });

    // ── SKILL.md-gated checks ────────────────────────────────────────────────

    if has_skill_md {
        let skill_key = if text_files.contains_key("SKILL.md") {
            "SKILL.md"
        } else {
            "skill.md"
        };
        let parsed = text_files
            .get(skill_key)
            .map(|c| parse_skill_md(c))
            .unwrap_or_else(|| ParsedSkillMd {
                name: "unknown".to_string(),
                description: String::new(),
                repository: String::new(),
                body: String::new(),
            });

        // Typosquatting check (VTD-0082) — runs before security scan, after name parse
        check_typosquat(&parsed.name, &mut findings);

        // Name validation (VTD-0099)
        if let Some(err) = validate_name(&parsed.name) {
            findings.push(f!(
                RULE_SKILL_NAME_VALIDITY,
                FindingCategory::Structure,
                Severity::Critical,
                "Invalid name field",
                err.to_string()
            ));
        } else {
            findings.push(f!(
                RULE_SKILL_NAME_VALIDITY,
                FindingCategory::Structure,
                Severity::Info,
                "Valid name field",
                format!(
                    "Name {:?} follows spec (lowercase, hyphens, \u{2264}64 chars)",
                    parsed.name
                )
            ));
        }

        // Name collision check (VTD-0100)
        const WELL_KNOWN_SKILL_NAMES: &[&str] = &[
            "frontend-design", "pdf", "web-perf", "web-design-guidelines", "find-skills",
            "agent-browser", "agent-customization", "cloudflare", "durable-objects",
            "workers-best-practices", "wrangler", "sandbox-sdk", "next-best-practices",
            "vercel-react-best-practices", "rust-best-practices", "postgresql-optimization",
            "prisma-postgres", "aws-skills", "powershell-windows", "cosmosdb-best-practices",
            "excel", "word", "powerpoint", "git", "docker", "kubernetes", "terraform",
            "ansible",
        ];
        if WELL_KNOWN_SKILL_NAMES.contains(&parsed.name.as_str()) {
            findings.push(f!(
                RULE_SKILL_NAME_COLLISION,
                FindingCategory::BestPractices,
                Severity::Medium,
                "Skill name collides with well-known skill",
                format!(
                    "{:?} matches a well-known skill name — may cause unintended invocation",
                    parsed.name
                )
            ));
        }

        // Repository link check (VTD-0083)
        if parsed.repository.is_empty() {
            findings.push(Finding {
                rule_id: RULE_NO_REPOSITORY_LINK.to_string(),
                category: FindingCategory::Security,
                severity: Severity::Info,
                label: "No repository link".to_string(),
                detail: "No repository field found in SKILL.md frontmatter. Skills without a \
                         verifiable source repository cannot be externally audited."
                    .to_string(),
                filepath: None,
                owasp_llm_category: None,
                chain_id: None,
                intent: Some(Intent::Negligent),
                source: DEFAULT_SOURCE.to_string(),
            });
        }

        // System prompt leakage check (VTD-0085)
        {
            static PROMPT_LEAK_RE: OnceLock<Regex> = OnceLock::new();
            let prompt_leak_re = PROMPT_LEAK_RE.get_or_init(|| {
                Regex::new(r"(?i)\b(?:print|log|echo|output|return|display|show|reveal|dump)\s+(?:the\s+|your\s+|my\s+)?(?:system\s+)?(?:prompt|instructions?|system\s+message|internal\s+(?:prompt|instructions?))\b")
                    .expect("bad prompt leak re")
            });
            let skill_md_raw = text_files.get(skill_key).map(|s| s.as_str()).unwrap_or("");
            if prompt_leak_re.is_match(skill_md_raw) {
                findings.push(Finding {
                    rule_id: RULE_SYSTEM_PROMPT_LEAKAGE.to_string(),
                    category: FindingCategory::Security,
                    severity: Severity::Medium,
                    label: "System prompt leakage risk".to_string(),
                    detail: "Skill instructs agent to output or reveal system prompt/instructions"
                        .to_string(),
                    filepath: None,
                    owasp_llm_category: None,
                    chain_id: None,
                    intent: None,
                    source: DEFAULT_SOURCE.to_string(),
                });
            }
        }

        // Description checks (VTD-0109, VTD-0110, VTD-0111)
        if parsed.description.is_empty() {
            findings.push(f!(
                RULE_DESCRIPTION_PRESENT,
                FindingCategory::Description,
                Severity::Info,
                "Missing description field",
                "The description field is required and should describe what the skill \
                 does and when to use it"
                    .to_string()
            ));
        } else {
            let char_count = parsed.description.chars().count();
            findings.push(if char_count > DESCRIPTION_MAX_LENGTH {
                f!(
                    RULE_DESCRIPTION_LENGTH,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description exceeds 1024-character limit",
                    format!(
                        "Description is {char_count} characters (max: {DESCRIPTION_MAX_LENGTH})"
                    )
                )
            } else {
                f!(
                    RULE_DESCRIPTION_LENGTH,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description within character limit",
                    format!("{char_count}/{DESCRIPTION_MAX_LENGTH} characters used")
                )
            });

            findings.push(if has_usage_context(&parsed.description) {
                f!(
                    RULE_DESCRIPTION_CONTEXT,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description includes usage context",
                    "Good: description explains when to activate the skill".to_string()
                )
            } else {
                f!(
                    RULE_DESCRIPTION_CONTEXT,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description lacks usage context",
                    "Add context like \"Use this skill when...\" to help agents know \
                    when to activate it"
                        .to_string()
                )
            });

            // VTD-0112 — description too brief (< 5 words, no negative case)
            if parsed.description.split_whitespace().count() < 5 {
                findings.push(f!(
                    RULE_DESCRIPTION_BREVITY,
                    FindingCategory::Description,
                    Severity::Info,
                    "Description too brief",
                    "A few sentences covering scope and trigger conditions improves \
                    activation accuracy"
                        .to_string()
                ));
            }

            // VTD-0113 — description overclaims scope
            {
                static OVERCLAIM_RE: OnceLock<Regex> = OnceLock::new();
                let overclaim_re = OVERCLAIM_RE.get_or_init(|| {
                    Regex::new(r"(?i)\b(?:anything|everything|all\s+(?:files?|data|tasks?|requests?|inputs?|things?)|any\s+(?:file|task|request|input|thing)|whatever)\b")
                        .expect("bad overclaim re")
                });
                if overclaim_re.is_match(&parsed.description) {
                    findings.push(f!(
                        RULE_DESCRIPTION_SCOPE,
                        FindingCategory::Description,
                        Severity::Low,
                        "Description overclaims scope",
                        "Broad trigger words (anything, everything, all files, etc.) widen attack surface — narrow to specific use cases"
                            .to_string()
                    ));
                }
            }
        }

        // Body quality checks — only when body has content
        if !parsed.body.trim().is_empty() {
            // .lines() excludes a trailing newline, matching JS regex $-before-trailing-\n
            let body_lines = parsed.body.split('\n').count();

            findings.push(if body_lines > SKILL_MD_BODY_MAX_LINES {
                f!(RULE_SKILL_MD_BODY_LENGTH, FindingCategory::BestPractices, Severity::Info,
                   "SKILL.md exceeds 500 lines",
                   format!("{body_lines} lines — consider moving detailed reference material to references/"))
            } else {
                f!(RULE_SKILL_MD_BODY_LENGTH, FindingCategory::BestPractices, Severity::Info,
                   "SKILL.md body length is reasonable",
                   format!("{body_lines} lines (recommended: under 500)"))
            });

            // VTD-0102 — gotchas section (only fires when present)
            if has_gotchas(&parsed.body) {
                findings.push(f!(
                    RULE_GOTCHAS_SECTION,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Gotchas section found",
                    "Documents environment-specific facts and common pitfalls".to_string()
                ));
            }

            findings.push(if has_examples(&parsed.body) {
                f!(
                    RULE_EXAMPLES_PRESENT,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Examples included",
                    "Found code blocks, input/output samples, or an examples section — \
                    concrete samples help agents pattern-match effectively"
                        .to_string()
                )
            } else {
                f!(
                    RULE_EXAMPLES_PRESENT,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "No examples found",
                    "Add code blocks, input/output samples, or before/after examples \
                    to improve agent accuracy"
                        .to_string()
                )
            });

            // VTD-0104 — checklist pattern (only fires when present)
            if has_checklist(&parsed.body) {
                findings.push(f!(
                    RULE_CHECKLIST_PRESENT,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Checklist pattern found",
                    "Explicit checklists help agents track progress in multi-step workflows"
                        .to_string()
                ));
            }

            findings.push(if has_workflow(&parsed.body) {
                f!(
                    RULE_WORKFLOW_STRUCTURE,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Step-by-step workflow found",
                    "Structured procedures improve reliability for complex tasks".to_string()
                )
            } else {
                f!(
                    RULE_WORKFLOW_STRUCTURE,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "No clear workflow structure",
                    "Consider adding numbered steps or a structured procedure for the \
                    agent to follow"
                        .to_string()
                )
            });

            // VTD-0105 only fires when validation keywords are present (no negative case).
            if has_validation(&parsed.body) {
                findings.push(f!(
                    RULE_VALIDATION_LOOP,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Validation loop referenced",
                    "Instructions for the agent to validate its own work before proceeding"
                        .to_string()
                ));
            }

            // VTD-0107 — progressive disclosure: body refs files that exist as dirs
            let body_refs_files = parsed.body.contains("references/")
                || parsed.body.contains("scripts/")
                || parsed.body.contains("assets/")
                || {
                    static READ_MD_RE: OnceLock<Regex> = OnceLock::new();
                    let re = READ_MD_RE
                        .get_or_init(|| Regex::new(r"(?i)read.*\.md").expect("bad read md re"));
                    re.is_match(&parsed.body)
                };
            if body_refs_files && (has_references || has_scripts || has_assets) {
                findings.push(f!(
                    RULE_PROGRESSIVE_DISCLOSURE,
                    FindingCategory::BestPractices,
                    Severity::Info,
                    "Progressive disclosure used",
                    "SKILL.md body references files in references/, scripts/, or assets/ — \
                     agents can load additional context on demand instead of consuming \
                     everything upfront"
                        .to_string()
                ));
            }

            // VTD-0108 — generic instruction phrases
            const GENERIC_PHRASES: &[&str] = &[
                "follow best practices",
                "handle errors appropriately",
                "use proper",
                "ensure quality",
            ];
            let body_lower = parsed.body.to_lowercase();
            for &phrase in GENERIC_PHRASES {
                if body_lower.contains(phrase) {
                    findings.push(f!(
                        RULE_GENERIC_INSTRUCTION,
                        FindingCategory::BestPractices,
                        Severity::Info,
                        "Generic instruction detected",
                        format!(
                            "\"{phrase}\" is too vague — provide specific, actionable guidance instead"
                        )
                    ));
                }
            }
        }
    }

    // ── Scripts checks ───────────────────────────────────────────────────────

    if has_scripts {
        let mut script_files: Vec<(&str, &str)> = text_files
            .iter()
            .filter(|(p, c)| is_likely_cli_script(p, c))
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        // Sort for deterministic output order
        script_files.sort_by_key(|(p, _)| *p);

        static INTERACTIVE_RE: OnceLock<Regex> = OnceLock::new();
        static STRUCTURED_RE: OnceLock<Regex> = OnceLock::new();
        let interactive_re = INTERACTIVE_RE.get_or_init(|| {
            Regex::new(r"(?i)input\s*\(|readline|prompt\s*\(|inquirer").expect("bad interactive re")
        });
        let structured_re = STRUCTURED_RE.get_or_init(|| {
            Regex::new(r"(?i)json\.dumps|JSON\.stringify|\.to_json|\.to_csv|csv\.writer")
                .expect("bad structured re")
        });
        static DEP_RE: OnceLock<Regex> = OnceLock::new();
        let dep_re = DEP_RE.get_or_init(|| {
            Regex::new(r"(?i)dependencies\s*=\s*\[|require\(|import\s").expect("bad dep re")
        });

        for (path, content) in script_files {
            // VTD-0114 — CLI help
            findings.push(Finding {
                rule_id: RULE_SCRIPT_CLI_HELP.to_string(),
                category: FindingCategory::Scripts,
                severity: Severity::Info,
                label: if has_cli_hint(content) {
                    "CLI help supported".to_string()
                } else {
                    "No --help support".to_string()
                },
                detail: if has_cli_hint(content) {
                    format!("{path}: Script documents its interface via --help or argument parsing")
                } else {
                    format!("{path}: Add argument parsing with --help output so agents know the script's interface")
                },
                filepath: Some(path.to_string()),
                owasp_llm_category: None,
                chain_id: None,
                intent: None,
                source: DEFAULT_SOURCE.to_string(),
            });

            // VTD-0115 — interactive prompts
            if interactive_re.is_match(content) {
                findings.push(Finding {
                    rule_id: RULE_SCRIPT_INTERACTIVE_PROMPTS.to_string(),
                    category: FindingCategory::Scripts,
                    severity: Severity::High,
                    label: "Interactive prompts detected".to_string(),
                    detail: format!(
                        "{path}: Agents run in non-interactive shells — replace prompts with CLI flags or stdin"
                    ),
                    filepath: Some(path.to_string()),
                    owasp_llm_category: None,
                    chain_id: None,
                    intent: None,
                    source: DEFAULT_SOURCE.to_string(),
                });
            }

            // VTD-0116 — structured output
            if structured_re.is_match(content) {
                findings.push(Finding {
                    rule_id: RULE_SCRIPT_STRUCTURED_OUTPUT.to_string(),
                    category: FindingCategory::Scripts,
                    severity: Severity::Info,
                    label: "Structured output format".to_string(),
                    detail: format!(
                        "{path}: Uses JSON/CSV output which is easily parseable by agents"
                    ),
                    filepath: Some(path.to_string()),
                    owasp_llm_category: None,
                    chain_id: None,
                    intent: None,
                    source: DEFAULT_SOURCE.to_string(),
                });
            }

            // VTD-0117 — unpinned dependency versions
            {
                let has_pinned_deps = dep_re.is_match(content);
                if has_pinned_deps && content.contains(">=") && !content.contains('<') {
                    findings.push(Finding {
                        rule_id: RULE_SCRIPT_DEPENDENCY_PINNING.to_string(),
                        category: FindingCategory::Scripts,
                        severity: Severity::Low,
                        label: "Unpinned dependency versions".to_string(),
                        detail: format!(
                            "{path}: Pin dependency versions for reproducibility \
                            (e.g., >=4.12,<5 instead of >=4.12)"
                        ),
                        filepath: Some(path.to_string()),
                        owasp_llm_category: None,
                        chain_id: None,
                        intent: None,
                        source: DEFAULT_SOURCE.to_string(),
                    });
                }
            }
        }
    }

    // ── Security scan ────────────────────────────────────────────────────────
    // Order mirrors vettd's checkSecurity():
    //   1. Sensitive patterns (SENSITIVE_PATTERNS array)
    //   2. Entropy scan (high-entropy assignment values)
    //   3. .env file detection
    //   4. Capture secretsCheckFailed (before behavioral scan)
    //   5. Behavioral scan (BEHAVIORAL_PATTERNS on all files)
    //   6. Base64 obfuscation scan
    //   7. VTD-0091 conditional (suppressed if secrets or base64 secrets found)
    //   8. VTD-0092 conditional (suppressed if behavioral or base64-behavioral findings found)

    let (sensitive_findings, secrets_check_failed_pat) = scan_sensitive_patterns(text_files);
    findings.extend(sensitive_findings);

    scan_entropy(text_files, &mut findings);
    scan_env_files(text_files, &mut findings);

    let secrets_check_failed = secrets_check_failed_pat
        || findings.iter().any(|f| {
            f.category == FindingCategory::Security
                && matches!(f.severity, Severity::Critical | Severity::High)
        });

    let (base64_secrets_failed, base64_behavioral_failed) =
        check_base64_payloads(text_files, &mut findings);

    // VTD-0091: only emit when no critical/high secrets/code-risk findings found.
    if !secrets_check_failed && !base64_secrets_failed {
        findings.push(f!(
            RULE_NO_SECRETS_DETECTED,
            FindingCategory::Security,
            Severity::Info,
            "No secrets or unsafe code patterns detected",
            "Scanned all files for credentials, private keys, and code-level risks \
             (eval, shell exec, destructive ops)"
                .to_string()
        ));
    }

    // Behavioral scan — mirrors vettd's inline scan over all text files.
    let (behavioral_findings, behavioral_check_failed) = scan_behavioral_patterns(text_files);
    findings.extend(behavioral_findings);

    // VTD-0092: only emit when no critical/high behavioral findings were found.
    if !behavioral_check_failed && !base64_behavioral_failed {
        findings.push(f!(
            RULE_NO_BEHAVIORAL_SIGNALS,
            FindingCategory::Security,
            Severity::Info,
            "No prompt injection or jailbreak signals detected",
            "Scanned text content for instruction override, jailbreak framing, credential \
             solicitation, and embedded injection markers"
                .to_string()
        ));
    }

    // Hidden Unicode detection (VTD-0081) — mirrors vettd's invisible-char scan in checkSecurity.
    scan_hidden_unicode(text_files, &mut findings);

    // External URL check — mirrors vettd's urlTargetFiles scan.
    // VTD-0088 fires on the first URL-containing SKILL.md or references/ file;
    // VTD-0093 (clean signal) fires only when no URL was found.
    let url_target_files: Vec<(&str, &str)> = {
        // SKILL.md first, then references/ sorted alphabetically to match the
        // sorted insertion order that the Python loader produces (vettd Map preserves it).
        let mut targets: Vec<(&str, &str)> = Vec::new();
        for name in &["SKILL.md", "skill.md"] {
            if let Some(c) = text_files.get(*name) {
                targets.push((name, c.as_str()));
            }
        }
        let mut refs: Vec<(&str, &str)> = text_files
            .iter()
            .filter(|(p, _)| p.to_lowercase().starts_with("references/"))
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        refs.sort_by_key(|(p, _)| *p);
        targets.extend(refs);
        targets
    };

    if !url_target_files.is_empty() {
        let url_file = url_target_files.iter().find(|(_, c)| has_external_url(c));
        if let Some((path, _)) = url_file {
            findings.push(Finding {
                rule_id: RULE_EXTERNAL_URL_REFERENCE.to_string(),
                category: FindingCategory::Security,
                severity: Severity::Medium,
                label: "References external URL — review for indirect prompt injection risk"
                    .to_string(),
                detail: format!(
                    "External URL(s) detected in {path} — referenced content can change after audit"
                ),
                filepath: Some(path.to_string()),
                owasp_llm_category: None,
                chain_id: None,
                intent: None,
                source: DEFAULT_SOURCE.to_string(),
            });
        } else {
            findings.push(f!(
                RULE_NO_EXTERNAL_URLS,
                FindingCategory::Security,
                Severity::Info,
                "No external URLs in skill definition",
                "SKILL.md and references/ files do not reference external URLs".to_string()
            ));
        }
    }

    // ── Evals quality check ──────────────────────────────────────────────────

    if has_evals {
        let eval_json_found = EVAL_JSON_CANDIDATES
            .iter()
            .any(|&candidate| text_files.contains_key(candidate));

        if !eval_json_found {
            // No standard JSON eval file — count non-trivial eval files
            let eval_dir_prefixes = ["evals/", "tests/", "test/"];
            let non_trivial_count = all_paths
                .iter()
                .filter(|p| eval_dir_prefixes.iter().any(|prefix| p.starts_with(prefix)))
                .filter(|p| {
                    let lower = p.to_lowercase();
                    lower.ends_with(".md")
                        || lower.ends_with(".yaml")
                        || lower.ends_with(".yml")
                        || lower.ends_with(".txt")
                        || lower.ends_with(".jsonl")
                        // .json files in textFiles — vettd detail says "non-JSON" for these too (vettd bug, reproduced as-is)
                        || (lower.ends_with(".json") && text_files.contains_key(p.as_str()))
                })
                .count();

            if non_trivial_count > 0 {
                findings.push(f!(
                    RULE_EVAL_FILES_FOUND,
                    FindingCategory::Evals,
                    Severity::Info,
                    "Eval files found",
                    format!(
                        "{non_trivial_count} evaluation file(s) detected in non-JSON format \
                             (markdown, YAML, JSONL, etc.)"
                    )
                ));
            }
        }
    }

    // ── Chain detection and mismatch checks ─────────────────────────────────
    // Order mirrors vettd: exfiltration chains → malicious activity chains →
    // description-behavior mismatch.
    detect_exfiltration_chains(&mut findings, text_files);
    detect_malicious_activity_chains(&mut findings);
    let description_for_mismatch = if has_skill_md {
        let key = if text_files.contains_key("SKILL.md") {
            "SKILL.md"
        } else {
            "skill.md"
        };
        text_files
            .get(key)
            .map(|c| parse_skill_md(c).description)
            .unwrap_or_default()
    } else {
        String::new()
    };
    check_description_behavior_mismatch(&description_for_mismatch, &mut findings);

    SkillScanResult {
        findings,
        has_skill_md,
        has_scripts,
        has_references,
        has_evals,
        file_count: all_paths.len(),
    }
}
