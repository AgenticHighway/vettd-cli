//! Stress-corpus guarantees. Slices 1-4 test layout and low-coverage states
//! against this corpus, so its shape (scale, long strings, duplicate names,
//! honest unknowns) must hold, or those tests prove less than they claim.

use crate::fixture::{self, Request, Session, Use, COMPONENTS, PLACEMENTS};
use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

fn requests(all: &[Session]) -> impl Iterator<Item = &Request> {
    all.iter().flat_map(|s| s.requests.iter())
}

fn uses(all: &[Session]) -> impl Iterator<Item = &Use> {
    all.iter().flat_map(|s| s.observed.iter())
}

#[test]
fn corpus_has_about_two_thousand_sessions() {
    // Layout and timing results only mean something near real session volume.
    let n = fixture::stress_sessions().len();
    assert!((1_900..=2_100).contains(&n), "sessions = {n}");
}

#[test]
fn corpus_uses_six_workspaces() {
    // The list layout is tuned for a fixed number of workspace rows.
    let all = fixture::stress_sessions();
    let set: BTreeSet<&str> = all.iter().map(|s| s.workspace).collect();
    assert_eq!(set.len(), 6, "workspaces = {set:?}");
}

#[test]
fn corpus_covers_exactly_the_stress_days() {
    // Day grouping must see every day of the window and nothing outside it.
    let all = fixture::stress_sessions();
    let days: BTreeSet<u32> = all.iter().map(|s| s.day).collect();
    let expected: BTreeSet<u32> = (12..=18).collect();
    assert_eq!(days, expected, "days = {days:?}");
}

#[test]
fn corpus_has_a_workspace_longer_than_80_chars() {
    // Long workspace paths must be truncated or wrapped without breaking rows.
    let all = fixture::stress_sessions();
    let longest = all
        .iter()
        .map(|s| s.workspace.chars().count())
        .max()
        .unwrap_or(0);
    assert!(longest > 80, "longest workspace = {longest} chars");
}

#[test]
fn corpus_has_a_model_longer_than_40_chars() {
    // Long model IDs must not push the cost columns off the row.
    let all = fixture::stress_sessions();
    let longest = requests(&all)
        .map(|r| r.model.chars().count())
        .max()
        .unwrap_or(0);
    assert!(longest > 40, "longest model = {longest} chars");
}

#[test]
fn corpus_repeats_a_session_name_at_least_ten_times() {
    // Duplicate names force the list to disambiguate by day and workspace.
    let all = fixture::stress_sessions();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in &all {
        *counts.entry(s.name).or_default() += 1;
    }
    let most = counts.values().copied().max().unwrap_or(0);
    assert!(most >= 10, "most repeated name appears {most} times");
}

#[test]
fn skill_copy_coverage_is_low_but_nonzero() {
    // Low coverage is the realistic case: most skill uses have no recorded copy.
    let all = fixture::stress_sessions();
    let skill: Vec<&Use> = uses(&all)
        .filter(|u| COMPONENTS[u.component].kind == "skill")
        .collect();
    let total = skill.len();
    let with = skill.iter().filter(|u| u.artifact.is_some()).count();
    assert!(
        with > 0 && with * 100 <= total * 15,
        "skill uses with artifact = {with} of {total}"
    );
}

#[test]
fn version_label_never_appears_without_an_artifact() {
    // A version label alone must not imply scan evidence, so it needs an artifact too.
    let all = fixture::stress_sessions();
    for u in uses(&all) {
        if u.version.is_some() {
            assert!(
                u.artifact.is_some(),
                "component {} has version {:?} but no artifact",
                u.component,
                u.version
            );
        }
    }
}

#[test]
fn every_recorded_artifact_matches_a_placement() {
    // Artifact identity is the join key; a dangling artifact would resolve to nothing.
    let all = fixture::stress_sessions();
    for u in uses(&all) {
        if let Some(artifact) = u.artifact {
            assert!(
                PLACEMENTS.iter().any(|p| {
                    p.component == u.component
                        && p.artifact == Some(artifact)
                        && p.version == u.version
                }),
                "component {} artifact {artifact} has no placement",
                u.component
            );
        }
    }
}

#[test]
fn only_skill_uses_record_an_artifact() {
    // Only skills have scanned copies in this corpus; a copy on a tool or hook would be a fake grade.
    let all = fixture::stress_sessions();
    for u in uses(&all) {
        if u.artifact.is_some() {
            assert_eq!(
                COMPONENTS[u.component].kind, "skill",
                "component {} records an artifact but is not a skill",
                u.component
            );
        }
    }
}

#[test]
fn corpus_has_unpriced_requests_but_no_zero_rates() {
    // Unpriced must show as unknown, not as a real $0.00 cost from an all-zero rate.
    let all = fixture::stress_sessions();
    assert!(
        requests(&all).any(|r| r.milli_usd_per_ktok.is_none()),
        "no unpriced request"
    );
    assert!(
        requests(&all).all(|r| r.milli_usd_per_ktok != Some([0; 4])),
        "a request has an all-zero rate"
    );
}

#[test]
fn corpus_has_an_incomplete_session() {
    // Incomplete sessions exercise the partial-data labels on day and session rows.
    let all = fixture::stress_sessions();
    assert!(all.iter().any(|s| s.incomplete), "no incomplete session");
}

#[test]
fn every_use_points_at_a_real_component() {
    // An out-of-range index would panic in the component views instead of failing here.
    let all = fixture::stress_sessions();
    for u in uses(&all) {
        assert!(
            u.component < COMPONENTS.len(),
            "component index {} >= {}",
            u.component,
            COMPONENTS.len()
        );
    }
}

#[test]
fn token_parts_are_bounded_and_sessions_have_one_to_four_requests() {
    // Bounded tokens keep cost math in range; 1-4 requests match the spike shape.
    let all = fixture::stress_sessions();
    for s in &all {
        assert!(
            (1..=4).contains(&s.requests.len()),
            "session has {} requests",
            s.requests.len()
        );
        // Summing costs exercises the u32 arithmetic in cents(), so overflow fails in debug builds.
        let _ = s.cents();
    }
    for r in requests(&all) {
        for part in r.tokens {
            assert!(part <= 1_000_000, "token part = {part}");
        }
    }
}

#[test]
fn corpus_json_is_identical_across_calls() {
    // Determinism: a reported layout or timing problem must reproduce from the same data.
    let first = serde_json::to_string(&fixture::stress_sessions()).unwrap();
    let second = serde_json::to_string(&fixture::stress_sessions()).unwrap();
    assert!(first == second, "corpus JSON differs between two calls");
}

#[test]
fn corpus_fingerprint_is_pinned() {
    // Pins the exact corpus so a silent generator change is caught. Update these
    // values deliberately, and only when the seed or generator changes on purpose.
    let all = fixture::stress_sessions();
    let count = all.len();
    let cents: u64 = all.iter().map(|s| u64::from(s.cents())).sum();
    assert_eq!(
        (count, cents),
        (1_991, 1_368_564),
        "fingerprint (count, cents)"
    );
}

#[test]
fn corpus_builds_within_one_second() {
    // Slow generation would make every stress test pay the cost before it asserts anything.
    let start = Instant::now();
    let n = fixture::stress_sessions().len();
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_secs_f64() < 1.0,
        "built {n} sessions in {elapsed:?}"
    );
}
