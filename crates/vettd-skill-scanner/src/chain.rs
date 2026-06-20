//! Chain detection — synthesises multi-finding attack chains from co-occurring signals.
//!
//! This module is a **stub**. The function signatures and doc comments describe
//! the full intended behaviour so the implementation can be filled in without
//! a refactor.

use std::collections::HashMap;

use crate::finding::Finding;

/// Detect exfiltration and malicious-activity chains in `findings`.
///
/// This is a **stub** — the body is a no-op. The real implementation should
/// mirror the two chain-detection passes in the vettd web scanner
/// (`skill-analyzer.ts`):
///
/// ## Pass 1 — credential exfiltration (`detectExfiltrationChains`)
///
/// 1. Group security findings whose label matches a credential-source fragment
///    (e.g. `"credential file access"`, `"hardcoded secret"`, `"API key"`) by the
///    file path parsed from `finding.detail` via the regex `Detected in ([^:]+):`.
/// 2. For each such file, scan the raw content (from `files`) for a network-sink
///    pattern (fetch POST, curl, socket send, etc.).
/// 3. If a sink is found: assign a shared `chain_id` (`"cred-exfil-{n}"`), set
///    `intent = Malicious`, and escalate `severity` to `Critical` on all source
///    findings. Also tag any existing network-related findings in the same file
///    with the same `chain_id`.
/// 4. Emit a new synthetic `Finding` (category `Security`, rule `VTD-0089`
///    `CREDENTIAL_EXFILTRATION_CHAIN`, severity `Critical`, intent `Malicious`)
///    summarising the chain.
///
/// ## Pass 2 — malicious activity (`detectMaliciousActivityChains`)
///
/// 1. Group findings whose label or category signals malicious intent by file.
/// 2. For each file with ≥ 2 distinct malicious-signal labels, assign a shared
///    `chain_id` (`"malicious-activity-{n}"`), escalate severity to `Critical`,
///    set `intent = Malicious`.
/// 3. Emit a new synthetic `Finding` (rule `VTD-0090`
///    `MALICIOUS_ACTIVITY_CHAIN`, severity `Critical`, intent `Malicious`).
///
/// ## Sequencing constraint
///
/// `detect_chains` mutates `severity` on existing findings. The caller
/// (`engine::scan_skill`) must invoke this **after** all other scan passes and
/// **before** returning `SkillScanResult`. Grade computation by the caller must
/// happen after receiving the result for the same reason.
// The real implementation pushes synthetic chain findings into the Vec, so
// &mut Vec<Finding> is correct here even though the stub body is a no-op.
#[allow(clippy::ptr_arg)]
pub(crate) fn detect_chains(_findings: &mut Vec<Finding>, _files: &HashMap<String, String>) {
    // TODO: implement exfiltration chain detection (VTD-0089)
    // TODO: implement malicious activity chain detection (VTD-0090)
}
