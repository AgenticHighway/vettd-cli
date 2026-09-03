//! POST an observation envelope to `/api/observations/ingest`.
//!
//! The loop shape — backoff schedule, attempt count, retryable statuses, `Retry-After` on 429,
//! `http_status_as_error(false)` so 4xx/5xx bodies can be read — is [`crate::submit`]'s, reused
//! rather than reimplemented: two retry policies that could drift is exactly the kind of thing
//! nobody notices until one of them is wrong under load.
//!
//! What is different here is the response. Scan ingest answers "accepted or not"; observation
//! ingest answers per run, because a record is the *cumulative* state of one harness run and a
//! resend under the same `run_id` REPLACES the row rather than adding one. The caller needs to
//! know which runs the server actually holds before it may advance a cursor, so [`SubmitOutcome`]
//! carries the run ids by status and the pipeline writes ledger rows only for those.

use std::collections::BTreeSet;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::submit::{is_retryable, AuthConfig, BACKOFF_SECONDS, MAX_ATTEMPTS};

/// Whole-request ceiling. Without it a server that accepts the connection and then stalls holds
/// the CLI open indefinitely; the cloud route's own deadline is 90 s, so this is that plus slack.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// What the server did with each run in the envelope.
///
/// Four sets rather than a count, because the pipeline has to write a ledger row keyed on the
/// specific `run_id`. `deadline_exceeded` is the server telling us it ran out of its own time
/// budget partway through a large envelope: those runs are NOT held, so they must not be
/// ledgered, and the next run re-sends them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SubmitOutcome {
    pub accepted: BTreeSet<String>,
    pub duplicate: BTreeSet<String>,
    pub replaced: BTreeSet<String>,
    pub deadline_exceeded: BTreeSet<String>,
}

impl SubmitOutcome {
    /// Fold one request's result into the whole logical submission.
    pub(crate) fn extend(&mut self, other: Self) {
        self.accepted.extend(other.accepted);
        self.duplicate.extend(other.duplicate);
        self.replaced.extend(other.replaced);
        self.deadline_exceeded.extend(other.deadline_exceeded);
    }

    /// The runs the server confirmed it holds — the only ones that may be ledgered.
    pub(crate) fn persisted(&self) -> impl Iterator<Item = &String> {
        self.accepted
            .iter()
            .chain(self.replaced.iter())
            .chain(self.duplicate.iter())
    }

    /// The stderr line a user sees on success.
    pub(crate) fn summary(&self) -> String {
        format!(
            "Observations accepted: {} new, {} replaced, {} duplicate",
            self.accepted.len(),
            self.replaced.len(),
            self.duplicate.len()
        )
    }
}

/// POST `bytes` to `auth.endpoint`, retrying transient failures.
///
/// `bytes` is the canonical envelope, sent verbatim: the payload that was gate-checked is the
/// payload that goes on the wire, with no re-serialisation in between that could reintroduce a
/// field the gate refused.
pub(crate) fn submit_envelope(bytes: &[u8], auth: &AuthConfig) -> Result<SubmitOutcome, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .into();

    let mut last_err = String::new();

    for (attempt, &backoff) in BACKOFF_SECONDS.iter().enumerate().take(MAX_ATTEMPTS) {
        if attempt > 0 {
            eprintln!("  Attempt {}/{MAX_ATTEMPTS}...", attempt + 1);
        }

        let response = agent
            .post(&auth.endpoint)
            .header("Content-Type", "application/json")
            .header("Authorization", &format!("Bearer {}", auth.api_key))
            .header("User-Agent", &crate::updater::user_agent_string())
            .send(bytes);

        let mut response = match response {
            Ok(response) => response,
            Err(e) => {
                last_err = format!("Connection error: {e}");
                if attempt < MAX_ATTEMPTS - 1 {
                    eprintln!("  {last_err}, retrying in {backoff}s...");
                    thread::sleep(Duration::from_secs(backoff));
                    continue;
                }
                break;
            }
        };

        let status = response.status().as_u16();
        match status {
            200..=208 | 226 => {
                let body: Value = response.body_mut().read_json().map_err(|e| {
                    format!("Server returned {status} with an unreadable body: {e}")
                })?;
                return Ok(parse_outcome(&body));
            }
            400 => {
                let body = response.body_mut().read_to_string().unwrap_or_default();
                return Err(format!(
                    "Server rejected payload (400): {body}\n\
                     This is likely a vettd bug — the envelope doesn't match \
                     telemetry-envelope.schema.json."
                ));
            }
            401 => {
                return Err(
                    "Authentication failed (401). Run `vettd auth --key <your-key>` to configure credentials."
                        .into(),
                );
            }
            413 => {
                let size_kb = bytes.len() / 1024;
                return Err(format!(
                    "Payload too large (413): ~{size_kb} KB. Reduce --window-days."
                ));
            }
            s if is_retryable(s) => {
                let wait = if s == 429 {
                    retry_after_seconds(&response).unwrap_or(backoff)
                } else {
                    backoff
                };
                last_err = format!("Server returned {s}: {}", detail_of(&mut response));
                if attempt < MAX_ATTEMPTS - 1 {
                    eprintln!("  Server returned {s}, retrying in {wait}s...");
                    thread::sleep(Duration::from_secs(wait));
                    continue;
                }
            }
            _ => {
                return Err(format!(
                    "Server error ({status}): {}",
                    detail_of(&mut response)
                ));
            }
        }
    }

    Err(format!(
        "Submission failed after {MAX_ATTEMPTS} attempts: {last_err}"
    ))
}

/// `Retry-After` as whole seconds, when the server sent an integer form.
///
/// The HTTP-date form is deliberately not parsed: it needs a clock the client and server agree on,
/// and getting it wrong means either hammering the server or stalling for hours. Falling back to
/// our own backoff is the safer failure.
fn retry_after_seconds(response: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn detail_of(response: &mut ureq::http::Response<ureq::Body>) -> String {
    let body = response.body_mut().read_to_string().unwrap_or_default();
    if body.trim().is_empty() {
        "no details provided".to_string()
    } else {
        body
    }
}

/// Read `{"results":[{"run_id":"…","status":"accepted|duplicate|replaced|deadline_exceeded"}]}`.
///
/// Unknown statuses and malformed entries are dropped rather than guessed at. A run the client
/// cannot classify is a run it must not ledger, and silently treating it as accepted would lose
/// the record: the cursor would advance past a run the server never stored.
fn parse_outcome(body: &Value) -> SubmitOutcome {
    let mut outcome = SubmitOutcome::default();
    let Some(results) = body.get("results").and_then(Value::as_array) else {
        return outcome;
    };
    for entry in results {
        let Some(run_id) = entry.get("run_id").and_then(Value::as_str) else {
            continue;
        };
        let bucket = match entry.get("status").and_then(Value::as_str) {
            Some("accepted") => &mut outcome.accepted,
            Some("duplicate") => &mut outcome.duplicate,
            Some("replaced") => &mut outcome.replaced,
            Some("deadline_exceeded") => &mut outcome.deadline_exceeded,
            _ => continue,
        };
        bucket.insert(run_id.to_string());
    }
    outcome
}

#[cfg(test)]
#[path = "submit_tests.rs"]
mod tests;
