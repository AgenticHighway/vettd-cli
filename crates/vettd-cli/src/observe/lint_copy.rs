//! Causal-language lint for the observer's user-facing copy.
//!
//! Port of `spikes/828-passive-observer/prototype/lint_copy.py`. Test-only: it is a check on the
//! copy, not shipped logic, and it runs over [`crate::observe::render::COPY`] and the disclosure
//! text so a reviewed phrase cannot quietly become an unreviewed claim.
//!
//! Everything the observer reports is observational. It records that an asset was loaded or invoked
//! in runs with certain outcomes; it cannot establish that the asset *caused* them. This lint
//! rejects the phrases that turn the first into the second — causes, improves, saves, proves,
//! better/faster than, guarantees, a currency amount, a bare "reliable" — and requires any line
//! naming a rate to carry the hedge that says where the number came from.

use regex::Regex;
use std::sync::LazyLock;

/// `(rule id, pattern)`, all matched case-insensitively.
///
/// Two of the Python's patterns use lookaround, which the `regex` crate cannot compile, so they are
/// hand-written in [`bare_reliable`] and [`names_a_rate`] instead. The rest compile verbatim.
const FORBIDDEN: [(&str, &str); 12] = [
    ("causes", r"(?i)\bcauses?\b"),
    ("because_of", r"(?i)\bbecause of\b"),
    ("improves", r"(?i)\bimproves?\b"),
    ("makes_you", r"(?i)\bmakes? (?:you|your|it)\b"),
    ("faster_than", r"(?i)\bfaster than\b"),
    ("better_than", r"(?i)\bbetter than\b"),
    ("worse_than", r"(?i)\bworse than\b"),
    ("percent_better_worse", r"(?i)% ?(?:better|worse)\b"),
    ("saves", r"(?i)\bsaves?\b"),
    ("proves", r"(?i)\bproves?\b"),
    ("guarantee", r"(?i)\bguarantee"),
    ("dollar_amount", r"\$\d"),
];

pub(crate) const RATE_RULE: &str = "rate_without_hedge";
pub(crate) const RELIABLE_RULE: &str = "bare_reliable";

static COMPILED: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    FORBIDDEN
        .iter()
        .map(|(rule, pattern)| {
            (
                *rule,
                Regex::new(pattern).expect("every forbidden pattern compiles"),
            )
        })
        .collect()
});

/// `\b(?:un)?reliable\b` not preceded by `"observed "`.
///
/// The Python is `(?<!observed )\b(?:un)?reliable\b`. The `regex` crate has no lookbehind, so the
/// word is found and the preceding slice inspected. "reliable" is a property claim about an asset;
/// "observed reliable" is a statement about what the logs showed, which is all this data supports.
fn bare_reliable(line: &str) -> Option<&str> {
    const PREFIX: &str = "observed ";
    let lowered = line.to_lowercase();
    let mut from = 0;
    while let Some(found) = lowered[from..].find("reliable") {
        let start = from + found;
        let end = start + "reliable".len();
        // `\b(?:un)?reliable\b`: the match starts at "un" when it is there, so a preceding "un"
        // is part of the word rather than a boundary violation.
        let (word_start, matched) = if lowered[..start].ends_with("un") {
            (start - 2, &line[start - 2..end])
        } else {
            (start, &line[start..end])
        };
        let boundary_before = word_start == 0
            || !lowered[..word_start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let boundary_after = !lowered[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let hedged = lowered[..word_start].ends_with(PREFIX);
        if boundary_before && boundary_after && !hedged {
            return Some(matched);
        }
        from = end;
    }
    None
}

/// `\brates?\b` not followed by `[ _-]limit`.
///
/// The Python is `\brates?\b(?![ _-]limit)`; the `regex` crate has no lookahead. "rate limit" is a
/// policy, not a statistic, so it is exempt.
fn names_a_rate(line: &str) -> bool {
    let lowered = line.to_lowercase();
    let mut from = 0;
    while let Some(found) = lowered[from..].find("rate") {
        let start = from + found;
        let mut end = start + "rate".len();
        if lowered[end..].starts_with('s') {
            end += 1;
        }
        let boundary_before = start == 0
            || !lowered[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let rest = &lowered[end..];
        let boundary_after = !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let exempt = [" limit", "_limit", "-limit"]
            .iter()
            .any(|suffix| rest.starts_with(suffix));
        if boundary_before && boundary_after && !exempt {
            return true;
        }
        from = end;
    }
    false
}

/// A line naming a rate must say where the number came from.
fn has_hedge(line: &str) -> bool {
    static HEDGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)observed|in \d+ calls").expect("hedge pattern compiles"));
    HEDGE.is_match(line)
}

/// One finding per `(line, rule)`, formatted `"<lineno>: <rule>: <detail>"`.
pub(crate) fn lint(text: &str) -> Vec<String> {
    let mut findings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let lineno = index + 1;
        for (rule, pattern) in COMPILED.iter() {
            if let Some(found) = pattern.find(line) {
                findings.push(format!(
                    "{lineno}: {rule}: forbidden phrase {:?}",
                    found.as_str()
                ));
            }
        }
        if let Some(found) = bare_reliable(line) {
            findings.push(format!(
                "{lineno}: {RELIABLE_RULE}: forbidden phrase {found:?}"
            ));
        }
        if names_a_rate(line) && !has_hedge(line) {
            findings.push(format!(
                "{lineno}: {RATE_RULE}: a line naming a rate must say 'observed' or 'in N calls'"
            ));
        }
    }
    findings
}

#[cfg(test)]
#[path = "lint_copy_tests.rs"]
mod tests;
