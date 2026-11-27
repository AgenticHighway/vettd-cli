# Design

## Context

See proposal.md, "Why". Current state that shapes the approach:

- `local::load` reads once, before the first screen, through
  `local::read(roots, now, cutoff, notes)`. `History::opens` skips files
  older than `HISTORY_DAYS` (28). Requests outside the 7-day period are
  dropped when sessions are built (`in_period` in `src/local/mod.rs`).
- `Data` holds the period's `sessions`. `View::Session(usize)` and
  `View::Tokens(usize)` index into it.
- The main loop blocks on `event::read()` (`src/main.rs`), so the screen
  redraws only on a key press.
- About 270 recent files (1.4 GB) take 7–14 s warm. The owner's disk holds
  about 3,100 Claude log files, so reading everything up front could take
  much longer than today's startup.

## Goals / Non-Goals

**Goals:**
- Dashboard startup and `--json` cost the same as today.
- History reuses the existing Claude and Codex readers, so a session's
  figures in History follow the same counting rules as on the dashboard.

**Non-Goals:**
- Caching History between runs, or reading it incrementally as files change.
- Any change to plain `--json` output or the golden file.

## Decisions

**1. A second, unrestricted read on a background thread.** After the first
screen is drawn, a `std::thread` runs the same `read` with no file cutoff
and no period filter, and sends the result over a `std::sync::mpsc` channel,
plus progress counts (files read, files found).
*Alternatives:* reading everything at startup is the simplest, but it makes
every run as slow as the whole history, even when History isn't opened.
Reading on the first `h` keeps startup fast but freezes the TUI the first
time. Both were rejected. The thread needs no new dependency.

**2. `read` takes a span instead of a fixed period.** The period filter
becomes a parameter: the dashboard passes the 7 days and 28-day cutoff as
now; History passes "all". This keeps one code path for the counting rules
(deduplication, subagents, pricing, partial flags) instead of a second
reader that could drift.
*Alternative:* derive History from the dashboard's read. Rejected: the
dashboard has already dropped out-of-period requests and old files.

**3. History sessions live in their own `Data`.** History's result is a
separate `Data` (its own `sessions`), and the session and token views gain
a way to say which set their index refers to (for example a `Set::Period`
or `Set::History` tag on the view). The dashboard's `Data` is never
mutated, so no dashboard figure can change.
*Alternative:* merge History into the dashboard's `sessions`. Rejected: Home,
Week and the export iterate `sessions`, and every figure would need a new
filter.

**4. The loop polls while History is reading.** While the thread is running,
`main` uses `event::poll` with a short timeout (for example 200 ms) so the
reading line updates and the list appears without a key press. Once the
result has arrived, it goes back to blocking on `event::read`.

**5. Filtering reuses Find's matcher.** History's filter uses the same
case-insensitive substring match and typed-text handling as Find, over name,
workspace, agent and date label. No fuzzy matching.

**6. Fixture History is the corpus's own sessions.** On `--source fixture`
there is nothing to read, so History is the loaded corpus's sessions,
available at once, with the "synthetic" chip. This makes History testable
without logs, and the stress corpus gives the long-list and scrolling cases.

**7. Row layout at 80 columns.** At 80 columns the list pane is 48 cells:
date (6, "Oct 03"), agent (6), requests (right-aligned, 5), dollars
(right-aligned, 8), gaps (4), leaving 19 cells for the name, cut with "…".
The date and figures use the existing bold and muted spans from
`screen-emphasis`. The owner accepted the 19-cell name on 2026-11-27.
Render tests at 80×24 and 60×24 still confirm it.

**8. History is exported only with `--history`.** `--json --history` adds a
`history[]` array. Each entry has the same fields and identifier rules as
`sessions[]`, so a local export still holds no identifiers. On
`--source local` it does the full read synchronously before printing, so it
is slow by design; plain `--json` never starts that read. `--history`
without `--json` is an error (exit 2), as the `--corpus` errors are.
*Alternative:* always export History. Rejected: it would change all three
export hashes and the golden file, and slow every local export.

**9. `h` works on Home, Week, Day and Session.** The owner chose this on
2026-11-27. Adding "h History" (12 cells) keeps every footer within 80
columns: Home 68, Week 66, Day and Session 56.

## Risks / Trade-offs

- [Reading all logs uses a lot of memory: today's reader keeps every parsed
  request in memory until sessions are built. This applies to
  `--json --history` too.] → Measure peak memory and
  read time on the owner's history and record both in
  `docs/screen-test-results.md`. If either is a problem, stop and ask before
  changing the readers.
- [The History thread competes with the TUI for disk and CPU on a large
  history.] → It starts only after the first screen is drawn. One thread,
  no parallel reads.
- [A session that spans the period's start shows different request counts
  on Day (period only) and in History (all).] → This is intended. The
  session view opened from History says it covers all requests.
- [Files are read twice: recent files once for the dashboard and again for
  History.] → Accepted for this slice to keep the dashboard path unchanged.
  Sharing the parse is a later optimisation, if timing shows it is needed.
- [Quitting while History is reading.] → The thread is detached, and `q`
  exits without waiting for it.

## Migration Plan

None. It's a preview tool with no stored state. Removing `h` and the thread
reverts it.

## Open Questions

None. Where `h` works was settled as decision 9.
