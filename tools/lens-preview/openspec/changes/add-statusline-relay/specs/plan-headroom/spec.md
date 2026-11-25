# Spec Delta

## Purpose

Plan headroom with the statusline relay: the Claude rows and the weekly
history on local data come from relay readings.

## MODIFIED Requirements

### Requirement: One row per window
Plan headroom SHALL show one row per provider window: the 5-hour and weekly
windows for Claude and for Codex. Each row with readings SHALL show the used
share, the projected share at reset, the reset time and the source: "logs"
for Codex, "relay" for Claude. A row without readings SHALL show "—" for
each of them, say why it has none, and SHALL NOT open anything.

Covered by: `headroom_answers_with_the_filling_window_and_cites_sources` and
`headroom_rows_fit_80_columns` (src/tests/render.rs), plus relay tests to be
added (tasks 3.1).

#### Scenario: Sources are cited
- **WHEN** Plan headroom is built from the fixture
- **THEN** it has one selectable row per window (4 in all), Claude rows end in "relay", and Codex rows end in "logs"

#### Scenario: Claude rows without a relay file
- **WHEN** Plan headroom is built with `--source local` and no relay file
- **THEN** both Claude rows show "—" for used, pace and reset, say "relay not set up", and are not selectable

#### Scenario: Claude rows from the relay
- **WHEN** Plan headroom is built with `--source local` and the relay file has two readings in the current Claude 5-hour window
- **THEN** the Claude 5-hour row shows the latest share and a pace, ends in "relay", and is selectable

### Requirement: No plan recommendation
Plan headroom SHALL NOT recommend a plan. Below the window rows it SHALL show
how Claude's recent weekly windows ended, how many times each agent's limit
stopped work, how to read the pace column, and that the provider's meter is
the source of truth. A history figure the source can't supply SHALL say what
it needs instead of showing a number.

Covered by: `plan_headroom_reaches_its_trailing_explanation`
(src/e2e/scrolling.rs), plus relay tests to be added (tasks 3.2).

#### Scenario: Trailing explanation
- **WHEN** a person scrolls to the bottom of Plan headroom
- **THEN** "Provider meters are the source of truth." is shown

#### Scenario: Weekly history without a relay file
- **WHEN** Plan headroom is shown with `--source local` and no relay file
- **THEN** the weekly history row says it needs the statusline relay, and no Claude weekly percentage is shown

#### Scenario: Weekly history from the relay
- **WHEN** the relay file spans two weekly resets
- **THEN** the weekly history row lists those two endings and says the relay has seen 2 weeks

### Requirement: Window data follows the source
On the fixture, every window, reading, weekly history and limit-hit count
SHALL come from the synthetic fixture, and nothing local SHALL be read. On
local data, Codex windows and limit hits SHALL come from local logs, and
Claude windows and weekly history from the statusline relay file.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs)
for the fixture, plus relay tests to be added (tasks 3.1).

#### Scenario: Windows come from the fixture
- **WHEN** the export is built with `--source fixture`
- **THEN** `windows` has the fixture's 4 windows, and no relay file is opened

#### Scenario: Codex windows come from logs
- **WHEN** the export is built with `--source local` from hand-written logs with two Codex 5-hour readings and no relay file
- **THEN** the Codex 5-hour window holds those 2 readings, and the Claude windows hold none

#### Scenario: Claude windows come from the relay
- **WHEN** the export is built with `--source local` and a hand-written relay file with one `seven_day` reading in the current window
- **THEN** the Claude weekly window holds that 1 reading
