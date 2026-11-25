//! Render tests: what each screen shows at real terminal sizes.

use super::*;
use crate::data::Data;
use crate::fixture::Window;
use ratatui::{backend::TestBackend, buffer::Buffer, style::Modifier};
use std::borrow::Cow;

fn render(stack: &[Page], (w, h): (u16, u16), mono: bool) -> Buffer {
    let all = Data::fixture();
    let rt = Runtime::default();
    let page = stack.last().unwrap();
    let (title, scope, rows) = content(page, &all, false, screen::list_width(w), &rt);
    let active = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.open.map(|open| (i, open)))
        .nth(page.selected);
    let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        screen::draw(
            f,
            stack,
            &all,
            false,
            (&title, &scope),
            &rows,
            active,
            &rt,
            mono,
        )
    })
    .unwrap();
    term.backend().buffer().clone()
}

fn screen_text(buffer: &Buffer) -> String {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Home is the first thing a developer sees. At 80x24 it must show what
// was used and where it went before any finding, with the findings still
// reachable on the same screen.
#[test]
fn home_dashboard_shows_usage_before_findings_at_80x24() {
    let text = screen_text(&render(&[Page::new(View::Home)], (80, 24), false));
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing:\n{text}"))
    };
    assert!(at("12 sessions") < at("WHERE TOKENS WENT"));
    assert!(text.contains("$12.36 priced"), "{text}");
    assert!(at("DAILY SPEND") < at("WORTH A LOOK"));
    // Every day is labelled with its value; the day without input says so.
    for caption in ["12 $0.80", "13 ·", "16 $8.52"] {
        assert!(
            text.contains(caption),
            "{caption} missing:
{text}"
        );
    }
    // Hooks are listed so people learn they cost nothing, not omitted.
    assert!(at("Hooks") > at("File reads"));
    assert!(
        text.contains("▸ Claude 5-hour: 62%, full by 13:22"),
        "{text}"
    );
    assert!(text.contains("Nov 16: 10× a normal day"), "{text}");
    assert!(text.contains("review-kit: unknown load in Codex"), "{text}");
}

// At 60 columns the dashboard stacks instead of truncating its findings.
#[test]
fn home_dashboard_stacks_at_60x24_and_keeps_selection() {
    let text = screen_text(&render(&[Page::new(View::Home)], (60, 24), false));
    assert!(
        text.contains("$12.36 priced"),
        "answer cut:
{text}"
    );
    assert!(text.contains("WHERE TOKENS WENT"), "{text}");
    assert!(text.contains("WORTH A LOOK"), "{text}");
    assert!(
        text.contains("▸ Claude 5-hour: 62%, full by 13:22"),
        "{text}"
    );
    assert!(text.contains("Nov 16: 10× a normal day"), "{text}");
}

// Shares on Home must add to 100% or the dashboard misstates the week.
#[test]
fn token_source_shares_sum_to_whole() {
    let total: u32 = fixture::SOURCE_SHARES.iter().map(|s| s.1).sum();
    assert_eq!(total, 10_000);
}

// Under NO_COLOR, focus must still be visible without colour.
#[test]
fn no_color_marks_selection_with_reverse_video_only() {
    let buffer = render(&[Page::new(View::Home)], (80, 24), true);
    let mut reversed = false;
    for y in 0..24 {
        for x in 0..80 {
            let cell = &buffer[(x, y)];
            assert_eq!(cell.fg, ratatui::style::Color::Reset, "colour at {x},{y}");
            reversed |= cell.modifier.contains(Modifier::REVERSED);
        }
    }
    assert!(reversed);
}

// Esc must always be able to find Home, so a deep breadcrumb keeps its
// root and current level even when it no longer fits.
#[test]
fn deep_breadcrumb_keeps_home_and_current_level() {
    let stack = [
        Page::new(View::Home),
        Page::new(View::Day(16)),
        Page::new(View::Session(6)),
        Page::new(View::Component(0, Scope::WEEK)),
        Page::new(View::Placement(1)),
    ];
    let text = screen_text(&render(&stack, (40, 24), false));
    let crumb = text.lines().nth(1).unwrap();
    assert!(crumb.trim_start().starts_with("Home ›"), "{crumb}");
    assert!(crumb.trim_end().ends_with("Copy"), "{crumb}");
}

#[test]
fn too_small_terminal_says_what_it_needs() {
    let text = screen_text(&render(&[Page::new(View::Home)], (30, 8), false));
    assert!(text.contains("38×10"), "{text}");
}

// The selected finding is what Enter opens, so it must be on screen at
// every size Lens accepts, not just at 80x24.
#[test]
fn selected_home_finding_is_drawn_at_every_size() {
    let all = Data::fixture();
    let rows = content(&Page::new(View::Home), &all, false, 80, &Runtime::default()).2;
    for size in [(80, 24), (60, 24), (80, 12), (50, 16), (38, 10)] {
        for (i, row) in rows.iter().enumerate() {
            let mut page = Page::new(View::Home);
            page.selected = i;
            let text = screen_text(&render(&[page], size, false));
            let start: String = row.text.chars().take(12).collect();
            assert!(
                text.contains(&format!("▸ {start}")),
                "{size:?} row {i} hidden:\n{text}"
            );
        }
    }
}

// Nov 18's input is incomplete and its model unpriced. Its total is a
// floor, so the chart must not show it as a complete cheap day, and it
// must not pull down what "normal" means.
#[test]
fn partial_day_reads_as_floor_and_stays_out_of_normal() {
    let all = Data::fixture();
    let (days, normal) = screen::daily(&all, false);
    let nov18 = days.iter().find(|d| d.day == 18).unwrap();
    assert!(nov18.partial);
    let mut complete: Vec<u32> = days
        .iter()
        .filter(|d| !d.partial)
        .filter_map(|d| d.value)
        .collect();
    complete.sort_unstable();
    assert_eq!(normal, complete[complete.len() / 2]);
    let text = screen_text(&render(&[Page::new(View::Home)], (80, 24), false));
    assert!(text.contains("18 ≥$0.54"), "{text}");
    assert!(!text.contains("18 $0.54"), "{text}");
}

// The answer's numbers come last, so a narrow terminal must wrap the
// line rather than cut off the spend.
#[test]
fn answer_wraps_instead_of_cutting_its_numbers() {
    for size in [(50, 24), (38, 10)] {
        let text = screen_text(&render(&[Page::new(View::Home)], size, false));
        assert!(text.contains("$12.36"), "{size:?}:\n{text}");
    }
}

fn window(agent: &str, name: &str) -> Window {
    fixture::WINDOWS
        .iter()
        .find(|w| w.agent == agent && w.name == name)
        .unwrap()
        .clone()
}

// One reading has no pace. Showing a projection from it would invent a
// trend, so the window shows "—" instead of a number.
#[test]
fn pace_needs_two_readings() {
    let codex = &window("Codex", "5-hour");
    assert_eq!(codex.readings.len(), 1);
    assert_eq!(pace(codex), None);
    assert_eq!(full_by(codex, 18), None);
    let row = headroom(&Data::fixture())
        .2
        .into_iter()
        .find(|r| r.text.starts_with("Codex · 5-hour"))
        .unwrap();
    assert!(row.text.contains(" —  "), "{}", row.text);
}

// Pace is a straight line through the first and last readings. Claude's
// 5-hour window goes 9% -> 62% over 135 minutes, so it reaches 100% about
// 97 minutes after the last reading, before the 14:00 reset.
#[test]
fn pace_extrapolates_to_reset_and_names_when_it_fills() {
    let claude = &window("Claude", "5-hour");
    assert_eq!(pace(claude), Some(115));
    assert_eq!(full_by(claude, 18).as_deref(), Some("13:22"));
    // A window on pace to stay under its limit has no fill time.
    let weekly = &window("Claude", "weekly");
    assert!(pace(weekly).unwrap() < 100);
    assert_eq!(full_by(weekly, 18), None);
}

// The answer names the window that will stop work first, and every row
// says where its number came from, because the two sources differ.
#[test]
fn headroom_answers_with_the_filling_window_and_cites_sources() {
    let (answer, _, rows) = headroom(&Data::fixture());
    assert_eq!(
        answer,
        "Claude 5-hour window is 62% used. At this pace it's full by 13:22."
    );
    let windows: Vec<_> = rows.iter().filter(|r| r.open.is_some()).collect();
    assert_eq!(windows.len(), fixture::WINDOWS.len());
    for row in windows {
        let source = if row.text.starts_with("Claude") {
            "relay"
        } else {
            "logs"
        };
        assert!(row.text.ends_with(source), "{}", row.text);
    }
}

// Home's first finding is the window that can stop work, and Enter on it
// opens Plan headroom.
#[test]
fn home_leads_with_the_filling_window() {
    let rows = content(
        &Page::new(View::Home),
        &Data::fixture(),
        false,
        80,
        &Runtime::default(),
    )
    .2;
    assert!(
        rows[0].text.starts_with("Claude 5-hour"),
        "{}",
        rows[0].text
    );
    assert!(matches!(rows[0].open, Some((View::Headroom, _))));
}

// Every column, SOURCE included, must survive an 80-column terminal.
#[test]
fn headroom_rows_fit_80_columns() {
    let text = screen_text(&render(
        &[Page::new(View::Home), Page::new(View::Headroom)],
        (80, 24),
        false,
    ));
    for needle in ["~115%", "Nov 23 09:00 relay", "15:00        logs"] {
        assert!(text.contains(needle), "{needle} missing:\n{text}");
    }
}

// The answer must name the window that stops work first. A weekly window
// projected at 155% that fills in two days matters less than a 5-hour window
// at 115% that fills this afternoon, so the earliest fill time wins, not the
// largest projection.
#[test]
fn soonest_window_wins_over_the_largest_projection() {
    let claude = &window("Claude", "5-hour");
    let fast_weekly = Window {
        agent: "Codex",
        name: "weekly",
        source: "logs",
        minutes: 7 * 1440,
        start: Some(-3 * 1440 + 22 * 60 + 10),
        readings: Cow::Borrowed(&[(1000, 20), (3695, 60)]),
        note: None,
        precise: Vec::new(),
    };
    assert!(pace(&fast_weekly) > pace(claude));
    let windows = [fast_weekly, claude.clone()];
    let (w, used) = soonest(&windows, fixture::NOW).unwrap();
    assert_eq!((w.agent, w.name, used), ("Claude", "5-hour", 62));
}

// Every number Home and Plan headroom show must be rebuildable from the
// export, or scripts and the TUI can disagree.
#[test]
fn export_carries_home_and_headroom_data() {
    let json = export(&Data::fixture());
    let bp: u64 = json["token_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["basis_points"].as_u64().unwrap())
        .sum();
    assert_eq!(bp, 10_000);
    assert_eq!(json["windows"].as_array().unwrap().len(), 4);
    assert_eq!(json["limit_hits"], 3);
    assert_eq!(
        json["claude_weekly_ends_percent"].as_array().unwrap().len(),
        4
    );
    assert_eq!(json["now_minutes_from_nov_18"], fixture::NOW);
    assert_eq!(json["source"], "fixture");
    for local_only in [
        "now",
        "synthetic_sections",
        "estimated_sections",
        "limit_hits_by_agent",
    ] {
        assert!(json.get(local_only).is_none(), "{local_only}");
    }
}

// A wrapped answer keeps its indent under the marker instead of starting
// at the screen edge.
#[test]
fn wrapped_answer_keeps_its_indent() {
    let text = screen_text(&render(
        &[Page::new(View::Home), Page::new(View::Headroom)],
        (60, 24),
        false,
    ));
    let second = text.lines().nth(3).unwrap();
    assert!(
        second.starts_with("   ") && second.contains("13:22"),
        "{text}"
    );
}
