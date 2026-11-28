# Proposal

## Why

The owner's real-data check (2026-11-26 and 27) and a follow-up run on the
same logs found four places where the preview showed something wrong or
unexplained:

- Week marked Nov 22–25 "partial", and the Day views of those days said
  nothing about it. All 599 unpriced requests were Codex prompts over 272K
  input tokens. The rate table left those unpriced, but OpenAI publishes
  long-context rates for every model in it.
- Day called a day partial only for incomplete input, while Week (and the
  local-source spec) also count unpriced requests. The two screens
  disagreed about the same day.
- Plan headroom said "No window is on pace to fill before it resets."
  beside a Codex weekly row at `~272%`. Both followed their rules, but
  together they read as a bug. The window may well be full: its pace filled
  it after the last reading.
- At 80 columns, every comparable Day lost its change chip to "…", and
  other days shortened "All workspaces" to "All w…paces".

## What Changes

- OpenAI entries get their published long-context rates (2x input and
  cache read, 1.5x output) for prompts over 272K input tokens.
- Day, its rows and Week agree: a day or session is partial when input is
  incomplete or, in USD, a request has no rate. A partly priced row ends in
  "≥".
- When a window's pace filled it before now and no reading confirms it,
  Plan headroom's answer says it "may be full", with its last share, when
  that was read and when the pace filled it. Home's first finding says the
  same.
- Day scope chips are shorter: "≥ means at least", "No comparison", and
  "+$7.69 vs prior day" in place of "Prior day … | … change". Every chip
  shows whole at 80 columns with a three-figure total.
- The README says that only terminal Claude Code runs `statusLine`, so the
  desktop app's sessions add no relay readings. Lens has no other source.

## Capabilities

### Modified Capabilities

- `local-source`: long-context Codex rates.
- `plan-headroom`: a "may be full" answer.
- `home-dashboard`: the first finding names a window that may be full.
- `screen-readability`: Day partial matches Week; Day chips fit.

## Impact

- `src/pricing.rs`, `src/views.rs`, `src/model.rs`, `src/plan.rs`, tests,
  README.
- The fixture and stress exports are unchanged: their prices don't come
  from the rate table.
- Out of scope: Claude readings from the desktop app. Claude Code writes
  them only to a statusline command, which the desktop app doesn't run.
