//! Corpus selection and export. Scripts read `--json` and the TUI reads the
//! same corpus, so selection, the pinned default export and stress loading
//! must not drift apart.

use super::*;
use ratatui::backend::TestBackend;

const GOLDEN: &str = include_str!("../../tests/golden/default-export.json");

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// (json, corpus, error) from a command line.
fn parsed(items: &[&str]) -> (bool, Option<&'static str>, Option<String>) {
    let a = parse_args(args(items));
    (a.json, a.corpus, a.error)
}

// No flags keeps the interactive default, so existing invocations are unchanged.
#[test]
fn no_flags_select_the_default_corpus() {
    assert_eq!(parsed(&[]), (false, None, None));
}

// --json alone must still export the default corpus, as scripts expect.
#[test]
fn json_alone_exports_the_default_corpus() {
    assert_eq!(parsed(&["--json"]), (true, None, None));
}

// Flag order must not change the result, or scripts break on reordering.
#[test]
fn corpus_and_json_are_order_independent() {
    assert_eq!(
        parsed(&["--corpus", "stress", "--json"]),
        (true, Some("stress"), None)
    );
    assert_eq!(
        parsed(&["--json", "--corpus", "stress"]),
        (true, Some("stress"), None)
    );
}

// Naming the default explicitly must be the same corpus as omitting it.
#[test]
fn explicit_default_matches_no_flag() {
    assert_eq!(
        parsed(&["--corpus", "default"]),
        (false, Some("default"), None)
    );
    assert_eq!(Data::fixture().len(), fixture::sessions().len());
}

// A typo must fail loudly rather than silently load the default corpus.
#[test]
fn unknown_or_missing_corpus_value_is_an_error() {
    let error = Some(CORPUS_ERROR.to_string());
    assert_eq!(parsed(&["--corpus", "other"]).2, error);
    assert_eq!(parsed(&["--json", "--corpus"]).2, error);
    assert_eq!(parsed(&["--corpus", "--json"]).2, error);
}

// A corpus is synthetic and local data is real. Accepting both would make
// the export claim a corpus it never read, so the pair is refused.
#[test]
fn corpus_with_local_source_is_an_error() {
    let error = Some(LOCAL_CORPUS_ERROR.to_string());
    for corpus in ["default", "stress"] {
        assert_eq!(parsed(&["--source", "local", "--corpus", corpus]).2, error);
        assert_eq!(parsed(&["--corpus", corpus, "--source=local"]).2, error);
    }
    // The fixture source takes either corpus.
    assert_eq!(
        parsed(&["--source", "fixture", "--corpus", "stress"]).2,
        None
    );
}

// Unknown arguments stay ignored, as they were before --corpus existed.
#[test]
fn other_arguments_are_ignored() {
    assert_eq!(parsed(&["--verbose", "x", "--json"]), (true, None, None));
}

// The pinned default export is what scripts diff against, so only the
// added source and corpus keys may differ from the golden file.
#[test]
fn default_export_matches_golden_except_source_and_corpus_keys() {
    let golden: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    let mut now = export(&Data::fixture(), "default");
    assert_eq!(now["corpus"], "default");
    assert_eq!(now["source"], "fixture");
    now.as_object_mut().unwrap().remove("corpus");
    now.as_object_mut().unwrap().remove("source");
    assert_eq!(now, golden);
}

// Each corpus must rebuild to identical bytes, or exports would not be
// comparable across runs.
#[test]
fn each_corpus_exports_identically_on_rebuild() {
    let default_a = export(&Data::fixture(), "default").to_string();
    let default_b = export(&Data::fixture(), "default").to_string();
    assert_eq!(default_a, default_b);
    let stress_a = export(&Data::stress(), "stress").to_string();
    let stress_b = export(&Data::stress(), "stress").to_string();
    assert_eq!(stress_a, stress_b);
    assert_eq!(export(&Data::stress(), "stress")["corpus"], "stress");
}

// Workspaces are derived from the sessions, so the stress list must be the
// sorted distinct set and not repeat a workspace per session.
#[test]
fn stress_workspaces_are_sorted_and_distinct() {
    let json = export(&Data::stress(), "stress");
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
    let default = export(&Data::fixture(), "default");
    let stress = export(&Data::stress(), "stress");
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
fn draw_stress(page: &Page, all: &Data) -> String {
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
    let all = Data::stress();
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
        exact: None,
    };
    let session = fixture::Session {
        day: 12,
        name: "big",
        workspace: "api",
        agent: "Claude",
        incomplete: false,
        requests: vec![request(), request()],
        observed: vec![],
        group: 0,
        used: vec![],
        sources: vec![],
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
    let all = Data::stress();
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

// Stress days cost thousands of dollars, so "12 $1885.33" is wider than its
// 9-cell bar and was cut to "12 $1885.", losing its cents mid-number. Labels
// must fit whole, falling back to whole dollars.
#[test]
fn stress_day_labels_fit_their_bars() {
    let text = draw_stress(&Page::new(View::Home), &Data::stress());
    for day in 12..=18 {
        let start = format!("{day} $");
        let at = text.find(&start).expect("day label missing") + start.len();
        let amount: String = text[at..].chars().take_while(|c| *c != ' ').collect();
        assert!(
            amount.chars().all(|c| c.is_ascii_digit()),
            "{day}: label cut to ${amount}
{text}"
        );
    }
}

// A use's signals explain its calls. A breakdown that sums to fewer calls
// than the observations it summarizes would misreport where calls came from.
#[test]
fn stress_signals_account_for_every_call() {
    for s in fixture::stress_sessions() {
        for u in &s.observed {
            let explained: u32 = u.signals.iter().map(|&(_, n)| n).sum();
            assert_eq!(explained, u.count, "{} on Nov {}", s.name, s.day);
        }
    }
}

// The qa-pack manifest (component 5) records its use on its children, so a
// direct call to it would be fictitious history in its usage total.
#[test]
fn stress_never_calls_the_plugin_manifest() {
    let all = fixture::stress_sessions();
    assert!(all
        .iter()
        .flat_map(|s| &s.observed)
        .all(|u| u.component != 5));
    // Its children are still called, so plugin activity stays visible.
    assert!(all
        .iter()
        .flat_map(|s| &s.observed)
        .any(|u| u.component == 6 || u.component == 7));
}

// Partial input is not the same as unpriced input. A session that is
// incomplete but fully priced must not be called unpriced, and its recorded
// context estimates count, as floors, instead of disappearing.
#[test]
fn incomplete_but_priced_session_is_not_called_unpriced() {
    let mut data = Data::fixture();
    let id = data
        .sessions
        .iter()
        .position(|s| s.day == 12 && priced(s) == s.requests.len())
        .unwrap();
    data.sessions[id].incomplete = true;
    let component = data.sessions[id].observed[0].component;
    let rt = Runtime::default();
    let home = content(&Page::new(View::Home), &data, false, 80, &rt).2;
    let finding = home
        .iter()
        .find(|r| r.text.contains("partial input"))
        .expect("partial-input finding");
    assert!(finding.text.starts_with("Nov 12"), "{}", finding.text);
    assert!(!finding.text.contains("unpriced"), "{}", finding.text);
    let page = Page::new(View::Component(component, Scope::WEEK));
    let detail: Vec<String> = content(&page, &data, false, 80, &rt)
        .2
        .into_iter()
        .map(|r| r.text)
        .collect();
    assert!(
        detail
            .iter()
            .any(|t| t.contains("in partial input: their estimates are floors")),
        "{detail:#?}"
    );
}

// The default fixture's Nov 18 session is incomplete and unpriced, so it
// stays "unpriced": the fix must not relabel a real unknown.
#[test]
fn incomplete_and_unpriced_session_is_still_called_unpriced() {
    let rt = Runtime::default();
    let home = content(&Page::new(View::Home), &Data::fixture(), false, 80, &rt).2;
    assert!(home
        .iter()
        .any(|r| r.text == "Nov 18: partial input, unpriced"));
}

// The title doesn't wrap. At 50 columns "every day partial" is cut off, so
// each bar must carry its own ≥ or nothing marks the totals as floors.
#[test]
fn narrow_stress_chart_keeps_lower_bound_markers() {
    let data = Data::stress();
    let page = Page::new(View::Home);
    let rt = Runtime::default();
    let (title, scope, rows) = content(&page, &data, false, screen::list_width(50), &rt);
    let active = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.open.map(|open| (i, open)))
        .nth(page.selected);
    let mut term = ratatui::Terminal::new(TestBackend::new(50, 24)).unwrap();
    term.draw(|f| {
        screen::draw(
            f,
            std::slice::from_ref(&page),
            &data,
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
    let text: String = (0..24)
        .map(|y| (0..50).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    let full_note = text.contains("every day partial (≥)");
    let marked = (12..=18).all(|d| text.contains(&format!("{d}≥")));
    assert!(full_note || marked, "{text}");
}
