//! End-to-end usability tests. Each test is a person pressing real keys
//! through `App::press`, the same code the terminal loop runs, and reading
//! what the rendered screen shows after each step. They check that every
//! journey can be finished from the screen alone and that Esc always brings
//! the person back to where they were.

use super::*;
use ratatui::{
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier},
};

struct User {
    app: App,
    all: Vec<Session>,
    size: (u16, u16),
    mono: bool,
    quit: bool,
}

impl User {
    fn at(size: (u16, u16)) -> Self {
        Self {
            app: App::new(),
            all: fixture::sessions(),
            size,
            mono: false,
            quit: false,
        }
    }

    fn press(&mut self, code: KeyCode) -> &mut Self {
        assert!(!self.quit, "pressed {code:?} after quitting");
        if !self.app.press(
            &self.all,
            self.size,
            KeyEvent::new(code, KeyModifiers::NONE),
        ) {
            self.quit = true;
        }
        self
    }

    fn times(&mut self, code: KeyCode, n: usize) -> &mut Self {
        for _ in 0..n {
            self.press(code);
        }
        self
    }

    fn typed(&mut self, text: &str) -> &mut Self {
        for ch in text.chars() {
            self.press(KeyCode::Char(ch));
        }
        self
    }

    fn buffer(&mut self) -> ratatui::buffer::Buffer {
        let shown = self.app.frame(&self.all, self.size);
        let mut term = ratatui::Terminal::new(TestBackend::new(self.size.0, self.size.1)).unwrap();
        term.draw(|f| self.app.draw(f, &self.all, &shown, self.mono))
            .unwrap();
        term.backend().buffer().clone()
    }

    fn screen(&mut self) -> String {
        let buffer = self.buffer();
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

    fn sees(&mut self, needle: &str) -> &mut Self {
        let screen = self.screen();
        assert!(
            screen.contains(needle),
            "expected {needle:?} on screen:\n{screen}"
        );
        self
    }

    fn doesnt_see(&mut self, needle: &str) -> &mut Self {
        let screen = self.screen();
        assert!(
            !screen.contains(needle),
            "did not expect {needle:?}:\n{screen}"
        );
        self
    }

    /// The selected row, from its marker on. Home draws panels side by side,
    /// so text left of the marker can belong to another panel.
    fn selected(&mut self) -> String {
        let screen = self.screen();
        let line = screen
            .lines()
            .find(|l| l.contains('▸'))
            .unwrap_or_else(|| panic!("no selection visible:\n{screen}"));
        line[line.find('▸').unwrap()..].trim_end().to_string()
    }

    fn crumb(&mut self) -> String {
        self.screen().lines().nth(1).unwrap().trim().to_string()
    }
}

use KeyCode::{Down, Enter, Esc};

// Journey: "am I using my plan well?" Home leads with the window that can
// stop work, Enter explains it, and Esc twice returns to the same finding.
#[test]
fn plan_journey_goes_deep_and_comes_back_to_the_same_finding() {
    let mut u = User::at((80, 24));
    assert!(u.selected().starts_with("▸ Claude 5-hour: 62%"));
    u.press(Enter);
    assert_eq!(u.crumb(), "Home › Plan headroom");
    u.sees("Claude 5-hour window is 62% used. At this pace it's full by 13:22.")
        .sees("Enter See today's sessions");
    u.press(Enter);
    assert_eq!(u.crumb(), "Home › Plan headroom › Nov 18");
    u.press(Esc);
    assert!(u.selected().starts_with("▸ Claude · 5-hour"));
    u.press(Esc);
    assert_eq!(u.crumb(), "Home");
    assert!(u.selected().starts_with("▸ Claude 5-hour: 62%"));
}

// Journey: "why did spend jump?" From the Nov 16 finding to the session and
// its tokens, then back to Home with Nov 16 still selected.
#[test]
fn spend_journey_reaches_the_run_and_restores_selection() {
    let mut u = User::at((80, 24));
    u.press(Down);
    assert!(u.selected().starts_with("▸ Nov 16: 10× a normal day"));
    u.press(Enter).sees("Nov 16 sessions").sees("+$7.69 change");
    assert!(u.selected().starts_with("▸ Index rebuild"));
    u.press(Enter);
    assert_eq!(u.crumb(), "Home › Nov 16 › Index rebuild");
    u.sees("Fresh input").sees("Main / Large");
    u.press(Enter).sees("TOKENS BY WORKER");
    u.times(Esc, 3);
    assert_eq!(u.crumb(), "Home");
    assert!(u.selected().starts_with("▸ Nov 16"));
}

// Journey: a maintainer narrows the inventory, clears it, opens a component
// and comes back to the same row.
#[test]
fn maintainer_journey_filters_clears_and_returns_to_the_row() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('i'));
    assert_eq!(u.crumb(), "Home › Components");
    u.sees("10 of 10");
    u.press(KeyCode::Tab).doesnt_see("10 of 10");
    u.press(KeyCode::Char('x')).sees("10 of 10");
    u.press(Down);
    let row = u.selected();
    u.press(Enter);
    assert!(
        u.crumb().starts_with("Home › Components › exec"),
        "{}",
        u.crumb()
    );
    u.press(Esc);
    assert_eq!(u.selected(), row);
}

// Lookup is a text field: letters type, they don't trigger commands, and q
// in particular must not quit mid-word.
#[test]
fn find_types_letters_instead_of_running_commands() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('/')).typed("rev");
    assert!(u.selected().contains("review-kit"));
    u.typed("q");
    assert!(!u.quit, "q quit while typing a search");
    u.sees("No matches. Edit search.");
    u.press(KeyCode::Backspace);
    u.press(Enter);
    assert!(u.crumb().ends_with("review-kit (week)"), "{}", u.crumb());
}

// Esc at Home has nowhere to go: it must not quit or change the screen.
#[test]
fn esc_at_home_keeps_the_person_at_home() {
    let mut u = User::at((80, 24));
    let before = u.screen();
    u.press(Esc);
    assert!(!u.quit);
    assert_eq!(u.screen(), before);
}

// State: a too-small window ignores everything but quit, and enlarging it
// brings back the same place.
#[test]
fn too_small_window_keeps_the_place_and_only_quits() {
    let mut u = User::at((80, 24));
    u.press(Down);
    u.size = (30, 8);
    u.sees("38×10");
    u.press(Enter).press(Down);
    u.size = (80, 24);
    assert_eq!(u.crumb(), "Home");
    assert!(u.selected().starts_with("▸ Nov 16"));
    u.size = (30, 8);
    u.press(KeyCode::Char('q'));
    assert!(u.quit);
}

// State: incomplete input and an unpriced model. The session says so in
// words and never shows a $0.00 that reads as free.
#[test]
fn partial_unpriced_session_says_so_and_never_shows_zero() {
    let mut u = User::at((80, 24));
    u.times(Down, 3);
    assert!(u.selected().starts_with("▸ Nov 18: partial input"));
    u.press(Enter)
        .sees("No priced requests")
        .sees("Input incomplete")
        .doesnt_see("$0.00");
}

// State: a day with no sessions opens to an explicit empty message.
#[test]
fn day_without_input_says_so() {
    let mut u = User::at((80, 24));
    u.times(Down, 4).press(Enter);
    assert_eq!(u.crumb(), "Home › Daily activity");
    u.press(Down);
    assert!(u.selected().contains("Nov 13"));
    u.sees("No input");
    u.press(Enter).sees("No session input.");
}

// State: filters that match nothing say which key recovers.
#[test]
fn empty_inventory_filter_names_the_way_out() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('i'))
        .times(KeyCode::Tab, 4)
        .times(KeyCode::Char('a'), 2)
        .sees("0 of 10")
        .sees("No components match these filters. x clears.");
    u.press(KeyCode::Char('x')).sees("10 of 10");
}

// State: the unit toggle changes what Home measures, and toggling back
// restores it.
#[test]
fn unit_toggle_switches_home_between_spend_and_requests() {
    let mut u = User::at((80, 24));
    u.sees("DAILY SPEND").sees("Priced USD");
    u.press(KeyCode::Char('c'))
        .sees("DAILY REQUESTS")
        .sees(" Requests ");
    u.press(KeyCode::Char('c')).sees("DAILY SPEND");
}

// State: NO_COLOR across a whole journey. No screen may rely on colour,
// and the selection must stay visible as reverse video on every step.
#[test]
fn no_color_journey_keeps_focus_visible_without_colour() {
    let mut u = User::at((80, 24));
    u.mono = true;
    for step in [None, Some(Enter), Some(Enter), Some(Esc), Some(Esc)] {
        if let Some(key) = step {
            u.press(key);
        }
        let buffer = u.buffer();
        let mut reversed = false;
        for cell in buffer.content() {
            assert_eq!(cell.fg, Color::Reset, "colour on {}", u.crumb());
            assert_eq!(cell.bg, Color::Reset, "background on {}", u.crumb());
            reversed |= cell.modifier.contains(Modifier::REVERSED);
        }
        assert!(reversed, "no visible focus on {}", u.crumb());
    }
}

// State: a narrow 60x24 terminal. Every screen in the plan and spend
// journeys still shows its selection and its breadcrumb.
#[test]
fn narrow_terminal_journeys_keep_selection_and_place() {
    let mut u = User::at((60, 24));
    for key in [Enter, Enter, Esc, Esc, Down, Enter, Enter, Esc, Esc] {
        u.press(key);
        assert!(u.crumb().starts_with("Home"), "{}", u.screen());
        u.selected();
    }
}

// State: help opens from anywhere and Esc returns to the same place.
#[test]
fn help_opens_and_returns() {
    let mut u = User::at((80, 24));
    u.press(Down);
    u.press(KeyCode::Char('?')).sees("Keyboard shortcuts");
    u.press(Esc);
    assert!(u.selected().starts_with("▸ Nov 16"));
}

// Rows without an action must still be reachable. In a 38x10 terminal only
// a few rows fit, so arrow and page keys scroll once there is no selectable
// row to move to.
mod scrolling {
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
}
