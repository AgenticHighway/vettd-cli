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
