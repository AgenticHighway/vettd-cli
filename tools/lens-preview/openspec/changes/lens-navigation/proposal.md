# Proposal

## Why

After Slice 1 the project owner said navigation "still feels hard". It is
hard to reach a particular day or session, destinations take too many key
presses, and screens are hard to scan because all text looks the same (see
`docs/screen-test-results.md`, "Owner feedback after Slice 1"). That is one
person who knows the tool, not test evidence. The fixes below are expert
review. The moderated screen test was waived for Slice 1 and has still not
been run.

The code bears the feedback out:

- Week is reached only through Home's last row. On the default corpus,
  Home → Nov 13's sessions takes 7 presses (↓×4, Enter, ↓, Enter). Home's
  daily bars can be seen but not selected.
- `/` finds components only. Sessions and days can't be found by name, so a
  session in a 280-row stress Day list can only be reached with ↓ or PgDn.
- No key jumps to the first or last row of a list.
- Each row is drawn in a single style (ink, or amber for flags). Figures,
  names, workspaces and agents look the same.

## What Changes

- Home's daily bars become selectable. ←/→ moves a day selection across the
  bars, and Enter opens that day's sessions. Esc from the day returns to Home
  with the same bar selected.
- On Home and Week, the keys 1–7 open the period's first to seventh day
  (Nov 12–18) directly.
- `/` Find matches days and sessions as well as components, grouped by kind.
  With an empty search it lists components as it does today.
- Home and End jump to the first or last row of any list or of Home's
  findings. g and G do the same everywhere except Find, where letters are
  typed.
- Figures (costs, request counts, token counts, shares) on list rows are
  drawn bold. Workspace and agent columns are drawn muted. This applies in
  colour and under `NO_COLOR`, where bold and dim carry the difference.
- The footer and the `?` help name the new keys on the screens where they
  work.
- Render, journey and tui-test coverage for each change, on both corpora at
  80×24 and 60×24.

No existing key changes meaning. The JSON export and both corpora do not
change.

## Capabilities

### New Capabilities
- `screen-navigation`: how a person moves between Home, days, sessions and
  components, including the keys that jump to a destination and what Find
  matches.
- `screen-emphasis`: how a screen's rows separate figures from labels and
  secondary columns so they can be scanned.

### Modified Capabilities
None. `screen-readability` (in `stress-readability`) and `synthetic-corpus`
(in `add-stress-corpus`) are not archived yet and their requirements don't
change.

## Impact

- `src/app.rs`: ←/→, 1–7, Home/End and g/G handling.
- `src/model.rs`: the Home day selection on `Page`, and a way for rows to
  mark their figure and secondary spans.
- `src/screen/home.rs`: drawing the selected bar.
- `src/screen/list.rs`: drawing marked spans bold or muted.
- `src/views.rs` and `src/plan.rs`: Find over days and sessions; Week, Day,
  Session and Tokens rows mark their spans.
- `src/screen/mod.rs`: footer keys. `src/views.rs`: help lines.
- `src/tests/`, `src/e2e/`, `tui-tests/`: new and updated tests. Tests that
  assert the Find title or help text change with it.
- `README.md` and `docs/screen-test.md`: the walkthroughs name the shorter
  paths. The screen test's tasks stay the same, so the test can still check
  them.
- No new dependencies. `tests/golden/default-export.json` and both corpora
  stay byte-stable.
- Out of scope: mouse support, a command palette, sorting or filtering Day
  lists, fuzzy matching, the hard-coded Nov 12–18 week, and colour palette
  changes beyond bold and muted.
