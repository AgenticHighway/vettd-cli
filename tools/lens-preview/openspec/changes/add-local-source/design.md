# Design

## Context

The preview builds everything from `src/fixture/`. `main.rs` calls
`fixture::sessions()`. `plan.rs` reads `fixture::WINDOWS`, `NOW`,
`CLAUDE_WEEKLY_ENDS` and `LIMIT_HITS` directly. Home reads
`fixture::SOURCE_SHARES`, and the top bar in `screen/mod.rs` hard-codes
"synthetic data · nothing leaves this machine". The model assumes the fixed
period Nov 12–18 (`Scope::WEEK`, `PERIOD`, `View::Day(18)` for "today") and
uses `&'static str` for workspace, agent, model and session names.

The `add-lens-tui-dashboard` design lists real Codex `rate_limits` and
Claude statusline reading as non-goals, and its Plan headroom spec has a
"Synthetic window data" requirement. This change replaces that requirement.

## Observed log formats

The formats below were inspected on 2026-11-24, on the machine this change
was written on. The check was read-only, and the script printed only key
paths, value types, enum-like `type` values, model-family prefixes and
counts. No values, prompts or paths were copied. The figures say what one
machine holds, not what every install holds.

### Claude Code: `~/.claude/projects/<project>/<session>.jsonl`

460 files were read: 167 top-level session files and 293 subagent files
under `<session>/subagents/`. All 250k+ lines parsed. Each line is one JSON
object with a `type`.

- `type: "assistant"` (83,155 lines) carries `sessionId`, `timestamp`,
  `cwd`, `isSidechain`, `requestId` (missing on 29 lines), `message.id`,
  `message.model` and `message.usage`.
- `message.usage` always has `input_tokens`, `output_tokens`,
  `cache_read_input_tokens` and `cache_creation_input_tokens`. It usually
  also has `cache_creation.ephemeral_5m_input_tokens` / `_1h_`,
  `output_tokens_details.thinking_tokens`, `server_tool_use.*`,
  `service_tier` and `speed`.
- **One API response is written as several lines,** one per content block
  (`apiBlockIndex`). 83,155 assistant lines held only 22,519 distinct
  `requestId`s. Within 6,913 of them, the lines disagree on usage, because
  output tokens fill in as the response streams. Counting lines, or summing
  usage across lines, over-counts.
- `message.model` includes `"<synthetic>"` on 96 lines. Those are Claude
  Code's own placeholders for errors, not API requests.
- **No per-request cost is recorded.** `costUSD` was absent on every line.
  138 `cost-state` lines carry a session-level `totalCostUSD`,
  `modelUsage` and `hasUnknownModelCost`. They can't be split per request
  or per day.
- **Claude rate-limit windows are not in the logs.** Only rejections appear:
  `quotaLimits` (48 lines) and `apiErrorParams.rate_limit_info` (28 lines),
  with `rateLimitType` `five_hour` or `seven_day`, `status: "rejected"` and
  a `resetsAt`. There is no used percent.
- Titles (`custom-title`, `ai-title`) and the attribution fields
  (`attributionSkill`, `attributionMcpTool`, `attributionPlugin`) exist.
  This change doesn't use them. See Open Questions.
- No `statusLine` is configured in `~/.claude/settings.json` on this
  machine, so no relay data exists to test against.

### Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`

68 files were read, and every line parsed. Each line has `type`,
`timestamp`, `ordinal` and `payload`. 4 more files sit in
`~/.codex/archived_sessions/`.

- `session_meta` (1 per file): `payload.id`, `session_id`, `cwd`,
  `thread_source` (`user` 14, `subagent` 50, `guardian_review` 4) and
  `parent_thread_id` (54 of 68). It also holds git remote, account IDs and
  base instructions, which Lens must never read into its model.
- `turn_context` (342): `payload.model` (families `gpt-6`, `gpt-6.1`,
  `gpt-6.2`, `codex-auto`) and `turn_id`.
- `token_usage_record` (6,240): `payload.response_id`, `turn_id` and
  `usage`, `turn_token_usage` and `thread_token_usage`, each with
  `input_tokens`, `cached_input_tokens`, `cache_write_input_tokens`,
  `output_tokens`, `reasoning_output_tokens` and `total_tokens`.
- `event_msg` / `token_count` (6,335): `payload.info` (null once), with
  `last_token_usage` and `total_token_usage` (the same fields),
  `last_usage_includes_reasoning` and `model_context_window`. It also has
  `payload.rate_limits` (null once), with:
  - `primary` / `secondary`: `used_percent` (float), `window_minutes`
    (int) and `resets_at` (int). Both were null on 43 events. Wherever they
    were present, primary was 300 and secondary 10080.
  - `plan_type`, `limit_id`, `credits.{balance,has_credits,unlimited}`.
    `limit_name`, `individual_limit`, `rate_limit_reached_type` and
    `spend_control_reached` were always null.
- `event_msg` / `task_complete` (329): `error.codex_error_info` was
  `usage_limit_exceeded` 43 times and `invalid_prompt` once.
- **No cost is recorded anywhere.**

### Unknown or absent fields

These shape the requirements, because Lens must not guess any of them.

| Wanted | Claude Code | Codex |
|---|---|---|
| Per-request USD | absent | absent |
| 5-hour / weekly used % | absent (relay only) | present |
| Window start | absent | derived: `resets_at − window_minutes` |
| Limit hits | rejections only | `usage_limit_exceeded` |
| Token source (file reads, skills, …) | absent | absent |
| Fresh vs cached input | separate fields | **unknown** whether `input_tokens` includes `cached_input_tokens` |
| Reasoning in output | `thinking_tokens` | **unknown** whether `output_tokens` includes `reasoning_output_tokens` |
| `resets_at` unit and zone | — | **unknown** (assumed Unix seconds, UTC) |
| Which copy of a skill ran | absent | absent |

## Goals / Non-Goals

**Goals:**
- `--source local` gives real sessions, requests, tokens, priced USD and
  Codex windows on Home and Plan headroom. It never shows a number the logs
  don't support.
- `--source fixture` stays the default and is byte-for-byte today's preview,
  apart from the new `source` export field. Every existing test still runs
  on the fixture.

**Non-Goals:**
- The Claude statusline relay. It is a later change (Decision 4).
- Real token-source attribution, a real component inventory, copies or load
  order.
- Watching files while the TUI runs. Logs are read once at start.
- Reading `archived_sessions/`, Claude's `cost-state` totals, titles or
  attribution fields.
- The main vettd-cli crate.

## Decisions

1. **A `--source` option, separate from `--corpus`.** `--source` says where
   data comes from, and `--corpus` (from `add-stress-corpus`) says which
   synthetic dataset to use. `--corpus` is valid only with
   `--source fixture`. `--source local --corpus …` is rejected on stderr
   with a non-zero exit. The alternative is one option,
   `--source fixture|stress|local`, which is simpler to parse but mixes two
   meanings (Open Question 1).
2. **The period on local data is the 7 UTC days ending today,** at the time
   the run starts. "Now" is the system clock, read once. Files whose
   modification time is before the period start are skipped without being
   opened. The fixture keeps its fixed Nov 12–18 and its fixed `NOW`.
3. **Requests and their usage.**
   - Claude: one request per distinct `requestId` (or `message.id` when it
     has none). Its usage is the usage on the last line for that request in
     file order, because lines are written as the response streams.
     `<synthetic>` model lines and `isApiErrorMessage` lines are not
     requests. The parts map to the preview's four: Fresh input =
     `input_tokens`, Cache read = `cache_read_input_tokens`, Cache write =
     `cache_creation_input_tokens`, Output = `output_tokens`.
   - Codex: one request per `token_usage_record`, deduplicated by
     `response_id`, using `payload.usage`. Its model is the `model` of the
     latest `turn_context` at or before it. Cache read =
     `cached_input_tokens`, Cache write = `cache_write_input_tokens`, Output
     = `output_tokens`, and Fresh input = `input_tokens −
     cached_input_tokens`. That subtraction is an assumption (see
     "Unknown or absent fields"), so the implementation must check it
     against `total_tokens` and treat the session as partial when they
     disagree.
4. **The Claude statusline relay is a later change.** The relay needs an
   opt-in setting in the user's Claude config and a small process that
   writes readings somewhere Lens can read. That is a side effect with its
   own privacy review. This machine has no statusline configured, so there
   is nothing real to test against. Without the relay, both Claude rows on
   local data show "—" for used, pace and reset, with the source "needs
   relay". They open nothing, and the answer only considers windows with
   readings. The rejection events in Claude logs give a reset time but no
   share, so they aren't used as a reading.
5. **Codex windows are matched by length, not by name.** `window_minutes`
   300 is the 5-hour row and 10080 the weekly row, whichever of
   `primary` / `secondary` carries it. The current window is the one with
   the latest `resets_at`. Its start is `resets_at − window_minutes`, and
   its readings are every (`timestamp`, `used_percent`) for that
   `resets_at`, across all sessions. A null side, or another length, is
   skipped and noted on stderr. The pace rule is unchanged: fewer than 2
   readings shows "—".
6. **Limit hits are counted in the same 4-week span the history row
   describes.** Claude: distinct requests with `quotaLimits.status ==
   "rejected"` or `apiErrorParams.rate_limit_info.rateLimitType ==
   "five_hour"`. Codex: `task_complete` with `codex_error_info ==
   "usage_limit_exceeded"`. The row names each agent separately, because
   they are different limits. The "Claude weekly ended at" row needs the
   relay. On local data it says so.
7. **Pricing is a rate table compiled into the preview** (`src/pricing.rs`).
   Each entry has a model-ID prefix, four per-million-token rates, the date
   the rates were checked, and the published page they came from. A model
   matches the longest prefix. A model with no entry is unpriced, and its
   requests have no rate, never a zero rate. That is the existing
   "unpriced" state, so a day or session holding such a request already
   reads as partial in USD. Codex models get no entries in this change (Open
   Question 3). Claude Code's `cost-state` totals aren't used, because they
   can't be split per day or per request.
8. **Session identity.** A Claude session is the top-level file's
   `sessionId`, with `subagents/*.jsonl` added in. A Codex session is the
   root of a `parent_thread_id` chain. A thread whose parent isn't on disk
   is its own session. The day is the UTC day of the session's first
   request. The workspace is the last path component of `cwd`. The session
   name is the start time ("09:12"), not a title (Open Question 2).
9. **Partial, not dropped.** A session is incomplete, using the existing
   flag, when any line in it fails to parse, its last line is cut off
   (still being written), a Claude request has no usage, or the Codex
   token totals don't reconcile. The rest of its requests still count.
10. **What stays synthetic on local data, and how it is labelled.**
    - Top bar: "local logs · read-only · nothing leaves this machine", in
      place of "synthetic data · …".
    - Home's "Where tokens went" table keeps the fixture split, titled
      "Where tokens went (illustrative)", because the logs don't record
      sources. Hiding it instead is Open Question 4.
    - The component inventory, component, copy, observation, change and
      apply views keep fixture data and add a "synthetic" scope chip. Local
      sessions have no `observed` uses, so a local session's component rows
      are empty rather than borrowed from the fixture.
    - The fixed "review-kit: unknown load in Codex" finding is only shown
      on the fixture.
11. **Missing data says so.** A missing `~/.claude/projects` or
    `~/.codex/sessions` is not an error. That agent has no sessions, and the
    preview says on stderr which directory it didn't find. With no sessions
    at all, Home's answer reads "No sessions in the last 7 days" and the
    preview still runs. `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, when set,
    replace `~/.claude` and `~/.codex`.
12. **Tests run on fixture data and hand-written log files.** Reader tests
    use small JSONL files under `tests/data/` (or inline strings), written
    by hand to match the shapes above. That includes the per-block Claude
    duplication and the null `rate_limits` sides. No line is copied from a
    real log. All current tests keep running on `--source fixture`.

## Ordering

The MODIFIED requirements here are written against the specs in
`add-lens-tui-dashboard` (`tui-shell`, `home-dashboard`, `plan-headroom` and
`json-export`). `openspec/specs/` is still empty, so **`add-lens-tui-dashboard`
must be archived before this change**. If it changes those requirements
first, this change's MODIFIED blocks must be rebased onto its final text.

## Interaction with add-stress-corpus

- Both changes touch argument parsing and the export. Whichever is applied
  second adds the other's option and the combination rule in Decision 1.
- `add-stress-corpus` says the default export is unchanged "apart from one
  added `corpus` field". This change adds a `source` field. Whichever is
  archived second must update its scenario to allow both. That's on top of
  the `windows` fields already noted in `add-lens-tui-dashboard`'s design.
- The stress corpus mimics real logs. Its `docs/coverage-note.md` should
  take its real aggregate figures from the same shape-only inspection
  method used here, not from a separate ad-hoc read.

## Risks / Trade-offs

- [Log formats are undocumented and change between versions] → Readers
  ignore unknown fields and line types. They mark a session partial when a
  required field is missing, rather than failing. The shapes are recorded
  above with a date, so a later drift is visible.
- [Duplicate Claude lines over-count] → Usage is deduplicated by
  `requestId` (Decision 3), and there is a test with a streamed request
  split over several lines.
- [Rates go stale] → Each rate entry carries the date it was checked and
  where it came from. An unknown model is unpriced, never guessed.
- [Mixed real and synthetic screens could mislead] → Every synthetic
  surface on local data carries a label (Decision 10), and the fixed
  review-kit finding is removed.
- [Startup time on large histories] → Only files modified within the period
  are opened, and the preview prints "Reading local logs…" on stderr while
  it reads. No hard time limit is set until it is measured on the stress
  corpus.
- [The real export contains personal usage] → It is aggregates only
  (local-source spec, "Export holds aggregates only"). It is printed to
  stdout only when asked, and is never written to a file by Lens.

## Open Questions

These need the user's call before or during apply.

1. **One option or two?** This change keeps `--source fixture|local` apart
   from `--corpus default|stress` and rejects `local` + `--corpus`. Should
   they merge into `--source fixture|stress|local`?
2. **Session names on local data.** Claude logs hold `custom-title` /
   `ai-title`. They read better than a start time, but they are written
   from the user's prompts. Should titles be shown in the TUI? (They stay
   out of the export either way.)
3. **Codex pricing.** Codex logs record no cost, and this account is on a
   subscription plan (`plan_type`), so API rates may not reflect what the
   person pays. Should Codex requests stay unpriced (the current decision),
   or should the table carry published API rates labelled "API-equivalent"?
4. **Token-source table on local data.** Keep the fixture split labelled
   "illustrative" (the current decision), or hide the table on local data
   until real attribution exists?
5. **Claude relay timing.** Is "later change" right, or should the relay
   (opt-in statusline command, reading format, and where readings are
   stored) be specced now as a follow-up change, so the "needs relay" rows
   point at something?
6. **Archived Codex sessions.** Should `~/.codex/archived_sessions/` count
   towards the period? It is excluded for now.
