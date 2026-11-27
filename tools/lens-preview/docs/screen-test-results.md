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
