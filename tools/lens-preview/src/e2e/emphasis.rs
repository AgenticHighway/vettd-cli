//! Figures bold, secondary columns muted: what a person scanning a list
//! should see first, checked on the drawn cells.
use super::*;
use ratatui::buffer::Cell;

const MUTED: Color = Color::Rgb(158, 179, 195);
const AMBER: Color = Color::Rgb(248, 194, 128);

/// The non-blank cells of `part` on the first drawn line that holds `row`.
fn cells(u: &mut User, row: &str, part: &str) -> Vec<Cell> {
    let buffer = u.buffer();
    let width = buffer.area.width;
    for y in 0..buffer.area.height {
        let line: Vec<&str> = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
        let text: String = line.concat();
        if !text.contains(row) {
            continue;
        }
        // Columns, not bytes: every symbol here is one cell wide.
        let start = line
            .windows(part.chars().count())
            .position(|w| w.concat() == part)
            .unwrap_or_else(|| panic!("{part:?} not on {text:?}"));
        return (start..start + part.chars().count())
            .map(|x| buffer[(x as u16, y)].clone())
            .filter(|c| c.symbol() != " ")
            .collect();
    }
    panic!("no line with {row:?}:\n{}", u.screen());
}

fn bold(cells: &[Cell]) -> bool {
    !cells.is_empty() && cells.iter().all(|c| c.modifier.contains(Modifier::BOLD))
}

fn plain(cells: &[Cell]) -> bool {
    cells.iter().all(|c| !c.modifier.contains(Modifier::BOLD))
}

fn day(u: &mut User, day: u32) {
    u.app.stack.push(Page::new(View::Day(day)));
}

// On a Day list the totals carry the weight: bold totals, plain names,
// muted workspace and agent, so the eye goes to name and amount.
#[test]
fn day_totals_are_bold_and_workspace_and_agent_are_muted() {
    let mut u = User::at((80, 24));
    day(&mut u, 16);
    // Release checks is the unselected row; Index rebuild is selected.
    assert!(bold(&cells(&mut u, "Release checks", "$0.32")));
    let name = cells(&mut u, "Release checks", "Release checks");
    assert!(plain(&name));
    assert!(name.iter().all(|c| c.fg != MUTED));
    for part in ["web", "Codex"] {
        let c = cells(&mut u, "Release checks", part);
        assert!(c.iter().all(|c| c.fg == MUTED), "{part} not muted");
    }
}

// The selected row keeps its selection style throughout: no muted cells,
// the same background, and the figure still bold.
#[test]
fn selected_row_keeps_its_style_with_a_bold_total() {
    let mut u = User::at((80, 24));
    day(&mut u, 16);
    let total = cells(&mut u, "Index rebuild", "$8.20");
    assert!(bold(&total));
    let ws = cells(&mut u, "Index rebuild", "api");
    assert!(ws.iter().all(|c| c.fg != MUTED));
    assert_eq!(ws[0].bg, total[0].bg);
}

// Dates are not amounts: "Nov 14" stays plain on Week, its amount is bold.
#[test]
fn week_bolds_amounts_not_dates() {
    let mut u = User::at((80, 24));
    u.times(Down, 4).press(Enter);
    assert!(plain(&cells(&mut u, "Nov 14", "Nov 14")));
    assert!(bold(&cells(&mut u, "Nov 14", "$0.82")));
    // A day with no input has no amount to weigh.
    assert!(plain(&cells(&mut u, "Nov 13", "No input")));
}

// Without colour, bold and dim carry the difference.
#[test]
fn emphasis_holds_without_colour() {
    let mut u = User::at((80, 24));
    u.mono = true;
    day(&mut u, 16);
    assert!(bold(&cells(&mut u, "Release checks", "$0.32")));
    let ws = cells(&mut u, "Release checks", "web");
    assert!(ws.iter().all(|c| c.modifier.contains(Modifier::DIM)));
    let name = cells(&mut u, "Release checks", "Release checks");
    assert!(name.iter().all(|c| !c.modifier.contains(Modifier::DIM)));
}

// A flagged row keeps its flag colour, and "unpriced" is a flag, not bold.
#[test]
fn flagged_row_keeps_its_colour() {
    let mut u = User::at((80, 24));
    day(&mut u, 18);
    let ws = cells(&mut u, "Release checks", "web");
    assert!(ws.iter().all(|c| c.fg == AMBER), "{ws:?}");
    assert!(plain(&cells(&mut u, "Release checks", "unpriced")));
}

// Session and Tokens figures are bold; their labels are not.
#[test]
fn session_and_token_figures_are_bold() {
    let mut u = User::at((80, 24));
    // The second word after the label: the cost on Session, the count on
    // Tokens.
    let figure = |u: &mut User| {
        let screen = u.screen();
        let line = screen.lines().find(|l| l.contains("Fresh input")).unwrap();
        let after = &line[line.find("Fresh input").unwrap() + "Fresh input".len()..];
        after.split_whitespace().next().unwrap().to_string()
    };
    u.press(Down).press(Enter).press(Enter);
    let cost = figure(&mut u);
    assert!(cost.starts_with('$'), "{cost}");
    assert!(bold(&cells(&mut u, "Fresh input", &cost)));
    assert!(plain(&cells(&mut u, "Fresh input", "Fresh input")));
    u.press(Enter);
    let count = figure(&mut u);
    assert!(count.chars().all(|c| c.is_ascii_digit()), "{count}");
    assert!(bold(&cells(&mut u, "Fresh input", &count)));
    assert!(plain(&cells(&mut u, "Fresh input", "tokens")));
}

// Rows without marks draw as before: no bold in the rows of help.
#[test]
fn unmarked_rows_have_no_emphasis() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('?'));
    assert!(plain(&cells(&mut u, "Move selection", "Move selection")));
}

// Stress rows with a shortened workspace and a narrow screen that clips
// the row still draw the total bold and the workspace muted.
#[test]
fn stress_rows_keep_emphasis_when_shortened_and_narrow() {
    for size in [(80, 24), (60, 24)] {
        let mut u = User::stress(size);
        day(&mut u, 16);
        let screen = u.screen();
        let row = screen
            .lines()
            .skip(6)
            .find(|l| l.contains('…') && !l.contains('▸'))
            .unwrap_or_else(|| panic!("no shortened row:\n{screen}"))
            .to_string();
        let words: Vec<&str> = row.split('│').next().unwrap().split_whitespace().collect();
        let total = words.last().unwrap().to_string();
        let ws = words.iter().find(|w| w.contains('…')).unwrap().to_string();
        assert!(bold(&cells(&mut u, &row, &total)), "{size:?} {row}");
        let ws = cells(&mut u, &row, &ws);
        assert!(ws.iter().all(|c| c.fg == MUTED), "{size:?} {row}");
    }
}

// Find session results share the Day row, so they carry the same weight.
#[test]
fn find_session_results_bold_totals_and_mute_columns() {
    let mut u = User::at((80, 24));
    u.press(KeyCode::Char('/')).typed("index");
    assert!(bold(&cells(&mut u, "Nov 14 Index", "$0.50")));
    let ws = cells(&mut u, "Nov 14 Index", "api");
    assert!(ws.iter().all(|c| c.fg == MUTED));
    assert!(plain(&cells(
        &mut u,
        "Nov 14 Index",
        "Nov 14 Index rebuild"
    )));
}
