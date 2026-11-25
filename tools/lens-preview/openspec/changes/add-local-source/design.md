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
  usage across lines, over-counts. For 2 requests, the last line had fewer
  output tokens than an earlier one.
- **The same request appears in several files.** 597 `requestId`s occur in
  more than one file, 484 of them in more than one top-level session file,
  because resumed and forked sessions copy earlier lines. Removing duplicates
  file by file still over-counts.
- **Sessions cross midnight.** 45 of 166 sessions have requests on more than
  one UTC day.
- **Subagent files aren't always filed under their own session.** 288
  subagent files name the session of the folder they sit in. 5 name another
  top-level session that exists on disk.
- **Most cache writes are 1-hour writes.** Of 807M cache-write tokens,
  647M were `ephemeral_1h_input_tokens` and 160M `ephemeral_5m_`. The two
  are priced differently. `service_tier` was always `standard`, and `speed`
  was `standard` or absent.
- Project folder names under `projects/` spell out the full working path.
- `message.model` includes `"<synthetic>"` on 96 lines. Those are Claude
  Code's own placeholders for errors, not API requests.
- **No per-request cost is recorded.** `costUSD` was absent on every line.
  138 `cost-state` lines carry a session-level `totalCostUSD`,
  `modelUsage` and `hasUnknownModelCost`. They can't be split per request
  or per day.
- **Claude rate-limit windows are not in the logs.** Only rejections appear:
  `quotaLimits` (48 lines) and `apiErrorParams.rate_limit_info` (28 lines),
  with `rateLimitType` `five_hour` or `seven_day`, `status: "rejected"` and
  a `resetsAt`. There is no used percent. **All 48 rejection lines are
  `<synthetic>` API-error lines,** so they are not requests, but they are
  the only record of a limit hit.
- Titles (`custom-title`, `ai-title`) and the attribution fields
  (`attributionSkill`, `attributionMcpTool`, `attributionPlugin`) exist.
  Titles are shown in the TUI (Decision 8). Attribution isn't used (Decision 10).
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
- **Token totals:** on all 6,349 usage records, `total_tokens` equals
  `input_tokens + output_tokens`. So `cached_input_tokens` is part of
  `input_tokens`, and `reasoning_output_tokens` is part of `output_tokens`.
  `last_usage_includes_reasoning` was true on all but one event, where it
  was absent. `cache_write_input_tokens` was 0 on every record.
- **`resets_at` drifts.** It is in Unix seconds (all values below 4×10⁹).
  The weekly window's readings carry `resets_at` values up to 16 seconds
  apart, so only 7 of 2,340 readings in the current weekly window share its
  exact latest value. Separate windows are at least 42 minutes apart.
- **Percentages arrive out of order.** There is one `limit_id`. In the
  current 5-hour window, `used_percent` drops 78 times between consecutive
  readings, by up to 6 points, always between different sessions and never
  within one. Concurrent sessions report slightly older figures.

### Unknown or absent fields

These shape the requirements, because Lens must not guess any of them.

| Wanted | Claude Code | Codex |
|---|---|---|
| Per-request USD | absent | absent |
| 5-hour / weekly used % | absent (relay only) | present |
| Window start | absent | derived: `resets_at − window_minutes` |
| Limit hits | rejections only | `usage_limit_exceeded` |
| Token source (file reads, skills, …) | absent | absent |
| Fresh vs cached input | separate fields | cached is inside `input_tokens` (totals reconcile) |
| Reasoning in output | `thinking_tokens` (whether inside `output_tokens` is **unknown**, and not needed for pricing) | inside `output_tokens` |
| Cache writes | 5-minute / 1-hour split | **unknown** whether inside `input_tokens` (always 0 here) |
| `resets_at` unit | — | Unix seconds, drifting by seconds |
| Which copy of a skill ran | absent | absent |

## Goals / Non-Goals

**Goals:**
- `--source local` gives real sessions, requests, tokens, API-equivalent USD and
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
- Claude's `cost-state` totals and attribution fields. The attribution
  fields tag whole requests, not where tokens came from, so they can't fill
  "Where tokens went" (Decision 10).
- The main vettd-cli crate.

## Decisions

1. **A `--source` option, separate from `--corpus`.** `--source` says where
   data comes from, and `--corpus` (from `add-stress-corpus`) says which
   synthetic dataset to use. `--corpus` is valid only with
   `--source fixture`. `--source local --corpus …` is rejected on stderr
   with a non-zero exit. One merged option was rejected because it mixes
   two meanings.
2. **The period on local data is the 7 UTC days ending today,** at the time
   the run starts. "Now" is the system clock, read once. Each request counts
   on its own UTC day, so a session that crosses midnight appears on both
   days and counts once in Home's total. Requests before the period are
   dropped. Day labels carry their month ("Nov 30", "Dec 1"), and days sort
   by date, not by day of month. Files last modified more than 28 days ago
   are never opened. Files from the last 28 days are read, but older ones
   only add limit hits. The fixture keeps its fixed Nov 12–18 and its
   fixed `NOW`.
3. **Requests and their usage.**
   - Claude: one request per distinct `requestId` (or `message.id` when it
     has none), **across all files**, because resumed and forked sessions
     copy earlier lines. A copied request counts in the session whose file
     has its earliest line. Its usage comes from its line with the most
     output tokens (the last on a tie), because lines are written as the
     response streams. `<synthetic>` model lines and `isApiErrorMessage`
     lines are not requests, but they are read for limit hits. Parts: Fresh
     input = `input_tokens`, Cache read = `cache_read_input_tokens`, Cache
     write = `cache_creation_input_tokens` (kept split into 5-minute and
     1-hour for pricing), Output = `output_tokens`.
   - Codex: one request per `token_usage_record`, deduplicated by
     `response_id`, using `payload.usage`. Its model is the `model` of the
     latest `turn_context` at or before it. Fresh input = `input_tokens −
     cached_input_tokens`, Cache read = `cached_input_tokens`, Output =
     `output_tokens` (reasoning included). This matches the data, where
     `total_tokens` always equals `input_tokens + output_tokens`. A record
     where that doesn't hold, or with `cache_write_input_tokens` above 0
     (where those tokens belong is unknown), marks its session partial.
4. **The Claude statusline relay is a later change.** The relay needs an
   opt-in setting in the user's Claude config and a small process that
   writes readings somewhere Lens can read. That is a side effect with its
   own privacy review. This machine has no statusline configured, so there
   is nothing real to test against. Without the relay, both Claude rows on
   local data show "—" for used, pace and reset, with the source "needs
   relay". They open nothing, and the answer only considers windows with
   readings. The rejection events in Claude logs give a reset time but no
   share, so they aren't used as a reading.
5. **Codex windows are matched by length and by time, not by name or exact
   reset.**
   - `window_minutes` 300 is the 5-hour row and 10080 the weekly row,
     whichever of `primary` / `secondary` carries it.
   - The current instance ends at the latest `resets_at` and starts
     `window_minutes` earlier. Its readings are every event whose timestamp
     falls in that span, across all sessions. Exact `resets_at` matching
     would split one weekly window into many, because the value drifts by
     seconds.
   - Readings are taken as a running maximum, because concurrent sessions
     report figures a few points behind.
   - When the reset time has passed, the row says it reset with no reading
     since, rather than showing the old share.
   - A null side, or another length, is skipped and noted on stderr. The
     pace rule is unchanged: fewer than 2 readings shows "—".
6. **Limit hits are counted in the same 4-week span the history row
   describes.**
   - Claude: distinct `requestId`s across all files on `<synthetic>` /
     API-error lines whose `quotaLimits` or
     `apiErrorParams.rate_limit_info` has `rateLimitType == "five_hour"`
     and `status == "rejected"`.
   - Codex: `task_complete` with `codex_error_info ==
     "usage_limit_exceeded"`, by `turn_id`.
   - The row names each agent separately, because they are different
     limits.
   - The "Claude weekly ended at" row needs the relay. On local data it
     says so.
7. **Pricing is a rate table compiled into the preview** (`src/pricing.rs`).
   - Each entry has a model-ID prefix and five per-million-token rates:
     fresh input, cache read, 5-minute cache write, 1-hour cache write and
     output. It also has the date the rates were checked and the published
     page they came from. Most cache-write tokens here are 1-hour writes,
     so a single cache-write rate would misprice most of them.
   - A model matches the longest prefix.
   - A request is unpriced (no rate, never a zero rate) when no entry
     matches, when its cache writes lack the 5-minute / 1-hour split, or
     when its `speed` or `service_tier` isn't `standard`. That is the
     existing "unpriced" state, so its day and session read as partial in
     USD.
   - Codex (OpenAI) models get entries at their published API rates, using
     fresh input, cache read and output. A model with no published rate,
     such as `codex-auto`, gets none and stays unpriced.
   - On local data, every dollar figure is labelled "API-equivalent", never
     "priced". Both agents may be paid through a plan, so this is what the
     usage would cost on the API, not what was spent. The fixture keeps its
     "priced" label.
   - Claude Code's `cost-state` totals aren't used, because they can't be
     split per day or per request.
8. **Session identity.**
   - A Claude session is a top-level session file plus the subagent lines
     that belong to it. A subagent line belongs to the session its
     `sessionId` names when that file exists, and otherwise to the folder
     it sits in.
   - A Codex session is the root of a `parent_thread_id` chain. A thread
     whose parent isn't on disk is its own session. Rollouts in
     `~/.codex/archived_sessions/` are read too, and a session found in both
     places counts once, by `session_meta.payload.id`.
   - The workspace label is the last component of `cwd`, never a
     `projects/` folder name, which spells out the full path.
   - The session name is its latest title, shown only in the TUI and never
     exported. For Claude, that's the last `custom-title` or `ai-title` line.
     For Codex, it's the `thread_name` in `~/.codex/session_index.jsonl`
     (`id`, `thread_name`, `updated_at`) for the root thread, with the
     latest `updated_at` winning. On this machine, 18 of 68 rollouts had an
     entry. Most of the rest are subagent threads, which take their root's
     title. A session with no title is named by its start time ("09:12").
9. **Partial, not dropped.** A session is incomplete, using the existing
   flag, when any line in it fails to parse, its last line is cut off
   (still being written), a Claude request has no usage, or a Codex usage
   record fails the check in Decision 3. The rest of its requests still
   count.
10. **What stays synthetic on local data, and how it is labelled.**
    - Top bar: "local logs · read-only · nothing leaves this machine", in
      place of "synthetic data · …".
    - Home's "Where tokens went" table is hidden on local data, and its rows
      go to the findings. Claude's `attribution*` fields were checked on
      2026-11-25 (by token count only). They tag a whole request, most of
      whose tokens are re-read cache, and 78.4% of tokens carry no tag.
      They can't say where tokens came from.
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
    duplication, a request copied into a resumed session, a session across
    midnight, a misfiled subagent, a drifting `resets_at`, an out-of-order
    reading and the null `rate_limits` sides. No line is copied from a
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
  `requestId` across all files (Decision 3). There are tests for a streamed
  request split over several lines and for one copied into a resumed
  session.
- [Codex readings from concurrent sessions disagree] → Readings are grouped
  by time and taken as a running maximum (Decision 5). There are tests with
  a drifting `resets_at` and an out-of-order reading.
- [Rates go stale] → Each rate entry carries the date it was checked and
  where it came from. An unknown model is unpriced, never guessed.
- [Mixed real and synthetic screens could mislead] → Every synthetic
  surface on local data carries a label (Decision 10), and the fixed
  review-kit finding is removed.
- [Startup time on large histories] → Only files modified in the last 28
  days are opened, and the preview prints "Reading local logs…" on stderr while
  it reads. No hard time limit is set until it is measured on the stress
  corpus.
- [The export's shape differs by source] → Shared fields keep one JSON type
  and are null where a source has nothing. Fields only local data has are
  separate (json-export spec).
- [The real export contains personal usage] → It is aggregates only
  (local-source spec, "Export holds aggregates only"). It is printed to
  stdout only when asked, and is never written to a file by Lens.

## Review log

An adversarial review on 2026-11-24 checked the first draft against the
logs again (counts only). It found several problems, all fixed in this
version:
- Duplicates were only removed within each file.
- Codex readings were grouped by exact `resets_at`, and percentages that
  arrive out of order were not handled.
- The 7-day file skip contradicted the 28-day limit-hit count.
- Requests were put on their session's first day.
- One rate covered both 5-minute and 1-hour cache writes.
- Rejection lines were excluded as requests, so Claude limit hits would
  always be 0.
- Two Codex token questions were left unknown when the data answers them,
  and the reconcile check would misfire on cache writes.
- A reset window would still show its old share.
- "Window rows open their sessions" still assumed Nov 18.
- Subagent files sit under the wrong session folder.
- Export field types changed by source, the `--source` / `--corpus` rule
  had no requirement, and nothing kept project folder names out of labels.

## Resolved Questions

The user settled these on 2026-11-25.

1. **One option or two?** Two: `--source` and `--corpus` (Decision 1).
2. **Session titles?** Yes, in the TUI only. The latest title wins, with
   Codex titles from `session_index.jsonl` (Decision 8).
3. **Codex pricing?** Priced at published API rates like Claude, with all
   local dollars labelled "API-equivalent" (Decision 7).
4. **Token-source table on local data?** Hidden (Decision 10).
5. **Claude relay timing?** It gets its own change, `add-statusline-relay`,
   next. Until then the rows say "needs relay" (Decision 4).
6. **Archived Codex sessions?** Included, each session counted once
   (Decision 8).
