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
