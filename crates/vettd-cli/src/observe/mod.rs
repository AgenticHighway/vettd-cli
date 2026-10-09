//! Passive observation of local AI-harness session logs (`vettd observe`).
//!
//! Opt-in, read-only, and privacy-preserving: session lines are projected to
//! hashes and counts on this machine, and the resulting envelope is checked
//! against the repo-root `telemetry-field-gate.json` egress allowlist before
//! anything is written or sent. See `docs/observe.md`.
//!
//! Ported from the Python prototype answered on vettd#828; the prototype was the
//! reference semantics and the Rust here is the product.
//!
//! # Where the prototype went
//!
//! Doc comments throughout this module cite paths under `spikes/828-passive-observer/prototype/`.
//! **That directory no longer exists**: it was deleted once the port was complete and its
//! 183-test suite had been run green against this code for the last time. The citations are kept
//! because they say precisely which Python function each Rust one came from, which is the useful
//! part; retrieve any of them from git rather than from the working tree:
//!
//! ```text
//! git log --diff-filter=D --oneline -- spikes/828-passive-observer   # find the deleting commit
//! git show <that commit>^:spikes/828-passive-observer/prototype/attribute.py
//! ```
//!
//! The spike's reasoning and the scope note survive as `docs/passive-observer-decision-828.md`
//! and `docs/passive-observer-scope-965.md`. The shipped behaviour is `docs/observe.md`, which is
//! the document to trust where it and the prototype disagree.

pub(crate) mod args;
pub(crate) mod attribute;
pub(crate) mod canonical;
pub(crate) mod claude_code;
pub(crate) mod disclosure;
pub(crate) mod envelope;
pub(crate) mod extract;
pub(crate) mod gate;
// A check on the copy rather than shipped logic: it walks `render::COPY` and the disclosure text
// in tests so a reviewed phrase cannot quietly become an unreviewed claim.
#[cfg(test)]
pub(crate) mod lint_copy;
pub(crate) mod pipeline;
pub(crate) mod rank;
pub(crate) mod render;
pub(crate) mod source;
pub(crate) mod store;
pub(crate) mod subcommands;
pub(crate) mod submit;
pub(crate) mod taskcat;
pub(crate) mod types;

pub(crate) use args::{ObserveArgs, ObserveSubcommand};

/// Entry point for `vettd observe`, returning the process exit code.
///
/// The subcommands short-circuit before any observation runs: `enable` and `status` inspect
/// configuration, and `check` audits a payload that already exists. Only the bare command reads
/// session logs, and it announces that first — see [`pipeline::run_observe`].
pub(crate) fn run(args: &ObserveArgs, action: Option<&ObserveSubcommand>, json: bool) -> i32 {
    match action {
        Some(ObserveSubcommand::Enable) => subcommands::enable(),
        // `--json` is global, so honour it as well as the subcommand's own flag.
        Some(ObserveSubcommand::Status { json: sub }) => subcommands::status(*sub || json),
        Some(ObserveSubcommand::Check { payload, dynamic }) => {
            subcommands::check(payload, dynamic.as_deref())
        }
        None => pipeline::run_observe(args, json),
    }
}
