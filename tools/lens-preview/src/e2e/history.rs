//! History: every session found, newest first, reached with `h` and left
//! with Esc, filtered by typing, opening a session over all its requests.

use super::*;
use std::sync::mpsc;

const H: KeyCode = KeyCode::Char('h');

/// The rows History lists, as content builds them.
fn history_rows(u: &User) -> Vec<Row> {
    content(&Page::new(View::History), &u.all, false, 48, &u.app.rt).2
}

// History is a side trip: Esc must land on the same finding, or a person
// checking an old session loses their place on Home.
#[test]
fn home_h_then_esc_keeps_the_second_finding() {
    let mut u = User::at((80, 24));
    u.press(Down);
    let second = u.selected();
    u.press(H);
    assert_eq!(u.crumb(), "Home › History");
    u.press(Esc);
    assert_eq!(u.crumb(), "Home");
    assert_eq!(u.selected(), second);
}

#[test]
fn day_h_then_esc_keeps_the_third_session() {
    let mut u = User::stress((80, 24));
    u.press(KeyCode::Char('1')).times(Down, 2);
    let third = u.selected();
    u.press(H).sees("1991 sessions, newest first");
    u.press(Esc);
    assert_eq!(u.selected(), third);
}

// Find types letters, so "h" is a search, not a jump away from it.
#[test]
fn h_is_typed_in_find() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('/')).press(H);
    u.sees("Search: h_");
    assert_eq!(u.crumb(), "Home › Lookup");
}

// History opens from the four screens a person checks spend on, and from
// nowhere else, so `h` never shadows another screen's key.
#[test]
fn h_opens_history_from_home_week_day_and_session() {
    let mut u = User::at((80, 24));
    u.press(H);
    assert_eq!(u.crumb(), "Home › History");
    u.press(Esc).times(Down, 4).press(Enter).press(H);
    assert_eq!(u.crumb(), "Home › Daily activity › History");
    u.press(Esc).press(Enter).press(H);
    assert_eq!(u.crumb(), "Home › Daily activity › Nov 12 › History");
    u.press(Esc).press(Enter).press(H);
    assert!(u.crumb().ends_with("› History"), "{}", u.crumb());
    // Inventory is not one of them.
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('i')).press(H);
    assert_eq!(u.crumb(), "Home › Components");
}

// A person returning to a filtered list must find the row they opened, or
// every look at a session costs them the search.
#[test]
fn filter_open_third_row_and_back_keeps_filter_and_row() {
    let mut u = User::stress((80, 24));
    u.press(H).typed("nov 17");
    u.sees("Filter: nov 17_");
    u.times(Down, 2);
    let third = u.selected();
    u.press(Enter);
    assert!(u.crumb().starts_with("Home › History › "), "{}", u.crumb());
    u.sees("All requests");
    u.press(Esc);
    u.sees("Filter: nov 17_");
    assert_eq!(u.selected(), third);
}

// Filtering uses Find's matcher, so "Nov 17" and "nov 17" find the same
// sessions, and the title says how many matched.
#[test]
fn filter_by_month_matches_the_date_label_and_counts() {
    let mut u = User::local((80, 24));
    u.app.rt.history = Past::Ready(crate::local::test_history());
    u.press(H).sees("2 sessions, newest first").typed("aug");
    u.sees("1 of 2 sessions match \"aug\"").sees("Aug 27");
    u.doesnt_see("Nov 18");
    u.press(KeyCode::Backspace).press(KeyCode::Backspace);
    u.sees("2 of 2 sessions match \"a\"");
}

#[test]
fn filter_with_no_match_says_so() {
    let mut u = User::at((80, 24));
    u.press(H).typed("zzz");
    u.sees("0 of 12 sessions match \"zzz\"")
        .sees("No session matches \"zzz\"");
    assert!(u.app.frame(&u.all, u.size).active.is_none());
}

// The session view from History is the whole session. The dashboard's Day
// shows the 2 requests in the period; History must show all 6, or its cost
// would be understated.
#[test]
fn session_spanning_the_period_start_shows_all_requests_from_history() {
    let mut u = User::local((80, 24));
    u.app.rt.history = Past::Ready(crate::local::test_history());
    u.press(H);
    assert!(u.selected().contains("Nov 18"), "{}", u.selected());
    assert!(u.selected().contains("    6 "), "{}", u.selected());
    u.press(Enter)
        .sees("All requests")
        .sees("Started Nov 18 UTC");
    u.sees("(6 requests)");
}

// Local dollars with no rate are unknown, never $0.00.
#[test]
fn unpriced_local_session_reads_unpriced() {
    let mut u = User::local((80, 24));
    u.app.rt.history = Past::Ready(crate::local::test_history());
    u.press(H).typed("aug");
    assert!(u.selected().contains("1 unpriced"), "{}", u.selected());
}

// Startup must not wait for History, so it may be opened mid-read. It has
// to say so, list nothing it hasn't finished counting, and then show the
// list by itself: the loop polls, nobody presses a key.
#[test]
fn history_opened_while_reading_fills_in_without_a_key() {
    let mut u = User::local((80, 24));
    let (tx, rx) = mpsc::channel();
    tx.send(Past::Reading(1, 4)).unwrap();
    assert!(u.app.take_history(&rx), "still reading");
    u.press(H)
        .sees("History is still reading local logs")
        .sees("Reading log files: 1 of 4 read so far");
    assert!(u.app.frame(&u.all, u.size).active.is_none(), "no rows yet");
    std::thread::spawn(move || {
        tx.send(Past::Reading(4, 4)).unwrap();
        tx.send(Past::Ready(crate::local::test_history())).unwrap();
    })
    .join()
    .unwrap();
    // What the loop does on each poll timeout: take what arrived, redraw.
    assert!(!u.app.take_history(&rx), "finished");
    u.sees("2 sessions, newest first").sees("Aug 27");
}

// At 80 columns the name gets what the 48-cell pane leaves; it is cut with
// "…", while the date, requests and dollars are never cut. At 60 columns
// the list takes the full width.
#[test]
fn history_rows_fit_with_names_cut_and_figures_whole() {
    for stress in [false, true] {
        for size in [(80, 24), (60, 24)] {
            let mut u = if stress {
                User::stress(size)
            } else {
                User::at(size)
            };
            u.press(H);
            let width = screen::list_width(size.0);
            let rows = content(&Page::new(View::History), &u.all, false, width, &u.app.rt).2;
            let screen = u.screen();
            let past = u.app.rt.past().unwrap();
            for (row, s) in rows.iter().zip(past.iter()).take(10) {
                assert!(
                    screen.contains(row.text.trim_end()),
                    "{size:?}: row cut: {:?}\n{screen}",
                    row.text
                );
                assert!(cells(&row.text) + 3 <= usize::from(width), "{}", row.text);
                assert!(row.text.contains(&format!(" {} ", s.requests.len())));
                assert!(row.text.contains(&usd(s.cents())) || row.text.contains("unpriced"));
                assert!(row.text.starts_with(&crate::data::day_label(s.day)));
            }
        }
    }
}

// Corpus names fit the column, so a long one is set by hand: it ends in "…"
// at both widths, and its row still shows date, requests and dollars whole.
#[test]
fn long_history_name_ends_in_an_ellipsis() {
    const LONG: &str = "Investigate the nightly release pipeline timeout";
    for size in [(80, 24), (60, 24)] {
        let mut u = User::at(size);
        let mut past = u.all.corpus_history();
        past.sessions[0].name = LONG;
        let s = &past.sessions[0];
        let (day, n, dollars) = (
            crate::data::day_label(s.day),
            s.requests.len(),
            usd(s.cents()),
        );
        u.app.rt.history = Past::Ready(past);
        u.press(H);
        let row = u.selected();
        let name = history_name_width(screen::list_width(size.0));
        let shown = format!("{}…", clip(LONG, name - 1));
        assert!(row.contains(&shown), "{size:?}: {row}");
        assert!(
            row.contains(&day) && row.contains(&format!(" {n} ")),
            "{row}"
        );
        assert!(row.contains(&dollars), "{row}");
    }
}

// Every footer naming `h` still fits 80 columns.
#[test]
fn four_footers_name_history_within_80_columns() {
    let mut u = User::at((80, 24));
    let check = |u: &mut User| {
        let footer = u.screen().lines().last().unwrap().trim_end().to_string();
        assert!(footer.contains("h History"), "{footer}");
        assert!(cells(&footer) <= 80, "{footer}");
    };
    check(&mut u);
    u.times(Down, 4).press(Enter);
    check(&mut u);
    u.press(Enter);
    check(&mut u);
    u.press(Enter);
    check(&mut u);
    u.press(KeyCode::Char('?'))
        .sees("h       History: all sessions, newest first");
}

// History reads apart from the dashboard: loading it must not move a
// single figure on Home.
#[test]
fn loading_history_leaves_home_unchanged() {
    let mut u = User::stress((80, 24));
    let before = u.screen();
    u.press(H).press(Esc);
    assert_eq!(u.screen(), before);
    assert_eq!(history_rows(&u).len(), 1991);
}
