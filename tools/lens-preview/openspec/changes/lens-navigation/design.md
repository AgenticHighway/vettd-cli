# Design

All judgments here are expert review. The screen test has not been run (see
proposal.md, Why).

## Context

- Keys are handled in `src/app.rs`. `press` routes Esc and Enter first, then
  `edit_page` (moving, Find typing, inventory filters), then `global`. Find
  takes printable characters before any command, so typing never fires one.
- `frame` builds the current view's `Row`s (`text` plus an optional `open`
  target). The selection indexes the selectable rows. `Viewport::navigate`
  moves or scrolls. The Enter hint and `n/total` come from the active row.
- Home is drawn by `src/screen/home.rs`: a bar chart from `daily()`, a token
  table, and the findings, which are Home's rows. The chart has no selection.
- List rows are drawn by `src/screen/list.rs::lines` as one styled string per
  row. `row_style` picks ink, muted (dividers), amber (flags) or cyan.
- Each `Page` on the stack keeps its own selection, scroll and Find filter,
  so Esc already restores them.
- Slice 1 builds Day rows and their header from shared widths
  (`day_ws_width`, `day_header`), so column positions are known when rows
  are built.

## Goals / Non-Goals

**Goals:**
- Bring any day within three presses of Home, and any session within a
  short search.
- Keep the existing keys, selection rules and Esc behaviour unchanged.

**Non-Goals:**
- No new screens, no mouse, no command palette.
- No change to row text. Emphasis is style only, so Slice 1's width and
  alignment tests stay valid.

## Decisions

### 1. Home day selection is a second focus on the Home page

Add `day: Option<u32>` to `Page`, used only on Home. ←/→ set it (the first →
picks Nov 12, the first ← picks Nov 18, then ±1, clamped). ↑/↓/PgUp/PgDn/
Home/End clear it and move the findings selection as they do now, which
keeps `selected` untouched while a bar is chosen. When `day` is set, Enter
opens `View::Day(day)` and the hint says "See Nov 13 sessions". `frame`
returns an active target built from `day` instead of the finding, so `open`
and `draw_hint` need no special case. `n/total` is hidden while a bar is
chosen.

Alternative: put the days into Home's rows as selectable rows. Rejected
because the findings panel would then list seven day rows above the
findings, which changes Home's layout and its tests.

Alternative: a Tab-cycled focus between chart and findings. Rejected
because it adds a key to learn, while ←/→ have no meaning on Home today.

The selected bar is drawn in the selection style and its caption gets a
`▸` marker in place of the space after the day, so it stays visible under
`NO_COLOR`. Where the caption has been shortened to the day alone, the
marker replaces the caption's first column. Slice 1's caption fitting runs
on the caption with the marker in it.

### 2. Number keys map to the fixed period

1–7 open `View::Day(11 + n)` on Home and Week, by pushing a page as Enter
would. They are handled in `global`, after `edit_page`, so Find still types
them. The period is the hard-coded Nov 12–18 (out of scope to change). If
it ever changes, this mapping follows the same list of days that Week
builds.

### 3. Find builds three groups from the corpus

`find` takes `all` and the measure. With an empty filter it returns today's
component rows unchanged. Otherwise it returns days, sessions and
components that match, each group preceded by a non-selectable divider
`── Days (n)` (`row_style` already mutes `──` rows). Session rows reuse the
Day row layout with a leading `Nov dd` column, built for the list width
like `session_row`. A group with no matches is left out. The title becomes
"Find a day, session or component", and the empty-result message "No
matches. Edit search." stays.

No match cap: at stress scale one letter can match about 2,000 sessions.
Building them is linear and the list scrolls, so it is fine for a preview.
Tests check that it draws without panic. A cap would hide results with no
way to reach them.

Alternative: fuzzy matching. Rejected as out of scope. Substring matching is
what the Find screen does today.

### 4. Home/End and g/G go through the viewport

Add `Viewport::jump(page, last: bool)`: select the first or last action and
set `top` to 0 or the maximum scroll, with `follow` on. With no actions it
only scrolls. `edit_page` maps Home/End always, and g/G when not in Find. On
Home, Home/End clear the bar selection like the other vertical keys.

### 5. Rows carry style spans, set by the builders that know the columns

Add `spans: Vec<(Range<usize>, Emphasis)>` to `Row`, with
`Emphasis::{Figure, Secondary}`, in char offsets of `text`. `line` and `link`
leave it empty, so every existing row draws as before. Week, Day, Session,
Tokens and Find session builders push spans where they format each column:
they already compute the widths. `list.rs::lines` splits the row into
`Span`s. Figure adds `BOLD` to the row's base style (ink, amber, or the
selection style). Secondary uses the muted colour when not selected, and
`DIM` under `NO_COLOR`. Spans that fall past the drawn width are clipped.

Alternative: detect figures in the text with a pattern (`$…`, digits with
k/M/%). Rejected because "Nov 16", "5-hour" and model names contain digits,
and a pattern would need exceptions on every screen. The builders know
exactly which column is which.

### 6. Footer and help

Help gains lines for ←/→ (Home: choose a day), 1–7 (Home, Week: open a
day), Home/End and g/G (first or last row). At 65 columns and more, Home's
footer replaces `q Quit` with `←→ Day`. `q` still works and is in help. Week
uses the measured footer with `1–7 Day` added. Below 65 columns, footers
are unchanged. The footer must not grow past the width, and an existing
test checks this.

## Risks / Trade-offs

- [The first ←/→ press on Home moves a selection people don't know exists]
  → The bar marker and the Enter hint change at once. The screen test
  should watch whether people find it.
- [1–7 are positional and unlabelled] → Help and Week's footer name them,
  and Week rows are in the same order. They are a shortcut, never the only
  way.
- [Find at stress scale lists thousands of sessions] → Dividers show counts.
  End and g/G make the far end reachable.
- [Span offsets drift from text when a builder changes] → A test per
  builder asserts that span ranges fall on the right cells. A shared check
  asserts every span is inside its row's char length.
- [Emphasis makes the selection harder to see] → The selection keeps its
  background, and only bold is added to figures within it.
- [Tests that compare whole buffers by style may change] → Only text is
  compared today. Any style-comparing test that changes is listed in the
  task evidence.

## Migration Plan

Preview-only binary, no data or API. Each section in tasks.md lands as its
own commit, so any part can be reverted alone.
