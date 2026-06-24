use crate::consts::{BENIGN_DESCRIPTION_KEYWORDS, DEFAULT_SOURCE};
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::rules::RULE_DESCRIPTION_BEHAVIOR_MISMATCH;

pub(crate) fn check_description_behavior_mismatch(description: &str, findings: &mut Vec<Finding>) {
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
