//! The `vettd observe` command-line surface.
//!
//! Flags only; every default that depends on the environment (the harness home, the output path,
//! the clock) is resolved by [`crate::observe::pipeline`] so a test can drive the whole command
//! without a real `$HOME`.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// The harnesses this collector can read. One today, by ruling 2 of the port plan.
pub(crate) const SUPPORTED_HARNESSES: [&str; 1] = ["claude_code"];

/// Written when `--out` is given with no value.
///
/// Deliberately matched by the repo's own `vettd-*.json` `.gitignore` entry, so a user who runs
/// this inside a checkout cannot accidentally commit a payload. That looks arbitrary otherwise.
pub(crate) const DEFAULT_OUT_FILE: &str = "vettd-observations.json";

/// Options for `vettd observe`.
#[derive(Args, Debug, Clone)]
pub(crate) struct ObserveArgs {
    /// Harness to read (only `claude_code` is supported today)
    #[arg(long, default_value = "claude_code", value_parser = SUPPORTED_HARNESSES)]
    pub harness: String,

    /// Harness home to read (default: ~/.claude)
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// The task this evidence is for; omitted means the pooled, unspecified view
    #[arg(long)]
    pub task: Option<String>,

    /// How many days back to consider a session file
    #[arg(long, default_value_t = 30)]
    pub window_days: u32,

    /// Narrow the report to one model id
    #[arg(long)]
    pub model: Option<String>,

    /// Build and check the payload, write it, and send nothing
    #[arg(long)]
    pub dry_run: bool,

    /// Write the payload; bare `--out` uses vettd-observations.json
    #[arg(long, num_args = 0..=1, default_missing_value = DEFAULT_OUT_FILE)]
    pub out: Option<PathBuf>,

    /// Replace asset names in the report with their type and hash prefix
    #[arg(long)]
    pub scrub: bool,

    /// Names that may be shown even when scrubbing, one per line
    #[arg(long)]
    pub public_names: Option<PathBuf>,

    /// Price table for the display-time cost lines (default: the compiled-in dated table)
    #[arg(long)]
    pub prices: Option<PathBuf>,

    /// Submit the payload; bare `--submit` uses the configured endpoint
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub submit: Option<String>,

    /// API key for submission (default: the configured credential)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Permit submission to a non-loopback plain-HTTP endpoint
    #[arg(long)]
    pub allow_public_endpoint: bool,

    /// Send records the ledger already recorded as accepted
    #[arg(long)]
    pub resend: bool,

    /// Test hook: read the HMAC secret from this file instead of ~/.vettd/observer_secret.
    /// Hidden because a secret supplied on the command line is not the product's consent model.
    #[arg(long, hide = true)]
    pub secret_file: Option<PathBuf>,

    /// Test hook: pin the clock, so discovery windows and truncation are reproducible.
    #[arg(long, hide = true)]
    pub now_ms: Option<i64>,

    /// Test hook: pin the emission day, so a golden payload is byte-reproducible.
    #[arg(long, hide = true)]
    pub today: Option<String>,
}

/// `vettd observe enable | status | check`.
#[derive(Subcommand, Debug, Clone)]
pub(crate) enum ObserveSubcommand {
    /// Record the opt-in in ~/.vettd/.vettd.toml
    Enable,

    /// Report whether observation is configured, and where its state lives
    Status {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },

    /// Check a written payload against the telemetry field gate
    Check {
        /// The payload to check
        #[arg(required = true)]
        payload: PathBuf,

        /// Local-only dynamic forbid sets, as the emitter would supply them
        #[arg(long)]
        dynamic: Option<PathBuf>,
    },
}

impl ObserveArgs {
    /// Where the payload should be written, or `None` when nothing should be.
    ///
    /// `--dry-run` implies the default path: a dry run that wrote nothing would give the user
    /// nothing to inspect, which is the entire point of the flag.
    pub(crate) fn out_path(&self) -> Option<PathBuf> {
        match (&self.out, self.dry_run) {
            (Some(path), _) => Some(path.clone()),
            (None, true) => Some(PathBuf::from(DEFAULT_OUT_FILE)),
            (None, false) => None,
        }
    }

    /// The submission endpoint override, if `--submit URL` was given a value.
    ///
    /// `--submit` with no value is `Some("")` from clap, meaning "submit to the configured
    /// endpoint"; that is distinct from the flag being absent, which means do not submit at all.
    pub(crate) fn submit_endpoint(&self) -> Option<&str> {
        self.submit.as_deref().filter(|url| !url.is_empty())
    }

    /// Whether submission was requested at all.
    pub(crate) fn wants_submit(&self) -> bool {
        self.submit.is_some()
    }
}

#[cfg(test)]
#[path = "args_tests.rs"]
mod tests;
