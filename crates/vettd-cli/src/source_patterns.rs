use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

// Pattern definitions in this module are adapted from Cisco DefenseClaw's
// plugin scanner and guardrail rule sets (Apache-2.0). See
// THIRD_PARTY_NOTICES for the exact upstream files and pattern families
// incorporated here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PatternDefinition {
    pub(crate) id: &'static str,
    pub(crate) signal: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) expression: &'static str,
}

#[derive(Debug)]
pub(crate) struct CompiledSourcePattern {
    pub(crate) id: &'static str,
    pub(crate) signal: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) regex: Regex,
}

pub(crate) const MAX_JSON_CONFIG_BYTES: usize = 256 * 1024;
pub(crate) const MAX_SOURCE_ANALYSIS_BYTES: usize = 256 * 1024;

const JSON_SKIP_BASENAMES: &[&str] = &[
    "package.json",
    "package-lock.json",
    "tsconfig.json",
    "openclaw.plugin.json",
];

const COGNITIVE_FILE_NAMES: &[&str] = &[
    ".cursorrules",
    "agents.md",
    "copilot-instructions.md",
    "gateway.json",
    "identity.md",
    "memory.md",
    "openclaw.json",
    "soul.md",
    "tools.md",
];

const SOURCE_CONTEXT_PATTERN_DEFS: &[PatternDefinition] = &[
    PatternDefinition {
        id: "dc_dyn_import",
        signal: "source:dynamic_import",
        summary: "Source uses import() with a non-literal argument",
        expression: r#"\bimport\s*\("#,
    },
    PatternDefinition {
        id: "dc_dyn_require",
        signal: "source:nonliteral_require",
        summary: "Source uses require() with a non-literal argument",
        expression: r#"\brequire\s*\("#,
    },
    PatternDefinition {
        id: "dc_dyn_spawn_var",
        signal: "source:nonliteral_spawn",
        summary: "Source spawns a process with a non-literal command",
        expression: r#"\b(?:spawn|spawnSync|execFile|execFileSync|fork|Bun\.spawn)\s*\("#,
    },
];

const SENSITIVE_PATH_PATTERN_DEFS: &[PatternDefinition] = &[
    PatternDefinition {
        id: "dc_sensitive_openclaw_store",
        signal: "source:sensitive_path_access",
        summary: "Source accesses agent credential stores or secrets files",
        expression: r#"(?i)(?:\.openclaw/(?:credentials|\.env|agents/)|readFile\w*\s*\([^)]*(?:\.env|credentials|secrets))"#,
    },
    PatternDefinition {
        id: "dc_sensitive_proc_environ",
        signal: "source:sensitive_path_access",
        summary: "Source accesses /proc environ entries",
        expression: r#"/proc/(?:\d+|self)/environ\b"#,
    },
    PatternDefinition {
        id: "dc_sensitive_shell_history",
        signal: "source:sensitive_path_access",
        summary: "Source accesses shell history files",
        expression: r#"(?:~|\$HOME|/home/\w+|/root)/\.(?:bash_history|zsh_history|python_history)\b"#,
    },
    PatternDefinition {
        id: "dc_sensitive_ssh_store",
        signal: "source:sensitive_path_access",
        summary: "Source accesses SSH keys or SSH configuration",
        expression: r#"(?i)(?:~|\$HOME|/home/\w+|/root)/\.ssh/|(?:^|[\\/])id_(?:rsa|ed25519|ecdsa|dsa)(?:\.pub)?\b"#,
    },
    PatternDefinition {
        id: "dc_sensitive_cloud_creds",
        signal: "source:sensitive_path_access",
        summary: "Source accesses cloud or registry credential files",
        expression: r#"(?:~|\$HOME|/home/\w+|/root)/(?:\.aws/(?:credentials|config)|\.kube/config|\.docker/config\.json|\.npmrc|\.pypirc|\.gnupg/|\.git-credentials|\.netrc)\b"#,
    },
];

const JSON_SECRET_PATTERN_DEFS: &[PatternDefinition] = &[
    PatternDefinition {
        id: "dc_json_connection_string",
        signal: "json_config:credential_connection_string",
        summary: "JSON config embeds credentials in a connection string",
        expression: r"(?:mongodb|postgres|mysql|redis)://[^:]+:[^@]+@",
    },
    PatternDefinition {
        id: "dc_json_generic_credential_value",
        signal: "json_config:credential_value",
        summary: "JSON config contains a likely credential key/value pair",
        expression: r#"["'](?:password|secret|api[_-]?key|access[_-]?token|auth[_-]?token)["']\s*:\s*["'][^"']{8,}["']"#,
    },
];

const JSON_URL_PATTERN_DEFS: &[PatternDefinition] = &[
    PatternDefinition {
        id: "dc_json_metadata_or_localhost_url",
        signal: "json_config:metadata_url",
        summary: "JSON config references a metadata or localhost URL",
        expression: r#"["']https?://(?:169\.254\.169\.254|metadata\.google\.internal|100\.100\.100\.200|localhost|127\.0\.0\.1)"#,
    },
    PatternDefinition {
        id: "dc_json_internal_url",
        signal: "json_config:internal_url",
        summary: "JSON config references an internal-only URL",
        expression: r#"["']https?://[^"']*(?:internal|corp|local|intranet|private)(?:[./:"']|$)"#,
    },
    PatternDefinition {
        id: "dc_json_c2_url",
        signal: "json_config:c2_url",
        summary: "JSON config references a known collector or C2 URL",
        expression: r#"["']https?://[^"']*(?:webhook\.site|ngrok\.io|pipedream\.net|requestbin\.com|interact\.sh|oast\.fun|burpcollaborator\.net)"#,
    },
];

static JSON_SECRET_PATTERNS: LazyLock<Vec<CompiledSourcePattern>> =
    LazyLock::new(|| compile_patterns(JSON_SECRET_PATTERN_DEFS));

static JSON_URL_PATTERNS: LazyLock<Vec<CompiledSourcePattern>> =
    LazyLock::new(|| compile_patterns(JSON_URL_PATTERN_DEFS));

static SOURCE_CONTEXT_PATTERNS: LazyLock<Vec<CompiledSourcePattern>> =
    LazyLock::new(|| compile_patterns(SOURCE_CONTEXT_PATTERN_DEFS));

static SENSITIVE_PATH_PATTERNS: LazyLock<Vec<CompiledSourcePattern>> =
    LazyLock::new(|| compile_patterns(SENSITIVE_PATH_PATTERN_DEFS));

static NETWORK_CALL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(?:fetch|axios|request|get|post|http|https|urllib\.request|requests\.(?:get|post|request)|httpx\.(?:get|post|request)|client\.(?:get|post|request))\b"#,
    )
    .expect("valid network call regex")
});

static PRIVATE_IP_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:^|\b)(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3}|127\.\d{1,3}\.\d{1,3}\.\d{1,3})(?:\b|$)"#,
    )
    .expect("valid private ip regex")
});

static LINK_LOCAL_IP_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"169\.254\.\d{1,3}\.\d{1,3}|100\.100\.100\.200"#)
        .expect("valid link-local ip regex")
});

static INTERNAL_HOSTNAME_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(?:localhost|internal|corp|local|intranet|private)\b.*\b(?:fetch|http|request|get|post)\b|\b(?:fetch|http|request|get|post)\b.*\b(?:localhost|internal|corp|local|intranet|private)\b"#,
    )
    .expect("valid internal hostname regex")
});

static WRITE_FUNCTION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:writeFile|appendFile|writeFileSync|appendFileSync|createWriteStream)\s*\("#)
        .expect("valid write function regex")
});

static COGNITIVE_TARGET_FUNCTION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:readFile|readFileSync|createReadStream|open|openSync|copyFile|copyFileSync|rename|renameSync|unlink|unlinkSync|rm|rmSync)\s*\("#,
    )
    .expect("valid cognitive target regex")
});

pub(crate) fn json_secret_patterns() -> &'static [CompiledSourcePattern] {
    &JSON_SECRET_PATTERNS
}

pub(crate) fn json_url_patterns() -> &'static [CompiledSourcePattern] {
    &JSON_URL_PATTERNS
}

pub(crate) fn source_context_patterns() -> &'static [CompiledSourcePattern] {
    &SOURCE_CONTEXT_PATTERNS
}

pub(crate) fn sensitive_path_patterns() -> &'static [CompiledSourcePattern] {
    &SENSITIVE_PATH_PATTERNS
}

pub(crate) fn network_call_pattern() -> &'static Regex {
    &NETWORK_CALL_PATTERN
}

pub(crate) fn private_ip_pattern() -> &'static Regex {
    &PRIVATE_IP_PATTERN
}

pub(crate) fn link_local_ip_pattern() -> &'static Regex {
    &LINK_LOCAL_IP_PATTERN
}

pub(crate) fn internal_hostname_context_pattern() -> &'static Regex {
    &INTERNAL_HOSTNAME_CONTEXT_PATTERN
}

pub(crate) fn write_function_pattern() -> &'static Regex {
    &WRITE_FUNCTION_PATTERN
}

pub(crate) fn cognitive_target_function_pattern() -> &'static Regex {
    &COGNITIVE_TARGET_FUNCTION_PATTERN
}

pub(crate) fn cognitive_file_names() -> &'static [&'static str] {
    COGNITIVE_FILE_NAMES
}

pub(crate) fn should_skip_json_config(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            JSON_SKIP_BASENAMES
                .iter()
                .any(|candidate| name.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or(false)
}

fn compile_patterns(definitions: &[PatternDefinition]) -> Vec<CompiledSourcePattern> {
    definitions
        .iter()
        .map(|definition| CompiledSourcePattern {
            id: definition.id,
            signal: definition.signal,
            summary: definition.summary,
            regex: Regex::new(definition.expression)
                .unwrap_or_else(|err| panic!("invalid source pattern {}: {err}", definition.id)),
        })
        .collect()
}
