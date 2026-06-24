use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::consts::DEFAULT_SOURCE;
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::rules::*;

/// a single entry in the built-in sensitive-pattern registry.
///
/// each entry is paired by index with a regex string in `SENSITIVE_PATTERN_STRS` and a
/// compiled `Regex` in `SENSITIVE_REGEXES`. the three arrays must stay in lock-step: adding
/// or removing a pattern requires the same change in all three.
pub(crate) struct SensitivePattern {
    /// rule ID written to the emitted finding's `rule_id` field.
    pub(crate) rule_id: &'static str,
    /// human-readable label written to the emitted finding's `label` field.
    pub(crate) label: &'static str,
    /// severity used for non-markdown files.
    pub(crate) severity: Severity,
    /// intent classification; `Malicious` for deliberate exfiltration patterns, `Negligent` for hygiene issues.
    pub(crate) intent: Intent,
    /// if true, skip this pattern entirely for `.md` files (mirrors vettd's `CODE_ONLY_LABELS`).
    pub(crate) code_only: bool,
    /// if set, overrides `severity` when the matched file is a `.md` file.
    pub(crate) doc_severity: Option<Severity>,
}

// Array order mirrors vettd's SENSITIVE_PATTERNS definition order.
pub(crate) static SENSITIVE_PATTERNS: &[SensitivePattern] = &[
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

pub(crate) fn get_sensitive_regexes() -> &'static [Regex] {
    SENSITIVE_REGEXES.get_or_init(|| {
        SENSITIVE_PATTERN_STRS
            .iter()
            .map(|s| Regex::new(s).expect("invalid sensitive pattern"))
            .collect()
    })
}

static ASSIGNMENT_QUOTED_VALUE_STR: &str =
    r#"(?:["']?([A-Za-z_][A-Za-z0-9_.:-]*)["']?\s*[:=]\s*["']([^"'\r\n]{20,})["'])"#;
static SUSPICIOUS_SECRET_KEY_STR: &str = r"(?i)(?:^|[-_.])(?:api[-_.]?key|access[-_.]?token|auth[-_.]?token|refresh[-_.]?token|token|secret|password|passwd|pwd|private[-_.]?key|client[-_.]?secret|credential|credentials|bearer)(?:$|[-_.])";

static ASSIGNMENT_QUOTED_VALUE_RE: OnceLock<Regex> = OnceLock::new();
static SUSPICIOUS_SECRET_KEY_RE: OnceLock<Regex> = OnceLock::new();

/// scans all text files against every pattern in `SENSITIVE_PATTERNS`.
///
/// for `.md` files, patterns with `code_only` set are skipped, and `doc_severity`
/// overrides `severity` when set. only the first matching line per pattern per file is reported.
///
/// # Parameters
/// - `text_files` — map of normalized relative paths to decoded UTF-8 file content.
///
/// # Returns
/// `(findings, secrets_check_failed)` — `secrets_check_failed` is `true` if any
/// critical or high-severity finding was produced.
pub(crate) fn scan_sensitive_patterns(
    text_files: &HashMap<String, String>,
) -> (Vec<Finding>, bool) {
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
                    let snippet = snippet
                        .char_indices()
                        .nth(120)
                        .map(|(i, _)| &snippet[..i])
                        .unwrap_or(snippet);
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

/// scans non-markdown files for high-entropy assignment expressions that resemble hardcoded secrets.
///
/// matches `key = "value"` and `key: "value"` forms where the key name contains a
/// secret-related word (token, api_key, password, etc.) and the value has Shannon entropy ≥ 3.5.
///
/// # Parameters
/// - `text_files` — map of normalized relative paths to decoded UTF-8 file content.
/// - `findings` — output vec; detected high-entropy assignments are appended.
pub(crate) fn scan_entropy(text_files: &HashMap<String, String>, findings: &mut Vec<Finding>) {
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
                    let snippet = snippet
                        .char_indices()
                        .nth(120)
                        .map(|(i, _)| &snippet[..i])
                        .unwrap_or(snippet);
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

pub(crate) fn scan_env_files(text_files: &HashMap<String, String>, findings: &mut Vec<Finding>) {
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
