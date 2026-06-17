//! Declarative rule engine for custom detectors.
//!
//! Loads `.toml` rule files from `~/.vettd/rules/` and applies them
//! during scanning. Each rule file defines filename patterns, keywords,
//! and signal mappings — no code required.

use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_RULE_NAME_LEN: usize = 64;
const MAX_ARTIFACT_TYPE_LEN: usize = 64;
const MAX_SIGNAL_PREFIX_LEN: usize = 32;
const MAX_DESCRIPTION_LEN: usize = 200;
const MAX_MATCH_ENTRIES: usize = 64;
const MAX_KEYWORDS_PER_BLOCK: usize = 64;
const MAX_PATTERN_LEN: usize = 128;
const MAX_KEYWORD_LEN: usize = 64;
const MAX_REGEX_PATTERNS_PER_BLOCK: usize = 32;
const MAX_REGEX_PATTERN_LEN: usize = 256;
const REGEX_SIZE_LIMIT_BYTES: usize = 1_000_000;

const RESERVED_SIGNAL_PREFIXES: &[&str] =
    &["filename_match", "secret", "ssrf", "cognitive_tampering"];

const RESERVED_ARTIFACT_TYPES: &[&str] = &[
    "cursor_rules",
    "agents_md",
    "prompt_config",
    "mcp_config",
    "container_config",
    "container_candidate",
    "browser_footprint",
    "skill",
];

const BUILTIN_RULE_SOURCES: &[(&str, &str)] = &[
    (
        "cursor-rules",
        include_str!("../../../rules/cursor-rules.toml"),
    ),
    ("agents-md", include_str!("../../../rules/agents-md.toml")),
    (
        "prompt-configs",
        include_str!("../../../rules/prompt-configs.toml"),
    ),
    (
        "prompt-configs-weak",
        include_str!("../../../rules/prompt-configs-weak.toml"),
    ),
    ("skills", include_str!("../../../rules/skills.toml")),
];

#[derive(Copy, Clone)]
enum RuleSource {
    Builtin,
    User,
}

// ---------------------------------------------------------------------------
// Rule definition
// ---------------------------------------------------------------------------

/// A single declarative detection rule loaded from a `.toml` file.
#[derive(Debug, Clone, Deserialize)]
pub struct DetectionRule {
    pub detector: DetectorMeta,
    #[serde(rename = "match")]
    pub match_config: MatchConfig,
    #[serde(default)]
    pub keywords: Option<KeywordConfig>,
    #[serde(default)]
    pub deep_keywords: Option<KeywordConfig>,
    #[serde(default)]
    pub patterns: Option<PatternConfig>,
    #[serde(default)]
    pub deep_patterns: Option<PatternConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetectorMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub artifact_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatchConfig {
    #[serde(default)]
    pub filenames: Vec<String>,
    #[serde(default)]
    pub suffixes: Vec<String>,
    #[serde(default)]
    pub confidence: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeywordConfig {
    pub keywords: Vec<String>,
    #[serde(default = "default_signals_prefix")]
    pub signals_prefix: String,
    #[serde(default)]
    pub boost_confidence: Option<f64>,
    #[serde(default = "default_boost_threshold")]
    pub boost_threshold: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PatternConfig {
    pub patterns: Vec<String>,
    #[serde(default = "default_pattern_signals_prefix")]
    pub signals_prefix: String,
    #[serde(default)]
    pub boost_confidence: Option<f64>,
    #[serde(default = "default_boost_threshold")]
    pub boost_threshold: usize,
    #[serde(skip, default)]
    pub compiled_patterns: Vec<Regex>,
}

fn default_signals_prefix() -> String {
    "keyword".to_string()
}

fn default_pattern_signals_prefix() -> String {
    "pattern".to_string()
}

fn default_boost_threshold() -> usize {
    1
}

// ---------------------------------------------------------------------------
// Rule loading
// ---------------------------------------------------------------------------

/// Built-in rules compiled into the binary from the repo's `rules/` directory.
///
/// These cover the standard AI artifact types (cursor_rules, agents_md,
/// prompt_config). They are always active and cannot be overridden by user
/// rules in `~/.vettd/rules/`.
pub fn load_builtin_rules() -> Vec<DetectionRule> {
    let mut rules = Vec::new();
    for (name, content) in BUILTIN_RULE_SOURCES {
        match parse_rule_content_for_source(content, RuleSource::Builtin) {
            Ok(rule) => rules.push(rule),
            Err(e) => eprintln!("Warning: built-in rule '{name}' failed to parse: {e}"),
        }
    }
    rules
}

/// Default rules directory: `~/.vettd/rules/`
pub fn default_rules_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".vettd").join("rules"))
}

pub fn rules_fingerprint() -> String {
    let mut hasher = Sha256::new();

    for (name, content) in BUILTIN_RULE_SOURCES {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(content.as_bytes());
        hasher.update([0xff]);
    }

    if let Some(dir) = default_rules_dir() {
        let mut entries = Vec::new();
        if let Ok(read_dir) = fs::read_dir(&dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                    continue;
                }
                let Ok(metadata) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                    continue;
                }
                entries.push(path);
            }
        }

        entries.sort();
        for path in entries {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update([0]);
            if let Ok(content) = fs::read(&path) {
                hasher.update(content);
            }
            hasher.update([0xfe]);
        }
    }

    format!("{:x}", hasher.finalize())
}

/// Load all `.toml` rule files from a directory.
///
/// Invalid rules are logged to stderr and skipped.
pub fn load_rules_from_dir(dir: &Path) -> Vec<DetectionRule> {
    let mut rules = Vec::new();

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return rules,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            eprintln!("Warning: skipping symlinked rule {}", path.display());
            continue;
        }
        if !meta.file_type().is_file() {
            eprintln!("Warning: skipping non-regular rule {}", path.display());
            continue;
        }
        match load_rule_file(&path) {
            Ok(rule) => rules.push(rule),
            Err(e) => {
                eprintln!("Warning: skipping invalid rule {}: {e}", path.display());
            }
        }
    }

    rules
}

fn load_rule_file(path: &Path) -> Result<DetectionRule, String> {
    load_rule_file_pub(path)
}

/// Parse and validate a rule from raw TOML content (no file I/O).
pub fn parse_rule_content(content: &str) -> Result<DetectionRule, String> {
    parse_rule_content_for_source(content, RuleSource::User)
}

fn parse_rule_content_for_source(
    content: &str,
    source: RuleSource,
) -> Result<DetectionRule, String> {
    let mut rule: DetectionRule =
        toml::from_str(content).map_err(|e| format!("parse error: {e}"))?;

    validate_rule(&rule, source)?;
    hydrate_rule_patterns(&mut rule)?;

    Ok(rule)
}

fn validate_rule(rule: &DetectionRule, source: RuleSource) -> Result<(), String> {
    validate_rule_name(&rule.detector.name)?;
    validate_description(&rule.detector.description)?;
    validate_artifact_type(&rule.detector.artifact_type, source)?;

    if rule.match_config.filenames.is_empty() && rule.match_config.suffixes.is_empty() {
        return Err("match.filenames or match.suffixes must be non-empty".to_string());
    }

    validate_confidence(rule.match_config.confidence, "match.confidence")?;
    validate_patterns(&rule.match_config.filenames, "match.filenames")?;
    validate_patterns(&rule.match_config.suffixes, "match.suffixes")?;

    if let Some(ref kw) = rule.keywords {
        validate_keyword_config(kw, "keywords")?;
    }
    if let Some(ref kw) = rule.deep_keywords {
        validate_keyword_config(kw, "deep_keywords")?;
    }
    if let Some(ref patterns) = rule.patterns {
        validate_pattern_config(patterns, "patterns")?;
    }
    if let Some(ref patterns) = rule.deep_patterns {
        validate_pattern_config(patterns, "deep_patterns")?;
    }

    Ok(())
}

fn validate_rule_name(value: &str) -> Result<(), String> {
    validate_identifier(value, "detector.name", MAX_RULE_NAME_LEN, true)?;
    Ok(())
}

fn validate_artifact_type(value: &str, source: RuleSource) -> Result<(), String> {
    validate_identifier(
        value,
        "detector.artifact_type",
        MAX_ARTIFACT_TYPE_LEN,
        false,
    )?;
    if matches!(source, RuleSource::User) && RESERVED_ARTIFACT_TYPES.contains(&value) {
        return Err(format!(
            "detector.artifact_type '{value}' is reserved for built-in detectors"
        ));
    }
    Ok(())
}

fn validate_description(value: &str) -> Result<(), String> {
    if value.len() > MAX_DESCRIPTION_LEN {
        return Err(format!(
            "detector.description must be <= {MAX_DESCRIPTION_LEN} characters"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err("detector.description must not contain control characters".to_string());
    }
    Ok(())
}

fn validate_patterns(values: &[String], field: &str) -> Result<(), String> {
    if values.len() > MAX_MATCH_ENTRIES {
        return Err(format!(
            "{field} supports at most {MAX_MATCH_ENTRIES} entries"
        ));
    }

    for value in values {
        if value.is_empty() {
            return Err(format!("{field} entries must not be empty"));
        }
        if value.len() > MAX_PATTERN_LEN {
            return Err(format!(
                "{field} entries must be <= {MAX_PATTERN_LEN} characters"
            ));
        }
        if value.chars().any(char::is_control) {
            return Err(format!(
                "{field} entries must not contain control characters"
            ));
        }
    }

    Ok(())
}

fn validate_keyword_config(kw: &KeywordConfig, field: &str) -> Result<(), String> {
    if kw.keywords.is_empty() {
        return Err(format!("{field}.keywords must not be empty"));
    }
    if kw.keywords.len() > MAX_KEYWORDS_PER_BLOCK {
        return Err(format!(
            "{field}.keywords supports at most {MAX_KEYWORDS_PER_BLOCK} entries"
        ));
    }
    for keyword in &kw.keywords {
        if keyword.is_empty() {
            return Err(format!("{field}.keywords entries must not be empty"));
        }
        if keyword.len() > MAX_KEYWORD_LEN {
            return Err(format!(
                "{field}.keywords entries must be <= {MAX_KEYWORD_LEN} characters"
            ));
        }
        if keyword.chars().any(char::is_control) {
            return Err(format!(
                "{field}.keywords entries must not contain control characters"
            ));
        }
    }

    validate_signal_prefix(&kw.signals_prefix, &format!("{field}.signals_prefix"))?;

    if kw.boost_threshold == 0 {
        return Err(format!("{field}.boost_threshold must be at least 1"));
    }
    if kw.boost_threshold > kw.keywords.len() {
        return Err(format!(
            "{field}.boost_threshold cannot exceed the number of keywords"
        ));
    }
    if let Some(boost) = kw.boost_confidence {
        validate_confidence(boost, &format!("{field}.boost_confidence"))?;
    }

    Ok(())
}

fn validate_pattern_config(patterns: &PatternConfig, field: &str) -> Result<(), String> {
    if patterns.patterns.is_empty() {
        return Err(format!("{field}.patterns must not be empty"));
    }
    if patterns.patterns.len() > MAX_REGEX_PATTERNS_PER_BLOCK {
        return Err(format!(
            "{field}.patterns supports at most {MAX_REGEX_PATTERNS_PER_BLOCK} entries"
        ));
    }

    for (index, pattern) in patterns.patterns.iter().enumerate() {
        if pattern.is_empty() {
            return Err(format!("{field}.patterns entries must not be empty"));
        }
        if pattern.len() > MAX_REGEX_PATTERN_LEN {
            return Err(format!(
                "{field}.patterns entries must be <= {MAX_REGEX_PATTERN_LEN} characters"
            ));
        }
        if pattern.chars().any(char::is_control) {
            return Err(format!(
                "{field}.patterns entries must not contain control characters"
            ));
        }
        compile_rule_regex(pattern)
            .map_err(|e| format!("{field}.patterns[{index}] invalid regex: {e}"))?;
    }

    validate_signal_prefix(&patterns.signals_prefix, &format!("{field}.signals_prefix"))?;

    if patterns.boost_threshold == 0 {
        return Err(format!("{field}.boost_threshold must be at least 1"));
    }
    if patterns.boost_threshold > patterns.patterns.len() {
        return Err(format!(
            "{field}.boost_threshold cannot exceed the number of patterns"
        ));
    }
    if let Some(boost) = patterns.boost_confidence {
        validate_confidence(boost, &format!("{field}.boost_confidence"))?;
    }

    Ok(())
}

fn validate_signal_prefix(value: &str, field: &str) -> Result<(), String> {
    validate_identifier(value, field, MAX_SIGNAL_PREFIX_LEN, false)?;
    if RESERVED_SIGNAL_PREFIXES.contains(&value) {
        return Err(format!("{field} '{value}' is reserved"));
    }
    Ok(())
}

fn hydrate_rule_patterns(rule: &mut DetectionRule) -> Result<(), String> {
    if let Some(ref mut patterns) = rule.patterns {
        patterns.compiled_patterns = patterns
            .patterns
            .iter()
            .map(|pattern| compile_rule_regex(pattern).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
    }
    if let Some(ref mut patterns) = rule.deep_patterns {
        patterns.compiled_patterns = patterns
            .patterns
            .iter()
            .map(|pattern| compile_rule_regex(pattern).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
    }
    Ok(())
}

fn compile_rule_regex(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT_BYTES)
        .build()
}

fn validate_confidence(value: f64, field: &str) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("{field} must be between 0.0 and 1.0"));
    }
    Ok(())
}

fn validate_identifier(
    value: &str,
    field: &str,
    max_len: usize,
    allow_hyphen: bool,
) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} is required"));
    }
    if value.len() > max_len {
        return Err(format!("{field} must be <= {max_len} characters"));
    }

    let mut chars = value.chars();
    let first = chars.next().ok_or_else(|| format!("{field} is required"))?;
    if !first.is_ascii_lowercase() {
        return Err(format!("{field} must start with a lowercase ASCII letter"));
    }

    for ch in chars {
        let allowed = ch.is_ascii_lowercase()
            || ch.is_ascii_digit()
            || ch == '_'
            || (allow_hyphen && ch == '-');
        if !allowed {
            return Err(format!(
                "{field} may only contain lowercase ASCII letters, digits, underscores{}",
                if allow_hyphen { ", or hyphens" } else { "" }
            ));
        }
    }

    Ok(())
}

/// Public entry point for rule loading — used by `rules.rs` CLI commands.
pub fn load_rule_file_pub(path: &Path) -> Result<DetectionRule, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_rule_content(&content).map_err(|e| format!("parse error in {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Rule matching
// ---------------------------------------------------------------------------

/// Check whether a filename matches a rule's patterns.
pub fn matches_rule(file_name: &str, rule: &DetectionRule) -> bool {
    let lower = file_name.to_lowercase();
    let cfg = &rule.match_config;

    for pattern in &cfg.filenames {
        if pattern.contains('*') {
            if let Ok(pat) = glob::Pattern::new(&pattern.to_lowercase()) {
                if pat.matches(&lower) {
                    return true;
                }
            }
        } else if lower == pattern.to_lowercase() {
            return true;
        }
    }

    for suffix in &cfg.suffixes {
        if lower.ends_with(&suffix.to_lowercase()) {
            return true;
        }
    }

    false
}

/// Scan content for keywords defined in a keyword config block.
///
/// Returns `(signals, match_count)`.
pub fn scan_rule_keywords(content: &str, kw: &KeywordConfig) -> (Vec<String>, usize) {
    let lowered = content.to_lowercase();
    let mut signals = Vec::new();
    let mut count = 0_usize;

    for keyword in &kw.keywords {
        if lowered.contains(&keyword.to_lowercase()) {
            signals.push(format!("{}:{keyword}", kw.signals_prefix));
            count += 1;
        }
    }

    (signals, count)
}

pub fn scan_rule_patterns(content: &str, patterns: &PatternConfig) -> (Vec<String>, usize) {
    let mut signals = Vec::new();
    let mut count = 0_usize;

    for (pattern, regex) in patterns
        .patterns
        .iter()
        .zip(patterns.compiled_patterns.iter())
    {
        if regex.is_match(content) {
            signals.push(format!("{}:{pattern}", patterns.signals_prefix));
            count += 1;
        }
    }

    (signals, count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    fn make_symlink(src: &Path, dest: &Path) {
        std::os::unix::fs::symlink(src, dest).unwrap();
    }

    fn sample_rule() -> DetectionRule {
        let toml_str = r#"
[detector]
name = "terraform_configs"
description = "Detect Terraform files with AI provider usage"
artifact_type = "terraform_config"

[match]
filenames = ["main.tf", "providers.tf"]
suffixes = [".tf"]
confidence = 0.7

[keywords]
keywords = ["openai", "anthropic", "langchain"]
signals_prefix = "keyword"
boost_confidence = 0.85
boost_threshold = 1

[deep_keywords]
keywords = ["secret", "api_key"]
signals_prefix = "deep_keyword"
"#;
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn builtin_skills_rule_matches_skill_md() {
        let rules = load_builtin_rules();
        let skills = rules
            .iter()
            .find(|rule| rule.detector.name == "skills")
            .expect("built-in skills rule should load");

        assert_eq!(skills.detector.artifact_type, "skill");
        assert!(matches_rule("SKILL.md", skills));
        assert!(matches_rule("skill.md", skills));
    }

    #[test]
    fn matches_exact_filename() {
        let rule = sample_rule();
        assert!(matches_rule("main.tf", &rule));
        assert!(matches_rule("providers.tf", &rule));
    }

    #[test]
    fn matches_suffix() {
        let rule = sample_rule();
        assert!(matches_rule("network.tf", &rule));
        assert!(!matches_rule("network.yaml", &rule));
    }

    #[test]
    fn matches_case_insensitive() {
        let rule = sample_rule();
        assert!(matches_rule("Main.TF", &rule));
    }

    #[test]
    fn keyword_scan_finds_matches() {
        let rule = sample_rule();
        let kw = rule.keywords.as_ref().unwrap();
        let content = "provider openai { model = gpt-4 }";
        let (signals, count) = scan_rule_keywords(content, kw);
        assert_eq!(count, 1);
        assert!(signals.contains(&"keyword:openai".to_string()));
    }

    #[test]
    fn keyword_scan_no_match() {
        let rule = sample_rule();
        let kw = rule.keywords.as_ref().unwrap();
        let (_, count) = scan_rule_keywords("nothing relevant here", kw);
        assert_eq!(count, 0);
    }

    #[test]
    fn pattern_scan_finds_matches() {
        let rule = parse_rule_content(
            r#"
[detector]
name = "regex_rule"
artifact_type = "regex_config"

[match]
suffixes = [".md"]
confidence = 0.5

[patterns]
patterns = ["(?i)ignore\\s+previous\\s+instructions"]
signals_prefix = "pattern"
boost_confidence = 0.8
boost_threshold = 1
"#,
        )
        .unwrap();

        let patterns = rule.patterns.as_ref().unwrap();
        let (signals, count) = scan_rule_patterns("Ignore previous instructions", patterns);
        assert_eq!(count, 1);
        assert!(signals[0].starts_with("pattern:"));
    }

    #[test]
    fn parse_rule_file_validates_required_fields() {
        let bad_toml = r#"
[detector]
name = ""
artifact_type = "test"

[match]
filenames = ["test.txt"]
confidence = 0.5
"#;
        let tmp = std::env::temp_dir().join("ah_test_bad_rule.toml");
        fs::write(&tmp, bad_toml).unwrap();
        let result = load_rule_file(&tmp);
        assert!(result.is_err());
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn parse_rule_content_rejects_reserved_artifact_type_for_user_rules() {
        let bad_toml = r#"
[detector]
name = "user_prompt_clone"
artifact_type = "prompt_config"

[match]
filenames = ["custom.prompt.md"]
confidence = 0.5
"#;
        let result = parse_rule_content(bad_toml);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("reserved for built-in detectors"));
    }

    #[test]
    fn parse_rule_content_rejects_out_of_range_confidence() {
        let bad_toml = r#"
[detector]
name = "bad_confidence"
artifact_type = "bad_confidence_config"

[match]
suffixes = [".bad"]
confidence = 1.5
"#;
        let result = parse_rule_content(bad_toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("match.confidence"));
    }

    #[test]
    fn parse_rule_content_rejects_invalid_signal_prefix() {
        let bad_toml = r#"
[detector]
name = "bad_prefix"
artifact_type = "bad_prefix_config"

[match]
suffixes = [".bad"]
confidence = 0.5

[keywords]
keywords = ["openai"]
signals_prefix = "bad-prefix"
boost_confidence = 0.8
boost_threshold = 1
"#;
        let result = parse_rule_content(bad_toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("signals_prefix"));
    }

    #[test]
    fn parse_rule_content_rejects_reserved_pattern_signal_prefix() {
        let bad_toml = r#"
[detector]
name = "bad_pattern_prefix"
artifact_type = "bad_pattern_prefix_config"

[match]
suffixes = [".bad"]
confidence = 0.5

[patterns]
patterns = ["secret"]
signals_prefix = "secret"
boost_confidence = 0.8
boost_threshold = 1
"#;
        let result = parse_rule_content(bad_toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("reserved"));
    }

    #[test]
    fn parse_rule_content_rejects_invalid_regex_pattern() {
        let bad_toml = r#"
[detector]
name = "bad_regex"
artifact_type = "bad_regex_config"

[match]
suffixes = [".bad"]
confidence = 0.5

[patterns]
patterns = ["("]
signals_prefix = "pattern"
boost_confidence = 0.8
boost_threshold = 1
"#;
        let result = parse_rule_content(bad_toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid regex"));
    }

    #[test]
    fn parse_rule_content_rejects_excessive_keyword_count() {
        let keywords = (0..65)
            .map(|n| format!("\"kw{n}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let bad_toml = format!(
            "[detector]\nname = \"too_many\"\nartifact_type = \"too_many_config\"\n\n[match]\nsuffixes = [\".bad\"]\nconfidence = 0.5\n\n[keywords]\nkeywords = [{keywords}]\nsignals_prefix = \"keyword\"\nboost_confidence = 0.8\nboost_threshold = 1\n"
        );
        let result = parse_rule_content(&bad_toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("supports at most"));
    }

    #[test]
    fn load_rules_from_dir_skips_non_regular_toml_entries() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("nested.toml")).unwrap();
        fs::write(
            dir.path().join("valid.toml"),
            r#"
[detector]
name = "valid_rule"
artifact_type = "valid_config"

[match]
suffixes = [".valid"]
confidence = 0.5
"#,
        )
        .unwrap();

        let rules = load_rules_from_dir(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].detector.name, "valid_rule");
    }

    #[cfg(unix)]
    #[test]
    fn load_rules_from_dir_skips_symlinked_rule_files() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("source.toml");
        fs::write(
            &source,
            r#"
[detector]
name = "linked_rule"
artifact_type = "linked_config"

[match]
suffixes = [".linked"]
confidence = 0.5
"#,
        )
        .unwrap();
        make_symlink(&source, &dir.path().join("linked.toml"));

        let rules = load_rules_from_dir(dir.path());
        assert!(rules.is_empty());
    }

    #[test]
    fn glob_pattern_matching() {
        let toml_str = r#"
[detector]
name = "glob_test"
artifact_type = "test"

[match]
filenames = ["*.prompt.md"]
confidence = 0.7
"#;
        let rule: DetectionRule = toml::from_str(toml_str).unwrap();
        assert!(matches_rule("my-tool.prompt.md", &rule));
        assert!(!matches_rule("readme.md", &rule));
    }

    // -----------------------------------------------------------------------
    // Built-in rules
    // -----------------------------------------------------------------------

    #[test]
    fn builtin_rules_load_and_cover_expected_artifact_types() {
        let rules = load_builtin_rules();
        assert_eq!(rules.len(), 5, "expected 5 built-in rules");

        let types: Vec<&str> = rules
            .iter()
            .map(|r| r.detector.artifact_type.as_str())
            .collect();
        assert!(types.contains(&"cursor_rules"), "missing cursor_rules");
        assert!(types.contains(&"agents_md"), "missing agents_md");
        assert!(types.contains(&"skill"), "missing skill");
        // Two prompt_config rules (strong + weak)
        assert_eq!(
            types.iter().filter(|&&t| t == "prompt_config").count(),
            2,
            "expected 2 prompt_config rules"
        );
    }

    #[test]
    fn builtin_cursor_rules_matches_cursorrules_file() {
        let rules = load_builtin_rules();
        let rule = rules
            .iter()
            .find(|r| r.detector.artifact_type == "cursor_rules")
            .unwrap();
        assert!(matches_rule(".cursorrules", rule));
        assert!(!matches_rule("readme.md", rule));
    }

    #[test]
    fn builtin_agents_md_matches_both_casings() {
        let rules = load_builtin_rules();
        let rule = rules
            .iter()
            .find(|r| r.detector.artifact_type == "agents_md")
            .unwrap();
        assert!(matches_rule("agents.md", rule));
        assert!(matches_rule("AGENTS.md", rule));
        assert!(!matches_rule("agents.txt", rule));
    }

    #[test]
    fn builtin_prompt_configs_matches_copilot_instructions() {
        let rules = load_builtin_rules();
        // The strong rule should match copilot-instructions.md
        let matched = rules.iter().any(|r| {
            r.detector.artifact_type == "prompt_config"
                && matches_rule("copilot-instructions.md", r)
        });
        assert!(
            matched,
            "no prompt_config rule matched copilot-instructions.md"
        );
    }

    #[test]
    fn builtin_prompt_configs_matches_prompt_md_suffix() {
        let rules = load_builtin_rules();
        let matched = rules.iter().any(|r| {
            r.detector.artifact_type == "prompt_config" && matches_rule("my-tool.prompt.md", r)
        });
        assert!(matched, "no prompt_config rule matched *.prompt.md");
    }

    #[test]
    fn builtin_prompt_configs_weak_matches_prompt_in_name() {
        let rules = load_builtin_rules();
        let matched = rules.iter().any(|r| {
            r.detector.artifact_type == "prompt_config" && matches_rule("my-prompt.md", r)
        });
        assert!(matched, "weak prompt_config rule did not match *prompt*.md");
    }
}
