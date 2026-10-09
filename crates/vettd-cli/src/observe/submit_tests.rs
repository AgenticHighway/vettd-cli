//! Tests for [`super`]. The wire behaviour (headers, retries, status handling) is exercised
//! end-to-end against a real binary and a real mock server in `tests/observe_integration.rs`;
//! what is tested here is the response parsing, which has no HTTP in it and where the failure
//! mode — misclassifying a run — is silent data loss rather than an error.

use super::*;
use serde_json::json;

/// Invariant: each status lands in its own bucket, and only the three the server confirms it
/// holds count as persisted.
///
/// `deadline_exceeded` means the server ran out of its own time budget partway through the
/// envelope: that run is NOT stored. Ledgering it would advance a cursor past a run nobody has,
/// and the record would never be sent again — the exact silent loss this split exists to prevent.
#[test]
fn each_status_lands_in_its_own_bucket_and_only_held_runs_are_persisted() {
    let outcome = parse_outcome(&json!({
        "results": [
            {"run_id": "a", "status": "accepted"},
            {"run_id": "b", "status": "duplicate"},
            {"run_id": "c", "status": "replaced"},
            {"run_id": "d", "status": "deadline_exceeded"},
        ]
    }));

    assert_eq!(outcome.accepted, ["a".to_string()].into());
    assert_eq!(outcome.duplicate, ["b".to_string()].into());
    assert_eq!(outcome.replaced, ["c".to_string()].into());
    assert_eq!(outcome.deadline_exceeded, ["d".to_string()].into());

    let persisted: Vec<&str> = outcome.persisted().map(String::as_str).collect();
    assert_eq!(
        persisted,
        ["a", "c", "b"],
        "accepted, replaced and duplicate"
    );
    assert!(
        !persisted.contains(&"d"),
        "a run the server could not finish must never be ledgered"
    );
}

/// Invariant: a status the client does not recognise is dropped, not guessed at.
///
/// A newer server that grows a fifth status must not have its runs quietly treated as stored by
/// an older CLI. Dropping them means the run is re-sent next time, which is recoverable; guessing
/// means it is lost, which is not.
#[test]
fn an_unknown_status_is_dropped_rather_than_assumed_accepted() {
    let outcome = parse_outcome(&json!({
        "results": [
            {"run_id": "known", "status": "accepted"},
            {"run_id": "future", "status": "quarantined"},
            {"run_id": "typed_wrong", "status": 200},
            {"status": "accepted"},
            {"run_id": "ok_after_junk", "status": "replaced"},
        ]
    }));

    assert_eq!(outcome.persisted().count(), 2, "only the two we understand");
    assert!(outcome.accepted.contains("known"));
    assert!(outcome.replaced.contains("ok_after_junk"));
    for bucket in [
        &outcome.accepted,
        &outcome.duplicate,
        &outcome.replaced,
        &outcome.deadline_exceeded,
    ] {
        assert!(
            !bucket.contains("future"),
            "unknown status must not classify"
        );
        assert!(
            !bucket.contains("typed_wrong"),
            "a non-string status is junk"
        );
    }
}

/// Invariant: a 2xx whose body is not the expected shape yields an EMPTY outcome, so nothing is
/// ledgered and the runs are re-sent.
///
/// This is the fail-safe direction. A proxy that returns 200 with an HTML error page, or a server
/// that changes its response shape, must cost one redundant resend — never a lost record.
#[test]
fn a_body_without_results_persists_nothing() {
    for body in [
        json!({}),
        json!({"results": null}),
        json!({"results": "accepted"}),
        json!({"ok": true, "count": 3}),
        json!([{"run_id": "a", "status": "accepted"}]),
    ] {
        let outcome = parse_outcome(&body);
        assert_eq!(
            outcome,
            SubmitOutcome::default(),
            "nothing may be ledgered from {body}"
        );
        assert_eq!(outcome.persisted().count(), 0);
    }
}

/// Invariant: the summary line counts new, replaced and duplicate, and never mentions
/// `deadline_exceeded` as success.
///
/// The user reads this line to decide whether their data arrived. Counting a run the server
/// abandoned would tell them it did.
#[test]
fn the_summary_line_reports_the_three_success_kinds() {
    let outcome = parse_outcome(&json!({
        "results": [
            {"run_id": "a", "status": "accepted"},
            {"run_id": "b", "status": "accepted"},
            {"run_id": "c", "status": "replaced"},
            {"run_id": "d", "status": "duplicate"},
            {"run_id": "e", "status": "deadline_exceeded"},
        ]
    }));
    assert_eq!(
        outcome.summary(),
        "Observations accepted: 2 new, 1 replaced, 1 duplicate"
    );

    assert_eq!(
        SubmitOutcome::default().summary(),
        "Observations accepted: 0 new, 0 replaced, 0 duplicate"
    );
}

/// Invariant: a run id repeated across entries does not double-count.
///
/// The buckets are sets because a ledger row is keyed on the run id; two entries for one run is a
/// server-side oddity, not two runs, and reporting "2 new" for one record would be wrong.
#[test]
fn a_repeated_run_id_counts_once() {
    let outcome = parse_outcome(&json!({
        "results": [
            {"run_id": "a", "status": "accepted"},
            {"run_id": "a", "status": "accepted"},
        ]
    }));
    assert_eq!(outcome.accepted.len(), 1);
    assert_eq!(outcome.persisted().count(), 1);
}

/// Invariant: the retry policy is [`crate::submit`]'s, not a second copy of it.
///
/// Two schedules that could drift is the failure this shares one constant to avoid — a change to
/// scan submission's backoff that silently left observation submission hammering a struggling
/// server. Asserting the values here would just restate them; asserting they are the same symbol
/// is what actually holds.
#[test]
fn the_retry_policy_is_shared_with_scan_submission() {
    assert_eq!(BACKOFF_SECONDS, crate::submit::BACKOFF_SECONDS);
    assert_eq!(MAX_ATTEMPTS, crate::submit::MAX_ATTEMPTS);
    assert_eq!(BACKOFF_SECONDS.len(), MAX_ATTEMPTS, "one delay per attempt");
    for status in [429, 500, 502, 503, 504] {
        assert!(is_retryable(status), "{status} must be retried");
    }
    for status in [200, 400, 401, 404, 413, 422, 501] {
        assert!(!is_retryable(status), "{status} must not be retried");
    }
}
