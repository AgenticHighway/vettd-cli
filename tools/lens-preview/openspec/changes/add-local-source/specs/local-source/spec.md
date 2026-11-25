# Spec Delta

## Purpose

Where the Lens preview's data comes from: the synthetic fixture or the
person's own local Claude Code and Codex logs, read offline and read-only.

## ADDED Requirements

### Requirement: Source selection
The preview SHALL accept `--source <name>`, where the name is `fixture` or
`local`. With no `--source`, it SHALL use `fixture`. An unknown name SHALL be
rejected with a message on stderr naming the valid sources, nothing on
stdout, and a non-zero exit.

#### Scenario: Fixture by default
- **WHEN** the preview runs with `--json` and no `--source`
- **THEN** the export's `source` field is `"fixture"` and every existing fixture figure is unchanged

#### Scenario: Unknown source
- **WHEN** the preview runs with `--source cloud`
- **THEN** stderr names `fixture` and `local`, stdout is empty, and the exit status is non-zero

### Requirement: Source and corpus don't mix
`--corpus` SHALL be accepted only with `--source fixture`. `--source local`
together with any `--corpus` SHALL be rejected with a message on stderr,
nothing on stdout, and a non-zero exit. This applies once `--corpus` exists.

#### Scenario: Local with a corpus
- **WHEN** the preview runs with `--source local --corpus stress`
- **THEN** stderr says `--corpus` needs `--source fixture`, stdout is empty, and the exit status is non-zero

### Requirement: Local period
With `--source local`, the period SHALL be the 7 UTC days ending on the day
the run starts, and "now" SHALL be the system clock, read once. Each day
label SHALL carry its own month, and days SHALL sort by date.

#### Scenario: Period across a month end
- **WHEN** the run starts on Dec 2
- **THEN** the daily view lists Nov 26 through Dec 2 in date order, each label naming its month

### Requirement: Files outside the history span are skipped
Log files last modified more than 28 days before the run SHALL NOT be opened.
Files modified within 28 days SHALL be read for limit hits. Only requests in
the 7-day period SHALL count towards usage.

#### Scenario: Old and recent files
- **WHEN** one file was last modified 30 days ago and another 20 days ago, with no request in the period
- **THEN** the first is not opened, the second's limit hits count, and neither adds usage

### Requirement: Requests count on their own day
Each request SHALL count on the UTC day of its own timestamp, not the day its
session started. A session with requests on several days SHALL appear on
each of those days, and SHALL count once in Home's session total. Requests
before the period SHALL NOT count.

#### Scenario: Session past midnight
- **WHEN** a session has 3 requests on Nov 17 and 2 on Nov 18
- **THEN** Nov 17 shows it with 3 requests, Nov 18 with 2, and Home counts 1 session

### Requirement: Where local logs are read
Local data SHALL be read from `~/.claude/projects/*/*.jsonl` with each
session's `subagents/*.jsonl`, and from `~/.codex/sessions/**/*.jsonl`
and `~/.codex/archived_sessions/*.jsonl`. A Codex session found in both
SHALL count once, by `session_meta.payload.id`.
`CLAUDE_CONFIG_DIR` and `CODEX_HOME`, when set, SHALL replace `~/.claude`
and `~/.codex`. A missing directory SHALL give that agent no sessions and a
note on stderr, not an error.

#### Scenario: No Codex directory
- **WHEN** `--source local` runs and the Codex sessions directory doesn't exist
- **THEN** stderr says which directory was not found, Claude sessions still load, and the exit status is zero

#### Scenario: Archived session
- **WHEN** a Codex session in the period was moved to `archived_sessions/`
- **THEN** its requests still count, once

### Requirement: Claude requests are counted once
A Claude request SHALL be one distinct `requestId` (or `message.id` when it
has none) across all Claude files, because resumed and forked sessions copy
earlier lines. Its usage SHALL come from its line with the most output
tokens (the last such line on a tie), never summed across lines.

#### Scenario: Streamed request over three lines
- **WHEN** one `requestId` appears on three assistant lines with output tokens 4, 310 and 4
- **THEN** it counts as one request with 310 output tokens

#### Scenario: Copied into a resumed session
- **WHEN** the same `requestId` appears in two session files
- **THEN** it counts once, in the session whose file has the earliest line for it

### Requirement: Claude placeholder lines aren't requests
Assistant lines whose model is `<synthetic>`, or that are API error
messages, SHALL NOT count as requests or add usage. They SHALL still be read
for limit hits.

#### Scenario: Rejected request
- **WHEN** a session has a `<synthetic>` API error line carrying a `five_hour` rejection
- **THEN** it adds no request and no tokens, and counts as one Claude limit hit

### Requirement: Claude sessions include subagents
A Claude session SHALL be one top-level session file plus the subagent files
that belong to it. A subagent line SHALL belong to the session its
`sessionId` names when that session's file exists, and to the session folder
it sits in otherwise. A session's workspace SHALL be the last component of
`cwd`.

#### Scenario: Subagent tokens roll up
- **WHEN** a session file has 2 requests and its subagents directory holds a file with 3
- **THEN** Lens shows one Claude session with 5 requests

#### Scenario: Subagent filed under another session
- **WHEN** a subagent file sits in session A's folder but its lines name session B, whose file exists
- **THEN** its requests count in session B

### Requirement: Codex requests from usage records
A Codex request SHALL be one `token_usage_record` line, deduplicated by
`response_id`, using its `usage` tokens. Its model SHALL be the `model` of
the latest `turn_context` at or before it. A thread SHALL count in the
session at the root of its `parent_thread_id` chain, or as its own session
when that parent isn't on disk.

#### Scenario: Subagent thread joins its root
- **WHEN** a subagent rollout names a `parent_thread_id` whose rollout is also on disk
- **THEN** its requests are counted in that root thread's session

### Requirement: Codex token parts
For a Codex request, Fresh input SHALL be `input_tokens` minus
`cached_input_tokens`, Cache read `cached_input_tokens`, and Output
`output_tokens`, which already includes reasoning. The session SHALL be
marked incomplete when `input_tokens + output_tokens` differs from
`total_tokens`, or `cache_write_input_tokens` is above 0, since how cache
writes are counted is unknown.

#### Scenario: Totals disagree
- **WHEN** a usage record's `input_tokens + output_tokens` differs from its `total_tokens`
- **THEN** its session reads as partial, and its tokens still count

#### Scenario: Cache write present
- **WHEN** a usage record has `cache_write_input_tokens` of 500
- **THEN** its session reads as partial rather than guessing where those tokens belong

### Requirement: Codex windows from rate_limits
Codex windows SHALL come from `rate_limits.primary` and `secondary` in
`token_count` events. A side with `window_minutes` 300 SHALL be the 5-hour
window, and 10080 the weekly window, whichever side carries it. Null sides
and other lengths SHALL be skipped.

#### Scenario: Sides swapped
- **WHEN** an event carries `window_minutes` 10080 in `primary` and 300 in `secondary`
- **THEN** the 10080 reading goes to the Codex weekly row and the 300 reading to the Codex 5-hour row

### Requirement: Codex window readings
A Codex window's current instance SHALL end at the latest `resets_at` and
start `window_minutes` earlier. Its readings SHALL be every event in that
span, across all sessions, in time order. Readings SHALL NOT be grouped by
exact `resets_at`, which drifts by seconds between events.

#### Scenario: Drifting reset time
- **WHEN** two events in the same weekly window report `resets_at` values 16 seconds apart
- **THEN** both are readings of one Codex weekly window

### Requirement: Codex readings never go down within a window
Each reading SHALL be taken as the running maximum of `used_percent` so far
in its window, because concurrent sessions report slightly older figures.
The used share SHALL be the last such value.

#### Scenario: Two sessions out of step
- **WHEN** readings in one window arrive as 30%, then 27% from another session, then 33%
- **THEN** the readings are 30%, 30% and 33%, and the row shows 33% used

### Requirement: A window that has reset says so
When the current instance's reset time is at or before "now", the window
SHALL show "—" for used and pace, and say it reset with no reading since.
It SHALL NOT show the earlier window's share as current.

#### Scenario: Reset since last reading
- **WHEN** the Codex 5-hour window's latest `resets_at` is 40 minutes before now
- **THEN** its row shows "—" for used and pace and says it reset with no reading since

### Requirement: Limit hits from logs
On local data, limit hits SHALL be counted per agent over the last 28 days.
Claude: distinct requests rejected with `rateLimitType` `five_hour`, across
all files. Codex: turns that ended with `usage_limit_exceeded`, by `turn_id`.

#### Scenario: Rejection copied into a resumed session
- **WHEN** the same rejected `requestId` appears in two Claude session files
- **THEN** it counts as one Claude limit hit

### Requirement: Rates from a dated table
Prices SHALL come from a rate table in the preview's source. Each entry
SHALL give a model-ID prefix, rates for fresh input, cache read, 5-minute
cache write, 1-hour cache write and output, the date they were checked and
where they were published. A model SHALL use its longest matching prefix.

#### Scenario: Longest prefix wins
- **WHEN** the table has entries for `claude-opus` and `claude-opus-5`, and a request's model is `claude-opus-5-5`
- **THEN** the request is priced at the `claude-opus-5` rates

#### Scenario: 1-hour cache writes
- **WHEN** a Claude request's `cache_creation` holds 1,000 `ephemeral_1h_input_tokens` and 200 `ephemeral_5m_input_tokens`
- **THEN** the 1,000 tokens use the 1-hour rate and the 200 the 5-minute rate

### Requirement: Unknown prices stay unpriced
A request SHALL have no rate, never a zero rate, when its model matches no
entry, or, for Claude, its cache writes lack the 5-minute / 1-hour split
or its `speed` or `service_tier` isn't `standard`. Totals SHALL leave it out,
and its day and session SHALL read as partial in USD mode.

#### Scenario: New model
- **WHEN** a local session's only requests use a model with no rate entry
- **THEN** the session shows "unpriced", never "$0.00"

### Requirement: Rates cover both agents
The rate table SHALL hold the published API rates for the Claude and Codex
(OpenAI) models it names. A Codex request SHALL use its fresh input, cache
read and output rates. A model with no published API rate, such as
`codex-auto`, SHALL have no entry.

#### Scenario: Codex priced
- **WHEN** a Codex request's model matches a table entry
- **THEN** the request is priced from that entry, the same as a Claude request

### Requirement: Dollars are labelled API-equivalent
On local data, every dollar figure SHALL be labelled "API-equivalent",
never "priced" or "spent", because the person may pay through a plan rather
than per token. The fixture's labels SHALL stay as they are.

#### Scenario: Home on local data
- **WHEN** a session view is shown with `--source local` for a session with priced requests
- **THEN** its dollar figure is followed by "API-equivalent"

### Requirement: Session titles in the TUI
On local data, a session's name SHALL be its latest title: Claude's latest
`custom-title` or `ai-title` line, or the Codex `session_index.jsonl` entry
for its root thread with the latest `updated_at`. A session with no title
SHALL be named by its start time. Titles SHALL appear only in the TUI.

#### Scenario: Renamed session
- **WHEN** a Claude session has an `ai-title` line and two later `custom-title` lines
- **THEN** its row shows the last `custom-title`

#### Scenario: No title
- **WHEN** a Codex root thread has no `session_index.jsonl` entry
- **THEN** its session is named by its start time, such as "09:12"

### Requirement: Unreadable input is partial
A session SHALL be marked incomplete when any of its lines fails to parse,
its last line is cut off, or a request it holds lacks usage. Its readable
requests SHALL still count. No such line SHALL stop the preview.

#### Scenario: File still being written
- **WHEN** a session file's last line is a truncated JSON object
- **THEN** the session reads as partial, and its earlier requests are counted

### Requirement: Offline and read-only
With `--source local`, the preview SHALL open log files for reading only,
SHALL write no file, and SHALL make no network connection.

#### Scenario: No writes or network
- **WHEN** `--source local --json` runs
- **THEN** no file under the Claude or Codex directories changes, no file is created, and no socket is opened

### Requirement: Export holds aggregates only
A local export SHALL hold only aggregates: per session, its days, workspace
label, agent and incomplete flag, and per request the day, model and token
counts. It SHALL NOT hold prompts, replies, tool data, titles, paths, folder
names, git, account or plan data, or any session, request or thread ID.

#### Scenario: No identifiers in the export
- **WHEN** `--source local --json` runs on hand-written test logs with known IDs, paths and prompt text
- **THEN** none of those strings appear in stdout

### Requirement: Folder names are never labels
Claude project folder names, which spell out full paths, SHALL NOT be used
as workspace labels or shown anywhere. The workspace label SHALL come only
from the last component of a line's `cwd`.

#### Scenario: Project folder ignored
- **WHEN** a session sits in a project folder named after its full path, and its lines' `cwd` ends in `api`
- **THEN** the workspace label is `api`, and the folder name appears in no screen or export

### Requirement: No real records in the repository
Tests, test data and docs SHALL use only hand-written log lines that mimic
the observed shapes. No line copied from a real local log SHALL be committed.

#### Scenario: Reader tests
- **WHEN** the reader tests run
- **THEN** they read only files under the preview's own test data or inline strings
