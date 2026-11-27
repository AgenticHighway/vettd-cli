# Lens preview screen test: results

## Status: waived for Slice 1

- Date: 2026-11-20
- Decided by: the project owner
- Reason: no participants have been recruited yet. The moderated test in
  [screen-test.md](screen-test.md) needs 3–5 people, covering the developer
  and maintainer roles, and none were available before Slice 1.

## What this means

- Slice 1 goes ahead without user evidence. Every design judgment in it is
  labelled expert review.
- No task results exist, so no task becomes a Slice 1 requirement under the
  "pass with changes" rule.
- The automated tui-test journeys (`tui-tests/`) passing is not a substitute.
  They check that scripted key sequences reach the expected screens, not
  whether people find their way there unprompted.
- The test should run before a later slice relies on user evidence. When it
  runs, replace this section with the per-task results.

## Owner feedback after Slice 1 (not test evidence)

On 2026-11-20 the project owner checked the Slice 1 build by eye. This is one
person who knows the tool, so it is a lead for the screen test, not a result.

- Layout at stress scale: "looks good".
- Navigation "still feels hard":
  - Finding things: getting to a particular day or session.
  - Too many key presses to reach a destination.
  - The text is uniform, so screens are hard to scan.
- These line up with screen-test questions 1 (reach the cause of an expensive
  day without being told which keys to press) and 4 (read Home at stress
  scale). The test should watch for them specifically.
- Day partial flag `≥`: "it works, it's hard to figure out what that means
  though". The Day screen's last scope chip now spells it out (`≥ partial:
  total is at least this`) in place of the previous-day comparison, which a
  partial day never has. The screen test should check whether people read
  it unprompted.

## Owner feedback after lens-navigation (not test evidence)

On 2026-11-26 the project owner checked the navigation build (`--corpus
stress`, merged with Slice 1) by eye: it "feels better". They gave no
detail on finding a day or session, key presses, or scanning, so this
does not show which change helped. It is a lead for screen-test questions
1 and 4, not a result.

## Owner real-data check of the navigation build (expert review)

On 2026-11-26 the project owner ran the navigation build on their own
logs (`--source local`, period Nov 21–27 UTC). The screen test is still
waived, so this is labelled expert review: one person who knows the tool,
not test evidence.

- Week keys 1–7: "all correct". Each opened the expected UTC day.
- 80 columns: "all readable".
- Claude sessions do load (18 Claude and 6 Codex sessions in the export).
  What the owner saw as "Claude not showing" is Plan headroom: both Claude
  windows had no reading. No relay file existed, and the `statusLine` in
  `~/.claude/settings.json` didn't call `--relay`. The installed binary
  records correctly when given a sample reading, so this is setup, not a
  defect.
- Codex weekly showed `~272%` at reset, which the owner read as wrong. It
  is the pace projection, not the used share: 95% used about 2.4 days into
  the week, carried on a straight line to the reset. The arithmetic is as
  designed (projections over 100% are allowed). Whether the column reads
  clearly enough is an open question for the screen test, not a fix here.

On 2026-11-27 the owner set the `statusLine` to `--relay` alone and sent a
message from Claude Code in a terminal. The owner asked Claude to judge
the rest, so the points below are Claude's checks, not the owner's
verdicts.

- Relay: it works. The relay file has one reading per Claude window, 49%
  (5-hour) and 39% (weekly), and Plan headroom shows the same with source
  `relay` and `—` at reset (one reading). Only terminal sessions add
  readings: the desktop app's Code tab doesn't run `statusLine`, so four
  checks after desktop messages found no file.
- Answer line next to the projection: Plan headroom says "No window is on
  pace to fill before it resets." while the Codex weekly row shows
  `~272%`. This is by design: the straight line crossed 100% after the
  last reading, and no later reading confirmed it, so it doesn't count
  (`soonest` in `plan.rs`). It reads as a contradiction, so it joins the
  `~272%` wording question for the screen test.
- Day totals: correct. Each Week row equals its Day view total and a
  recount from the sessions. The six days with input add up to the
  header's $167.08.
- `≥ partial` chip: never shown on this data, because no session is
  incomplete. Week marks Nov 22–25 `partial` for a different reason:
  some requests are unpriced, so the dollars are a floor. Opening those
  days shows no `≥` and no chip, only "No comparable previous day", so
  the Week `partial` goes unexplained. This is a wording finding for the
  next slice, not a fix here.
- 80 columns: on days whose scope line carries a total over $10 and "No
  comparable previous day", "All workspaces" is shortened to
  "All w…paces". It is still legible.

## Owner check of session History on real logs (expert review)

On 2026-11-27 the project owner opened History (`h`) on their own logs
(`--source local`). They didn't time it, but called it "quick enough". The
read runs in the background after the first screen, so the wait isn't felt.
This is expert review, not test evidence.

At the owner's request, Claude timed the export on the same logs. Only
counts were printed, and the output file was deleted afterwards. Each run
listed 181 History sessions and 32 sessions in the period. Times are
wall-clock; the peak is the process's peak working set.

| Build   | Command                           | Time   | Peak memory |
|---------|-----------------------------------|--------|-------------|
| release | `--source local --json`           | 6.9 s  | 1,843 MB    |
| release | `--source local --json --history` | 16.2 s | 1,855 MB    |
| debug   | `--source local --json`           | 25 s   | 1,844 MB    |
| debug   | `--source local --json --history` | 51 s   | 1,856 MB    |

- Time: `--history` adds about 9 s in release, because it reads every file,
  not just the ones changed in the last 28 days.
- Memory: History adds about 12 MB. The ~1.84 GB peak was already there
  with plain `--json`, so it comes from the existing readers. The owner
  chose to record it and fix the reader memory in a separate change
  (`stream-local-logs`, below).
- Not covered by the owner's check: the filter, or Enter and Esc on a real
  History row.

## Reader memory on real logs (`stream-local-logs`)

Measured on 2026-11-27 on the owner's logs, release builds, with
`PeakWorkingSet64` sampled every 20 ms. Only counts and numbers were
printed, and every export file was deleted afterwards. "Before" is the
`add-session-history` head. Both builds ran in the same session, so their
times can be compared with each other but not with the 6.9 s run above.

| Command                           | Before          | After         |
|-----------------------------------|-----------------|---------------|
| `--source local --json`           | 8.9 s, 1,841 MB | 8.3 s, 128 MB |
| `--source local --json --history` | 18.0 s, 1,854 MB | 15.8 s, 172 MB |

- Cause: one recent Claude log is 652 MB. The line reader read each file
  whole, then held every parsed line of it at once. Reading through a
  buffer alone brought the peak to 1,197 MB. Having the Claude reader
  handle one line at a time brought it to 128 MB.
- Output: old and new binaries were run at the same moment and their
  exports compared field by field. There were no content differences in
  `sessions[]` (33) or `history[]` (182). Requests inside a session can come
  out in a different order from run to run. The old binary does this too,
  so it existed before this change and is not fixed here.

