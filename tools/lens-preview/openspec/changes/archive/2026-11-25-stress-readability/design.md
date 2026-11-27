# Design

## Context

Slice 0 made the stress corpus load. Rendering it (expert review, via the
render-test harness at 80×24 and 60×24) showed the defects listed in the
proposal. The screen test was waived for this slice because no participants
were available (`docs/screen-test-results.md`), so every judgment here is
expert review.

The relevant code today:

- `screen/home.rs::draw_chart` gives ratatui's `BarChart` raw values, so the
  axis starts at zero. The title is one `format!` with no width check.
- `model.rs::session_row` formats `{:<16} {:<4} {:<7} {:>8}`. A workspace
  over 4 characters shifts everything after it.
- `screen/mod.rs::chips` replaces the first chip that doesn't fit, and every
  chip after it, with "…".
- `views.rs::cost_rows` and `tokens` format `"{worker} / {model} ..."` with no
  width. The preview pane reuses these rows at the pane's width.
- `app.rs` cycles `w` over `fixture::WORKSPACES` (`api`, `web`), and
  `components/inventory.rs` picks from the same list.

## Goals / Non-Goals

**Goals:**
- Keep every figure whole and aligned on Home, Day, Session, Tokens and the
  preview pane at 80×24 and 60×24 on both corpora, with and without colour.
- Make day-to-day spend differences visible on the stress Home chart.
- Let the inventory filter reach every workspace in the corpus.
- Leave the default corpus's screens unchanged wherever no stress-only
  condition arises, so existing render tests keep passing unedited.

**Non-Goals:**
- The hardcoded Nov 12–18 week (`Scope::WEEK`, the Week loop in `views.rs`).
- Plan windows and token sources that don't scale with stress volume (Slice 0
  design decision 11).
- The low-coverage threshold (Slice 3).
- Thousands separators in dollar figures.
- Which workspaces user-scoped copies apply to when no workspace is filtered
  (`components/mod.rs::contexts` keeps `WORKSPACES`). Changing it would change
  copy states, which is outside layout.

## Decisions

1. **Raised baseline rule.** Use a baseline of `min − (peak − min)` when
   `peak/2 ≤ min < peak`, where `min` and `peak` cover days with data only.
   Round it down to a whole dollar or request, and never go below zero. Bars
   draw `value − baseline`. The smallest bar then sits at about half the
   peak bar's height, so differences show without the chart suggesting that
   a small day was zero. Expert review: a raised axis is a known way to
   mislead, so the title always names it (decision 2). Alternatives
   considered: a log scale (unreadable for a non-technical reader), and
   always starting at `min` (makes the smallest day look like nothing).
   The default corpus has a $0.80 day against an $8.52 peak, so its spend
   chart keeps a zero baseline. Its requests chart is checked in task 2.1. If
   that chart changes, the change is recorded rather than suppressed.

2. **Title parts, full then short.** Build the title from parts: unit, normal,
   partial note, axis start. Each part has a full form and a short form, for
   example `DAILY SPEND` → `SPEND`, `normal: no complete day` → `no normal`,
   `every day partial (≥)` → `all partial ≥`, `axis from $1510` →
   `from $1510`. Use the full title if it fits the room the block title has.
   Otherwise shorten parts in this order: unit and axis first, then normal
   and the partial note. Then drop trailing parts whole until it fits, but
   never the unit. The unit and axis go first because Slice 0's
   `stress_home_shows_unknown_normal_and_whole_token_figures` needs the full
   normal and partial wording at 80 columns. All-full then all-short, the
   first plan, couldn't meet both. Stress at 80 columns reads
   `SPEND  ·  normal: no complete day  ·  every day partial (≥)  ·  from $1510`.
   The default title is unchanged because its full form fits at 80 columns.
   The default requests chart (min 2, peak 3) gets a baseline of 1 and an
   `axis from 1` note. This is recorded as intended behavior under the rule,
   not suppressed.

3. **One middle-shortening helper.** Add a single
   `shorten(text, max) -> String` in `model.rs`, next to the other text
   helpers. It keeps `ceil((max−1)/2)` leading and `floor((max−1)/2)` trailing
   characters around "…", counted in chars. Text that fits comes back
   unchanged. Workspaces, model IDs and chip values all use it. The middle is
   dropped because workspace paths and model IDs differ at both ends: the
   first segment and the leaf directory, the family and the date.

4. **Day rows take the width.** `session_row` takes the list's text width and
   the list's workspace column width. `day_sessions` computes the column as
   `clamp(longest workspace, 4, width − fixed)`, where `fixed` is the name,
   agent and total columns plus spaces. When the longest workspace is 4
   characters or fewer, the column is 4 and rows are byte-identical to today.
   The Day header is built from the same widths, so `list.rs::header` takes
   the widths for `View::Day`. The trailing `  partial` flag stays after the
   total. If it doesn't fit, it is clipped as it is today, and the amber row
   style still applies because it is chosen from the full row text before
   drawing.

5. **Chips shorten before dropping.** In `chips`, when a chip doesn't fit,
   shorten its value to the room left minus the room the "…" marker needs,
   if at least 8 characters remain. Only drop it, and the chips after it,
   when fewer remain. Session and Tokens scopes are then readable at 80
   columns: day, shortened workspace, agent, cost.

6. **Worker rows take the width.** `cost_rows` and `tokens` take the text
   width and shorten the model so `worker / model` plus the figure fits. The
   preview pane already builds rows at the pane's width, so it gets one-line
   rows with no extra code. On the default corpus, models are `Small` or
   `Large`, so rows are unchanged.

7. **Corpus workspaces for the filter.** Add
   `fixture::workspaces(&[Session]) -> Vec<&'static str>`, sorted and
   distinct, matching the export's `workspaces` field. The inventory and the
   `w` key use it instead of `WORKSPACES`. On the default corpus it returns
   `["api", "web"]`, so behavior is unchanged.

8. **Paging test, fix only if it fails.** The `Viewport` paging already
   exists. Add an end-to-end test on the stress corpus first. Change
   `Viewport` only if the test fails, and record the defect in the tasks.

9. **Model routing for apply.** Haiku subagents write each task from its
   task text. Sonnet reviews each diff against this design and the spec.
   Opus runs the Docker checks and makes the final verification.

## Risks / Trade-offs

- [A raised axis exaggerates small differences] → The title always names the
  axis start, and the baseline only applies when every day is within a
  factor of two of the peak.
- [Middle-shortened workspaces can collide, for example two long paths with
  the same ends] → The full workspace stays in the preview pane and in the
  JSON export. Rows remain distinct by name, agent and total.
- [Passing width into row builders ties content to layout] → `content`
  already takes `width` and uses it for bars, so this follows the existing
  pattern rather than adding a new one.
- [The default render tests encode today's strings] → Decisions 1, 4, 6 and 7
  are designed to be no-ops on the default corpus. Any default render that
  changes is a defect to investigate, not a test to update.
