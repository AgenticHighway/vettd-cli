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

### Requirement: Local period
With `--source local`, the period SHALL be the 7 UTC days ending on the day
the run starts, and "now" SHALL be the system clock, read once at start.
Log files last modified before the period starts SHALL NOT be opened.

#### Scenario: Old file skipped
- **WHEN** a session file was last modified 10 days before the run
- **THEN** it is not opened, and none of its requests count

### Requirement: Where local logs are read
Local data SHALL be read from `~/.claude/projects/*/*.jsonl` with each
session's `subagents/*.jsonl`, and from `~/.codex/sessions/**/*.jsonl`.
`CLAUDE_CONFIG_DIR` and `CODEX_HOME`, when set, SHALL replace `~/.claude`
and `~/.codex`. A missing directory SHALL give that agent no sessions and a
note on stderr, not an error.

#### Scenario: No Codex directory
- **WHEN** `--source local` runs and the Codex sessions directory doesn't exist
- **THEN** stderr says which directory was not found, Claude sessions still load, and the exit status is zero

### Requirement: Claude requests are counted once
A Claude request SHALL be one distinct `requestId` (or `message.id` when it
has none) on `assistant` lines. Its usage SHALL be taken from its last line
in file order, never summed across lines. Lines whose model is `<synthetic>`
or that are API error messages SHALL NOT count as requests.

#### Scenario: Streamed request over three lines
- **WHEN** one `requestId` appears on three assistant lines with output tokens 4, 4 and 310
- **THEN** it counts as one request with 310 output tokens

### Requirement: Claude sessions include subagents
A Claude session SHALL be one top-level session file plus every file in its
`subagents/` directory. Its workspace SHALL be the last path component of
`cwd`, and its day the UTC day of its first request.

#### Scenario: Subagent tokens roll up
- **WHEN** a session file has 2 requests and its subagents directory holds a file with 3
- **THEN** Lens shows one Claude session with 5 requests

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
For a Codex request, Cache read SHALL be `cached_input_tokens`, Cache write
`cache_write_input_tokens`, Output `output_tokens`, and Fresh input
`input_tokens` minus `cached_input_tokens`. When the four parts don't
reconcile with `total_tokens`, the session SHALL be marked incomplete rather
than corrected.

#### Scenario: Totals disagree
- **WHEN** a usage record's parts don't add up to its `total_tokens` under that rule
- **THEN** its session reads as partial, and its tokens still count

### Requirement: Codex windows from rate_limits
Codex windows SHALL come from `rate_limits.primary` and `secondary` in
`token_count` events. A side with `window_minutes` 300 SHALL be the 5-hour
window, and 10080 the weekly window, whichever side carries it. Null sides
and other lengths SHALL be skipped.

#### Scenario: Sides swapped
- **WHEN** an event carries `window_minutes` 10080 in `primary` and 300 in `secondary`
- **THEN** the 10080 reading goes to the Codex weekly row and the 300 reading to the Codex 5-hour row

### Requirement: Codex window readings
A Codex window's current instance SHALL be the one with the latest
`resets_at`, starting at `resets_at` minus `window_minutes`. Its readings
SHALL be every event timestamp and `used_percent` for that `resets_at`,
across all sessions, in time order.

#### Scenario: Readings from two sessions
- **WHEN** two Codex sessions each report the 5-hour window with the same `resets_at`, at 12% and later 30%
- **THEN** the Codex 5-hour row has 2 readings and shows 30% used with a pace

### Requirement: Limit hits from logs
On local data, limit hits SHALL be counted over the last 28 days, per agent:
Claude requests rejected with `rateLimitType` `five_hour`, and Codex turns
that ended with `usage_limit_exceeded`. A hit SHALL be counted once per
request or turn.

#### Scenario: Repeated rejection lines
- **WHEN** one Claude request has two lines carrying the same `five_hour` rejection
- **THEN** it counts as one Claude limit hit

### Requirement: Rates from a dated table
Request prices SHALL come from a rate table in the preview's source. Each
entry SHALL give a model-ID prefix, the four per-part rates, the date they
were checked and where they were published. A model SHALL use its longest
matching prefix.

#### Scenario: Longest prefix wins
- **WHEN** the table has entries for `claude-opus` and `claude-opus-5`, and a request's model is `claude-opus-5-5`
- **THEN** the request is priced at the `claude-opus-5` rates

### Requirement: Unknown models stay unpriced
A request whose model matches no rate entry SHALL have no rate, never a zero
rate. Totals SHALL leave it out of priced USD, and its day and session SHALL
read as partial in USD mode, as unpriced fixture requests do.

#### Scenario: New model
- **WHEN** a local session's only requests use a model with no rate entry
- **THEN** the session shows "unpriced", never "$0.00"

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
A local export SHALL hold only aggregates: per session, the day, workspace
label, agent, incomplete flag, and per request the model and four token
counts. It SHALL NOT hold prompts, replies, tool input or output, titles,
full paths, git data, account or plan data, or any session, request or
thread ID.

#### Scenario: No identifiers in the export
- **WHEN** `--source local --json` runs on hand-written test logs with known IDs, paths and prompt text
- **THEN** none of those strings appear in stdout

### Requirement: No real records in the repository
Tests, test data and docs SHALL use only hand-written log lines that mimic
the observed shapes. No line copied from a real local log SHALL be committed.

#### Scenario: Reader tests
- **WHEN** the reader tests run
- **THEN** they read only files under the preview's own test data or inline strings
