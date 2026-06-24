use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::consts::{
    COVERT_CHANNEL_FRAGS, CRED_SOURCE_FRAGS, DEFAULT_SOURCE, EVASION_FRAGS, EXECUTION_FRAGS,
    FETCH_FRAGS, PERSISTENCE_FRAGS,
};
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::rules::{RULE_CREDENTIAL_EXFILTRATION_CHAIN, RULE_MALICIOUS_ACTIVITY_CHAIN};

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

pub(crate) fn get_network_sink_regexes() -> &'static [Regex] {
    NETWORK_SINK_REGEXES.get_or_init(|| {
        NETWORK_SINK_STRS
            .iter()
            .map(|s| Regex::new(s).expect("invalid network sink pattern"))
            .collect()
    })
}

fn extract_filepath_from_detail(detail: &str) -> Option<&str> {
    let rest = detail.strip_prefix("Detected in ")?;
    let colon = rest.find(':')?;
    Some(&rest[..colon])
}

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

pub(crate) fn detect_malicious_activity_chains(findings: &mut Vec<Finding>) {
    let mut buckets_by_file: HashMap<String, Vec<&'static str>> = HashMap::new();
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
            if !buckets.contains(&bucket) {
                buckets.push(bucket);
            }
        }
    }

    let mut chain_index: u32 = 0;
    let mut new_findings: Vec<Finding> = Vec::new();

    for (file_path, buckets) in &buckets_by_file {
        let file_indices = indices_by_file.get(file_path).cloned().unwrap_or_default();

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

pub(crate) fn detect_exfiltration_chains(
    findings: &mut Vec<Finding>,
    text_files: &HashMap<String, String>,
) {
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
        for &idx in source_indices {
            findings[idx].chain_id = Some(chain_id.clone());
            findings[idx].intent = Some(Intent::Malicious);
            if !matches!(findings[idx].severity, Severity::Critical) {
                findings[idx].severity = Severity::Critical;
            }
        }
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
