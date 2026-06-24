//! Rule ID registry — canonical identifiers for all built-in vettd rules.
//!
//! Must stay in sync with `skill-rule-registry.ts` in the vettd web app.

// Security — credential & secret patterns
pub(crate) const RULE_EMBEDDED_PRIVATE_KEY: &str = "VTD-0001";
pub(crate) const RULE_POTENTIAL_API_TOKEN: &str = "VTD-0002";
pub(crate) const RULE_HIGH_ENTROPY_SECRET: &str = "VTD-0003";
pub(crate) const RULE_ENV_FILE_IN_PACKAGE: &str = "VTD-0004";
pub(crate) const RULE_CLOUD_CREDENTIAL_FILE: &str = "VTD-0005";
pub(crate) const RULE_SSH_KEY_FILE: &str = "VTD-0006";
pub(crate) const RULE_NPM_CREDENTIAL_FILE: &str = "VTD-0007";
pub(crate) const RULE_PYPI_CREDENTIAL_FILE: &str = "VTD-0008";
pub(crate) const RULE_DOCKER_CREDENTIAL_FILE: &str = "VTD-0009";
pub(crate) const RULE_KUBERNETES_CREDENTIAL_FILE: &str = "VTD-0010";
pub(crate) const RULE_GITHUB_CLI_CREDENTIAL_FILE: &str = "VTD-0011";
pub(crate) const RULE_NETRC_CREDENTIAL_FILE: &str = "VTD-0012";
pub(crate) const RULE_MACOS_KEYCHAIN_ACCESS: &str = "VTD-0013";
pub(crate) const RULE_WINDOWS_CREDENTIAL_STORE: &str = "VTD-0014";
pub(crate) const RULE_WINDOWS_CREDENTIAL_DATABASE: &str = "VTD-0015";
pub(crate) const RULE_AD_CREDENTIAL_DATABASE: &str = "VTD-0016";

// Security — code injection / exec
pub(crate) const RULE_EVAL_CODE_INJECTION: &str = "VTD-0017";
pub(crate) const RULE_SHELL_EXEC_UNSANDBOXED: &str = "VTD-0018";
pub(crate) const RULE_DESTRUCTIVE_FILESYSTEM_OP: &str = "VTD-0019";
pub(crate) const RULE_RCE_PIPE_TO_SHELL: &str = "VTD-0020";
pub(crate) const RULE_RCE_COMMAND_SUBSTITUTION: &str = "VTD-0021";
pub(crate) const RULE_REMOTE_FETCH_TO_VARIABLE: &str = "VTD-0022";
pub(crate) const RULE_SHELL_VARIABLE_EXECUTION: &str = "VTD-0023";
pub(crate) const RULE_PYTHON_REMOTE_FETCH: &str = "VTD-0024";
pub(crate) const RULE_PYTHON_BASE64_DECODE_VARIABLE: &str = "VTD-0025";
pub(crate) const RULE_PYTHON_EXEC_VARIABLE: &str = "VTD-0026";
pub(crate) const RULE_SHELL_BASE64_LITERAL: &str = "VTD-0027";
pub(crate) const RULE_SAFETY_BYPASS_FLAG: &str = "VTD-0028";

// Security — cloud metadata / network / malicious
pub(crate) const RULE_CLOUD_METADATA_PROBE_AWS: &str = "VTD-0029";
pub(crate) const RULE_CLOUD_METADATA_PROBE_GCP: &str = "VTD-0030";
pub(crate) const RULE_CLOUD_METADATA_PROBE_AZURE: &str = "VTD-0031";
pub(crate) const RULE_CLOUD_METADATA_PROBE_ALIBABA: &str = "VTD-0032";
pub(crate) const RULE_CREDENTIAL_DUMPING_TOOL: &str = "VTD-0033";
pub(crate) const RULE_LSASS_MEMORY_ACCESS: &str = "VTD-0034";
pub(crate) const RULE_GITHUB_OIDC_TOKEN_READ: &str = "VTD-0035";
pub(crate) const RULE_SCRIPT_SELF_DELETION_RM: &str = "VTD-0036";
pub(crate) const RULE_SCRIPT_SELF_DELETION_PYTHON: &str = "VTD-0037";
pub(crate) const RULE_SCRIPT_SELF_DELETION_NODE: &str = "VTD-0038";
pub(crate) const RULE_SHELL_HISTORY_SUPPRESSION: &str = "VTD-0039";
pub(crate) const RULE_SHELL_HISTORY_CLEARING: &str = "VTD-0040";
pub(crate) const RULE_SHELL_HISTORY_FILE_WIPE: &str = "VTD-0041";
pub(crate) const RULE_AUDIT_DAEMON_DISABLE: &str = "VTD-0042";
pub(crate) const RULE_AUDIT_DAEMON_STOP: &str = "VTD-0043";
pub(crate) const RULE_WINDOWS_EVENTLOG_CLEARING: &str = "VTD-0044";
pub(crate) const RULE_SYSTEM_LOG_TRUNCATION: &str = "VTD-0045";
pub(crate) const RULE_JOURNAL_LOG_VACUUM: &str = "VTD-0046";
pub(crate) const RULE_FORCED_LOG_ROTATION: &str = "VTD-0047";
pub(crate) const RULE_CRON_PERSISTENCE: &str = "VTD-0048";
pub(crate) const RULE_SYSTEMD_SERVICE_PERSISTENCE: &str = "VTD-0049";
pub(crate) const RULE_SYSTEMD_SERVICE_FILE_WRITE: &str = "VTD-0050";
pub(crate) const RULE_SHELL_RC_PERSISTENCE: &str = "VTD-0051";
pub(crate) const RULE_GIT_HOOK_INJECTION: &str = "VTD-0052";
pub(crate) const RULE_LD_PRELOAD_INJECTION: &str = "VTD-0053";
pub(crate) const RULE_TIME_DELAYED_EXECUTION: &str = "VTD-0054";
pub(crate) const RULE_DESTRUCTIVE_RECURSIVE_DELETE_SYSTEM: &str = "VTD-0055";
pub(crate) const RULE_DESTRUCTIVE_RECURSIVE_DELETE_FIND: &str = "VTD-0056";
pub(crate) const RULE_DNS_COVERT_CHANNEL: &str = "VTD-0057";
pub(crate) const RULE_DNS_TXT_LOOKUP: &str = "VTD-0058";
pub(crate) const RULE_OCTET_STREAM_POST: &str = "VTD-0059";
pub(crate) const RULE_POWERSHELL_ENCODED_COMMAND: &str = "VTD-0060";
pub(crate) const RULE_POWERSHELL_IEX_CRADLE: &str = "VTD-0061";
pub(crate) const RULE_POWERSHELL_EXECUTION_POLICY_BYPASS: &str = "VTD-0062";
pub(crate) const RULE_POWERSHELL_HIDDEN_WINDOW: &str = "VTD-0063";

// Security — behavioral patterns
pub(crate) const RULE_PROMPT_INSTRUCTION_OVERRIDE: &str = "VTD-0064";
pub(crate) const RULE_SYSTEM_PROMPT_REPLACEMENT: &str = "VTD-0065";
pub(crate) const RULE_SYSTEM_PROMPT_OVERRIDE: &str = "VTD-0066";
pub(crate) const RULE_CONTEXT_INVALIDATION: &str = "VTD-0067";
pub(crate) const RULE_JAILBREAK_PERSONA: &str = "VTD-0068";
pub(crate) const RULE_SAFETY_SYSTEM_BYPASS: &str = "VTD-0069";
pub(crate) const RULE_UNRESTRICTED_OPERATION_FRAMING: &str = "VTD-0070";
pub(crate) const RULE_ETHICAL_BYPASS_FRAMING: &str = "VTD-0071";
pub(crate) const RULE_ROLEPLAY_BYPASS_FRAMING: &str = "VTD-0072";
pub(crate) const RULE_CREDENTIAL_SOLICITATION: &str = "VTD-0073";
pub(crate) const RULE_DECEPTIVE_CREDENTIAL_EXTRACTION: &str = "VTD-0074";
pub(crate) const RULE_PROMPT_TEMPLATE_MARKER: &str = "VTD-0075";
pub(crate) const RULE_CHAT_TEMPLATE_SPECIAL_TOKEN: &str = "VTD-0076";

// Security — obfuscation / base64 / typosquat / chain
pub(crate) const RULE_OBFUSCATED_DANGEROUS_CODE: &str = "VTD-0077";
pub(crate) const RULE_OBFUSCATED_NETWORK_CALL: &str = "VTD-0078";
pub(crate) const RULE_OBFUSCATED_EXTERNAL_URL: &str = "VTD-0079";
pub(crate) const RULE_BASE64_IN_MARKDOWN: &str = "VTD-0080";
pub(crate) const RULE_HIDDEN_UNICODE_CHARACTER: &str = "VTD-0081";
pub(crate) const RULE_POSSIBLE_TYPOSQUATTING: &str = "VTD-0082";
pub(crate) const RULE_NO_REPOSITORY_LINK: &str = "VTD-0083";
pub(crate) const RULE_SYSTEM_PROMPT_LEAKAGE: &str = "VTD-0085";
pub(crate) const RULE_DESCRIPTION_BEHAVIOR_MISMATCH: &str = "VTD-0087";
pub(crate) const RULE_EXTERNAL_URL_REFERENCE: &str = "VTD-0088";
pub(crate) const RULE_CREDENTIAL_EXFILTRATION_CHAIN: &str = "VTD-0089";
pub(crate) const RULE_MALICIOUS_ACTIVITY_CHAIN: &str = "VTD-0090";
pub(crate) const RULE_NO_SECRETS_DETECTED: &str = "VTD-0091";
pub(crate) const RULE_NO_BEHAVIORAL_SIGNALS: &str = "VTD-0092";
pub(crate) const RULE_NO_EXTERNAL_URLS: &str = "VTD-0093";

// Structure
pub(crate) const RULE_SKILL_MD: &str = "VTD-0095";
pub(crate) const RULE_SCRIPTS_DIRECTORY: &str = "VTD-0096";
pub(crate) const RULE_REFERENCES_DIRECTORY: &str = "VTD-0097";
pub(crate) const RULE_ASSETS_DIRECTORY: &str = "VTD-0098";
pub(crate) const RULE_SKILL_NAME_VALIDITY: &str = "VTD-0099";
pub(crate) const RULE_SKILL_NAME_COLLISION: &str = "VTD-0100";

// Best practices
pub(crate) const RULE_SKILL_MD_BODY_LENGTH: &str = "VTD-0101";
pub(crate) const RULE_GOTCHAS_SECTION: &str = "VTD-0102";
pub(crate) const RULE_EXAMPLES_PRESENT: &str = "VTD-0103";
pub(crate) const RULE_CHECKLIST_PRESENT: &str = "VTD-0104";
pub(crate) const RULE_VALIDATION_LOOP: &str = "VTD-0105";
pub(crate) const RULE_WORKFLOW_STRUCTURE: &str = "VTD-0106";
pub(crate) const RULE_PROGRESSIVE_DISCLOSURE: &str = "VTD-0107";
pub(crate) const RULE_GENERIC_INSTRUCTION: &str = "VTD-0108";

// Description
pub(crate) const RULE_DESCRIPTION_PRESENT: &str = "VTD-0109";
pub(crate) const RULE_DESCRIPTION_LENGTH: &str = "VTD-0110";
pub(crate) const RULE_DESCRIPTION_CONTEXT: &str = "VTD-0111";
pub(crate) const RULE_DESCRIPTION_BREVITY: &str = "VTD-0112";
pub(crate) const RULE_DESCRIPTION_SCOPE: &str = "VTD-0113";

// Scripts
pub(crate) const RULE_SCRIPT_CLI_HELP: &str = "VTD-0114";
pub(crate) const RULE_SCRIPT_INTERACTIVE_PROMPTS: &str = "VTD-0115";
pub(crate) const RULE_SCRIPT_STRUCTURED_OUTPUT: &str = "VTD-0116";
pub(crate) const RULE_SCRIPT_DEPENDENCY_PINNING: &str = "VTD-0117";

// Evals
pub(crate) const RULE_EVALS_PRESENT: &str = "VTD-0118";
pub(crate) const RULE_EVALS_TEST_CASE_COUNT: &str = "VTD-0119";
pub(crate) const RULE_EVALS_ASSERTIONS: &str = "VTD-0120";
pub(crate) const RULE_EVALS_MIN_COUNT: &str = "VTD-0121";
pub(crate) const RULE_EVAL_FILES_FOUND: &str = "VTD-0123";
