//! Keys that jump to a place, so a destination takes few presses.
use super::*;
use KeyCode::{End, Home};

/// The "n/total" counter at the right of the hint row.
fn counter(u: &mut User) -> String {
    let screen = u.screen();
    let hint = screen.lines().rev().nth(1).unwrap();
    hint.split_whitespace().last().unwrap().to_string()
}

fn day_rows(all: &[Session], day: u32) -> Vec<Row> {
    let rt = Runtime::default();
    content(
        &Page::new(View::Day(day)),
        all,
        false,
        screen::list_width(80),
        &rt,
    )
    .2
}

// A 280-row stress day: End reaches the last session and g comes back to
// the first, one press each instead of a PgDn per screen.
#[test]
fn end_and_g_reach_both_ends_of_a_long_day() {
    let mut u = User::stress((80, 24));
    u.app.stack.push(Page::new(View::Day(16)));
    let rows = day_rows(&u.all, 16);
    let total = rows.iter().filter(|r| r.open.is_some()).count();
    let head = |r: &Row| r.text.chars().take(10).collect::<String>();
    u.press(End);
    assert_eq!(counter(&mut u), format!("{total}/{total}"));
    let last = rows.iter().rev().find(|r| r.open.is_some()).unwrap();
    assert!(u.selected().starts_with(&format!("▸ {}", head(last))));
    u.sees(rows.last().unwrap().text.trim_end());
    u.press(KeyCode::Char('g'));
    assert_eq!(counter(&mut u), format!("1/{total}"));
    let first = rows.iter().find(|r| r.open.is_some()).unwrap();
    assert!(u.selected().starts_with(&format!("▸ {}", head(first))));
    u.press(KeyCode::Char('G'));
    assert_eq!(counter(&mut u), format!("{total}/{total}"));
    u.press(Home);
    assert_eq!(counter(&mut u), format!("1/{total}"));
}

// Home's findings take the same keys.
#[test]
fn end_on_home_selects_the_last_finding() {
    let mut u = User::at((80, 24));
    u.press(End);
    assert!(u.selected().starts_with("▸ Every day and its sessions"));
    u.press(Home);
    assert!(u.selected().starts_with("▸ Claude 5-hour"));
}

// Find is a text field: g is a letter there, not a jump.
#[test]
fn g_in_find_is_typed() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('/')).typed("g");
    u.sees("Search: g_");
    u.press(KeyCode::Char('G')).sees("Search: gG_");
}

/// The chart caption of the chosen bar, whichever form fits: "13▸$1.20",
/// "13▸·" or "▸13".
fn chosen_caption(u: &mut User, day: u32) -> bool {
    let screen = u.screen();
    screen.contains(&format!("{day}▸")) || screen.contains(&format!("▸{day}"))
}

// Home → any day in three presses: → picks Nov 12, → Nov 13, Enter opens
// it. Before this, Nov 13 took seven presses through Week.
#[test]
fn arrows_choose_a_day_on_home_and_enter_opens_it() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Right);
    assert!(chosen_caption(&mut u, 12), "{}", u.screen());
    u.press(KeyCode::Right).sees("Enter See Nov 13 sessions");
    assert!(chosen_caption(&mut u, 13));
    u.press(Enter);
    assert_eq!(u.crumb(), "Home › Nov 13");
    // Nov 13 has no input: it is still a day you can open.
    u.sees("No session input.");
    u.press(Esc);
    assert_eq!(u.crumb(), "Home");
    assert!(chosen_caption(&mut u, 13), "{}", u.screen());
}

// The first ← picks the last day, and selection stops at the ends.
#[test]
fn bar_selection_starts_at_the_ends_and_stops_there() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Left);
    assert!(chosen_caption(&mut u, 18));
    u.press(KeyCode::Right);
    assert!(chosen_caption(&mut u, 18));
    assert!(!chosen_caption(&mut u, 17));
    u.times(KeyCode::Left, 10);
    assert!(chosen_caption(&mut u, 12));
}

// Choosing a bar hides the findings' marker and counter; ↓ goes back to
// the finding that was selected before, not the next one.
#[test]
fn vertical_key_returns_to_the_same_finding() {
    let mut u = User::at((80, 24));
    u.press(Down);
    assert!(u.selected().starts_with("▸ Nov 16"));
    u.press(KeyCode::Left);
    u.doesnt_see("▸ Nov 16").doesnt_see("2/5");
    u.press(Down);
    assert!(u.selected().starts_with("▸ Nov 16"), "{}", u.screen());
    assert!(!chosen_caption(&mut u, 18));
    u.press(Enter);
    assert_eq!(u.crumb(), "Home › Nov 16");
}

// Without colour, the chosen bar still shows by its marker.
#[test]
fn chosen_bar_shows_without_colour() {
    let mut u = User::at((80, 24));
    u.mono = true;
    u.press(KeyCode::Right);
    assert!(chosen_caption(&mut u, 12), "{}", u.screen());
}

// Every bar can be chosen on both corpora, at both widths and without
// colour, and the
// caption keeps its marker whatever form it takes.
#[test]
fn every_bar_can_be_chosen_at_both_widths() {
    for size in [(80, 24), (60, 24)] {
        for (stress, mono) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut u = if stress {
                User::stress(size)
            } else {
                User::at(size)
            };
            u.mono = mono;
            for day in 12..=18 {
                u.press(KeyCode::Right);
                assert!(
                    chosen_caption(&mut u, day),
                    "{size:?} stress={stress} mono={mono} day {day}:\n{}",
                    u.screen()
                );
            }
        }
    }
}

// 1–7 open a day from Home and Week directly.
#[test]
fn number_keys_open_a_day() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('5'));
    assert_eq!(u.crumb(), "Home › Nov 16");
    u.sees("Index rebuild");
    u.press(Esc);
    assert_eq!(u.crumb(), "Home");
    u.times(Down, 4).press(Enter).press(KeyCode::Char('1'));
    assert_eq!(u.crumb(), "Home › Daily activity › Nov 12");
}

// Elsewhere the digits do nothing, and Find types them.
#[test]
fn number_keys_do_nothing_elsewhere_and_type_in_find() {
    let mut u = User::at((80, 24));
    u.press(Down).press(Enter).press(Enter);
    let crumb = u.crumb();
    u.press(KeyCode::Char('1'));
    assert_eq!(u.crumb(), crumb);
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('/')).typed("16").sees("Search: 16_");
    assert_eq!(u.crumb(), "Home › Lookup");
}

// Find reaches a session by name: one search instead of a day and a scroll.
#[test]
fn find_opens_a_session_by_name() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('/')).typed("index");
    // Index rebuild runs daily: six sessions, in day order.
    u.sees("Find a day, session or component")
        .sees("── Sessions (6)");
    assert!(
        u.selected().starts_with("▸ Nov 12 Index rebuild"),
        "{}",
        u.screen()
    );
    u.times(Down, 3);
    assert!(
        u.selected().starts_with("▸ Nov 16 Index rebuild"),
        "{}",
        u.screen()
    );
    u.press(Enter);
    assert!(u.crumb().ends_with("Index rebuild"), "{}", u.crumb());
    u.sees("$8.20");
    u.press(Esc);
    assert!(u.selected().starts_with("▸ Nov 16 Index rebuild"));
    u.press(Enter);
    assert!(u.crumb().ends_with("Index rebuild"), "{}", u.crumb());
    u.sees("Fresh input");
}

// A day is found by its label and opens to its sessions.
#[test]
fn find_opens_a_day() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('/')).typed("nov 16");
    u.sees("── Days (1)");
    assert!(u.selected().starts_with("▸ Nov 16  "), "{}", u.screen());
    u.press(Enter).sees("Nov 16 sessions");
}

// A workspace finds every session in it, counted under the divider.
#[test]
fn find_lists_a_workspace_and_returns_to_the_same_result() {
    let mut u = User::at((80, 24));
    let n = u
        .all
        .iter()
        .filter(|s| s.workspace.contains("api") || s.name.to_lowercase().contains("api"))
        .count();
    assert!(n > 1, "fixture has {n} api sessions");
    u.press(KeyCode::Char('/')).typed("api");
    u.sees(&format!("── Sessions ({n})"));
    u.press(Down);
    let row = u.selected();
    u.press(Enter).press(Esc);
    u.sees("Search: api_");
    assert_eq!(u.selected(), row);
}

// One letter at stress scale matches about everything: it must still draw,
// with counts on every divider, at both widths.
#[test]
fn find_with_many_stress_matches_draws() {
    for size in [(80, 24), (60, 24)] {
        let mut u = User::stress(size);
        u.press(KeyCode::Char('/')).typed("e");
        u.sees("── Sessions (");
        u.press(KeyCode::End);
        u.selected();
    }
}

/// The footer row: the last line of the screen.
fn footer(u: &mut User) -> String {
    u.screen().lines().last().unwrap().trim_end().to_string()
}

// The new keys are named where they work, so nobody needs help to find
// them: Home names the day arrows, Week the number keys.
#[test]
fn footers_name_the_day_keys_where_they_work() {
    let mut u = User::at((80, 24));
    assert!(footer(&mut u).contains("←→ Day"), "{}", footer(&mut u));
    u.times(Down, 4).press(Enter);
    assert!(footer(&mut u).contains("1–7 Day"), "{}", footer(&mut u));
}

// Narrow footers keep their earlier items whole.
#[test]
fn narrow_footers_are_unchanged() {
    let mut u = User::at((60, 24));
    assert_eq!(footer(&mut u), " c USD/req   i Inventory   ? Keys");
    u.times(Down, 4).press(Enter);
    assert_eq!(footer(&mut u), " i Inventory   / Find   Esc Back");
}

// Help lists every new key and what it does.
#[test]
fn help_lists_the_new_keys() {
    let mut u = User::at((80, 40));
    u.press(KeyCode::Char('?'))
        .sees("Home/End First or last row (g / G too)")
        .sees("← / →   Home: choose a day")
        .sees("1–7     Home, Week: open that day")
        .sees("Find a day, session or component");
}
