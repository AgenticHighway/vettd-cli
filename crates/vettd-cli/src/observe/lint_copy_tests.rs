//! Tests for [`super`], ported from `spikes/828-passive-observer/prototype/tests/test_lint.py`.
//!
//! The lint exists because everything this collector reports is correlational. It records that an
//! asset was loaded or invoked in runs with certain outcomes; it cannot establish that the asset
//! caused them. These tests prove the lint catches the phrases that erase that distinction — and,
//! just as importantly, that it *can* fail, so a clean report is evidence rather than a tautology.

use super::*;

/// Every phrase the contract forbids has an offender and is flagged. If any pattern silently
/// stopped matching, the copy tests over `COPY` would keep passing while the guarantee was gone.
#[test]
fn every_forbidden_phrase_has_an_offender_and_is_flagged() {
    for line in [
        "this causes failures",
        "slow because of the server",
        "it improves throughput",
        "this makes you faster",
        "faster than the alternative",
        "better than the default",
        "worse than nothing",
        "30% better on average",
        "it saves time",
        "this proves the point",
        "we guarantee uptime",
        "costs $12 per run",
    ] {
        assert!(!lint(line).is_empty(), "not flagged: {line:?}");
    }
}

/// The phrases are matched case-insensitively: a capitalised claim is still a claim.
#[test]
fn phrases_are_case_insensitive() {
    assert!(!lint("This IMPROVES Your Results").is_empty());
    assert!(!lint("It CAUSES failures").is_empty());
}

/// "reliable" is a property claim about an asset; "observed reliable" is a statement about what the
/// logs showed, which is all this data supports. The bare form is flagged and the hedged one is not
/// — this is the rule whose Python form uses a lookbehind the regex crate cannot compile, so it is
/// hand-written and needs its own coverage.
#[test]
fn bare_reliable_is_flagged_but_observed_reliable_is_not() {
    assert!(!lint("a reliable server").is_empty());
    assert!(!lint("an unreliable server").is_empty());
    assert!(lint("observed reliable in this stratum").is_empty());
    assert!(lint("observed unreliable in this stratum").is_empty());
    // A word merely containing the letters is not the word.
    assert!(lint("the reliability engineering team").is_empty());
}

/// A line naming a rate must say where the number came from, either "observed" or "in N calls".
/// An unhedged rate is the shape a reader takes as a property of the asset.
#[test]
fn a_rate_line_needs_a_hedge() {
    assert!(!lint("failure rate 3%").is_empty());
    assert!(lint("observed non-success rate 3%").is_empty());
    assert!(lint("2 non-successes in 40 calls").is_empty());
    assert!(!lint("rates of 3% and 4%").is_empty());
}

/// "rate limit" is a policy, not a statistic, so it is exempt — this is the second hand-written
/// rule, replacing a Python lookahead. A bare "rate" on the same line still needs its hedge.
#[test]
fn rate_limit_is_exempt_but_a_bare_rate_is_not() {
    assert!(lint("the server returned a rate limit error").is_empty());
    assert!(lint("rate_limit reached").is_empty());
    assert!(lint("rate-limit reached").is_empty());
    assert!(!lint("the rate was high").is_empty());
}

/// "rate" inside another word is not a rate: `accurate` and `generated` must not trip the hedge
/// rule, or every honest sentence would need boilerplate.
#[test]
fn rate_inside_another_word_does_not_trigger_the_hedge_rule() {
    assert!(lint("an accurate count").is_empty());
    assert!(lint("the generated report").is_empty());
    assert!(lint("corporate policy").is_empty());
}

/// A compliant observational sentence yields nothing, so the lint is not simply always-failing.
#[test]
fn a_compliant_sentence_passes() {
    assert!(lint("10 non-successes in 135 calls (95% interval 4.1%-13.1%)").is_empty());
    assert!(
        lint("Every figure above is an observation from harness logs on this machine.").is_empty()
    );
}

/// Findings name the line number, so a multi-line report points at the offending line rather than
/// merely saying something is wrong somewhere.
#[test]
fn findings_name_the_line_number() {
    let findings = lint("clean line\nthis causes failures\nanother clean line");
    assert_eq!(findings.len(), 1);
    assert!(findings[0].starts_with("2: causes:"), "{:?}", findings[0]);
}
