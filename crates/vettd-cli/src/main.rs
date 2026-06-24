// Many internal modules expose pub(crate) functions as a reusable API
// that isn't fully consumed by every code-path in the binary.
#![allow(dead_code)]

mod capabilities;
mod cli;
mod content_patterns;
mod contract;
mod contract_sync;
mod detectors;
mod directory;
mod discovery;
mod formatters;
mod identity;
mod lite_mode;
mod models;
mod network;
mod network_evidence;
mod output;
mod progress;
mod read_client;
mod risk_engine;
mod rule_engine;
mod rules;
mod scan;
mod scan_cache;
mod scan_refresh;
mod scoring;
mod semver;
mod source_analysis;
mod source_patterns;
mod submit;
mod updater;
mod verifier;
mod wizard;

fn main() {
    cli::run();
}
