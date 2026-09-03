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
