//! Rows without an action must still be reachable. In a 38x10 terminal only
//! a few rows fit, so arrow and page keys scroll once there is no selectable
//! row to move to.
use super::*;
use KeyCode::{PageDown, Up};

const SMALL: (u16, u16) = (38, 10);

// Token detail has no actions at all: Down must scroll to the last worker.
#[test]
fn token_detail_scrolls_to_its_last_row() {
    let mut u = User::at(SMALL);
    u.press(Down).press(Enter).press(Enter).press(Enter);
    u.sees("Main / Large").doesnt_see("Subagent / Small");
    u.times(Down, 12).sees("Output").sees("21000 tokens");
    u.times(Up, 12).sees("Main / Large");
}

#[test]
fn help_scrolls_to_quit() {
    let mut u = User::at(SMALL);
    u.press(KeyCode::Char('?')).doesnt_see("Quit (Ctrl+C");
    u.times(PageDown, 6).sees("Quit (Ctrl+C");
}

// An exact copy lists its locator before its first action. Up from the
// first action scrolls back to it, and Down returns to the action.
#[test]
fn exact_copy_reveals_rows_above_its_first_action() {
    let mut u = User::at(SMALL);
    u.app.stack.push(Page::new(View::Placement(0)));
    u.doesnt_see("Locator");
    u.times(Up, 12).sees("Locator");
    // Down scrolls back until the selected action is on screen again.
    u.times(Down, 12);
    u.selected();
}

// Plan headroom's explanation sits below its last window row.
#[test]
fn plan_headroom_reaches_its_trailing_explanation() {
    let mut u = User::at(SMALL);
    u.press(Enter).doesnt_see("Provider meters are");
    u.times(Down, 14).sees("Provider meters are");
    u.times(Up, 14);
    assert!(u.selected().contains("Claude · 5-hour"));
}
