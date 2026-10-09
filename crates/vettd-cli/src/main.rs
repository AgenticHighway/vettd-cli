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
mod directory_download;
mod discovery;
mod formatters;
mod freshness;
mod identity;
mod inventory;
mod inventory_client;
mod lite_mode;
mod models;
mod network;
mod network_evidence;
mod observe;
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
    // Manual `--version` handler.
    //
    // clap's `#[command(version = ...)]` only accepts literals, but we need to
    // show the pinned `vettd_skill_scanner::VERSION` (a `pub const`, not a
    // literal) in the long version. So we intercept `--version` / `-V` before
    // clap sees it, print the two-line version, and exit.
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && (args[1] == "--version" || args[1] == "-V") {
        println!("{}", cli::long_version_string());
        std::process::exit(0);
    }

    cli::run();
}
