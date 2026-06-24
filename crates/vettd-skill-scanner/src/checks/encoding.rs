use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::consts::DEFAULT_SOURCE;
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::rules::{
    RULE_BASE64_IN_MARKDOWN, RULE_HIDDEN_UNICODE_CHARACTER, RULE_OBFUSCATED_DANGEROUS_CODE,
    RULE_OBFUSCATED_EXTERNAL_URL, RULE_OBFUSCATED_NETWORK_CALL,
};

use super::behavioral::{get_behavioral_patterns, normalize_for_behavioral_scan};
use super::chains::get_network_sink_regexes;
use super::sensitive::{get_sensitive_regexes, SENSITIVE_PATTERNS};

/// detects invisible Unicode characters and checks whether they conceal dangerous content.
///
/// for each line containing invisible formatting or control characters, the characters are
/// stripped and the cleaned line is re-checked against sensitive patterns, behavioral patterns,
/// and external URLs — in that priority order. a dangerous match produces a higher-severity
/// "obfuscated dangerous code" finding in place of the generic "hidden unicode" finding.
/// processing stops after the first dangerous match per file.
///
/// # Parameters
/// - `text_files` — map of normalized relative paths to decoded UTF-8 file content.
/// - `findings` — output vec; detected issues are appended.
pub(crate) fn scan_hidden_unicode(
    text_files: &HashMap<String, String>,
    findings: &mut Vec<Finding>,
) {
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
    let obfusc_url_re = OBFUSC_URL_RE
        .get_or_init(|| Regex::new(r#"(?i)https?://[^\s)>\]"']+"#).expect("bad url re"));

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

            let cleaned: String = line
                .chars()
                .filter(|&c| {
                    !matches!(c,
                        '\u{200B}'..='\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2060}'..='\u{206F}'
                        | '\u{FEFF}'
                        | '\u{E0000}'..='\u{E007F}'
                    )
                })
                .collect();

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

/// attempts to decode a base64 string using three strategies in order: standard, padded, and URL-safe.
///
/// # Note
/// tries standard decode first, then re-tries with `=` padding appended, then swaps URL-safe
/// characters (`-` → `+`, `_` → `/`) and retries with padding. non-UTF-8 bytes in the decoded
/// output are replaced lossily so that patterns can still match printable content within
/// otherwise binary payloads. returns `None` only if all three strategies fail.
fn decode_base64_lenient(s: &str) -> Option<String> {
    use base64::{engine::general_purpose, Engine as _};
    if let Ok(bytes) = general_purpose::STANDARD.decode(s) {
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
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

/// scans source text for adjacent quoted base64 segments joined by `+` and concatenates them.
///
/// detects patterns like `"aGVsbG8="` + `"d29ybGQ="` where a base64 value is split across
/// multiple string literals. returns each joined group that spans at least two segments
/// and totals ≥ 40 characters.
fn join_concatenated_strings(content: &str) -> Vec<String> {
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

/// scans files for base64-encoded payloads that decode to dangerous patterns.
///
/// three candidate extraction strategies run per file: raw base64 chunk extraction,
/// concatenated string literal detection, and shell variable assignment extraction.
/// each decoded candidate is checked — in priority order — against sensitive patterns,
/// network sinks, and external URLs. for markdown files, any decodable printable-majority
/// payload also produces a lower-severity advisory finding.
///
/// # Parameters
/// - `text_files` — map of normalized relative paths to decoded UTF-8 file content.
/// - `findings` — output vec; detected issues are appended.
///
/// # Returns
/// `(secrets_failed, behavioral_failed)`.
///
/// # Note
/// `behavioral_failed` is always `false` in this implementation — the return slot is
/// reserved for a future behavioral pass over decoded payloads.
pub(crate) fn check_base64_payloads(
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
        if path.starts_with("evals/") || path.starts_with("references/") {
            continue;
        }
        let is_doc = path.to_lowercase().ends_with(".md");
        let mut warn_emitted = false;

        // three extraction strategies: raw chunks, concatenated literals, shell var assignments.
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
