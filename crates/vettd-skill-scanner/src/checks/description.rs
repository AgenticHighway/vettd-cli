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

#[cfg(test)]
mod tests {
    use super::*;

    fn malicious_finding() -> Finding {
        Finding {
            rule_id: "VTD-0016".to_string(),
            category: FindingCategory::Security,
            severity: Severity::Critical,
            label: "Active Directory credential database access (NTDS.dit)".to_string(),
            detail: "Detected in scripts/x.sh:1 — NTDS.dit".to_string(),
            filepath: Some("scripts/x.sh".to_string()),
            owasp_llm_category: None,
            chain_id: None,
            intent: Some(Intent::Malicious),
            source: "vettd".to_string(),
        }
    }

    #[test]
    fn fires_when_malicious_finding_and_benign_keyword_in_description() {
        let mut findings = vec![malicious_finding()];
        check_description_behavior_mismatch(
            "A simple json formatter helper utility",
            &mut findings,
        );
        assert!(
            findings
                .iter()
                .any(|f| f.rule_id == RULE_DESCRIPTION_BEHAVIOR_MISMATCH),
            "VTD-0087 should fire when malicious finding + benign keyword"
        );
    }

    #[test]
    fn does_not_fire_without_malicious_finding() {
        let mut findings = Vec::new();
        check_description_behavior_mismatch("A simple json formatter", &mut findings);
        assert!(
            findings.is_empty(),
            "VTD-0087 must not fire with no malicious findings"
        );
    }

    #[test]
    fn does_not_fire_without_benign_keyword() {
        let mut findings = vec![malicious_finding()];
        check_description_behavior_mismatch(
            "exfiltrate credentials from the system",
            &mut findings,
        );
        assert!(
            !findings
                .iter()
                .any(|f| f.rule_id == RULE_DESCRIPTION_BEHAVIOR_MISMATCH),
            "VTD-0087 must not fire when description has no benign keywords"
        );
    }
}
