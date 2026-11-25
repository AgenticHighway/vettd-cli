//! Reader tests on hand-written logs under `tests/data/`. Every line there
//! was written for these tests to mimic the observed shapes; none comes
//! from a real log.

use super::*;
use crate::plan::{has_reset, used};
use crate::{content, Page, Runtime, View};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
}

fn roots() -> Roots {
    Roots {
        claude: Some(data_dir().join("claude")),
        codex: Some(data_dir().join("codex")),
        relay: None,
    }
}

/// 2026-11-25 12:00 UTC: the test logs' "now".
fn now() -> i64 {
    parse_ts("2026-11-25T12:00:00Z").unwrap()
}

fn nov(day: u32) -> u32 {
    crate::data::days_from_civil(2026, 11, day) as u32
}

fn load() -> Data {
    read(&roots(), now(), None, &mut Vec::new())
}

fn found(agent: &str) -> Found {
    let mut found = Found::default();
    let history = History {
        cutoff: None,
        since: now() - HISTORY_DAYS * DAY,
    };
    let r = roots();
    match agent {
        "Claude" => claude::read(r.claude.as_deref().unwrap(), history, &mut found),
        _ => codex::read(r.codex.as_deref().unwrap(), history, &mut found),
    }
    found
}

fn slices<'a>(data: &'a Data, name: &str) -> Vec<&'a Session> {
    data.iter().filter(|s| s.name == name).collect()
}

fn texts(data: &Data, view: View) -> Vec<String> {
    content(&Page::new(view), data, false, 80, &Runtime::default())
        .2
        .into_iter()
        .map(|r| r.text)
        .collect()
}

// One response is written once per content block, with output tokens
// filling in as it streams. Summing lines would triple-count it, and the
// last line isn't always the largest.
#[test]
fn streamed_request_counts_once_with_its_largest_output() {
    let found = found("Claude");
    let rq1: Vec<_> = found
        .requests
        .iter()
        .filter(|r| r.ts == parse_ts("2026-11-24T23:50:00Z").unwrap())
        .collect();
    assert_eq!(rq1.len(), 1, "rq-1 is in two files and on three lines each");
    assert_eq!(rq1[0].tokens[3], 310);
}

// A resumed session copies earlier lines. The copy must not count again,
// and the request stays in the session that first recorded it.
#[test]
fn copied_request_and_rejection_count_once_in_the_first_session() {
    let found = found("Claude");
    let in_b: Vec<_> = found
        .requests
        .iter()
        .filter(|r| r.session == "s-bbb")
        .collect();
    assert_eq!(in_b.len(), 1, "only rq-4 is B's own");
    assert_eq!(found.limit_hits, 1, "rq-rej appears in two files");
}

#[test]
fn session_across_midnight_shows_on_both_days_and_counts_once() {
    let data = load();
    let a = slices(&data, "Renamed session title");
    let days: Vec<(u32, usize)> = a.iter().map(|s| (s.day, s.requests.len())).collect();
    // Nov 24: rq-1, rq-2. Nov 25: rq-3, two subagent requests and the
    // misfiled one from B's folder.
    assert_eq!(days, [(nov(24), 2), (nov(25), 4)]);
    assert_eq!(a[0].group, a[1].group);
    // A, B, C, the Codex root and the archived Codex session.
    assert_eq!(data.session_count(), 5);
    assert!(texts(&data, View::Home)
        .iter()
        .all(|t| !t.contains("review-kit")));
}

#[test]
fn subagents_roll_up_by_session_id_even_when_misfiled() {
    let data = load();
    let a = slices(&data, "Renamed session title");
    let subagent = a
        .iter()
        .flat_map(|s| &s.requests)
        .filter(|r| r.worker == "Subagent")
        .count();
    assert_eq!(subagent, 3);
    let b = slices(&data, "08:00");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].requests.len(), 1);
}

// A file still being written ends in a cut-off line. The session is a
// floor, not wrong, and the preview keeps going.
#[test]
fn truncated_last_line_marks_partial_and_keeps_earlier_requests() {
    let data = load();
    let b = slices(&data, "08:00");
    assert!(b[0].incomplete);
    assert_eq!(b[0].requests[0].tokens[0], 40);
    assert!(!slices(&data, "Renamed session title")[0].incomplete);
}

#[test]
fn titles_use_the_latest_or_the_start_time() {
    let data = load();
    assert!(!slices(&data, "Renamed session title").is_empty());
    assert!(slices(&data, "Middle title").is_empty());
    assert_eq!(slices(&data, "Release title").len(), 2);
    assert_eq!(slices(&data, "07:05").len(), 1, "archived, no index entry");
}

// The workspace comes from cwd. The project folder spells out a full
// path and must never become a label.
#[test]
fn workspace_comes_from_cwd_never_the_project_folder() {
    let data = load();
    let mut workspaces: Vec<&str> = data.iter().map(|s| s.workspace).collect();
    workspaces.sort_unstable();
    workspaces.dedup();
    assert_eq!(workspaces, ["api", "docs", "web"]);
}

// rq-2's context grew by 1,000 tokens after a 3,000-character Read result
// and a 1,000-character Bash result.
#[test]
fn growth_splits_by_block_characters() {
    let data = load();
    let nov24 = slices(&data, "Renamed session title")[0];
    assert_eq!(nov24.sources[0], 750, "File reads");
    assert_eq!(nov24.sources[1], 250, "Shell output");
    assert_eq!(nov24.sources[7], 10, "rq-1's whole context, after a prompt");
}

// The hook's context reaches the model; the prompt snapshot isn't known
// to, so it adds nothing even though it is the largest block.
#[test]
fn hook_context_counts_and_prompt_snapshot_does_not() {
    let data = load();
    let nov25 = slices(&data, "Renamed session title")[1];
    assert_eq!(nov25.sources[6], 100, "Hooks");
    assert_eq!(nov25.sources.iter().sum::<u64>(), 100);
    let shares: u32 = data.token_sources.iter().map(|s| s.basis_points).sum();
    assert_eq!(shares, 10_000);
}

#[test]
fn codex_exec_output_stays_one_bucket() {
    let data = load();
    let root = slices(&data, "Release title")[0];
    assert_eq!(root.sources[0] + root.sources[1], 0);
    // resp-1: 1,000 tokens over 100 + 18 + 2,000 characters. resp-1b: 350.
    assert_eq!(root.sources[2], 944 + 350);
    assert_eq!(root.sources[5], 47);
}

#[test]
fn skills_count_once_per_request_and_codex_uses_are_estimated() {
    let data = load();
    let nov25 = slices(&data, "Renamed session title")[1];
    let skill = nov25.used.iter().find(|u| u.kind == "skill").unwrap();
    assert_eq!(
        (skill.name, skill.count, skill.copy),
        ("sample-skill", 1, "unknown")
    );
    assert!(!skill.estimated);
    let nov24 = slices(&data, "Renamed session title")[0];
    let mcp = nov24.used.iter().find(|u| u.kind == "MCP").unwrap();
    assert_eq!((mcp.name, mcp.count), ("sample-server", 1));
    // cloud_skills claims 9 uses; only the one SKILL.md read counts.
    let codex = slices(&data, "Release title")[0];
    let est = codex.used.iter().find(|u| u.kind == "skill").unwrap();
    assert_eq!(
        (est.name, est.count, est.copy),
        ("sample-skill", 1, "workspace")
    );
    assert!(est.estimated);
    let rows = texts(
        &data,
        View::Session(data.iter().position(|s| std::ptr::eq(s, codex)).unwrap()),
    );
    assert!(
        rows.iter()
            .any(|t| t.contains("sample-skill") && t.ends_with("(estimated)")),
        "{rows:?}"
    );
}

#[test]
fn codex_threads_join_their_root_and_archived_copies_count_once() {
    let data = load();
    let root = slices(&data, "Release title");
    let requests: Vec<_> = root.iter().flat_map(|s| &s.requests).collect();
    assert_eq!(
        requests.len(),
        3,
        "resp-1, resp-1b and the subagent's resp-2"
    );
    assert_eq!(
        requests.iter().filter(|r| r.worker == "Subagent").count(),
        1
    );
    // resp-2's totals disagree, so the whole session reads as partial.
    assert!(root.iter().all(|s| s.incomplete));
    // Cached input is inside input_tokens: fresh = 1,000 - 200.
    assert_eq!(requests[0].tokens[..2], [800, 200]);
    assert_eq!(data.limit_hits_by_agent.as_ref().unwrap()[1], ("Codex", 1));
}

#[test]
fn unpriced_models_show_unpriced_never_zero() {
    let data = load();
    let id = data.iter().position(|s| s.name == "09:12").unwrap();
    let day = texts(&data, View::Day(nov(25)));
    let row = day.iter().find(|t| t.starts_with("09:12")).unwrap();
    assert!(row.contains("unpriced"), "{row}");
    let (_, scope, _) = content(
        &Page::new(View::Session(id)),
        &data,
        false,
        80,
        &Runtime::default(),
    );
    assert!(scope.contains("No request has a rate"), "{scope}");
    assert!(!scope.contains("$0.00"));
}

#[test]
fn local_dollars_are_api_equivalent() {
    let data = load();
    let id = data.iter().position(|s| s.name == "Release title").unwrap();
    let (_, scope, _) = content(
        &Page::new(View::Session(id)),
        &data,
        false,
        80,
        &Runtime::default(),
    );
    assert!(scope.contains("USD API-equivalent"), "{scope}");
    assert!(!scope.contains("priced)"), "{scope}");
    let (answer, chips, _) = content(
        &Page::new(View::Home),
        &data,
        false,
        80,
        &Runtime::default(),
    );
    assert!(answer.ends_with("API-equivalent"), "{answer}");
    assert!(chips.ends_with("API-equivalent USD"), "{chips}");
}

// Readings from concurrent sessions arrive out of step, resets_at drifts
// by seconds, and sides can swap. All of them are one window each.
#[test]
fn codex_windows_group_by_time_with_a_running_maximum() {
    let mut notes = Vec::new();
    let data = read(&roots(), now(), None, &mut notes);
    let window = |name: &str| {
        data.windows
            .iter()
            .find(|w| w.agent == "Codex" && w.name == name)
            .unwrap()
    };
    let five: Vec<u32> = window("5-hour").readings.iter().map(|r| r.1).collect();
    assert_eq!(five, [30, 30, 33]);
    assert_eq!(used(window("5-hour"), data.now), Some(33));
    let weekly: Vec<u32> = window("weekly").readings.iter().map(|r| r.1).collect();
    assert_eq!(weekly, [40, 45, 50]);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("1 empty rate_limits sides")),
        "{notes:?}"
    );
    // Claude windows need the relay: no reading, nothing selectable.
    let rows = crate::headroom(&data).2;
    let claude: Vec<_> = rows
        .iter()
        .filter(|r| r.text.starts_with("Claude ·"))
        .collect();
    assert_eq!(claude.len(), 2);
    assert!(claude
        .iter()
        .all(|r| r.open.is_none() && r.text.ends_with("relay")));
    assert!(rows
        .iter()
        .any(|r| r.text == "  Claude 5-hour: relay not set up"));
}

#[test]
fn window_that_reset_shows_no_share() {
    let midnight = now().div_euclid(DAY) * DAY;
    let readings = [Reading {
        ts: now() - 5 * 3600,
        minutes: 300,
        percent: 80.0,
        resets_at: now() - 40 * 60,
    }];
    let windows = codex::windows(&readings, midnight, &mut Vec::new());
    let five = &windows[0];
    let now_minutes = ((now() - midnight) / 60) as i32;
    assert!(has_reset(five, now_minutes));
    assert_eq!(used(five, now_minutes), None);
}

#[test]
fn missing_directory_is_a_note_not_an_error() {
    let roots = Roots {
        claude: roots().claude,
        codex: Some(data_dir().join("no-such-codex")),
        relay: None,
    };
    let mut notes = Vec::new();
    let data = read(&roots, now(), None, &mut notes);
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("Codex: no sessions directory")),
        "{notes:?}"
    );
    assert!(data.iter().all(|s| s.agent == "Claude"));
    assert!(!data.is_empty());
}

#[test]
fn no_sessions_still_answers() {
    let empty = Roots {
        claude: None,
        codex: None,
        relay: None,
    };
    let data = read(&empty, now(), None, &mut Vec::new());
    let (answer, _, _) = content(
        &Page::new(View::Home),
        &data,
        false,
        80,
        &Runtime::default(),
    );
    assert_eq!(answer, "No sessions in the last 7 days");
}

#[test]
fn period_across_a_month_end_sorts_by_date() {
    let dec2 = parse_ts("2026-12-02T08:00:00Z").unwrap();
    let data = read(
        &Roots {
            claude: None,
            codex: None,
            relay: None,
        },
        dec2,
        None,
        &mut Vec::new(),
    );
    let labels: Vec<String> = data
        .days
        .iter()
        .map(|&d| crate::data::day_label(d))
        .collect();
    assert_eq!(
        labels,
        ["Nov 26", "Nov 27", "Nov 28", "Nov 29", "Nov 30", "Dec 1", "Dec 2"]
    );
    assert_eq!(data.period(), "Nov 26-Dec 2, 2026 UTC");
}

// Old files are never opened; files from the last 28 days only add
// limit hits when none of their requests fall in the period.
#[test]
fn files_older_than_28_days_are_not_opened() {
    let tmp = std::env::temp_dir().join(format!("lens-mtime-{}", std::process::id()));
    let project = tmp.join("projects").join("p");
    std::fs::create_dir_all(&project).unwrap();
    let line = |id: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-10-30T10:00:00Z","isApiErrorMessage":true,"requestId":"{id}","quotaLimits":{{"rateLimitType":"five_hour","status":"rejected"}},"message":{{"model":"<synthetic>"}}}}"#
        )
    };
    let day = Duration::from_secs(DAY as u64);
    let run_at = UNIX_EPOCH + Duration::from_secs(now() as u64);
    for (name, age) in [("old", 30), ("recent", 20)] {
        let path = project.join(format!("{name}.jsonl"));
        std::fs::write(&path, line(name)).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(run_at - day * age).unwrap();
    }
    let roots = Roots {
        claude: Some(tmp.clone()),
        codex: None,
        relay: None,
    };
    let cutoff = run_at - day * 28;
    let since = parse_ts("2026-10-28T12:00:00Z").unwrap();
    let data = read(&roots, now(), Some(cutoff), &mut Vec::new());
    std::fs::remove_dir_all(&tmp).unwrap();
    assert!(since < parse_ts("2026-10-30T10:00:00Z").unwrap());
    assert_eq!(data.limit_hits_by_agent.as_ref().unwrap()[0], ("Claude", 1));
    assert!(data.is_empty());
}

// The export may be shared. It holds aggregates only: none of the test
// logs' IDs, paths, folder names, titles, skill or server names or text.
#[test]
fn local_export_holds_no_identifiers() {
    let data = load();
    let json = export(&data);
    let text = json.to_string();
    for secret in [
        "s-aaa",
        "s-bbb",
        "rq-1",
        "msg-rq",
        "tu-1",
        "th-root",
        "th-sub",
        "resp-1",
        "c-1",
        "t-9",
        "tester",
        "secret-project",
        "example.invalid",
        "SECRET-PROMPT-TEXT",
        "Renamed session title",
        "Release title",
        "sample-skill",
        "sample-server",
        ".agents/skills/sample",
    ] {
        assert!(!text.contains(secret), "{secret} leaked into the export");
    }
    assert_eq!(json["source"], "local");
    assert_eq!(json["synthetic"], false);
    assert!(json["now_minutes_from_nov_18"].is_null());
    assert!(json["claude_weekly_ends_percent"].is_null());
    let by_agent: u64 = json["limit_hits_by_agent"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_u64().unwrap())
        .sum();
    assert_eq!(json["limit_hits"], by_agent);
    for section in ["components", "placements", "scans"] {
        assert!(json["synthetic_sections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == section));
    }
    assert_eq!(json["estimated_sections"][0], "token_sources");
    let claude_windows = json["windows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["agent"] == "Claude");
    assert!(claude_windows
        .into_iter()
        .all(|w| w["readings"].as_array().unwrap().is_empty()));
}

fn screen(data: &Data, stack: &[Page], (w, h): (u16, u16)) -> String {
    use ratatui::backend::TestBackend;
    let rt = Runtime::default();
    let page = stack.last().unwrap();
    let (title, scope, rows) = content(page, data, false, crate::screen::list_width(w), &rt);
    let active = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.open.map(|open| (i, open)))
        .nth(page.selected);
    let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        crate::screen::draw(
            f,
            stack,
            data,
            false,
            (&title, &scope),
            &rows,
            active,
            &rt,
            false,
        )
    })
    .unwrap();
    let buffer = term.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

// The top bar is the first place a person learns whose numbers they see.
#[test]
fn top_bar_names_the_source_at_80x24() {
    let local = screen(&load(), &[Page::new(View::Home)], (80, 24));
    assert!(
        local.contains("local logs · read-only · nothing leaves this machine"),
        "{local}"
    );
    assert!(local.contains("WHERE TOKENS WENT (ESTIMATED)"), "{local}");
    let fixture = screen(&Data::fixture(), &[Page::new(View::Home)], (80, 24));
    assert!(
        fixture.contains("synthetic data · nothing leaves this machine"),
        "{fixture}"
    );
}

// Real sessions beside a synthetic inventory could mislead, so every
// synthetic view says so on local data.
#[test]
fn inventory_views_carry_a_synthetic_chip_on_local_data() {
    let data = load();
    for view in [
        View::Inventory,
        View::Component(0, crate::Scope::WEEK),
        View::Placement(0),
        View::Change(0, true),
        View::Find,
    ] {
        let (_, scope, _) = content(&Page::new(view), &data, false, 80, &Runtime::default());
        assert!(scope.starts_with("synthetic | "), "{view:?}: {scope}");
        let (_, fixture, _) = content(
            &Page::new(view),
            &Data::fixture(),
            false,
            80,
            &Runtime::default(),
        );
        assert!(!fixture.contains("synthetic"), "{view:?}: {fixture}");
    }
}

// Token parts are read from usage; where they came from is estimated.
// Only the estimate may carry the word.
#[test]
fn token_detail_marks_only_estimates() {
    let data = load();
    let id = data
        .iter()
        .position(|s| s.name == "Renamed session title")
        .unwrap();
    let rows = texts(&data, View::Tokens(id));
    assert!(rows
        .iter()
        .any(|t| t.starts_with("File reads") && t.ends_with("estimated")));
    assert!(rows
        .iter()
        .filter(|t| t.starts_with("Fresh input") || t.starts_with("Output"))
        .all(|t| !t.contains("estimated")));
    assert!(
        rows.iter().any(|t| t.contains("prompt_snapshot")),
        "{rows:?}"
    );
}

// A 5-hour window opens the day of its latest reading, wherever "now" is.
#[test]
fn five_hour_row_opens_its_latest_reading_day() {
    let data = load();
    let rows = crate::headroom(&data).2;
    let five = rows
        .iter()
        .find(|r| r.text.starts_with("Codex · 5-hour"))
        .unwrap();
    assert!(
        matches!(five.open, Some((View::Day(d), _)) if d == nov(25)),
        "{}",
        five.text
    );
    let screen = screen(
        &data,
        &[Page::new(View::Home), Page::new(View::Headroom)],
        (80, 24),
    );
    assert!(
        screen.contains("Limits stopped work: Claude 1×, Codex 1×"),
        "{screen}"
    );
}

// A window that already reached 100% says when it frees up, not a "full
// by" time that has passed. The test reset is 14:00:03; minutes round
// up, so a reset is never shown before it happens.
#[test]
fn full_window_says_until_when() {
    let mut data = load();
    let five = data
        .windows
        .iter_mut()
        .find(|w| w.agent == "Codex" && w.name == "5-hour")
        .unwrap();
    five.readings = Cow::Owned(vec![(10, 40), (60, 100)]);
    let (answer, _, _) = crate::headroom(&data);
    assert!(
        answer.ends_with("is 100% used. It's full until 14:01."),
        "{answer}"
    );
}

/// A hand-written relay file in a temporary directory, from (when, window,
/// share, reset) tuples given as timestamps.
fn relay_file(name: &str, records: &[(&str, &str, f64, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lens-relay-read-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("relay.jsonl");
    let text: String = records
        .iter()
        .map(|&(t, w, pct, resets)| {
            format!(
                "{}\n",
                serde_json::json!({
                    "t": parse_ts(t).unwrap(),
                    "w": w,
                    "pct": pct,
                    "resets_at": parse_ts(resets).unwrap(),
                })
            )
        })
        .collect();
    std::fs::write(&file, text + "{\"t\":17").unwrap();
    file
}

fn with_relay(file: &Path) -> Data {
    let roots = Roots {
        relay: Some(file.to_path_buf()),
        ..roots()
    };
    read(&roots, now(), None, &mut Vec::new())
}

#[test]
fn relay_readings_feed_claude_windows_like_codex() {
    let file = relay_file(
        "feed",
        &[
            (
                "2026-11-04T10:00:00Z",
                "seven_day",
                30.0,
                "2026-11-11T00:00:00Z",
            ),
            (
                "2026-11-10T10:00:00Z",
                "seven_day",
                38.0,
                "2026-11-11T00:00:00Z",
            ),
            (
                "2026-11-12T10:00:00Z",
                "seven_day",
                20.0,
                "2026-11-18T00:00:00Z",
            ),
            (
                "2026-11-17T10:00:00Z",
                "seven_day",
                55.0,
                "2026-11-18T00:00:05Z",
            ),
            (
                "2026-11-25T09:30:00Z",
                "five_hour",
                20.0,
                "2026-11-25T14:00:00Z",
            ),
            (
                "2026-11-25T10:30:00Z",
                "five_hour",
                40.0,
                "2026-11-25T14:00:00Z",
            ),
        ],
    );
    let data = with_relay(&file);
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    let rows = crate::headroom(&data).2;
    let five = rows
        .iter()
        .find(|r| r.text.starts_with("Claude · 5-hour"))
        .unwrap();
    assert!(
        five.text.contains("40%") && five.text.contains('~'),
        "{}",
        five.text
    );
    assert!(
        five.text.ends_with("relay") && five.open.is_some(),
        "{}",
        five.text
    );
    // The only weekly readings belong to a window that already reset.
    let weekly = rows
        .iter()
        .find(|r| r.text.starts_with("Claude · weekly"))
        .unwrap();
    assert!(weekly.text.contains(" —  "), "{}", weekly.text);
    assert!(weekly.open.is_none());
    assert!(rows
        .iter()
        .any(|r| r.text == "  Claude weekly reset; no relay reading since"));
    assert!(rows
        .iter()
        .any(|r| r.text == "Claude weekly ended at 38%, 55% (2 weeks of relay data)"));
    assert!(!rows
        .iter()
        .any(|r| r.text.contains("relay; Lens doesn't guess")));
    let json = export(&data);
    assert_eq!(
        json["claude_weekly_ends_percent"],
        serde_json::json!([38, 55])
    );
    let claude5 = json["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["agent"] == "Claude" && w["name"] == "5-hour")
        .unwrap();
    assert_eq!(claude5["readings"].as_array().unwrap().len(), 2);
}

#[test]
fn relay_file_without_a_window_says_no_reading_yet() {
    let file = relay_file(
        "partial",
        &[(
            "2026-11-25T10:30:00Z",
            "five_hour",
            12.0,
            "2026-11-25T14:00:00Z",
        )],
    );
    let data = with_relay(&file);
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    let rows = crate::headroom(&data).2;
    assert!(rows
        .iter()
        .any(|r| r.text == "  Claude weekly: no relay reading yet"));
    assert!(rows
        .iter()
        .any(|r| r.text == "Claude weekly: no full week of relay data yet"));
    assert!(export(&data)["claude_weekly_ends_percent"].is_null());
}

// The fixture never reads local files, the relay included.
#[test]
fn fixture_keeps_its_synthetic_claude_windows() {
    let data = Data::fixture();
    assert!(data
        .windows
        .iter()
        .filter(|w| w.agent == "Claude")
        .all(|w| w.readings.len() >= 2 && w.note.is_none()));
}

// Review fixes (PR #338).

fn window_of(data: &Data, agent: &str, name: &str) -> Window {
    data.windows
        .iter()
        .find(|w| w.agent == agent && w.name == name)
        .unwrap()
        .clone()
}

// The synthetic inventory describes the fixture's sessions. Feeding it
// real sessions would show components that were never observed.
#[test]
fn synthetic_component_screens_keep_their_history_on_local_data() {
    let data = load();
    let rows = texts(&data, View::Component(0, crate::Scope::WEEK));
    assert!(rows.iter().any(|t| t.starts_with("Nov 16")), "{rows:?}");
}

// A fork copies requests with their timestamps. On a tie, the file
// created first (the original) owns them, whatever the names sort as.
#[test]
fn copied_request_belongs_to_the_earlier_created_file() {
    let tmp = std::env::temp_dir().join(format!("lens-fork-{}", std::process::id()));
    let project = tmp.join("projects").join("p");
    std::fs::create_dir_all(&project).unwrap();
    let line = |sid: &str, cwd: &str| {
        format!(
            r#"{{"type":"assistant","sessionId":"{sid}","cwd":"{cwd}","timestamp":"2026-11-25T10:00:00Z","requestId":"rq","message":{{"model":"claude-opus-5-5","usage":{{"input_tokens":5,"output_tokens":5}}}}}}"#
        )
    };
    let original = project.join("z-original.jsonl");
    let fork = project.join("a-fork.jsonl");
    std::fs::write(&original, line("z-original", "/w/orig")).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(&fork, line("a-fork", "/w/fork")).unwrap();
    let created = |f: &Path| std::fs::metadata(f).and_then(|m| m.created()).ok();
    let ordered = created(&original).is_some() && created(&original) < created(&fork);
    let roots = Roots {
        claude: Some(tmp.clone()),
        codex: None,
        relay: None,
    };
    let data = read(&roots, now(), None, &mut Vec::new());
    std::fs::remove_dir_all(&tmp).unwrap();
    assert_eq!(data.len(), 1, "the copied request counts once");
    // Where the platform records no creation time, file names decide.
    let owner = if ordered { "orig" } else { "fork" };
    assert_eq!(data[0].workspace, owner);
}

// A forecast whose time passed with no newer reading says nothing about
// now; only a reading of 100% says a window is full.
#[test]
fn elapsed_forecast_is_not_full_and_a_full_reading_is() {
    let mut data = load();
    let now = data.now;
    let mut w = window_of(&data, "Codex", "5-hour");
    w.precise.clear();
    // 20% then 60% project 100% within the hour, long before now, but
    // the last reading is 60%.
    w.readings = Cow::Owned(vec![(10, 20), (30, 60)]);
    assert!(crate::plan::full_at(&w).unwrap() < now);
    assert!(crate::soonest(std::slice::from_ref(&w), now).is_none());
    // One reading of 100%, no trend: full now.
    w.readings = Cow::Owned(vec![(60, 100)]);
    let (_, share) = crate::soonest(std::slice::from_ref(&w), now).unwrap();
    assert_eq!(share, 100);
    data.windows = vec![w];
    let (answer, _, _) = crate::headroom(&data);
    assert!(answer.contains("full until"), "{answer}");
}

#[test]
fn no_rate_at_all_reads_unpriced_never_zero() {
    let mut data = load();
    data.sessions.retain(|s| s.name == "09:12");
    let (answer, _, _) = content(
        &Page::new(View::Home),
        &data,
        false,
        80,
        &Runtime::default(),
    );
    assert!(answer.ends_with("unpriced API-equivalent"), "{answer}");
    let (week, _, rows) = content(
        &Page::new(View::Week),
        &data,
        false,
        80,
        &Runtime::default(),
    );
    assert!(week.contains("unpriced"), "{week}");
    let nov25 = rows.iter().find(|r| r.text.starts_with("Nov 25")).unwrap();
    assert!(
        nov25.text.contains("unpriced") && nov25.text.ends_with("partial"),
        "{}",
        nov25.text
    );
}

fn tiny_session(group: usize) -> Session {
    Session {
        day: nov(25),
        name: "tiny",
        workspace: "api",
        agent: "Claude",
        incomplete: false,
        requests: vec![Request {
            worker: "Main",
            model: "m",
            tokens: [0; 4],
            milli_usd_per_ktok: None,
            exact: Some([0.4, 0.0, 0.0, 0.0]),
        }],
        observed: Vec::new(),
        group,
        used: Vec::new(),
        sources: vec![0; BUCKETS.len()],
    }
}

// Many sessions under half a cent each still add up.
#[test]
fn dollar_totals_round_once() {
    let mut data = load();
    data.sessions = (0..4).map(tiny_session).collect();
    assert!(data.iter().all(|s| s.cents() == 0));
    assert_eq!(crate::total(data.iter(), false), 2);
    let (answer, _, _) = content(
        &Page::new(View::Home),
        &data,
        false,
        80,
        &Runtime::default(),
    );
    assert!(answer.contains("$0.02"), "{answer}");
}

// A request whose usage is null or cut short is missing, not zero, and
// its session still shows, as incomplete.
#[test]
fn null_usage_is_missing_and_its_session_still_shows() {
    let tmp = std::env::temp_dir().join(format!("lens-null-{}", std::process::id()));
    let project = tmp.join("projects").join("p");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("s-null.jsonl"),
        r#"{"type":"assistant","sessionId":"s-null","timestamp":"2026-11-25T10:00:00Z","requestId":"rq","message":{"model":"claude-opus-5-5","usage":null}}"#,
    )
    .unwrap();
    let codex = tmp.join("codex").join("sessions");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(
        codex.join("rollout-null.jsonl"),
        concat!(
            r#"{"type":"session_meta","timestamp":"2026-11-25T09:00:00Z","payload":{"id":"th-null","cwd":"/w/x"}}"#,
            "\n",
            r#"{"type":"token_usage_record","timestamp":"2026-11-25T09:05:00Z","payload":{"response_id":"r","usage":{"input_tokens":5}}}"#,
            "\n"
        ),
    )
    .unwrap();
    let roots = Roots {
        claude: Some(tmp.clone()),
        codex: Some(tmp.join("codex")),
        relay: None,
    };
    let data = read(&roots, now(), None, &mut Vec::new());
    std::fs::remove_dir_all(&tmp).unwrap();
    assert_eq!(data.len(), 2, "both sessions show");
    assert!(data.iter().all(|s| s.incomplete && s.requests.is_empty()));
}

// Readings sit on one minute grid: a reset at 04:00:30 hasn't happened at
// 04:00:10, and a reading just after midnight stays on its own day.
#[test]
fn window_minutes_never_reset_early_or_shift_days() {
    let midnight = parse_ts("2026-11-25T00:00:00Z").unwrap();
    let readings = [Reading {
        ts: parse_ts("2026-11-25T00:00:20Z").unwrap(),
        minutes: 300,
        percent: 10.0,
        resets_at: parse_ts("2026-11-25T04:00:30Z").unwrap(),
    }];
    let w = codex::windows(&readings, midnight, &mut Vec::new()).remove(0);
    let at = |hms: &str| ((parse_ts(hms).unwrap() - midnight) / 60) as i32;
    assert!(!has_reset(&w, at("2026-11-25T04:00:10Z")));
    assert!(has_reset(&w, at("2026-11-25T04:01:00Z")));
    let reading_minute = w.start.unwrap() + w.readings[0].0 as i32;
    assert_eq!(
        reading_minute.div_euclid(1440),
        0,
        "the reading is on Nov 25"
    );
}

// Review fixes (PR #339).

// Two relay readings in one minute still have a trend, and a fractional
// rise isn't rounded into a false fill.
#[test]
fn pace_uses_unrounded_relay_readings() {
    let file = relay_file(
        "precise",
        &[
            (
                "2026-11-25T10:00:05Z",
                "five_hour",
                40.0,
                "2026-11-25T14:00:00Z",
            ),
            (
                "2026-11-25T10:00:50Z",
                "five_hour",
                40.4,
                "2026-11-25T14:00:00Z",
            ),
        ],
    );
    let data = with_relay(&file);
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    let w = window_of(&data, "Claude", "5-hour");
    assert_eq!(w.readings[0].0, w.readings[1].0, "both in one minute");
    assert!(
        crate::pace(&w).is_some(),
        "same-minute readings keep a pace"
    );
    // Rounded, 40% -> 40% looks flat; 0.4 points in 45 s is a steep trend.
    assert!(crate::pace(&w).unwrap() > 100);
    let file = relay_file(
        "fraction",
        &[
            (
                "2026-11-25T10:00:00Z",
                "five_hour",
                0.4,
                "2026-11-25T14:00:00Z",
            ),
            (
                "2026-11-25T11:00:00Z",
                "five_hour",
                0.6,
                "2026-11-25T14:00:00Z",
            ),
        ],
    );
    let data = with_relay(&file);
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    // Rounded, 0% -> 1% in an hour would read as 1 point an hour; it is 0.2.
    assert_eq!(crate::pace(&window_of(&data, "Claude", "5-hour")), Some(1));
}

// A window that has reset has no current reading, so its row opens
// nothing.
#[test]
fn reset_relay_window_row_opens_nothing() {
    let file = relay_file(
        "expired",
        &[(
            "2026-11-17T10:00:00Z",
            "seven_day",
            55.0,
            "2026-11-18T00:00:00Z",
        )],
    );
    let data = with_relay(&file);
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    let rows = crate::headroom(&data).2;
    let weekly = rows
        .iter()
        .find(|r| r.text.starts_with("Claude · weekly"))
        .unwrap();
    assert!(weekly.open.is_none(), "{}", weekly.text);
}

// Backported from the PR #339 review: pace and reset rows on Codex windows.

fn codex_five_hour(points: &[(&str, f64)]) -> Window {
    let midnight = parse_ts("2026-11-25T00:00:00Z").unwrap();
    let resets_at = parse_ts("2026-11-25T14:00:00Z").unwrap();
    let readings: Vec<Reading> = points
        .iter()
        .map(|&(ts, percent)| Reading {
            ts: parse_ts(ts).unwrap(),
            minutes: 300,
            percent,
            resets_at,
        })
        .collect();
    codex::windows(&readings, midnight, &mut Vec::new()).remove(0)
}

// Two readings in one minute still have a trend, and a fractional rise
// isn't rounded into a false fill.
#[test]
fn pace_uses_unrounded_readings() {
    let w = codex_five_hour(&[
        ("2026-11-25T10:00:05Z", 40.0),
        ("2026-11-25T10:00:50Z", 40.4),
    ]);
    assert_eq!(w.readings[0].0, w.readings[1].0, "both in one minute");
    // Rounded, 40% -> 40% looks flat; 0.4 points in 45 s is a steep trend.
    assert!(crate::pace(&w).unwrap() > 100);
    let w = codex_five_hour(&[("2026-11-25T10:00:00Z", 0.4), ("2026-11-25T11:00:00Z", 0.6)]);
    // Rounded, 0% -> 1% in an hour would project 4%; it is 0.2 points an hour.
    assert_eq!(crate::pace(&w), Some(1));
}

// A window that has reset has no current reading, so its row opens
// nothing, even when the day of its last reading has sessions.
#[test]
fn reset_codex_window_row_opens_nothing() {
    let mut data = load();
    let midnight = data.today as i64 * DAY;
    let readings = [Reading {
        ts: midnight + 60,
        minutes: 300,
        percent: 55.0,
        resets_at: midnight + 3600,
    }];
    data.windows = codex::windows(&readings, midnight, &mut Vec::new());
    assert!(data.days.contains(&data.today));
    assert!(has_reset(&data.windows[0], data.now));
    let rows = crate::headroom(&data).2;
    let five = rows
        .iter()
        .find(|r| r.text.starts_with("Codex · 5-hour"))
        .unwrap();
    assert!(five.open.is_none(), "{}", five.text);
}
