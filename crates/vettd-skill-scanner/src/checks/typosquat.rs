use crate::consts::{DEFAULT_SOURCE, POPULAR_SKILL_NAMES};
use crate::finding::{Finding, FindingCategory, Intent, Severity};
use crate::rules::RULE_POSSIBLE_TYPOSQUATTING;

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

/// checks whether the skill name is within Levenshtein distance 2 of any well-known skill name.
///
/// a single close match produces a medium-severity finding; two or more escalate to critical.
/// names equal to `"unknown"` or empty are skipped.
///
/// # Parameters
/// - `name` — the skill's `name` field from `SKILL.md` frontmatter.
/// - `findings` — output vec; a typosquatting finding is appended if matches are found.
pub(crate) fn check_typosquat(name: &str, findings: &mut Vec<Finding>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_close_match_produces_medium() {
        // "cod-review" is distance 1 from "code-review".
        let mut findings = Vec::new();
        check_typosquat("cod-review", &mut findings);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Medium);
    }

    #[test]
    fn single_match_finding_has_correct_metadata() {
        let mut findings = Vec::new();
        check_typosquat("cod-review", &mut findings);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_POSSIBLE_TYPOSQUATTING);
        assert_eq!(findings[0].severity, Severity::Medium);
        assert!(!findings[0].detail.is_empty());
    }

    // Note: the Critical (≥2 matches) branch requires a name within Levenshtein
    // distance 2 of two or more entries in POPULAR_SKILL_NAMES. The list entries
    // are all distance ≥ 7 apart (verified by DP), so no real name can reach two
    // simultaneously. The branch is correct but unreachable with the current list.

    #[test]
    fn exact_name_produces_no_finding() {
        let mut findings = Vec::new();
        check_typosquat("code-review", &mut findings);
        assert!(
            findings.is_empty(),
            "exact match must not trigger typosquat"
        );
    }

    #[test]
    fn unknown_name_skipped() {
        let mut findings = Vec::new();
        check_typosquat("unknown", &mut findings);
        assert!(findings.is_empty());
    }

    #[test]
    fn empty_name_skipped() {
        let mut findings = Vec::new();
        check_typosquat("", &mut findings);
        assert!(findings.is_empty());
    }

    #[test]
    fn clearly_different_name_produces_no_finding() {
        let mut findings = Vec::new();
        check_typosquat("my-unique-skill-xyz", &mut findings);
        assert!(findings.is_empty());
    }
}
