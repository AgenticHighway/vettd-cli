//! Passive observation of local AI-harness session logs (`vettd observe`).
//!
//! Opt-in, read-only, and privacy-preserving: session lines are projected to
//! hashes and counts on this machine, and the resulting envelope is checked
//! against the repo-root `telemetry-field-gate.json` egress allowlist before
//! anything is written or sent. See `docs/observe.md`.
//!
//! Ported from the Python prototype answered on vettd#828; the prototype is the
//! reference semantics and the Rust here is the product. Submodules are declared
//! here as each phase of `docs/vettd-observe-port-plan.md` lands them.

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
pub(crate) mod rank;
pub(crate) mod render;
pub(crate) mod source;
pub(crate) mod taskcat;
pub(crate) mod types;
