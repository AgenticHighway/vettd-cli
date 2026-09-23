//! Tests for [`super`], ported from the rendering and copy halves of
//! `spikes/828-passive-observer/prototype/tests/test_rank.py`.
//!
//! Every asset, count, model and price below is invented. None of these can prove the copy reads
//! well; they prove it says only what the data supports, and that no string outside [`COPY`]
//! reaches the output — which is what makes linting `COPY` sufficient.

use std::fs;
use std::path::PathBuf;

use regex::Regex;
use serde_json::json;

use super::*;
use crate::observe::canonical::hex_sha256;
use crate::observe::lint_copy::lint;
use crate::observe::rank::{rank, tests::populated};

const HARNESS: &str = "claude_code";
const MODEL: &str = "claude-sonnet-5";

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/observe")
}

fn hex64(label: &str) -> String {
    hex_sha256(format!("fixture:{label}").as_bytes())
}

/// The invented display names the prototype's `names_for` builds: `{asset_type}:{label}`.
fn names() -> BTreeMap<String, String> {
    [
        ("zeta-invented", "mcp_server"),
        ("eta-invented", "mcp_server"),
        ("theta-invented", "mcp_server"),
        ("iota-invented", "mcp_server"),
        ("kappa-invented", "mcp_server"),
        ("lambda-invented", "rules_file"),
        ("mu-invented", "prompt"),
        ("nu-invented", "agent"),
    ]
    .into_iter()
    .map(|(label, kind)| (hex64(label), format!("{kind}:{label}")))
    .collect()
}

/// An invented price table, so the test does not depend on the shipped one's numbers.
fn prices() -> Value {
    json!({
        "as_of": "2031-01-01",
        "per_million_tokens": {
            MODEL: {"input": 1.0, "cache_creation": 1.0, "cache_read": 1.0, "output": 2.0},
            "other": Value::Null,
        },
    })
}

fn populated_result() -> RankResult {
    let env = populated();
    rank(&env, &names(), "fix the invented module", HARNESS, None)
}

fn out(scrub: bool, public: &[&str]) -> String {
    let public: BTreeSet<String> = public.iter().map(|s| (*s).to_string()).collect();
    render_with_prices(&populated_result(), scrub, &public, Some(&prices()))
}

/// The Rust renderer reproduces the prototype's committed report BYTE FOR BYTE from the same
/// envelope. This is the acceptance test of the phase: every template, every float format, every
/// section order and the EN DASH inside the interval agree with the reference. The golden was
/// generated with --scrub and no public names, so every display name is `type:asset_id[:12]` and
/// the text follows from the envelope alone.
///
/// The committed fixture is `render()`'s exact output: one trailing newline. It originally carried
/// two, because the prototype's CLI does `print(render(...))` and `render` already ends in a
/// newline, so the generation recipe's `tail -n +3` inherited `print`'s. The extra byte was
/// stripped rather than tolerated here — the golden should be the renderer's contract, and the
/// command that prints it must therefore use `print!`, not `println!`.
/// Cannot prove the prototype's wording was right — only that the port did not change it.
#[test]
fn golden_ranking_matches_prototype() {
    let envelope: Value = serde_json::from_slice(
        &fs::read(fixtures_dir().join("golden/envelope.json")).expect("golden envelope committed"),
    )
    .expect("golden envelope parses");
    let result = rank(
        &envelope,
        &BTreeMap::new(),
        "exercise passive observer resume",
        "claude_code",
        None,
    );
    let ours = render(&result, true, &BTreeSet::new());
    let golden =
        fs::read_to_string(fixtures_dir().join("golden/ranking.txt")).expect("golden committed");
    if ours != golden {
        // Report the first differing line AND the line counts: a pure length difference (a section
        // emitted or omitted) shows up as no differing line at all, which is the confusing case.
        let mismatch = ours
            .lines()
            .zip(golden.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b);
        if let Ok(dir) = std::env::var("OBSERVE_GOLDEN_DUMP") {
            let _ = fs::write(PathBuf::from(dir).join("ranking.ours.txt"), &ours);
        }
        panic!(
            "ranking mismatch: ours {} lines, golden {} lines\n  first diff at {:?}\n  ours:   {:?}\n  golden: {:?}",
            ours.lines().count(),
            golden.lines().count(),
            mismatch.map(|(i, _)| i + 1),
            mismatch.map(|(_, (a, _))| a),
            mismatch.map(|(_, (_, b))| b),
        );
    }
}

/// Every ranked row carries its tier, the state `observed`, and the count phrase with a 95%
/// interval — one row per ranked asset, in ranked order. The count phrase is what makes the figure
/// checkable: "10 non-successes in 135 calls" can be argued with, "unreliable" cannot.
/// Cannot prove the row is readable; only that the required elements are present.
#[test]
fn every_ranked_row_shows_tier_state_and_the_count_phrase() {
    let text = out(false, &[]);
    let result = populated_result();
    let row_re = Regex::new(
        r"(?m)^\s*\d+\. (\S+)  tier=(\w+) state=(\w+)  (\d+) non-successes in (\d+) calls \(95% interval [\d.]+%\u{2013}[\d.]+%\)",
    )
    .expect("row pattern compiles");
    let rows: Vec<_> = row_re.captures_iter(&text).collect();
    assert_eq!(rows.len(), result.ranked.len(), "one row per ranked asset");
    for (caps, row) in rows.iter().zip(&result.ranked) {
        assert_eq!(&caps[1], result.names[&row.asset_id]);
        assert_eq!(&caps[2], row.tier);
        assert_eq!(&caps[3], "observed");
        assert_eq!(caps[4].parse::<u64>().unwrap(), row.k);
        assert_eq!(caps[5].parse::<u64>().unwrap(), row.n);
    }
    assert!(text.contains("early_evidence"));
    assert!(text.contains("needs 8 more"));
    assert!(text.contains("needs 13 more"));
    // Loaded-only rows show the state of the one signal that applies to them, the context cost:
    // observed when an estimate exists, no_coverage when there is no basis for one.
    assert!(text.contains(
        "rules_file:lambda-invented  tier=inferred state=observed  context cost est. 800 tokens (file_bytes_div4)"
    ));
    assert!(
        text.contains("prompt:mu-invented  tier=inferred state=no_coverage  no context-cost basis")
    );
    assert!(text.contains("child tokens mean 5000 in 4 exactly attributed runs (observed)"));
    assert!(text.contains("3 user denials and 0 interruptions excluded"));
}

/// With scrub on, every name becomes `type:asset_id[:12]` and no local name survives; a name the
/// operator listed as public survives; without scrub names appear. The 12-character prefix is the
/// point — a longer prefix would make the id itself a fingerprint of the name.
/// Cannot prove the public list was curated correctly by the person running it.
#[test]
fn scrub_replaces_names_unless_public() {
    let scrubbed = out(true, &[]);
    for display in names().values() {
        assert!(
            !scrubbed.contains(display.as_str()),
            "{display} survived scrubbing"
        );
    }
    let zeta = hex64("zeta-invented");
    assert!(scrubbed.contains(&format!("mcp_server:{}", &zeta[..12])));
    assert!(
        !scrubbed.contains(&zeta[..13]),
        "only 12 characters of the id are shown"
    );

    let partly = out(true, &["mcp_server:zeta-invented"]);
    assert!(partly.contains("mcp_server:zeta-invented"));
    assert!(!partly.contains("mcp_server:eta-invented"));
    assert!(out(false, &[]).contains("mcp_server:eta-invented"));
}

/// The cost line is derived from the stratum's tokens times the dated table, names that date, says
/// it is a display-time derivation, and a model with no price entry says so rather than inventing a
/// figure. A missing table yields the unavailable line, not an exception and not a zero.
/// Cannot prove the prices are current — that is what the date is for.
#[test]
fn cost_line_names_the_price_table_date_and_derives_nothing_it_cannot() {
    let env = json!({
        "envelope_version": "0.1.0", "extractor_version": "proto-0.1.0+taskcat-1",
        "gate_version": 1, "emitted_day": "2026-03-06",
        "resource": {"device_id": "00000000-0000-4000-8000-000000000000",
                     "device_id_source": "placeholder", "harness": HARNESS,
                     "harness_version": "1.0.0", "collector": "prototype",
                     "collector_version": "0.1.0"},
        "records": [
            cost_record("big", MODEL, 1_000_000, 500_000),
            cost_record("np", "other", 1000, 500),
        ],
        "bom": [], "coverage": {},
    });
    let result = rank(&env, &BTreeMap::new(), "fix", HARNESS, None);
    let text = render_with_prices(&result, false, &BTreeSet::new(), Some(&prices()));
    assert!(text.contains("price table dated 2031-01-01"));
    assert!(text.contains("(display-time derivation, not stored)"));
    assert!(text.contains(&format!("{MODEL}: USD 2.00 over 1 runs")));
    assert!(text.contains("other: no price entry in the table dated 2031-01-01"));

    let missing = render_with_prices(&result, false, &BTreeSet::new(), None);
    assert!(missing.contains("price table unavailable"));
    assert!(!missing.contains("USD"), "no figure without a table");
}

fn cost_record(label: &str, model: &str, input: i64, output: i64) -> Value {
    json!({
        "run_id": hex64(&format!("run-{label}")), "observed_day": "2026-03-05", "model": model,
        "entrypoint_class": "cli", "effort": "medium", "permission_mode": "default",
        "task_category": "code_edit", "bom_version": hex64("bom"),
        "loaded_set_basis": "harness_log", "run_outcome": "completed",
        "counts": {"turns": 1, "tool_calls": 1, "tool_failures": 0, "user_denials": 0,
                   "subagent_runs": 0, "compactions": 0, "unpaired_tool_uses": 0,
                   "repeated_tool_calls": 0},
        "tokens": {"input": input, "cache_creation": Value::Null, "cache_read": Value::Null,
                   "cached_input": Value::Null, "output": output, "thinking": Value::Null,
                   "reasoning": Value::Null, "basis": "harness_usage"},
        "assets": [],
    })
}

/// With runs only in other categories the view pools them and SAYS so, still naming the others as
/// context; with no runs at all the empty-state line renders and no rows. Silence that looks like
/// good news is the failure mode both of those lines exist to prevent.
#[test]
fn empty_and_pooled_strata_say_which_they_are() {
    let env = json!({
        "envelope_version": "0.1.0", "extractor_version": "proto-0.1.0+taskcat-1",
        "gate_version": 1, "emitted_day": "2026-03-06",
        "resource": {"device_id": "00000000-0000-4000-8000-000000000000",
                     "device_id_source": "placeholder", "harness": HARNESS,
                     "harness_version": "1.0.0", "collector": "prototype",
                     "collector_version": "0.1.0"},
        "records": [{
            "run_id": hex64("run-x"), "observed_day": "2026-03-05", "model": MODEL,
            "entrypoint_class": "cli", "effort": "medium", "permission_mode": "default",
            "task_category": "code_explore", "bom_version": hex64("bom"),
            "loaded_set_basis": "harness_log", "run_outcome": "completed",
            "counts": {"turns": 1, "tool_calls": 1, "tool_failures": 0, "user_denials": 0,
                       "subagent_runs": 0, "compactions": 0, "unpaired_tool_uses": 0,
                       "repeated_tool_calls": 0},
            "tokens": {"input": 1000, "cache_creation": Value::Null, "cache_read": Value::Null,
                       "cached_input": Value::Null, "output": 500, "thinking": Value::Null,
                       "reasoning": Value::Null, "basis": "harness_usage"},
            "assets": [],
        }],
        "bom": [], "coverage": {},
    });
    let pooled = render_with_prices(
        &rank(&env, &BTreeMap::new(), "fix", HARNESS, None),
        false,
        &BTreeSet::new(),
        Some(&prices()),
    );
    assert!(!pooled.contains(copy("empty")));
    assert!(pooled.contains("pools every task category"));
    assert!(pooled.contains("code_explore 1 runs"));

    let mut bare = env.clone();
    bare["records"] = json!([]);
    let empty = render_with_prices(
        &rank(&bare, &BTreeMap::new(), "fix", HARNESS, None),
        false,
        &BTreeSet::new(),
        Some(&prices()),
    );
    assert!(empty.contains(copy("empty")));
    assert!(!empty.contains("non-successes"));
}

/// The full rendered text of a populated stratum — names, cost lines, denials, every list present —
/// contains no forbidden causal phrase and every line naming a rate is hedged. This is the property
/// that matters: the data is correlational, and the report must never imply the asset *caused* an
/// outcome. Cannot prove the substituted values are clean; the task text and names are local input.
#[test]
fn rendered_output_passes_the_copy_lint() {
    assert_eq!(lint(&out(false, &[])), Vec::<String>::new());
    assert_eq!(lint(&out(true, &[])), Vec::<String>::new());
    let golden =
        fs::read_to_string(fixtures_dir().join("golden/ranking.txt")).expect("golden committed");
    assert_eq!(lint(&golden), Vec::<String>::new());
}

/// Every template is free of the forbidden causal phrases, and every template naming a rate
/// contains "observed". The lint's hedge rule cannot see `{n}` as a number, so a template that
/// says "rate" has to carry the word itself.
/// Cannot prove prose assembled outside COPY is clean — the next test covers that.
#[test]
fn no_copy_template_contains_a_forbidden_phrase() {
    assert!(COPY.len() > 10);
    let rate = Regex::new(r"(?i)\brates?\b").expect("rate pattern compiles");
    for (key, template) in COPY {
        assert_eq!(
            lint(template),
            Vec::<String>::new(),
            "COPY[{key:?}] is not clean"
        );
        if rate.is_match(template) {
            assert!(
                template.to_lowercase().contains("observed"),
                "COPY[{key:?}] names a rate without 'observed'"
            );
        }
    }
}

/// Every rendered line matches some template once placeholders are wildcarded, so no string
/// outside `COPY` reaches the output. That is what makes linting `COPY` sufficient rather than
/// merely suggestive — a renderer free to build sentences inline would put the product's voice
/// beyond review. Cannot prove the substituted values are clean.
#[test]
fn render_uses_only_copy_templates() {
    let text = render_with_prices(
        &rank(&populated(), &names(), "fix", HARNESS, None),
        false,
        &BTreeSet::new(),
        // No table, so the cost_unavailable template is exercised too.
        None,
    );
    let patterns: Vec<Regex> = COPY
        .iter()
        .map(|(_, template)| {
            let escaped = regex::escape(template);
            let placeholder = Regex::new(r"\\\{[^}]*\\?\}").expect("placeholder pattern compiles");
            Regex::new(&format!("^{}$", placeholder.replace_all(&escaped, ".*")))
                .expect("template pattern compiles")
        })
        .collect();
    for line in text.lines() {
        assert!(
            patterns.iter().any(|p| p.is_match(line)),
            "line not from COPY: {line:?}"
        );
    }
}

/// The committed worked example is real output from the author's machine, and its five public names
/// are NOT in the payload — so rendering it scrubbed must still reproduce the figures verbatim.
/// These are the float-formatting baselines the plan pins: the EN DASH interval and the two-decimal
/// cost. Cannot prove the example is representative of any other machine.
#[test]
fn worked_example_render_is_structurally_stable() {
    let example: Value = serde_json::from_slice(
        &fs::read(fixtures_dir().join("worked-example/observations.example.json"))
            .expect("worked example committed"),
    )
    .expect("worked example parses");
    let reference = fs::read_to_string(fixtures_dir().join("worked-example/ranking.example.txt"))
        .expect("reference report committed");

    let harness = example["resource"]["harness"]
        .as_str()
        .expect("the example names its harness")
        .to_string();
    let result = rank(
        &example,
        &BTreeMap::new(),
        "audit the skills",
        &harness,
        None,
    );
    let text = render(&result, true, &BTreeSet::new());

    for expected in [
        "10 non-successes in 135 calls (95% interval 4.1%\u{2013}13.1%)",
        "USD 213.60",
        "USD 35.81",
    ] {
        assert!(
            text.contains(expected),
            "missing {expected:?} from the rendered example"
        );
        assert!(
            reference.contains(expected),
            "the committed reference no longer contains {expected:?}"
        );
    }
    for (key, template) in COPY {
        // Section headers carry no placeholders, so they must appear verbatim when their section
        // does. Only check the ones the reference itself shows, so this cannot drift into asserting
        // sections the example does not have.
        if key.starts_with("section_") && reference.contains(template) {
            assert!(text.contains(template), "missing section {key:?}");
        }
    }
    assert_eq!(lint(&text), Vec::<String>::new());
}
