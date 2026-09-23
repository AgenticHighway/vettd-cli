//! Tests for [`super`]. Parsing is exercised through a throwaway `clap` command so the assertions
//! are about the real derive output, not about a hand-built struct.

use clap::{CommandFactory, Parser};

use super::*;

/// A minimal parser wrapping the real args, so `--` handling and defaults are clap's own.
#[derive(Parser, Debug)]
#[command(name = "observe-test")]
struct Harness {
    #[command(flatten)]
    args: ObserveArgs,
    #[command(subcommand)]
    action: Option<ObserveSubcommand>,
}

fn parse(argv: &[&str]) -> ObserveArgs {
    let mut full = vec!["observe-test"];
    full.extend_from_slice(argv);
    Harness::try_parse_from(full)
        .unwrap_or_else(|e| panic!("expected {argv:?} to parse, got:\n{e}"))
        .args
}

/// Invariant: `--out` with no value writes the default filename, and that filename is one the
/// repo's `.gitignore` already covers. A payload is derived from a user's session logs; the default
/// must not be a path they can commit by accident.
/// Cannot prove the .gitignore entry still exists — the integration tests read the real file.
#[test]
fn out_defaults_to_the_gitignored_filename_and_accepts_an_explicit_path() {
    assert_eq!(
        parse(&["--out"]).out_path(),
        Some(PathBuf::from("vettd-observations.json"))
    );
    assert_eq!(
        parse(&["--out", "elsewhere.json"]).out_path(),
        Some(PathBuf::from("elsewhere.json"))
    );
    assert_eq!(parse(&[]).out_path(), None, "no --out writes nothing");
}

/// Invariant: `--dry-run` implies an output path. A dry run that wrote nothing would leave the user
/// with nothing to inspect, which is the whole reason the flag exists.
#[test]
fn dry_run_implies_the_default_out_path() {
    assert_eq!(
        parse(&["--dry-run"]).out_path(),
        Some(PathBuf::from("vettd-observations.json"))
    );
    assert_eq!(
        parse(&["--dry-run", "--out", "chosen.json"]).out_path(),
        Some(PathBuf::from("chosen.json")),
        "an explicit path still wins"
    );
}

/// Invariant: `--submit` distinguishes three states — absent (do not submit), present with no value
/// (submit to the configured endpoint), and present with a URL (submit there). Collapsing the first
/// two would make a plain run start sending.
#[test]
fn submit_distinguishes_absent_from_bare_from_explicit() {
    let absent = parse(&[]);
    assert!(!absent.wants_submit());
    assert_eq!(absent.submit_endpoint(), None);

    let bare = parse(&["--submit"]);
    assert!(bare.wants_submit(), "bare --submit still submits");
    assert_eq!(
        bare.submit_endpoint(),
        None,
        "with no override, the configured endpoint is used"
    );

    let explicit = parse(&[
        "--submit",
        "https://app.invented.test/api/observations/ingest",
    ]);
    assert!(explicit.wants_submit());
    assert_eq!(
        explicit.submit_endpoint(),
        Some("https://app.invented.test/api/observations/ingest")
    );
}

/// Invariant: the discovery window defaults to 30 days rather than to everything. An unbounded
/// default would read every transcript a machine has ever produced on the first run.
#[test]
fn window_days_defaults_to_thirty() {
    assert_eq!(parse(&[]).window_days, 30);
    assert_eq!(parse(&["--window-days", "3650"]).window_days, 3650);
}

/// Invariant: an unsupported harness is rejected at parse time, and the message names what is
/// accepted. Accepting it and finding nothing would look like "you have no sessions".
#[test]
fn an_unsupported_harness_is_rejected_and_names_the_accepted_value() {
    assert_eq!(parse(&[]).harness, "claude_code");
    let err = Harness::try_parse_from(["observe-test", "--harness", "codex"])
        .expect_err("codex is not supported in v1");
    let text = err.to_string();
    assert!(
        text.contains("claude_code"),
        "the error must name the accepted value: {text}"
    );
}

/// Invariant: the three test hooks stay out of `--help`. They pin the clock, the day and the
/// secret; a user who found them in the help text could produce a payload whose `run_id` does not
/// match their device, which is not a supported thing to do.
#[test]
fn the_test_hooks_are_hidden_from_help() {
    let help = Harness::command().render_long_help().to_string();
    for hidden in ["--secret-file", "--now-ms", "--today"] {
        assert!(!help.contains(hidden), "{hidden} must be hidden");
    }
    // The real flags are documented, so this is not passing because help is empty.
    for shown in ["--harness", "--root", "--task", "--dry-run", "--submit"] {
        assert!(help.contains(shown), "{shown} must be documented");
    }
    // ...and they still parse when given.
    let hooked = parse(&["--now-ms", "1800000000000", "--today", "2027-01-15"]);
    assert_eq!(hooked.now_ms, Some(1_800_000_000_000));
    assert_eq!(hooked.today.as_deref(), Some("2027-01-15"));
}

/// Invariant: `--task` is optional. The report has a defined pooled view for "no stated task", so
/// requiring one would force the user to invent a task to see anything.
#[test]
fn task_is_optional() {
    assert_eq!(parse(&[]).task, None);
    assert_eq!(
        parse(&["--task", "fix the parser"]).task.as_deref(),
        Some("fix the parser")
    );
    assert_eq!(parse(&["--task", ""]).task.as_deref(), Some(""));
}

/// Invariant: the subcommands parse as named, and `check` requires its payload argument — a `check`
/// with no payload that silently succeeded would report a clean gate for nothing at all.
#[test]
fn subcommands_parse_and_check_requires_a_payload() {
    let parsed = Harness::try_parse_from(["observe-test", "enable"]).expect("enable parses");
    assert!(matches!(parsed.action, Some(ObserveSubcommand::Enable)));

    let parsed = Harness::try_parse_from(["observe-test", "status", "--json"]).expect("status");
    assert!(matches!(
        parsed.action,
        Some(ObserveSubcommand::Status { json: true })
    ));

    let parsed = Harness::try_parse_from([
        "observe-test",
        "check",
        "payload.json",
        "--dynamic",
        "d.json",
    ])
    .expect("check parses");
    match parsed.action {
        Some(ObserveSubcommand::Check { payload, dynamic }) => {
            assert_eq!(payload, PathBuf::from("payload.json"));
            assert_eq!(dynamic, Some(PathBuf::from("d.json")));
        }
        other => panic!("expected a check subcommand, got {other:?}"),
    }

    Harness::try_parse_from(["observe-test", "check"]).expect_err("check needs a payload");
}
