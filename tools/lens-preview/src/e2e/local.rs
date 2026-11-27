//! End-to-end journeys on `--source local`, driven by real key presses on
//! the hand-written logs under `tests/data/` (as of Nov 25 12:00 UTC).

use super::*;

// Journey: "what did my agents cost this week?" From Home to a real day,
// a session and its estimated token sources, then back to Home with the
// same row selected.
#[test]
fn local_journey_reaches_a_session_and_its_sources_and_comes_back() {
    let mut u = User::local((100, 30));
    u.sees("local logs · read-only · nothing leaves this machine")
        .sees("5 sessions")
        .sees("API-equivalent")
        .sees("WHERE TOKENS WENT (ESTIMATED)")
        .sees("Instructions and files")
        .doesnt_see("review-kit");
    // The last finding opens daily activity; Nov 25 is the last day.
    u.times(Down, 5);
    let home = u.selected();
    assert_eq!(home, "▸ Every day and its sessions");
    u.press(Enter).sees("Nov 19").sees("Nov 25");
    assert_eq!(u.crumb(), "Home › Daily activity");
    u.times(Down, 6).press(Enter).sees("Nov 25 sessions");
    assert_eq!(u.crumb(), "Home › Daily activity › Nov 25");
    // Sessions sort by spend; open the one with a title and a skill use.
    let mut found = false;
    for _ in 0..5 {
        if u.selected().contains("Renamed session ") {
            found = true;
            break;
        }
        u.press(Down);
    }
    assert!(found, "titled session not listed:\n{}", u.screen());
    u.press(Enter)
        .sees("USD API-equivalent")
        .sees("Skills and MCP used")
        .sees("copy unknown");
    u.press(Enter)
        .sees("Where tokens came from (estimated)")
        .sees("Hooks")
        .sees("Not counted (unverified)");
    u.times(Esc, 4);
    assert_eq!(u.crumb(), "Home");
    assert_eq!(u.selected(), home);
}

// Journey: "how close am I to my limits?" With no window filling, Plan
// headroom is still one Enter away, and the Claude rows say what's missing
// instead of showing a number.
#[test]
fn local_plan_headroom_is_reachable_and_names_the_missing_relay() {
    let mut u = User::local((80, 24));
    assert!(u.selected().starts_with("▸ Plan windows: none filling"));
    u.press(Enter)
        .sees("Codex · 5-hour")
        .sees("33%")
        .sees("Claude 5-hour: relay not set up");
    assert_eq!(u.crumb(), "Home › Plan headroom");
    u.press(Esc);
    assert!(u.selected().starts_with("▸ Plan windows"));
}

// Journey: the inventory stays synthetic on local data, and says so.
#[test]
fn local_inventory_is_labelled_synthetic() {
    let mut u = User::local((80, 24));
    u.press(KeyCode::Char('i')).sees("synthetic");
    u.press(Esc).press(KeyCode::Char('/')).sees("synthetic");
}

// 1–7 open the period's days by position. Local days are not Nov 12–18,
// so a key mapped to fixed dates would open an empty day here.
#[test]
fn local_number_keys_open_the_periods_own_days() {
    let mut u = User::local((80, 24));
    u.press(KeyCode::Char('1')).sees("Nov 19 sessions");
    assert_eq!(u.crumb(), "Home › Nov 19");
    u.press(Esc)
        .press(KeyCode::Char('7'))
        .sees("Nov 25 sessions");
    assert_eq!(u.crumb(), "Home › Nov 25");
}

// The unit toggle works on real data too: requests instead of dollars.
#[test]
fn local_unit_toggle_switches_to_requests() {
    let mut u = User::local((80, 24));
    u.press(KeyCode::Char('c'))
        .sees("DAILY REQUESTS")
        .doesnt_see("API-equivalent USD");
}
