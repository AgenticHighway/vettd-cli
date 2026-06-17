//! Custom rules detector — applies declarative TOML rule files.
//!
//! Loads rules from `~/.vettd/rules/` and evaluates them against
//! scan candidates during built-in detection.

use crate::discovery::Candidate;
use crate::models::{
    check_for_dangerous_patterns, check_for_secrets, gather_file_primitives,
    is_content_read_allowed, ArtifactReport,
};
use crate::rule_engine::{
    default_rules_dir, load_builtin_rules, load_rules_from_dir, scan_rule_keywords,
    scan_rule_patterns, DetectionRule,
};

use super::base::Detector;
use super::read_utf8_head;
use serde_json::json;
use std::collections::HashMap;

const MAX_READ_BYTES: usize = 8192;

pub struct CustomRulesDetector {
    rules: Vec<DetectionRule>,
    exact_filename_rule_indexes: HashMap<String, Vec<usize>>,
    suffix_rule_matchers: Vec<(usize, Vec<String>)>,
    glob_filename_rule_matchers: Vec<(usize, Vec<glob::Pattern>)>,
}

impl CustomRulesDetector {
    pub fn load() -> Self {
        // Always start with the built-in rules (compiled from rules/ in the repo)
        let mut rules = load_builtin_rules();

        // Supplement with user-installed rules from ~/.vettd/rules/
        if let Some(dir) = default_rules_dir() {
            if dir.is_dir() {
                let user_rules = load_rules_from_dir(&dir);
                if !user_rules.is_empty() {
                    eprintln!(
                        "Loaded {} custom rule(s) from {}",
                        user_rules.len(),
                        dir.display()
                    );
                }
                rules.extend(user_rules);
            }
        }

        Self::from_rules(rules)
    }

    fn from_rules(rules: Vec<DetectionRule>) -> Self {
        let mut exact_filename_rule_indexes: HashMap<String, Vec<usize>> = HashMap::new();
        let mut suffix_rule_matchers = Vec::new();
        let mut glob_filename_rule_matchers = Vec::new();

        for (index, rule) in rules.iter().enumerate() {
            let mut lower_suffixes = Vec::new();
            let mut compiled_globs = Vec::new();

            for pattern in &rule.match_config.filenames {
                if pattern.contains('*') {
                    if let Ok(compiled) = glob::Pattern::new(&pattern.to_lowercase()) {
                        compiled_globs.push(compiled);
                    }
                } else {
                    let indexes = exact_filename_rule_indexes
                        .entry(pattern.to_lowercase())
                        .or_default();
                    if !indexes.contains(&index) {
                        indexes.push(index);
                    }
                }
            }

            for suffix in &rule.match_config.suffixes {
                lower_suffixes.push(suffix.to_lowercase());
            }

            if !lower_suffixes.is_empty() {
                suffix_rule_matchers.push((index, lower_suffixes));
            }

            if !compiled_globs.is_empty() {
                glob_filename_rule_matchers.push((index, compiled_globs));
            }
        }

        Self {
            rules,
            exact_filename_rule_indexes,
            suffix_rule_matchers,
            glob_filename_rule_matchers,
        }
    }

    fn candidate_rule_indexes(&self, file_name: &str) -> Vec<usize> {
        let lower = file_name.to_lowercase();
        let mut matched = self
            .exact_filename_rule_indexes
            .get(&lower)
            .cloned()
            .unwrap_or_default();

        for (index, suffixes) in &self.suffix_rule_matchers {
            if suffixes.iter().any(|suffix| lower.ends_with(suffix)) && !matched.contains(index) {
                matched.push(*index);
            }
        }

        for (index, patterns) in &self.glob_filename_rule_matchers {
            if patterns.iter().any(|pattern| pattern.matches(&lower)) && !matched.contains(index) {
                matched.push(*index);
            }
        }

        matched
    }
}

impl Detector for CustomRulesDetector {
    fn name(&self) -> &str {
        "custom_rules"
    }

    fn detect(&self, candidates: &[Candidate], deep: bool) -> Vec<ArtifactReport> {
        let mut results = Vec::new();
        for candidate in candidates {
            let Some(file_name) = candidate.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };

            for rule_index in self.candidate_rule_indexes(file_name) {
                if let Some(report) =
                    apply_rule(candidate, file_name, &self.rules[rule_index], deep)
                {
                    results.push(report);
                }
            }
        }
        results
    }
}

fn apply_rule(
    candidate: &Candidate,
    file_name: &str,
    rule: &DetectionRule,
    deep: bool,
) -> Option<ArtifactReport> {
    let mut confidence = rule.match_config.confidence;
    let mut signals = Vec::new();
    let mut metadata = serde_json::Map::new();

    // File primitives — gather once, avoid re-reads downstream
    let file_prims = gather_file_primitives(&candidate.path);
    metadata.extend(file_prims);

    signals.push(format!("filename_match:{file_name}"));
    metadata.insert("paths".into(), json!([candidate.path.to_string_lossy()]));
    metadata.insert("rule_name".into(), json!(rule.detector.name));

    // Content analysis (if allowed and readable)
    if is_content_read_allowed(&candidate.path) {
        if let Some(content) = read_utf8_head(&candidate.path, MAX_READ_BYTES) {
            // Primary keywords
            if let Some(ref kw) = rule.keywords {
                let (kw_signals, kw_count) = scan_rule_keywords(&content, kw);
                signals.extend(kw_signals);
                if kw_count >= kw.boost_threshold {
                    if let Some(boost) = kw.boost_confidence {
                        confidence = confidence.max(boost);
                    }
                }
            }

            if let Some(ref patterns) = rule.patterns {
                let (pattern_signals, pattern_count) = scan_rule_patterns(&content, patterns);
                signals.extend(pattern_signals);
                if pattern_count >= patterns.boost_threshold {
                    if let Some(boost) = patterns.boost_confidence {
                        confidence = confidence.max(boost);
                    }
                }
            }

            // Deep keywords (only in deep mode)
            if deep {
                if let Some(ref dk) = rule.deep_keywords {
                    let (dk_signals, dk_count) = scan_rule_keywords(&content, dk);
                    signals.extend(dk_signals);
                    if dk_count >= dk.boost_threshold {
                        if let Some(boost) = dk.boost_confidence {
                            confidence = confidence.max(boost);
                        }
                    }
                }

                if let Some(ref patterns) = rule.deep_patterns {
                    let (pattern_signals, pattern_count) = scan_rule_patterns(&content, patterns);
                    signals.extend(pattern_signals);
                    if pattern_count >= patterns.boost_threshold {
                        if let Some(boost) = patterns.boost_confidence {
                            confidence = confidence.max(boost);
                        }
                    }
                }
            }

            signals.extend(check_for_secrets(&content));
            signals.extend(check_for_dangerous_patterns(&content));
        }
    }

    confidence = confidence.min(1.0);

    let mut report = ArtifactReport::new(&rule.detector.artifact_type, confidence);
    report.signals = signals;
    report.metadata = metadata;
    report.artifact_scope = candidate.origin.clone();
    report.compute_hash();
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule_engine::parse_rule_content;

    #[test]
    fn candidate_rule_indexes_matches_exact_suffix_and_glob_rules() {
        let exact = parse_rule_content(
            r#"
[detector]
name = "agents_rule"
artifact_type = "custom_agent"

[match]
filenames = ["agents.md"]
confidence = 0.8
"#,
        )
        .unwrap();
        let suffix = parse_rule_content(
            r#"
[detector]
name = "prompt_rule"
artifact_type = "custom_prompt"

[match]
suffixes = [".prompt.md"]
confidence = 0.8
"#,
        )
        .unwrap();
        let glob = parse_rule_content(
            r#"
[detector]
name = "instructions_rule"
artifact_type = "custom_instructions"

[match]
filenames = ["*instructions.md"]
confidence = 0.8
"#,
        )
        .unwrap();

        let detector = CustomRulesDetector::from_rules(vec![exact, suffix, glob]);

        assert_eq!(detector.candidate_rule_indexes("AGENTS.md"), vec![0]);
        assert_eq!(detector.candidate_rule_indexes("team.prompt.md"), vec![1]);
        assert_eq!(
            detector.candidate_rule_indexes("copilot-instructions.md"),
            vec![2]
        );
        assert!(detector.candidate_rule_indexes("README.md").is_empty());
    }
}
