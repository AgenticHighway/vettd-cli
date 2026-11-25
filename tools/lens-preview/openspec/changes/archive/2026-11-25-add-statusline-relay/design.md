# Design

## Context

After `add-local-source`, the Claude rows on Plan headroom have no readings.
Claude Code's session logs carry only rate-limit *rejections*:
`quotaLimits` and `apiErrorParams.rate_limit_info`, 48 lines on the machine
checked, all with a `resetsAt` but no share. The used share exists only in
the JSON that Claude Code passes to a statusline command.

## What the statusline gives

From the Claude Code statusline documentation
(<https://code.claude.com/docs/en/statusline>, read 2026-11-25). The page
was truncated partway in the read. Points that came only from its summary
are marked **unverified**.

- Configured in `~/.claude/settings.json` (or project settings) as
  `statusLine: { type: "command", command, padding?, refreshInterval? }`.
  The command runs in a shell, gets one JSON object on stdin, and its
  stdout becomes the status line.
- `rate_limits.five_hour` and `rate_limits.seven_day`, each with
  `used_percentage` (0–100; whole numbers on the real feed checked
  2026-11-25, fractional per the documentation) and `resets_at` (Unix epoch
  seconds). `rate_limits.spend_limit` is for gateway spend limits and is
  not a plan window.
- `rate_limits` appears only for claude.ai Pro, Max and Ultra subscribers
  (or a gateway with a spend limit), and only after the first API response
  in a session. Each window can be absent on its own, and Claude Code drops
  a window once its `resets_at` has passed.
- The command runs at session start, on each new assistant message, on
  `/compact`, on permission or vim mode changes, when a rate-limit window
  reaches `resets_at`, and every `refreshInterval` seconds if set. Runs are
  debounced at 300 ms, and a run still going when the next one fires is
  cancelled.
- The stdin also carries `session_id`, `transcript_path`, `cwd`,
  `workspace`, `model`, `cost`, `context_window` and more. The relay must
  not keep any of it.
- **Unverified:** whether output is shown only on exit status 0, and the
  Windows-specific setup guidance.

**Checked on a real feed (2026-11-25, task 0.1).** A temporary probe
recorded only key paths and value types from one signed-in subscriber
session. `rate_limits.five_hour` and `rate_limits.seven_day` each carried
`used_percentage` (an integer) and `resets_at` (a 10-digit integer, so
epoch seconds), as documented. `rate_limits` was absent before the first
response and while signed out. No `spend_limit` appeared.

## Goals / Non-Goals

**Goals:**
- Real Claude 5-hour and weekly readings on Plan headroom with
  `--source local`, by the same rules as Codex windows.
- A relay that is opt-in, records almost nothing, can't grow without
  bound, and can't break the person's status line.

**Non-Goals:**
- Editing `~/.claude/settings.json`, or any setup step Lens does for the
  person.
- A Relay setup screen in the TUI.
- `rate_limits.spend_limit`, cost or context-window fields.
- Reading the relay with `--source fixture`. The fixture keeps its
  synthetic Claude windows.

## Decisions

1. **The relay is a mode of the preview binary:** `lens-preview --relay`.
   It is the same build, so the record format and its reader can't drift
   apart. The README tells the person to build it and point
   `statusLine.command` at the binary's absolute path.
2. **What a record holds.** One JSON line per changed reading:
   `{"t": <unix seconds>, "w": "five_hour" | "seven_day", "pct": <number>,
   "resets_at": <unix seconds>}`. That's one line per window present. There
   is nothing else: no session, path, model or cost. A reading that matches
   the last record for that window (same `pct` and `resets_at`) is not
   written again, because the command runs on every message.
3. **Where.** `~/.vettd/lens/relay.jsonl`, beside vettd's other user files
   (`~/.vettd/rules/`). `--relay-file <path>` overrides it for both the
   writer and the reader, which is how tests run. The directory is created
   when missing. Nothing else is created.
4. **Concurrency and cancellation.** Several Claude sessions can run the
   relay at once, and Claude cancels a run when the next one starts.
   - Each record is written with a single append of one complete line, and
     no read-modify-write while appending.
   - The reader skips any line that doesn't parse, so a cut-off line is
     lost rather than corrupting the file.
   - Whether a single append is atomic on Windows is **unverified**, so
     the skip-bad-lines rule is what guarantees safety, not the append.
5. **Size bound by pruning.**
   - At most once a day, the relay rewrites the file without records whose
     `t` is more than 35 days old. 35 days covers 4 weekly windows plus a
     margin.
   - It writes to a temporary file in the same directory, then renames it
     over the old file. A failed rename leaves the old file as it was.
   - Appends racing with a rewrite can lose a line. That's acceptable,
     because the next run writes the reading again.
6. **Never breaks the status line.**
   - `--relay` always exits 0 and prints at most one line.
   - Its own output is `5h 23% · 7d 41%`, from the windows present, or
     nothing when there are none.
   - With `--then <command>`, it runs that command with the same stdin and
     prints that command's stdout instead. That's how a person keeps an
     existing statusline. The relay's record is written first.
   - Errors (bad JSON, a full disk, a read-only home) go to stderr and are
     otherwise ignored.
7. **Reading it back.** With `--source local`, the Claude rows get
   readings from the relay file, by the same rules as Codex windows:
   - The current instance ends at the latest `resets_at` and starts 300 or
     10080 minutes earlier. It uses every record in that span, a running
     maximum, and the reset-since-last-reading state.
   - The source column reads "relay".
   - With no relay file, the rows say "relay not set up". With a file but
     no reading in the current window, they say "no relay reading yet".
     Both show "—".
8. **Weekly history.** "Claude weekly ended at" lists, for each of the
   last 4 `seven_day` `resets_at` values in the file, the highest `pct`
   before that reset. With fewer than 4 weekly windows recorded, it lists
   those it has and says how many weeks the relay has seen. It never pads
   with guesses.
9. **Privacy.** The relay file holds only the four record fields. It is
   local and never sent anywhere. It is not part of `--json` output except
   as the readings already exported under `windows`. Tests use hand-written
   stdin and relay files, never a real feed.

## Ordering

The MODIFIED requirements are written against `plan-headroom` and
`json-export` as `add-local-source` leaves them. Archive
`add-lens-tui-dashboard`, then `add-local-source`, then this change. If
either earlier change edits these requirements first, rebase the MODIFIED
blocks here.

## Risks / Trade-offs

- [The documented fields differ from the real feed] → Each one is marked
  as documented, not observed. The first apply task is to capture field
  *names* (not values) from one real run, and fix this design if they
  differ.
- [Readings only arrive while Claude is in use] → Plan headroom shows the
  last reading's time, and the pace rule needs 2 readings. An idle
  window's share is the last one seen. It is never projected from nothing.
- [Plans without `rate_limits`] → The rows say "no relay reading yet",
  which is true. Specific plan detection is not attempted.
- [The relay slows the status line] → It only parses stdin and appends one
  line. Pruning happens at most once a day.
- [`--then` runs a person's own command] → It runs only the command the
  person wrote into their own settings, with the stdin Claude already gave
  the relay.

## Resolved Questions

The user settled these on 2026-11-25.

1. **Relay output.** Without `--then`, it prints `5h 23% · 7d 41%` for the
   windows present (Decision 6).
2. **Pruning window.** 35 days (Decision 5).
3. **File location.** `~/.vettd/lens/relay.jsonl` (Decision 3).
