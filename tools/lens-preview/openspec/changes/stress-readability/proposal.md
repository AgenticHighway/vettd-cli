# Proposal

## Why

Slice 0 made the preview load the stress corpus, about 2,000 sessions with
workspaces of up to 94 characters and model IDs of up to 63. It did not make
the screens read well at that scale. Rendering the stress corpus at 80×24 and
60×24 shows these defects (expert review, not user evidence; the screen test
was waived, see `docs/screen-test-results.md`):

- Home's daily bars are all nearly full height because the chart starts at
  zero, so the day-to-day differences can't be seen. At 60 columns the chart
  title is cut mid-word ("every day parti").
- Day lists (about 280 sessions) fix the workspace column at 4 characters.
  Longer workspaces push the agent and total out of line, and the 94-character
  one cuts the total off the row.
- On Session and Tokens views the long workspace chip doesn't fit, so it and
  every chip after it (agent, cost) become "…".
- A worker row with a 63-character model loses its cost. The preview pane
  breaks the same ID mid-word.
- The inventory workspace filter (`w`) cycles the fixed `api` and `web`
  list, so stress workspaces can't be filtered.
- Scrolling, PgUp and PgDn through a 295-row Day list has no test at stress
  scale.

## What Changes

- Home's chart uses a raised baseline when every day with data is close to
  the peak. The title says where the axis starts.
- Home's chart title shortens instead of being cut.
- Day list rows size the workspace column to the data and the room available.
  Over-long workspaces are shortened in the middle, and the agent and total
  stay aligned and whole.
- Scope chips shorten an over-long value instead of dropping it and every
  chip after it.
- Worker rows on Session, Tokens and the preview pane shorten long model IDs
  in the middle so the cost and token figures stay on the row.
- The inventory workspace filter cycles the workspaces in the loaded corpus.
- Render and end-to-end tests cover each fix on both corpora at 80×24 and
  60×24, and under `NO_COLOR`, and fail on the old behavior.

The default corpus's screens stay as they are wherever the stress-only
conditions don't arise. The JSON export does not change.

## Capabilities

### New Capabilities
- `screen-readability`: how the preview's screens keep figures whole and
  aligned when names are long, lists are long, and daily values are close
  together.

### Modified Capabilities
None. `synthetic-corpus` (in `add-stress-corpus`, not yet archived) is
unchanged.

## Impact

- `src/screen/home.rs`: chart baseline and title fitting.
- `src/screen/mod.rs`: chip shortening, and a shared middle-shortening helper.
- `src/model.rs` and `src/views.rs`: Day rows and worker rows built for the
  available width.
- `src/screen/list.rs`: the Day column header matches the row layout.
- `src/components/` and `src/app.rs`: the workspace filter reads the corpus's
  workspaces.
- `src/tests/` and `src/e2e/`: new render and journey tests.
- No new dependencies. `tests/golden/default-export.json` and both corpora
  stay byte-stable.
- Out of scope: the hardcoded Nov 12–18 week and `Scope::WEEK`, the plan
  windows and token sources that don't scale with stress volume, the
  low-coverage threshold (Slice 3), and thousands separators in dollar
  figures.
