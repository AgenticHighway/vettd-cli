# Spec Delta

## Purpose

Defines how a person moves around the Lens preview: from Home to a day,
from a day to a session, and to any day, session or component by name, in
few key presses.

## ADDED Requirements

### Requirement: Home daily bars are selectable
On Home, ← and → SHALL move a day selection across the seven daily bars.
The first → with no bar selected selects the first day, and the first ←
selects the last day. Selection SHALL stop at the first and last day rather
than wrap. The selected bar SHALL be marked so it can be told apart without
colour. Days without data SHALL be selectable.

#### Scenario: Bar selection without colour
- **WHEN** Home is drawn under `NO_COLOR` with a bar selected
- **THEN** the selected bar's day is marked by a character, not only by colour

#### Scenario: Selection stops at the last day
- **WHEN** Nov 18 is selected and the person presses →
- **THEN** Nov 18 stays selected

### Requirement: A selected bar opens its day
While a bar is selected, Enter SHALL open that day's sessions and the Enter
hint SHALL name that day. ↑ or ↓ SHALL return the selection to Home's
findings, on the finding that was selected before. A day without data SHALL
open to a day with no session input.

#### Scenario: Open a day from Home in few presses
- **WHEN** the default corpus is on Home at 80×24 and the person presses →, →, Enter
- **THEN** Nov 13's sessions are shown

#### Scenario: Back to the same bar
- **WHEN** the person opens Nov 13 from Home's bars and presses Esc
- **THEN** Home is shown with the Nov 13 bar still selected

#### Scenario: Findings keep their selection
- **WHEN** the second finding is selected, the person presses ← and then ↓
- **THEN** the selection is back in the findings on the second finding, and no bar is marked

### Requirement: Number keys open a day
On Home and Week, the keys 1 to 7 SHALL open the sessions of the period's
first to seventh day (1 is Nov 12, 7 is Nov 18). On other screens these keys
SHALL do nothing, except in Find, where they are typed into the search.

#### Scenario: Jump to a day from Home
- **WHEN** the default corpus is on Home and the person presses 5
- **THEN** Nov 16's sessions are shown, and Esc returns to Home

#### Scenario: Digits are typed in Find
- **WHEN** the person is in Find and presses 1 and 6
- **THEN** the search reads "16" and no day opens

### Requirement: Find matches days, sessions and components
With a non-empty search, Find SHALL list matching days, then sessions, then
components, each group under a divider that names it and its count. Matching
is a case-insensitive substring match: a day on its label (for example
"Nov 16"), a session on its name or workspace, a component as today.
Sessions SHALL be ordered by day, then by the current measure, largest
first. With an empty search Find SHALL list components only, as before.

#### Scenario: Empty search is unchanged
- **WHEN** Find is opened and nothing is typed
- **THEN** only components are listed, in the same order as before

#### Scenario: A workspace finds its sessions
- **WHEN** the default corpus is in Find and the person types "api"
- **THEN** every session in workspace api is listed under a Sessions divider with its count

#### Scenario: Many matches at stress scale
- **WHEN** the stress corpus is in Find at 80×24 and the person types one common letter
- **THEN** the results draw without panic and every group divider shows its count

### Requirement: Find results open their target
Each session result SHALL show its day, name, workspace and total. Opening a
day result SHALL show that day's sessions, and opening a session result SHALL
show that session. Esc SHALL return to Find with the search and selection
kept. Find's title SHALL say it finds days, sessions and components.

#### Scenario: Find a session by name
- **WHEN** the default corpus is in Find and the person types "index"
- **THEN** the Index rebuild session is listed under a Sessions divider, and Enter opens it

#### Scenario: Find a day
- **WHEN** the person types "nov 16" in Find
- **THEN** Nov 16 is listed under a Days divider, and Enter opens its sessions

#### Scenario: Back to the search
- **WHEN** the person opens a session result and presses Esc
- **THEN** Find is shown with the same search and the same result selected

### Requirement: Jump to the first or last row
On every list screen and on Home's findings, Home and End SHALL move the
selection to the first or last selectable row and scroll so the list's first
or last row is shown. g and G SHALL do the same everywhere except Find,
where they are typed into the search. A list with no selectable rows SHALL
scroll to its top or bottom.

#### Scenario: End of a long Day list
- **WHEN** the stress Nov 16 list is shown at 80×24 and the person presses End
- **THEN** the last session is selected and the list's last row is visible

#### Scenario: Back to the top
- **WHEN** the person then presses g
- **THEN** the first session is selected and the list's first row is visible

### Requirement: New keys are discoverable
The `?` help SHALL list ←/→, 1–7, Home/End and g/G with what each does. At
widths of 65 columns and more, Home's footer SHALL name ←/→ for days and
Week's footer SHALL name 1–7. Footers SHALL still fit their width.

#### Scenario: Home footer names the day keys
- **WHEN** Home is drawn at 80×24
- **THEN** the footer names ←/→ as moving between days

#### Scenario: Narrow footer still fits
- **WHEN** Home is drawn at 60×24
- **THEN** no footer item is cut
