# screen-readability Specification

## Purpose
Defines how the Lens preview's screens stay readable at stress scale: figures
stay whole and aligned when names are long, lists are long, and daily values
are close together.

## Requirements

### Requirement: Raised chart baseline for close daily values
When the smallest day with data on Home's daily chart is at least half the
peak and below it, the chart SHALL draw bars from a raised baseline. The
baseline is the smallest day minus the gap between the peak and the smallest
day, rounded down to a whole dollar or a whole request, and never below zero.
In every other case bars SHALL start at zero. Days without data SHALL still
show "·" and no bar.

#### Scenario: Stress spend differences are visible
- **WHEN** Home is drawn from the stress corpus in priced USD at 80×24
- **THEN** the shortest bar is at most two-thirds the height of the tallest bar

#### Scenario: Default spend keeps a zero baseline
- **WHEN** Home is drawn from the default corpus in priced USD at 80×24
- **THEN** the chart is drawn from zero and its title names no axis start

#### Scenario: Equal days keep a zero baseline
- **WHEN** every day with data has the same value
- **THEN** the chart is drawn from zero

### Requirement: Chart title names a raised baseline
When the chart uses a raised baseline, its title SHALL say where the axis
starts, in the chart's current unit, for example `axis from $1510`.

#### Scenario: Stress title names the axis start
- **WHEN** Home is drawn from the stress corpus in priced USD at 80×24
- **THEN** the chart title contains the axis start as a whole-dollar amount

### Requirement: Chart title fits its width
Home's chart title SHALL never be cut mid-word. When the full title does not
fit, it SHALL use short forms of its parts. If the short title still does not
fit, it SHALL drop trailing parts whole.

#### Scenario: Stress title at 60 columns
- **WHEN** Home is drawn from the stress corpus at 60×24
- **THEN** every word of the chart title is drawn whole and the title states that every day is partial

#### Scenario: Default title unchanged at 80 columns
- **WHEN** Home is drawn from the default corpus at 80×24
- **THEN** the chart title is the same as before this change

### Requirement: Day list columns stay aligned
On a Day list, every session row SHALL put its agent and total in the same
columns as the column header. The workspace column SHALL be as wide as the
longest workspace in the list, but no wider than the room left after the
name, agent and total, and never narrower than 4 characters.

#### Scenario: Stress day at 80 and 60 columns
- **WHEN** the Nov 16 Day list is drawn from the stress corpus at 80×24 and at 60×24
- **THEN** on every visible row the total ends in the same column, and that column is inside the list area

#### Scenario: Default day unchanged
- **WHEN** a Day list is drawn from the default corpus at 80×24
- **THEN** its rows read the same as before this change

### Requirement: Long values shortened in the middle
A workspace, model ID or chip value that does not fit its space SHALL be
shortened by replacing its middle with "…", keeping its start and end.
Values that fit SHALL be shown whole.

#### Scenario: Long workspace in a Day row
- **WHEN** a stress Day row's workspace is longer than the workspace column
- **THEN** the row shows the workspace's start, "…" and its end, at exactly the column width

#### Scenario: Short workspace shown whole
- **WHEN** a workspace fits the workspace column
- **THEN** it is shown in full with no "…"

### Requirement: Scope chips survive a long value
When a scope chip is too wide for the chip row, its value SHALL be shortened
in the middle so the chips after it can still be drawn. A chip SHALL only be
dropped, replaced by "…", when even a shortened chip cannot fit.

#### Scenario: Long workspace in the Session scope
- **WHEN** a stress Session view for the 94-character workspace is drawn at 80×24
- **THEN** the chip row shows the day, a shortened workspace chip, the agent and the cost

### Requirement: Worker rows keep their figures
Worker rows on Session and Tokens views, and in the preview pane, SHALL
shorten a long model ID so the row's cost or token figure stays fully drawn
within its area.

#### Scenario: Long model on a Session view
- **WHEN** a stress Session view with a 63-character model is drawn at 80×24 and at 60×24
- **THEN** that worker row ends with its cost or `unpriced`, fully drawn

#### Scenario: Long model in the preview pane
- **WHEN** a stress Day list at 80×24 selects a session with a 63-character model
- **THEN** the preview pane draws that model's row on one line, ending with its cost

### Requirement: Workspace filter follows the corpus
The inventory workspace filter SHALL cycle "All workspaces" and then each
distinct workspace in the loaded corpus, in sorted order.

#### Scenario: Stress workspaces can be filtered
- **WHEN** the inventory is open on the stress corpus and `w` is pressed six times
- **THEN** each of the six stress workspaces is shown in turn, and a seventh press returns to "All workspaces"

#### Scenario: Default filter unchanged
- **WHEN** the inventory is open on the default corpus and `w` is pressed
- **THEN** it cycles `api`, `web`, then "All workspaces", as before this change

### Requirement: Long lists page to both ends
On a stress Day list of about 300 sessions, PgDn SHALL move the selection
down by one screen until the last session is selected and drawn, and PgUp
SHALL move it back until the first session is selected.

#### Scenario: Page through a stress day
- **WHEN** the stress Nov 16 Day list is open at 80×24 and PgDn is pressed until the selection stops moving
- **THEN** the counter reads the last position (for example `295/295`), the last session is drawn and selected, and pressing PgUp the same number of times returns to `1/295`

### Requirement: Readability under NO_COLOR
Every rule in this capability SHALL hold the same way when colour is off.

#### Scenario: Stress screens without colour
- **WHEN** the stress Home, Nov 16 Day list and long-workspace Session view are drawn with colour off at 80×24
- **THEN** the chart title, Day rows and scope chips meet the same rules as with colour on, and the selected row is still marked by reverse video
