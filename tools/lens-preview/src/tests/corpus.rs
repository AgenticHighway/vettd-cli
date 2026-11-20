//! Corpus selection and export. Scripts read `--json` and the TUI reads the
//! same corpus, so selection, the pinned default export and stress loading
//! must not drift apart.

use super::*;
use ratatui::backend::TestBackend;

const GOLDEN: &str = include_str!("../../tests/golden/default-export.json");

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// No flags keeps the interactive default, so existing invocations are unchanged.
#[test]
fn no_flags_select_the_default_corpus() {
    assert_eq!(parse_args(&args(&[])), Ok((false, "default")));
}

// --json alone is the scripts' export of the default corpus.
#[test]
fn json_alone_exports_the_default_corpus() {
    assert_eq!(parse_args(&args(&["--json"])), Ok((true, "default")));
}

// Scripts assemble argv in any order, so both flag orders must select stress.
#[test]
fn corpus_and_json_are_order_independent() {
    assert_eq!(
        parse_args(&args(&["--corpus", "stress", "--json"])),
        Ok((true, "stress"))
    );
    assert_eq!(
        parse_args(&args(&["--json", "--corpus", "stress"])),
        Ok((true, "stress"))
    );
}

// Naming the default explicitly must behave exactly like passing no flag.
#[test]
fn explicit_default_matches_no_flag() {
    assert_eq!(
        parse_args(&args(&["--corpus", "default"])),
        Ok((false, "default"))
    );
}

// A typo must fail loudly, or a script would silently export the wrong corpus.
#[test]
fn unknown_or_missing_corpus_value_is_an_error() {
    assert_eq!(CORPUS_ERROR, "error: --corpus must be default or stress");
    assert_eq!(parse_args(&args(&["--corpus", "other"])), Err(CORPUS_ERROR));
    assert_eq!(
        parse_args(&args(&["--json", "--corpus"])),
        Err(CORPUS_ERROR)
    );
    assert_eq!(
        parse_args(&args(&["--corpus", "--json"])),
        Err(CORPUS_ERROR)
    );
}

// Unrecognised arguments are ignored, as they were before the corpus flag.
#[test]
fn other_arguments_are_ignored() {
    assert_eq!(
        parse_args(&args(&["--verbose", "x", "--json"])),
        Ok((true, "default"))
    );
}

// The pinned default export is what scripts diff against, so only the new
// corpus key may differ from the golden file.
#[test]
fn default_export_matches_golden_except_corpus_key() {
    let golden: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    let mut now = export("default", &fixture::sessions());
    assert_eq!(now["corpus"], "default");
    now.as_object_mut().unwrap().remove("corpus");
    assert_eq!(now, golden);
}

// Each corpus must rebuild to identical bytes, or exports would not be
// comparable across runs.
#[test]
fn each_corpus_exports_identically_on_rebuild() {
    let default_a = export("default", &fixture::sessions()).to_string();
    let default_b = export("default", &fixture::sessions()).to_string();
    assert_eq!(default_a, default_b);
    let stress_a = export("stress", &fixture::stress_sessions()).to_string();
    let stress_b = export("stress", &fixture::stress_sessions()).to_string();
    assert_eq!(stress_a, stress_b);
    assert_eq!(
        export("stress", &fixture::stress_sessions())["corpus"],
        "stress"
    );
}

// Workspaces are derived from the sessions, so the stress list must be the
// sorted distinct set and not repeat a workspace per session.
#[test]
fn stress_workspaces_are_sorted_and_distinct() {
    let json = export("stress", &fixture::stress_sessions());
    let ws: Vec<&str> = json["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert_eq!(ws.len(), 6, "{ws:?}");
    assert!(ws.windows(2).all(|p| p[0] < p[1]), "{ws:?}");
}

// The stress corpus has no plan or source records of its own, so it must
// carry the default values or its Home and headroom numbers would be invented.
#[test]
fn stress_reuses_default_plan_and_sources() {
    let default = export("default", &fixture::sessions());
    let stress = export("stress", &fixture::stress_sessions());
    for key in [
        "windows",
        "token_sources",
        "limit_hits",
        "claude_weekly_ends_percent",
        "now_minutes_from_nov_18",
    ] {
        assert_eq!(stress[key], default[key], "{key}");
    }
    let bp: u64 = stress["token_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["basis_points"].as_u64().unwrap())
        .sum();
    assert_eq!(bp, 10_000);
}

// Long stress strings must not panic in the views or the frame. This copies
// the render helper's steps, on the stress corpus instead of the default one.
fn draw_stress(page: &Page, all: &[fixture::Session]) -> String {
    let rt = Runtime::default();
    let stack = std::slice::from_ref(page);
    let (title, scope, rows) = content(page, all, false, screen::list_width(80), &rt);
    let active = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.open.map(|open| (i, open)))
        .nth(page.selected);
    let mut term = ratatui::Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| {
        screen::draw(
            f,
            stack,
            all,
            false,
            (&title, &scope),
            &rows,
            active,
            &rt,
            false,
        )
    })
    .unwrap();
    let buffer = term.backend().buffer();
    (0..24)
        .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

// Every screen the TUI can open must build and draw on the stress corpus, or
// the stress flag would crash a view a developer then navigates to.
#[test]
fn stress_views_build_and_draw_at_80x24() {
    let all = fixture::stress_sessions();
    let rt = Runtime::default();
    let mut views = vec![
        View::Home,
        View::Headroom,
        View::Week,
        View::Find,
        View::Help,
        View::Inventory,
    ];
    views.extend((12..=18).map(View::Day));
    for view in views {
        for count in [false, true] {
            let (title, _scope, rows) = content(&Page::new(view), &all, count, 80, &rt);
            assert!(!rows.is_empty() || !title.is_empty(), "{view:?}");
        }
        let _ = draw_stress(&Page::new(view), &all);
    }
}

// Real logs already hold more sessions than the stress corpus, which is
// at 3.36B tokens. A u32 total would panic (debug) or wrap to a wrong total
// (release) at about 4.29B, so the sum must not depend on that ceiling.
#[test]
fn token_total_does_not_overflow_u32() {
    let request = || fixture::Request {
        worker: "main",
        model: "m",
        tokens: [u32::MAX; 4],
        milli_usd_per_ktok: None,
    };
    let session = fixture::Session {
        day: 12,
        name: "big",
        workspace: "api",
        agent: "Claude",
        incomplete: false,
        requests: vec![request(), request()],
        observed: vec![],
    };
    assert_eq!(screen::total_tokens(&[session]), 8 * u64::from(u32::MAX));
}

// The token column is six cells wide. A longer figure loses its leading
// digit, so 1,276.5M read as 276.5M, smaller than the row below it.
#[test]
fn compact_tokens_fit_six_cells() {
    for tokens in [
        999,
        999_999,
        999_949_999,
        999_950_000,
        1_276_500_000,
        999_900_000_000,
    ] {
        let text = screen::compact(tokens);
        assert!(text.chars().count() <= 6, "{tokens} -> {text}");
    }
    assert_eq!(screen::compact(999_950_000), "1.0B");
    assert_eq!(screen::compact(1_276_500_000), "1.3B");
}

// Every stress day has an unpriced or incomplete session, so none is
// complete. The chart must say there is no normal, never "normal ≈ $0.00",
// and mark the floors once in the title rather than on every bar.
#[test]
fn stress_home_shows_unknown_normal_and_whole_token_figures() {
    let all = fixture::stress_sessions();
    let (days, normal) = screen::daily(&all, false);
    assert!(days.iter().all(|d| d.partial));
    assert_eq!(normal, None);
    let text = draw_stress(&Page::new(View::Home), &all);
    assert!(text.contains("normal: no complete day"), "{text}");
    assert!(text.contains("every day partial (≥)"), "{text}");
    assert!(!text.contains("$0.00"), "{text}");
    assert_eq!(text.matches('≥').count(), 1, "{text}");
    assert!(
        text.contains("File reads") && text.contains("1.3B"),
        "{text}"
    );
}
