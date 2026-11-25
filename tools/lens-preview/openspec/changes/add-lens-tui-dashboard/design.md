# Design

## Context

This change records behavior that PR #332 (branch
`claude/tui-feedback-refinement-d96f70`, head `aa37924`) already built and
tested. Every requirement was checked against `src/` and the tests in
`src/tests/` and `src/e2e/` at that commit. Where the handoff brief and the
code disagreed, the spec follows the code. Each case is listed under
"Where the code differs from the brief".

Where things live:

- `src/main.rs`: the terminal guard, the draw loop on stderr, and `export()`
  for `--json`.
- `src/app.rs`: `App::frame` and `App::press`, which handle the keys,
  `Viewport::navigate` for scrolling, and `toggle_unit`.
- `src/views.rs` and `src/plan.rs`: each view's answer, scope and rows.
  `plan.rs` also holds window pace and Home's findings.
- `src/screen/`: drawing. `mod.rs` draws the frame, `home.rs` the dashboard
  and `list.rs` every other view, with the preview pane.
- `src/fixture/`: the synthetic corpus, including `WINDOWS`,
  `SOURCE_SHARES`, `CLAUDE_WEEKLY_ENDS`, `LIMIT_HITS` and `NOW`.

## Goals / Non-Goals

**Goals:**
- One requirement per behavior the PR shipped. Each has a WHEN/THEN scenario
  and names the test that covers it.

**Non-Goals:**
- Any code change.
- Screens that exist only in the design proposal: Cache, Risky commands,
  Unused skills, Update every copy, Relay setup.
- Reading real data. Real reading of Codex `rate_limits` from session logs,
  and of the Claude statusline `rate_limits` through an opt-in relay, is
  **not built**. All window data is synthetic (`fixture::WINDOWS`), and the
  `source` column ("logs" or "relay") only labels where a real reading would
  come from. Reading real data is future work.
- The main vettd-cli crate.
- `--corpus`. It belongs to `add-stress-corpus`.

## Decisions

1. **Five capabilities, not one.** The shell, navigation, Home, Plan headroom
   and the export change for different reasons and in different slices.
   Separate specs keep later deltas small.
2. **Tests are named in each requirement body** ("Covered by: …"), so
   archiving the change keeps the link from requirement to test.
3. **Requirements with no automated test say so**, rather than citing a test
   that doesn't really check them. Wholly untested: the stderr/stdout split,
   the separate CI workflow, chip overflow to "…" and the preview pane
   appearing at 76 columns. Partly untested: the "n/total" counter, the
   weekly window row opening the daily view, and `--json` argument handling.
   Each one names the code that implements it, and the matching task names
   the file that was read to verify it.
4. **One behavior per requirement.** Requirements were split until each is
   under `openspec validate --strict`'s 500-character limit, so the change
   passes strict validation.

## Where the code differs from the brief

The code won in each case, and the spec describes the code.

1. **Partial days in requests mode.** The brief says a day is partial when its
   input is incomplete or unpriced. In code (`comparable` in `model.rs`),
   unpriced input makes a day partial only in USD mode. In requests mode only
   incomplete input does, because a request count doesn't depend on price.
2. **The ">3× normal" finding.** The brief says "a complete day more than 3×
   normal". The code makes at most one such finding: the largest complete
   day, and only when it is more than 3× normal. The multiple is shown rounded
   down ("10× a normal day").
3. **The partial-session finding.** It links to the first incomplete session,
   and its text always ends "partial input, unpriced", whether or not that
   session is unpriced. In the fixture it is (Nov 18). Recorded as an open
   item below.
4. **The preview pane.** Beyond the 76-column rule, it also needs a body at
   least 9 rows tall and a selected row that opens something. Home never shows
   a pane, because it has its own layout.
5. **Plan headroom history.** "How recent weekly windows ended" covers Claude
   only ("Claude weekly ended at 41%, 63%, 38%, 55%"). The limit-hit count is
   for the 5-hour limit ("5-hour limit stopped work 3×").
6. **Window rows open something.** The brief doesn't mention this. A 5-hour
   window's row opens Nov 18 (today's sessions), and a weekly window's row
   opens the daily view. It is specced because Enter on these rows is
   exercised by a journey test.
7. **NO_COLOR chips.** Under NO_COLOR, scope chips draw as `[value]` so they
   stay distinct without a background colour. This is specced as part of the
   NO_COLOR requirement.
8. **Help item for `q`.** Keys lists "q Quit (Ctrl+C also works in search)".
   In Find, `q` types and Ctrl+C quits, as the brief says.

## Overlap with add-stress-corpus

`add-stress-corpus` (not implemented yet) has a requirement, "Default corpus
unchanged", saying that the default `--json` export is unchanged "apart from
one added `corpus` field". PR #332 has since added `windows`,
`token_sources`, `limit_hits`, `claude_weekly_ends_percent` and
`now_minutes_from_nov_18`. That change's design decision 10 also captures a
golden file from "today's `--json` output", which now already includes these
fields.

This change doesn't edit `add-stress-corpus`. **Whichever of the two changes
is archived second must update its export scenario** so the two agree:

- If this change is archived first, `add-stress-corpus` must rebase its
  "Default corpus unchanged" requirement onto the `json-export` spec here.
- If it is archived second, this change's `json-export` spec must add the
  `corpus` field.

## Risks / Trade-offs

- [The spec describes a preview, so it may be read as a product promise]
  → Every data requirement says the data is synthetic. Real-data reading is
  listed as a non-goal.
- [Some requirements have no automated test] → They are listed in Decision 3.
  A later slice can add render tests (for example, the PREVIEW title at 80
  columns, or "…" in a narrow scope line) without changing the spec.

## Open Items

These are known gaps. They are not requirements.

- **review-kit finding is fixed text.** "review-kit: unknown load in Codex" is
  a constant row in `plan.rs::findings`. It is not derived from load-order
  data, so it would show even if the gap were gone.
- **Fixture load order is reversed.** In `fixture::PLACEMENTS`, the workspace
  copy of review-kit (`api/.claude/skills/...`, 1.5) wins over the user copy
  (`~/.claude/skills/...`, 1.4). That is the reverse of Claude's documented
  load order.
- **README walkthrough is stale.** `README.md` still says to "start at
  Activity" and to "Press i from Activity". The first screen is now Home.
- **Partial-session text says "unpriced" unconditionally.** See difference 3.
