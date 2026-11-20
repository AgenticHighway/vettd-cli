//! Readability of Home's chart: bars must show how days differ, and the
//! title must fit its block without cutting a word.

use super::*;
use ratatui::{backend::TestBackend, buffer::Buffer, style::Modifier};

fn day(value: Option<u32>) -> screen::Day {
    screen::Day {
        day: 12,
        value,
        partial: false,
    }
}

/// Renders Home over an explicit corpus, as render.rs does for the default one.
fn home_over(all: &[Session], (w, h): (u16, u16)) -> Buffer {
    let rt = Runtime::default();
    let stack = [Page::new(View::Home)];
    let page = stack.last().unwrap();
    let (title, scope, rows) = content(page, all, false, screen::list_width(w), &rt);
    let active = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.open.map(|open| (i, open)))
        .nth(page.selected);
    let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        screen::draw(
            f,
            &stack,
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
    term.backend().buffer().clone()
}

fn grid(buffer: &Buffer) -> Vec<Vec<char>> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect()
        })
        .collect()
}

fn line(row: &[char]) -> String {
    row.iter().collect()
}

/// The chart's title is drawn on its top border, after the first gap.
fn title_of(row: &[char]) -> String {
    line(row)
        .trim()
        .split('─')
        .next()
        .unwrap()
        .trim()
        .to_string()
}

/// The title is "DAILY SPEND ..." when it fits, else "SPEND ...".
fn title_row(grid: &[Vec<char>]) -> usize {
    grid.iter()
        .position(|row| {
            let text = line(row);
            text.contains("SPEND") || text.contains("REQUESTS")
        })
        .expect("chart title missing")
}

/// The caption row is the first row under the title that starts with day 12.
fn caption_row(grid: &[Vec<char>], title: usize) -> usize {
    (title + 1..grid.len())
        .find(|&y| line(&grid[y]).trim_start().starts_with("12"))
        .expect("day captions missing")
}

/// Filled cells in each of the seven bar columns. Captions start at the
/// first drawn column, and bars are nine cells wide with one cell of gap.
fn bar_heights(grid: &[Vec<char>], title: usize, caption: usize) -> Vec<usize> {
    const BLOCKS: &str = "▁▂▃▄▅▆▇█";
    let x0 = grid[caption].iter().position(|c| *c != ' ').unwrap();
    (0..7)
        .map(|i| {
            let x = x0 + i * 10;
            let digits = format!("{}", 12 + i);
            assert!(
                line(&grid[caption][x..]).starts_with(&digits),
                "day {} caption not at column {x}",
                12 + i
            );
            (title + 1..caption)
                .filter(|&y| BLOCKS.contains(grid[y][x]))
                .count()
        })
        .collect()
}

#[test]
fn stress_bars_show_the_spread_between_days() {
    let grid = grid(&home_over(&fixture::stress_sessions(), (80, 24)));
    let title = title_row(&grid);
    let heights = bar_heights(&grid, title, caption_row(&grid, title));
    let min = *heights.iter().min().unwrap();
    let max = *heights.iter().max().unwrap();
    // A floor under the lowest day keeps the spread visible. Without it the
    // lowest day is a fraction of the peak and draws as a sliver.
    assert!(min * 3 <= max * 2, "bar heights {heights:?}");
}

#[test]
fn stress_title_names_the_floor_in_whole_dollars() {
    let grid = grid(&home_over(&fixture::stress_sessions(), (80, 24)));
    let title = title_of(&grid[title_row(&grid)]);
    let after = title
        .split("from $")
        .nth(1)
        .unwrap_or_else(|| panic!("no axis in {title:?}"));
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    assert!(!digits.is_empty(), "{title:?}");
    assert!(!after[digits.len()..].starts_with('.'), "{title:?}");
}

#[test]
fn stress_title_stays_whole_when_narrow() {
    let all = fixture::stress_sessions();
    let grid = grid(&home_over(&all, (60, 24)));
    let title = title_of(&grid[title_row(&grid)]);
    // Stacked at 60 columns the full wording does not fit, so the short
    // wording is drawn whole rather than clipped mid-phrase.
    assert!(title.contains("all partial ≥"), "{title:?}");
    assert!(title.contains("from $"), "{title:?}");
}

// Too narrow for every note: the notes go, but the unit and the raised
// axis stay, because a raised axis that isn't named misleads.
#[test]
fn stress_title_keeps_unit_and_axis_when_very_narrow() {
    let all = fixture::stress_sessions();
    let (days, _) = screen::daily(&all, false);
    let base = screen::baseline(&days, false);
    let grid = grid(&home_over(&all, (40, 24)));
    let title = title_of(&grid[title_row(&grid)]);
    assert!(title.starts_with("SPEND"), "{title:?}");
    assert!(
        title.ends_with(&format!("from ${}", base / 100)),
        "{title:?}"
    );
    assert!(title.chars().count() + 2 <= 40, "{title:?}");
}

#[test]
fn default_title_keeps_its_full_wording_without_an_axis() {
    let grid = grid(&home_over(&fixture::sessions(), (80, 24)));
    let title = title_of(&grid[title_row(&grid)]);
    assert!(title.starts_with("DAILY SPEND  ·  normal ≈"), "{title:?}");
    assert!(!title.contains("from $"), "{title:?}");
}

#[test]
fn stress_floor_is_whole_dollars_below_the_lowest_day() {
    let all = fixture::stress_sessions();
    let (days, _) = screen::daily(&all, false);
    let base = screen::baseline(&days, false);
    let lowest = days.iter().filter_map(|d| d.value).min().unwrap();
    assert!(base > 0);
    assert_eq!(base % 100, 0);
    assert!(base <= lowest);
}

#[test]
fn default_corpus_has_no_floor_in_dollars() {
    let (days, _) = screen::daily(&fixture::sessions(), false);
    assert_eq!(screen::baseline(&days, false), 0);
}

#[test]
fn equal_days_have_no_floor() {
    let days = [day(Some(500)), day(Some(500)), day(None)];
    assert_eq!(screen::baseline(&days, false), 0);
}

#[test]
fn peak_over_twice_the_lowest_day_has_no_floor() {
    let days = [day(Some(100)), day(Some(300))];
    assert_eq!(screen::baseline(&days, false), 0);
}

#[test]
fn floor_mirrors_the_gap_below_the_lowest_day() {
    // The peak sits 200 above the lowest day, so the floor is 600 cents
    // short of it: whole dollars, rounded down.
    let days = [day(Some(800)), day(Some(1000)), day(None)];
    assert_eq!(screen::baseline(&days, false), 600);
    // 850 less its 200 gap is 650 cents, rounded down to $6.
    let days = [day(Some(850)), day(Some(1050))];
    assert_eq!(screen::baseline(&days, false), 600);
    // At exactly twice the lowest day the floor reaches zero, never below.
    let days = [day(Some(500)), day(Some(1000))];
    assert_eq!(screen::baseline(&days, false), 0);
    let days = [day(Some(u32::MAX - 1)), day(Some(u32::MAX))];
    assert_eq!(screen::baseline(&days, true), u32::MAX - 2);
    // Requests have no cents, so the floor keeps every request.
    let days = [day(Some(8)), day(Some(10))];
    assert_eq!(screen::baseline(&days, true), 6);
}

#[test]
fn no_data_has_no_floor() {
    let days = [day(None), day(None)];
    assert_eq!(screen::baseline(&days, false), 0);
    assert_eq!(screen::baseline(&days, true), 0);
}

/// Renders any stack over an explicit corpus, with each page's own selection
/// and scroll, as the app draws it.
fn stack_over(all: &[Session], stack: &[Page], (w, h): (u16, u16), mono: bool) -> Buffer {
    let rt = Runtime::default();
    let page = stack.last().unwrap();
    let (title, scope, rows) = content(page, all, false, screen::list_width(w), &rt);
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
            all,
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

/// The column where a row's total ends. "$" amounts and "unpriced" are both
/// right-aligned, so every total must end in the same place.
fn total_end(text: &str) -> Option<usize> {
    if let Some(at) = text.find("unpriced") {
        return Some(text[..at].chars().count() + "unpriced".len());
    }
    let chars: Vec<char> = text.chars().collect();
    let at = chars.iter().position(|&c| c == '$')?;
    let digits = chars[at + 1..]
        .iter()
        .take_while(|c| c.is_ascii_digit() || **c == '.' || **c == ',')
        .count();
    Some(at + 1 + digits)
}

fn is_session(row: &Row, id: usize) -> bool {
    matches!(row.open, Some((View::Session(i), _)) if i == id)
}

#[test]
fn shorten_leaves_text_that_fits_unchanged() {
    assert_eq!(shorten("api", 3), "api");
    assert_eq!(shorten("api", 10), "api");
    assert_eq!(shorten("", 0), "");
}

#[test]
fn shorten_keeps_both_ends_and_fills_exactly_the_width() {
    // Odd: 5 is 2 + "…" + 2.
    assert_eq!(shorten("abcdefghij", 5), "ab…ij");
    // Even: 6 is 3 + "…" + 2, the extra character going to the start.
    assert_eq!(shorten("abcdefghij", 6), "abc…ij");
    for max in 1..10 {
        assert_eq!(shorten("abcdefghij", max).chars().count(), max);
    }
}

#[test]
fn shorten_handles_the_smallest_widths() {
    assert_eq!(shorten("abcdef", 0), "");
    assert_eq!(shorten("abcdef", 1), "…");
    assert_eq!(shorten("abcdef", 2), "a…");
}

#[test]
fn shorten_counts_characters_not_bytes() {
    // Each "≥" is three bytes; cutting by bytes would split one.
    assert_eq!(shorten("≥≥≥≥≥≥", 4), "≥≥…≥");
    assert_eq!(shorten("≥≥≥≥≥≥", 6), "≥≥≥≥≥≥");
}

#[test]
fn shorten_treats_an_existing_ellipsis_as_ordinary_text() {
    // Only the length decides; the "…" in the input is kept as a character.
    assert_eq!(shorten("a…b…c…d", 5), "a………d");
}

// The Day column header must match the rows' fixed layout exactly at the
// width the default corpus always had, so the default render is unchanged.
#[test]
fn day_header_keeps_the_narrow_column_for_default_corpus() {
    assert_eq!(
        day_header(16, &fixture::sessions(), 80),
        "SESSION          WS   AGENT      TOTAL"
    );
}

// Each stress day's rows share one workspace column, so the totals line up.
#[test]
fn stress_day_totals_end_in_one_column_inside_the_list() {
    let all = fixture::stress_sessions();
    let stack = [Page::new(View::Home), Page::new(View::Day(16))];
    for (w, h) in [(80u16, 24u16), (60, 24)] {
        let buffer = stack_over(&all, &stack, (w, h), false);
        let lines: Vec<String> = grid(&buffer).iter().map(|row| line(row)).collect();
        // Header is row 4, items run to the Enter hint on row h - 2.
        let bound = usize::from(screen::list_width(w));
        let ends: Vec<usize> = lines[5..usize::from(h) - 2]
            .iter()
            .map(|row| row.chars().take(bound).collect::<String>())
            .filter(|left| left.contains('$') || left.contains("unpriced"))
            .map(|left| total_end(&left).expect("total without a value"))
            .collect();
        assert!(!ends.is_empty(), "no session rows at {w}x{h}");
        assert!(
            ends.iter().all(|&end| end == ends[0]),
            "{ends:?} at {w}x{h}"
        );
        // `ends` is exclusive: the last character sits at ends[0] - 1.
        assert!(
            ends[0] <= bound,
            "total at {} beyond {bound} at {w}x{h}",
            ends[0]
        );
    }
}

// The 94-char workspace cannot fit the 11-column name at 80 wide, so it is
// shortened to exactly the column; at 60 wide it keeps its start and end.
#[test]
fn long_workspace_is_shortened_to_the_day_column() {
    let all = fixture::stress_sessions();
    let id = all
        .iter()
        .position(|s| s.day == 16 && s.workspace.chars().count() == 94)
        .expect("no 94-char workspace on day 16");
    let rt = Runtime::default();
    for w in [80u16, 60] {
        let width = screen::list_width(w);
        let rows = content(&Page::new(View::Day(16)), &all, false, width, &rt).2;
        let column = day_ws_width(16, &all, width);
        let row = rows.iter().find(|r| is_session(r, id)).unwrap();
        let short = shorten(all[id].workspace, column);
        assert_eq!(short.chars().count(), column);
        assert!(row.text.contains(&short), "{:?} lacks {short:?}", row.text);
    }
    let width = screen::list_width(60);
    let rows = content(&Page::new(View::Day(16)), &all, false, width, &rt).2;
    let row = rows.iter().find(|r| is_session(r, id)).unwrap();
    assert!(row.text.contains("platform/"), "{:?}", row.text);
    assert!(row.text.contains('…'), "{:?}", row.text);
    assert!(row.text.contains("-stress"), "{:?}", row.text);
}

// A long scope must keep the agent and cost chips, shortening the workspace
// chip rather than dropping every chip after it.
#[test]
fn long_scope_keeps_agent_and_cost_chips() {
    let all = fixture::stress_sessions();
    let id = all
        .iter()
        .position(|s| s.workspace.chars().count() == 94)
        .unwrap();
    let s = &all[id];
    for mono in [false, true] {
        let stack = [Page::new(View::Home), Page::new(View::Session(id))];
        let buffer = stack_over(&all, &stack, (80, 24), mono);
        let chips = line(&grid(&buffer)[3]);
        assert!(chips.contains(&format!("Nov {} UTC", s.day)), "{chips:?}");
        assert!(chips.contains('…'), "{chips:?}");
        assert!(chips.contains(s.agent), "{chips:?}");
        assert!(chips.contains(&cost_label(s)), "{chips:?}");
        assert!(!chips.trim_end().ends_with('…'), "{chips:?}");
    }
}

// A 63-char model must still end its row with the cost, in the cost list
// and in the token detail, at both widths.
#[test]
fn long_model_session_rows_end_with_their_cost() {
    let all = fixture::stress_sessions();
    let id = all
        .iter()
        .position(|s| s.requests.iter().any(|r| r.model.chars().count() == 63))
        .unwrap();
    for (w, h) in [(80u16, 24u16), (60, 24)] {
        let stack = [Page::new(View::Home), Page::new(View::Session(id))];
        let buffer = stack_over(&all, &stack, (w, h), false);
        let mut seen = 0;
        for row in grid(&buffer).iter().map(|r| line(r)) {
            let left = row.split('│').next().unwrap().trim_end();
            if left.contains("synthetic-") {
                seen += 1;
                assert_eq!(
                    total_end(left),
                    Some(left.chars().count()),
                    "{left:?} at {w}x{h}"
                );
            }
        }
        assert!(seen > 0, "no long-model row at {w}x{h}");
    }
}

// The preview pane explains the selected session, so its cost row must end
// with the cost too.
#[test]
fn long_model_session_preview_ends_with_its_cost() {
    let all = fixture::stress_sessions();
    let id = all
        .iter()
        .position(|s| s.requests.iter().any(|r| r.model.chars().count() == 63))
        .unwrap();
    let day = all[id].day;
    let rt = Runtime::default();
    let rows = content(
        &Page::new(View::Day(day)),
        &all,
        false,
        screen::list_width(80),
        &rt,
    )
    .2;
    let at = rows.iter().position(|r| is_session(r, id)).unwrap();
    let mut page = Page::new(View::Day(day));
    page.selected = rows[..at].iter().filter(|r| r.open.is_some()).count();
    page.top = at;
    let stack = [Page::new(View::Home), page];
    let buffer = stack_over(&all, &stack, (80, 24), false);
    let mut seen = 0;
    for row in grid(&buffer).iter().map(|r| line(r)) {
        if let Some(pane) = row.split('│').nth(1) {
            let pane = pane.trim_end();
            if pane.contains("synth") {
                seen += 1;
                assert_eq!(total_end(pane), Some(pane.chars().count()), "{pane:?}");
            }
        }
    }
    assert!(seen > 0, "preview lacks the long model");
}

// The smallest supported terminal has no room for the fitting rules, but
// the shortening arithmetic must still never underflow or panic there.
#[test]
fn stress_views_draw_at_the_smallest_size() {
    let all = fixture::stress_sessions();
    let long = all
        .iter()
        .position(|s| s.workspace.chars().count() > 80)
        .unwrap();
    let home = || Page::new(View::Home);
    for day in 12..=18 {
        let _ = stack_over(&all, &[home(), Page::new(View::Day(day))], (38, 10), false);
    }
    for view in [View::Session(long), View::Tokens(long)] {
        let _ = stack_over(&all, &[home(), Page::new(view)], (38, 10), true);
    }
}

/// The selected row must stay visible without colour: its marker cell is
/// drawn in reverse video, the only cue NO_COLOR leaves.
fn assert_selection_reversed(buffer: &Buffer) {
    let cells = grid(buffer);
    let y = cells
        .iter()
        .position(|row| row.contains(&'▸'))
        .unwrap_or_else(|| panic!("no selection marker"));
    let x = cells[y].iter().position(|c| *c == '▸').unwrap();
    let cell = &buffer[(x as u16, y as u16)];
    assert!(
        cell.modifier.contains(Modifier::REVERSED),
        "selected row {:?} is not reversed",
        line(&cells[y])
    );
}

// Under NO_COLOR the chart keeps its whole floor title, and the selected row
// is still marked by reverse video.
#[test]
fn no_color_home_title_names_the_floor_in_whole_dollars() {
    let all = fixture::stress_sessions();
    let buffer = stack_over(&all, &[Page::new(View::Home)], (80, 24), true);
    let grid = grid(&buffer);
    let title = title_of(&grid[title_row(&grid)]);
    let after = title
        .split("from $")
        .nth(1)
        .unwrap_or_else(|| panic!("no axis in {title:?}"));
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    assert!(!digits.is_empty(), "{title:?}");
    assert!(!after[digits.len()..].starts_with('.'), "{title:?}");
    assert_selection_reversed(&buffer);
}

// Under NO_COLOR the day's totals still end in one column, and the selected
// session is reverse video.
#[test]
fn no_color_day_totals_end_in_one_column() {
    let all = fixture::stress_sessions();
    let stack = [Page::new(View::Home), Page::new(View::Day(16))];
    let (w, h) = (80u16, 24u16);
    let buffer = stack_over(&all, &stack, (w, h), true);
    let lines: Vec<String> = grid(&buffer).iter().map(|row| line(row)).collect();
    let bound = usize::from(screen::list_width(w));
    let ends: Vec<usize> = lines[5..usize::from(h) - 2]
        .iter()
        .map(|row| row.chars().take(bound).collect::<String>())
        .filter(|left| left.contains('$') || left.contains("unpriced"))
        .map(|left| total_end(&left).expect("total without a value"))
        .collect();
    assert!(!ends.is_empty(), "no session rows");
    assert!(ends.iter().all(|&end| end == ends[0]), "{ends:?}");
    assert_selection_reversed(&buffer);
}

// Under NO_COLOR a long workspace's session scope keeps its date, agent and
// cost as bracketed chips, and shortens only the workspace chip.
#[test]
fn no_color_long_session_chips_stay_in_bracket_form() {
    let all = fixture::stress_sessions();
    let id = all
        .iter()
        .position(|s| s.workspace.chars().count() == 94)
        .unwrap();
    let s = &all[id];
    let stack = [Page::new(View::Home), Page::new(View::Session(id))];
    let buffer = stack_over(&all, &stack, (80, 24), true);
    let chips = line(&grid(&buffer)[3]);
    assert!(chips.contains(&format!("[Nov {} UTC]", s.day)), "{chips:?}");
    assert!(chips.contains('…'), "{chips:?}");
    assert!(chips.contains(&format!("[{}]", s.agent)), "{chips:?}");
    assert!(chips.contains(&format!("[{}]", cost_label(s))), "{chips:?}");
    assert_selection_reversed(&buffer);
}
