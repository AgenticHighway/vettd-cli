# Spec Delta

## Purpose

The statusline relay: an opt-in `--relay` mode that saves Claude Code's
5-hour and weekly usage readings from its statusline input, so the Lens
preview can read them with `--source local`.

## ADDED Requirements

### Requirement: Relay mode
`lens-preview --relay` SHALL read one JSON object from stdin, record its
`rate_limits.five_hour` and `rate_limits.seven_day` readings, print at most
one line to stdout, and exit 0. It SHALL NOT start the TUI.

#### Scenario: Both windows present
- **WHEN** stdin has `five_hour` at 23.5% and `seven_day` at 41.2%
- **THEN** two records are appended, stdout is one line, and the exit status is 0

### Requirement: Records hold only readings
Each record SHALL be one JSON line with exactly `t` (Unix seconds), `w`
(`five_hour` or `seven_day`), `pct` and `resets_at`. No other stdin field
SHALL be stored, including session ID, paths, workspace, model and cost.

#### Scenario: Identifying fields dropped
- **WHEN** stdin carries `session_id`, `transcript_path`, `cwd` and `cost` with known values
- **THEN** none of those values appear in the relay file

### Requirement: Unchanged readings aren't repeated
A reading whose `pct` and `resets_at` match the last record for that window
SHALL NOT be written again.

#### Scenario: Repeated message
- **WHEN** the relay runs twice with the same `five_hour` reading
- **THEN** the file holds one `five_hour` record

### Requirement: Absent windows are skipped
A missing `rate_limits`, or a missing window within it, SHALL write nothing
for that window. `rate_limits.spend_limit` SHALL be ignored.

#### Scenario: No rate_limits
- **WHEN** stdin has no `rate_limits`
- **THEN** nothing is appended, stdout is empty, and the exit status is 0

### Requirement: Relay file location
The relay SHALL write to `~/.vettd/lens/relay.jsonl`, creating the directory
if missing, unless `--relay-file <path>` names another file. Both the
relay and `--source local` SHALL honour `--relay-file`.

#### Scenario: Test path
- **WHEN** the relay runs with `--relay-file` pointing into a temporary directory
- **THEN** only that file is written, and nothing is created under the home directory

### Requirement: Appends are whole lines
Each record SHALL be appended as one complete line in a single write. A
reader SHALL skip any line that doesn't parse, so a cut-off write from a
cancelled run loses only that line.

#### Scenario: Truncated line
- **WHEN** the relay file ends with half a record
- **THEN** the reader uses every earlier record and ignores the half line

### Requirement: Old records are pruned
At most once a day, the relay SHALL rewrite the file without records older
than 35 days, through a temporary file and a rename in the same directory.
A failed rewrite SHALL leave the old file as it was.

#### Scenario: 40-day-old record
- **WHEN** the relay prunes a file holding records from 40 and 2 days ago
- **THEN** only the 2-day-old records remain

### Requirement: Relay never breaks the status line
The relay SHALL exit 0 on every input. A parse error, a full disk or an
unwritable file SHALL be reported on stderr only. Without `--then`, its
stdout SHALL be `5h <pct>% · 7d <pct>%` for the windows present, or empty.

#### Scenario: Bad stdin
- **WHEN** stdin is not JSON
- **THEN** stdout is empty, stderr explains, nothing is appended, and the exit status is 0

### Requirement: Chaining an existing status line
With `--then <command>`, the relay SHALL first record the readings, then run
that command with the same stdin, and print that command's stdout as its
own.

#### Scenario: Existing statusline kept
- **WHEN** the relay runs with `--then` naming a command that prints "main ✓"
- **THEN** the readings are recorded and stdout is "main ✓"

### Requirement: Lens never edits Claude settings
Neither `--relay` nor any other mode SHALL write Claude Code's settings
files. Setup SHALL be a documented `statusLine` snippet the person adds.

#### Scenario: First run
- **WHEN** the relay runs and no `statusLine` is configured anywhere
- **THEN** no Claude settings file changes

### Requirement: Relay readings feed Claude windows
With `--source local`, the Claude rows SHALL take readings from the relay
file, by the same time-span grouping, running maximum, pace and
reset-since-last-reading rules as Codex windows. Their source SHALL read
"relay".

#### Scenario: Two readings in the 5-hour window
- **WHEN** the relay file holds `five_hour` readings of 20% and then 40% with the same `resets_at`
- **THEN** the Claude 5-hour row shows 40% used, a pace, and ends in "relay"

### Requirement: Relay state is named
When there is no relay file, the Claude rows SHALL say "relay not set up".
When the file has no reading in a window's current span, that row SHALL say
"no relay reading yet". Both SHALL show "—" for used, pace and reset.

#### Scenario: File but stale readings
- **WHEN** the relay file's only `seven_day` reading belongs to a window that has already reset
- **THEN** the Claude weekly row shows "—" and says it reset with no relay reading since

### Requirement: Weekly history from the relay
"Claude weekly ended at" SHALL list, for each of the last 4 weekly resets in
the relay file, the highest `pct` before that reset. With fewer than 4, it
SHALL list those it has and say how many weeks the relay has seen.

#### Scenario: Two weeks recorded
- **WHEN** the relay file spans two weekly resets, ending at 38% and 55%
- **THEN** the row reads "Claude weekly ended at 38%, 55% (2 weeks of relay data)"
