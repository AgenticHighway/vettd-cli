# tui-shell Specification

## Purpose
How the interactive Lens preview is drawn and laid out, and how it is built
and checked apart from vettd-cli.

## Requirements

### Requirement: Drawn on stderr with ratatui
The interactive preview SHALL draw with ratatui using the crossterm backend,
writing to stderr, so that stdout stays free for machine output. On exit it
SHALL restore the terminal (cursor shown, alternate screen left, raw mode off).

Covered by: no automated test. Verified in `src/main.rs` (`run`, and the
`Terminal` guard's `Drop`).

#### Scenario: stdout carries nothing while the TUI runs
- **WHEN** the preview runs interactively with stdout redirected to a file
- **THEN** the screen is drawn on stderr and the file stays empty

### Requirement: Separate workspace and CI
The preview SHALL be its own Cargo workspace, with its own CI workflow
(`.github/workflows/lens-preview.yml`). That workflow SHALL run only when
files under `tools/lens-preview/` or the workflow itself change, and a failing
preview SHALL NOT block vettd-cli's CI or release.

Covered by: no automated test. Verified in `Cargo.toml` and the workflow's
`paths:` filters.

#### Scenario: A change outside the preview
- **WHEN** a pull request changes only files outside `tools/lens-preview/` and the workflow file
- **THEN** the Lens preview workflow does not run

#### Scenario: A broken preview
- **WHEN** the preview fails `cargo fmt --check`, clippy or `cargo test`
- **THEN** the Lens preview workflow fails, and vettd-cli's own CI jobs are unaffected

### Requirement: Frame order
Every frame at a supported size SHALL show, top to bottom: a top bar, a
breadcrumb, the answer line, scope chips, the body, an Enter hint, and the key
footer.

Covered by: `deep_breadcrumb_keeps_home_and_current_level` and
`wrapped_answer_keeps_its_indent` (src/tests/render.rs), which read the
breadcrumb from line 2 and the answer's second row from line 4. The rest of
the order is verified in `src/screen/mod.rs` (`draw`).

#### Scenario: Breadcrumb is the second line
- **WHEN** any view is drawn at a supported size
- **THEN** line 2 is the breadcrumb and the answer starts on line 3

### Requirement: Breadcrumb keeps Home and the current level
The breadcrumb SHALL always keep "Home" and the current level. When the path
doesn't fit, it SHALL put "…" in place of the middle levels.

Covered by: `deep_breadcrumb_keeps_home_and_current_level`
(src/tests/render.rs).

#### Scenario: Deep breadcrumb at 40 columns
- **WHEN** the stack is Home › Nov 16 › session › review-kit (week) › Copy at 40×24
- **THEN** the breadcrumb starts with "Home ›" and ends with "Copy"

### Requirement: Answer line wraps
The answer line SHALL start with a ◆ marker. When it doesn't fit, it SHALL
wrap onto a second row with a hanging indent rather than cut off its text.

Covered by: `answer_wraps_instead_of_cutting_its_numbers` and
`wrapped_answer_keeps_its_indent` (src/tests/render.rs).

#### Scenario: Answer wraps
- **WHEN** Plan headroom is shown at 60×24
- **THEN** the answer's second row starts indented under the text, past the ◆ marker, and still contains "13:22"

#### Scenario: Narrow answer keeps its numbers
- **WHEN** Home is shown at 50×24 or 38×10
- **THEN** "$12.36" is on screen

### Requirement: Scope chips overflow to an ellipsis
Scope chips that don't fit SHALL be replaced by a single "…" rather than cut
mid-word.

Covered by: no automated test. Verified in `src/screen/mod.rs` (`chips`).

#### Scenario: Too many chips
- **WHEN** a view's scope chips are wider than the terminal
- **THEN** the chips that fit are drawn whole, followed by "…"

### Requirement: Enter hint
When the selected row opens something, the Enter hint SHALL name what Enter
does and show an "n/total" counter of the selectable rows.

Covered by: `plan_journey_goes_deep_and_comes_back_to_the_same_finding`
(src/e2e/mod.rs) for the hint text. The counter is verified in
`src/screen/mod.rs` (`draw_hint`).

#### Scenario: Hint on Plan headroom
- **WHEN** Plan headroom is open with the Claude 5-hour row selected
- **THEN** the hint reads "Enter See today's sessions"

### Requirement: Too-small terminal
Below 38 columns or 10 rows, the preview SHALL show a screen saying that Lens
needs at least 38×10 and giving the current size. At that size only `q` and
Ctrl+C SHALL act. Every other key SHALL be ignored, and the view stack and
selection SHALL be kept until the window is enlarged.

Covered by: `too_small_terminal_says_what_it_needs` (src/tests/render.rs) and
`too_small_window_keeps_the_place_and_only_quits` (src/e2e/mod.rs).

#### Scenario: Keys ignored while too small
- **WHEN** Nov 16 is selected on Home, the window shrinks to 30×8, and Enter and Down are pressed
- **THEN** the screen mentions "38×10", and after the window returns to 80×24 the breadcrumb is "Home" with Nov 16 still selected

#### Scenario: Quit while too small
- **WHEN** `q` is pressed at 30×8
- **THEN** the preview quits

### Requirement: NO_COLOR
When `NO_COLOR` is set to any non-empty value, the preview SHALL draw no
foreground or background colours. The selection SHALL be shown in reverse
video, and scope chips SHALL be drawn as `[value]`. An empty `NO_COLOR` SHALL
NOT turn colour off.

Covered by: `no_color_marks_selection_with_reverse_video_only`
(src/tests/render.rs) and `no_color_journey_keeps_focus_visible_without_colour`
(src/e2e/mod.rs). The empty-value rule is verified in `screen::no_color`.

#### Scenario: Journey without colour
- **WHEN** the preview runs in mono mode and goes Home → Plan headroom → Nov 18 → back → back at 80×24
- **THEN** every cell on every step has the default foreground and background, and some cell is reversed

### Requirement: Preview pane at wide sizes
List views SHALL show a preview pane beside the list, describing what Enter
would open, when the body is at least 76 columns wide and 9 rows tall and the
selected row opens something.

Covered by: no automated test. Verified in `src/screen/list.rs`
(`draw_list`).

#### Scenario: Daily view at 80 columns
- **WHEN** the daily view is shown at 80×24 with a day selected
- **THEN** a PREVIEW pane beside the list describes that day

### Requirement: Full-width views
Plan headroom, copy (Placement), observation and change views SHALL always
use the full width, with no preview pane. Home SHALL use its own dashboard
layout instead.

Covered by: `headroom_rows_fit_80_columns` (src/tests/render.rs) for Plan
headroom. The other views are verified in `src/screen/list.rs`.

#### Scenario: Plan headroom at 80 columns
- **WHEN** Plan headroom is shown at 80×24
- **THEN** each window row shows every column through SOURCE (for example "Nov 23 09:00 relay"), with no preview pane taking the width
