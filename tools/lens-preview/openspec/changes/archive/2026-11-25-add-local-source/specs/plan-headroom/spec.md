# Spec Delta

## Purpose

Plan headroom: how full each provider usage window is, its pace to reset,
and where each number comes from. This delta covers Plan headroom on local
data, where Codex windows are real and Claude windows need the relay.

## MODIFIED Requirements

### Requirement: One row per window
Plan headroom SHALL show one row per provider window: the 5-hour and weekly
windows for Claude and for Codex. Each row with readings SHALL show the used
share, the projected share at reset, the reset time and the source: "logs"
for Codex, "relay" for Claude. A row without readings SHALL show "—" for
each of them and say where readings would come from, and SHALL NOT open
anything.

Covered by: `headroom_answers_with_the_filling_window_and_cites_sources` and
`headroom_rows_fit_80_columns` (src/tests/render.rs), plus a local-data test
to be added (tasks 4.4).

#### Scenario: Sources are cited
- **WHEN** Plan headroom is built from the fixture
- **THEN** it has one selectable row per window (4 in all), Claude rows end in "relay", and Codex rows end in "logs"

#### Scenario: Claude rows without the relay
- **WHEN** Plan headroom is built with `--source local` and no relay data
- **THEN** both Claude rows show "—" for used, pace and reset, end in "needs relay", and are not selectable

### Requirement: Window rows open their sessions
A 5-hour window's row SHALL open the sessions of the day of its latest
reading, which is Nov 18 on the fixture. A weekly window's row SHALL open the
daily view. A row without readings SHALL open nothing.

Covered by: `plan_journey_goes_deep_and_comes_back_to_the_same_finding`
(src/e2e/mod.rs). The weekly case is verified in `plan.rs::window_row`. The
local case needs a test to be added (tasks 4.4).

#### Scenario: A 5-hour row opens today
- **WHEN** Enter is pressed on the Claude 5-hour row on the fixture
- **THEN** the breadcrumb is "Home › Plan headroom › Nov 18"

#### Scenario: Codex 5-hour row on local data
- **WHEN** the Codex 5-hour window's latest reading is on Dec 2 and Enter is pressed on its row
- **THEN** the Dec 2 sessions open

### Requirement: Answer names the soonest fill
The answer SHALL name the window that fills soonest, meaning the earliest
fill time and not the largest projection, with its used share and when it
fills. Only windows with readings SHALL count. When none is on pace to fill
before it resets, the answer SHALL say so, and on local data without relay
readings it SHALL add that Claude windows need the statusline relay.

Covered by: `headroom_answers_with_the_filling_window_and_cites_sources` and
`soonest_window_wins_over_the_largest_projection` (src/tests/render.rs),
plus a local-data test to be added (tasks 4.4).

#### Scenario: Default fixture
- **WHEN** Plan headroom is shown
- **THEN** the answer is "Claude 5-hour window is 62% used. At this pace it's full by 13:22."

#### Scenario: Larger projection that fills later
- **WHEN** a weekly window projects above the Claude 5-hour window's 115% but fills days later
- **THEN** the Claude 5-hour window is the one named

#### Scenario: Nothing filling, no relay
- **WHEN** Plan headroom is shown with `--source local`, no relay data, and no Codex window on pace to fill
- **THEN** the answer says no window with readings is on pace to fill and that Claude windows need the statusline relay

### Requirement: No plan recommendation
Plan headroom SHALL NOT recommend a plan. Below the window rows it SHALL show
how Claude's recent weekly windows ended, how many times each agent's limit
stopped work, how to read the pace column, and that the provider's meter is
the source of truth. A history figure the source can't supply SHALL say what
it needs instead of showing a number.

Covered by: `plan_headroom_reaches_its_trailing_explanation`
(src/e2e/scrolling.rs), plus a local-data test to be added (tasks 4.4).

#### Scenario: Trailing explanation
- **WHEN** a person scrolls to the bottom of Plan headroom
- **THEN** "Provider meters are the source of truth." is shown

#### Scenario: Weekly history without the relay
- **WHEN** Plan headroom is shown with `--source local` and no relay data
- **THEN** the weekly history row says it needs the statusline relay, and no Claude weekly percentage is shown

## REMOVED Requirements

### Requirement: Synthetic window data
**Reason**: Plan headroom can now read Codex windows from local logs, so
window data is no longer always synthetic.
**Migration**: Replaced by "Window data follows the source" below.

## ADDED Requirements

### Requirement: Window data follows the source
On the fixture, every window, reading, weekly history and limit-hit count
SHALL come from the synthetic fixture, and nothing local SHALL be read. On
local data, Codex windows and limit hits SHALL come from local logs, and
Claude windows SHALL have no readings until the statusline relay exists.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs)
for the fixture, plus a local-data test to be added (tasks 4.4).

#### Scenario: Windows come from the fixture
- **WHEN** the export is built with `--source fixture`
- **THEN** `windows` has the fixture's 4 windows

#### Scenario: Codex windows come from logs
- **WHEN** the export is built with `--source local` from hand-written logs with two Codex 5-hour readings
- **THEN** the Codex 5-hour window holds those 2 readings, and the Claude windows hold none
