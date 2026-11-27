# Spec Delta

## Purpose

Defines the History screen: a list of every session the preview can find,
however old, kept apart from the 7-day dashboard, and how a person reaches,
filters and leaves it.

## ADDED Requirements

### Requirement: History lists every session
History SHALL list every session found by the selected source, one row per
session, newest first by start time. On `--source local` that is every
session in the Claude and Codex logs on disk, with no age limit. On
`--source fixture` it is the selected corpus's sessions. Ties SHALL be
broken by agent, then name, so the order is the same on every run.

#### Scenario: Session older than the history span
- **WHEN** `--source local` runs and a Claude session's only file was last modified 90 days ago
- **THEN** History lists that session, and the dashboard's figures don't include it

#### Scenario: Stress corpus
- **WHEN** History is opened on `--corpus stress`
- **THEN** it lists 1991 sessions, the first being the latest to start

### Requirement: What a History row shows
Each row SHALL show the session's start date (with its month), agent, name,
request count and dollars. On local data the dollar figure SHALL be
API-equivalent and labelled as on the rest of the screen, and a session with
no priced request SHALL read "unpriced", never "$0.00". A session marked
incomplete SHALL carry the same partial flag as on its session view. At 80
columns a row SHALL fit the list pane: the name is cut with "…" to fit, and
the date, request count and dollars are never cut.

#### Scenario: Long name at 80 columns
- **WHEN** History is drawn at 80×24 and a session's name is longer than its column
- **THEN** the name ends in "…", and the row's date, requests and dollars are shown in full

#### Scenario: Unpriced session
- **WHEN** a local session's only requests use a model with no rate entry
- **THEN** its History row reads "unpriced" for dollars

### Requirement: Reaching and leaving History
On Home and Week, `h` SHALL open History. Esc SHALL return to the screen it
was opened from, with that screen's selection unchanged. The footer and `?`
help SHALL name `h` on Home and Week.

#### Scenario: Open from Home and return
- **WHEN** the second finding is selected on Home, the person presses `h` and then Esc
- **THEN** Home is shown with the second finding still selected

#### Scenario: Letters are typed in Find
- **WHEN** the person is in Find and presses `h`
- **THEN** the search reads "h" and History does not open

### Requirement: Filtering History
Typing in History SHALL filter its rows with a case-insensitive substring
match on the session's name, workspace, agent and date label, as Find
matches. Backspace SHALL remove the last character. The title SHALL show the
filter and how many sessions match. With no matches the list SHALL say so,
rather than show an empty pane.

#### Scenario: Filter by month
- **WHEN** History is open on local data and the person types "Oct"
- **THEN** only sessions whose date label, name, workspace or agent contains "oct" are listed, and the title shows the match count

#### Scenario: Nothing matches
- **WHEN** the filter matches no session
- **THEN** the list says no session matches the filter

### Requirement: A History row opens its session
Enter on a History row SHALL open that session's session view, with its
figures taken from all of its requests, not only those in the dashboard's
period. Esc from that view SHALL return to History with the same row
selected and the same filter.

#### Scenario: Session that spans the period's start
- **WHEN** a session has 4 requests the day before the period and 2 on its first day, and it is opened from History
- **THEN** its session view counts 6 requests

#### Scenario: Back to the same row
- **WHEN** the person filters History, opens the third row and presses Esc
- **THEN** History shows the same filter with the third row selected

### Requirement: History says when it is still reading
On `--source local`, reading for History SHALL NOT delay the first screen
or the export. While it is still reading, History SHALL say so and show how
many log files it has read out of how many it found, and SHALL list no rows.
When reading finishes, the list SHALL appear without a key press. A file
that can't be read SHALL mark its sessions partial, as on the dashboard, and
SHALL NOT stop the read.

#### Scenario: Opened while reading
- **WHEN** the person presses `h` before History has finished reading
- **THEN** History says it is reading and shows files read out of files found, and the rows appear once it finishes

#### Scenario: Export unaffected
- **WHEN** `--source local --json` runs
- **THEN** the export is printed without waiting for History, and holds no History field

### Requirement: History stays private
History SHALL show session names only in the TUI, as the rest of the
preview does. Nothing from History SHALL be written to stdout, a file or the
network, and the JSON export SHALL NOT change because of it.

#### Scenario: Export unchanged
- **WHEN** `--json` runs on the default corpus
- **THEN** stdout is byte-identical to `tests/golden/default-export.json`
