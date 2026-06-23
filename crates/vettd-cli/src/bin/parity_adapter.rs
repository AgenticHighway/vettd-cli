//! Parity adapter — vettd-cli side of the cross-scanner parity test harness.
//!
//! Reads a JSON file-map envelope from stdin, calls `scan_skill`, and writes
//! the findings as JSON to stdout.
//!
//! # Protocol
//!
//! **stdin (JSON)**
//! ```json
//! {
//!   "textFiles": { "<rel-path>": "<utf8-content>", ... },
//!   "allPaths":  ["<rel-path>", ...]
//! }
//! ```
//!
//! **stdout (JSON)**
//! ```json
//! { "findings": [ ... ] }
//! ```
//!
//! **stderr**: human-oriented diagnostics only.
//! **exit code**: 0 on success, non-zero on input/IO error.
//!
//! This binary lives in `vettd-cli` (which already depends on the scanner crate
//! and serde_json) rather than in `vettd-skill-scanner` itself, preserving
//! that crate's zero-I/O guarantee.

use std::collections::HashMap;
use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use vettd_skill_scanner::scan_skill;
use vettd_skill_scanner::Finding;

// ── Input / output envelopes ─────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InputEnvelope {
    text_files: HashMap<String, String>,
    all_paths: Vec<String>,
}

#[derive(Serialize)]
struct OutputEnvelope<'a> {
    findings: &'a [Finding],
}

// ── Entry point ──────────────────────────────────────────────────────────────

fn main() {
    let mut raw = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut raw) {
        eprintln!("parity-adapter: failed to read stdin: {e}");
        std::process::exit(1);
    }

    let envelope: InputEnvelope = match serde_json::from_str(&raw) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("parity-adapter: failed to parse input JSON: {e}");
            std::process::exit(1);
        }
    };

    let result = scan_skill(&envelope.text_files, &envelope.all_paths);

    let output = OutputEnvelope {
        findings: &result.findings,
    };

    let json = match serde_json::to_string(&output) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("parity-adapter: failed to serialize findings: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = writeln!(io::stdout(), "{json}") {
        eprintln!("parity-adapter: failed to write stdout: {e}");
        std::process::exit(1);
    }
}
