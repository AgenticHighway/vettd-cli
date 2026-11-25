# plan-headroom Specification

## Purpose
Plan headroom: how full each provider usage window is, its pace to reset,
and where each number comes from. In the preview, all window data is
synthetic.

## Requirements

### Requirement: One row per window
Plan headroom SHALL show one row per provider window: the 5-hour and weekly
windows for Claude and for Codex. Each row SHALL show the used share (the
last reading), the projected share at reset, the reset time, and the source:
"logs" for Codex, "relay" for the Claude statusline relay.

Covered by: `headroom_answers_with_the_filling_window_and_cites_sources` and
`headroom_rows_fit_80_columns` (src/tests/render.rs).

#### Scenario: Sources are cited
- **WHEN** Plan headroom is built
- **THEN** it has one selectable row per window (4 in all), Claude rows end in "relay", and Codex rows end in "logs"

### Requirement: Window rows open their sessions
A 5-hour window's row SHALL open today's sessions (Nov 18), and a weekly
window's row SHALL open the daily view.

Covered by: `plan_journey_goes_deep_and_comes_back_to_the_same_finding`
(src/e2e/mod.rs). The weekly case is verified in `plan.rs::window_row`.

#### Scenario: A 5-hour row opens today
- **WHEN** Enter is pressed on the Claude 5-hour row
- **THEN** the breadcrumb is "Home › Plan headroom › Nov 18"

### Requirement: Straight-line pace
The projected share at reset SHALL be a straight line through the window's
first and last readings, extended to the reset. With fewer than 2 readings,
the projection SHALL show "—" and no fill time, never a guessed number.

Covered by: `pace_needs_two_readings` and
`pace_extrapolates_to_reset_and_names_when_it_fills` (src/tests/render.rs).

#### Scenario: Two or more readings
- **WHEN** Claude's 5-hour window goes from 9% to 62% between its first and last readings
- **THEN** its projection is 115% and it fills at 13:22

#### Scenario: One reading
- **WHEN** Codex's 5-hour window has a single reading
- **THEN** its row shows "—" for the projection, and it has no fill time

### Requirement: Answer names the soonest fill
The answer SHALL name the window that fills soonest, meaning the earliest
fill time and not the largest projection, with its used share and when it
fills. When no window is on pace to fill before it resets, the answer SHALL
say so.

Covered by: `headroom_answers_with_the_filling_window_and_cites_sources` and
`soonest_window_wins_over_the_largest_projection` (src/tests/render.rs).

#### Scenario: Default fixture
- **WHEN** Plan headroom is shown
- **THEN** the answer is "Claude 5-hour window is 62% used. At this pace it's full by 13:22."

#### Scenario: Larger projection that fills later
- **WHEN** a weekly window projects above the Claude 5-hour window's 115% but fills days later
- **THEN** the Claude 5-hour window is the one named

### Requirement: No plan recommendation
Plan headroom SHALL NOT recommend a plan. Below the window rows it SHALL show
how Claude's recent weekly windows ended, how many times the 5-hour limit
stopped work, how to read the pace column, and that the provider's meter is
the source of truth.

Covered by: `plan_headroom_reaches_its_trailing_explanation`
(src/e2e/scrolling.rs). The history rows are verified in
`plan.rs::history_rows`.

#### Scenario: Trailing explanation
- **WHEN** a person scrolls to the bottom of Plan headroom
- **THEN** "Provider meters are the source of truth." is shown

### Requirement: Synthetic window data
In the preview, every window, reading, weekly history and limit-hit count
SHALL come from the synthetic fixture. The preview SHALL NOT read Codex
session logs or the Claude statusline.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs),
which shows the window data is the fixture's. That nothing real is read is
verified by grep: outside tests, `src/` has no `std::fs`, `std::net` or
`File::` use. Its only environment read is `NO_COLOR`.

#### Scenario: Windows come from the fixture
- **WHEN** the export is built
- **THEN** `windows` has the fixture's 4 windows
