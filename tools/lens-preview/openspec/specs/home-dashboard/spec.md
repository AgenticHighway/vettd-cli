# home-dashboard Specification

## Purpose
Home, the first screen of the Lens preview: what was used this period, where
the tokens went, and the few findings worth opening.

## Requirements

### Requirement: Home is first
The preview SHALL start on Home, and Home SHALL be the root of every
breadcrumb.

Covered by: `plan_journey_goes_deep_and_comes_back_to_the_same_finding`
(src/e2e/mod.rs, which starts with the Home finding selected) and
`esc_at_home_keeps_the_person_at_home`.

#### Scenario: Start
- **WHEN** the preview starts at 80×24
- **THEN** the breadcrumb is "Home" and the first finding is selected

### Requirement: Answer line
Home's answer line SHALL read: sessions · requests · tokens · USD, for
every session from the selected source in the period. The USD figure SHALL
be labelled "priced" on the fixture and "API-equivalent" on local data. With
no local sessions, it SHALL read "No sessions in the last 7 days".

Covered by: `home_dashboard_shows_usage_before_findings_at_80x24`
(src/tests/render.rs), plus a local-data test to be added (tasks 4.3).

#### Scenario: Default corpus
- **WHEN** Home is shown with the default corpus
- **THEN** the answer starts "12 sessions" and contains "$12.36 priced"

#### Scenario: Local dollars
- **WHEN** Home is shown with `--source local`
- **THEN** the dollar figure is followed by "API-equivalent"

#### Scenario: No local sessions
- **WHEN** Home is shown with `--source local` and no session in the period
- **THEN** the answer reads "No sessions in the last 7 days" and the preview keeps running

### Requirement: Daily chart with a normal level
Home SHALL show one bar per day of the period, in the current unit, with a
"normal" level that is the median of complete days. A partial day SHALL NOT
count towards the normal level.

Covered by: `partial_day_reads_as_floor_and_stays_out_of_normal`
(src/tests/render.rs).

#### Scenario: Normal ignores partial days
- **WHEN** the daily totals are computed in USD
- **THEN** Nov 18 is partial, and the normal level equals the median of the days that are not partial

### Requirement: Empty and partial days are marked
A day with no input SHALL be labelled "·" and SHALL NOT be drawn as a $0 bar.
A partial day SHALL show "≥" before its total. A day is partial when any of
its input is incomplete or, in USD mode only, unpriced. No partial total
SHALL read as exact.

Covered by: `home_dashboard_shows_usage_before_findings_at_80x24` and
`partial_day_reads_as_floor_and_stays_out_of_normal` (src/tests/render.rs),
and `narrow_stress_chart_keeps_lower_bound_markers` (src/tests/corpus.rs).

#### Scenario: No-input and partial days
- **WHEN** Home is shown at 80×24 in USD
- **THEN** the chart shows "13 ·" for Nov 13 and "18 ≥$0.54" for Nov 18, never "18 $0.54"

#### Scenario: Every day partial on a narrow terminal
- **WHEN** Home is shown with `--corpus stress` at 50×24, where every day is partial
- **THEN** either the title reads "every day partial (≥)" in full, in place of the per-bar markers, or every day's bar is marked "≥"

### Requirement: Where tokens went
Home SHALL show a "Where tokens went" table by source, with shares adding up
to 100% and Hooks always listed. On the fixture, Hooks SHALL show 0. On
local data the shares are estimated from block sizes, and the title SHALL
read "Where tokens went (estimated)".

Covered by: `token_source_shares_sum_to_whole` and
`home_dashboard_shows_usage_before_findings_at_80x24` (src/tests/render.rs),
plus a local-data test to be added (tasks 4.3).

#### Scenario: Shares and Hooks
- **WHEN** Home is shown at 80×24
- **THEN** the table lists Hooks after File reads, and the source basis points sum to 10,000

#### Scenario: Estimated on local data
- **WHEN** Home is shown at 80×24 with `--source local`
- **THEN** the table title reads "Where tokens went (estimated)", Hooks is listed, and the shares sum to 10,000 basis points

### Requirement: Findings in order of urgency
Home SHALL list its "Worth a look" findings most urgent first, each shown only
when it applies: (1) the plan window that fills soonest; (2) the largest
complete day, when it is more than 3× normal; (3) the
known gap in which copy of review-kit loads in Codex; (4) the first session
with partial input; (5) a link to every day and its sessions.

Covered by: `home_leads_with_the_filling_window`,
`home_dashboard_shows_usage_before_findings_at_80x24` and two
`incomplete_*_called_unpriced` tests.

#### Scenario: Order in USD
- **WHEN** Home is shown at 80×24 in USD
- **THEN** the findings are, in order: "Claude 5-hour: 62%, full by 13:22", "Nov 16: 10× a normal day", "review-kit: unknown load in Codex", "Nov 18: partial input, unpriced", and "Every day and its sessions"

#### Scenario: Partial input is not always unpriced
- **WHEN** the first session with partial input has every request priced
- **THEN** its finding reads "<day>: partial input" with no ", unpriced"; it says ", unpriced" only when a request has no rate

### Requirement: Review-kit finding only on the fixture
The review-kit finding SHALL be shown only on the fixture. Home on local
data SHALL NOT show it.

Covered by: `session_across_midnight_shows_on_both_days_and_counts_once`
(src/local/tests.rs).

#### Scenario: No review-kit finding on local data
- **WHEN** Home is shown with `--source local`
- **THEN** no finding mentions review-kit

### Requirement: Findings open their views
Each finding SHALL open the view that explains it: Plan headroom, that day,
review-kit for the week, that session, and the daily view.

Covered by: `plan_journey_goes_deep_and_comes_back_to_the_same_finding`,
`spend_journey_reaches_the_run_and_restores_selection`,
`partial_unpriced_session_says_so_and_never_shows_zero` and
`day_without_input_says_so` (src/e2e/mod.rs).

#### Scenario: Opening findings
- **WHEN** Enter is pressed on the first, second, fourth and fifth findings in turn
- **THEN** they open Plan headroom, the Nov 16 sessions, the Nov 18 session ("Input incomplete"), and Daily activity

### Requirement: Findings get their rows first
At every supported size, Home SHALL give the findings their rows before the
chart and table. The chart and table SHALL shrink or hide to make room, and
the findings SHALL scroll so the selected finding is always drawn.

Covered by: `selected_home_finding_is_drawn_at_every_size` and
`home_dashboard_stacks_at_60x24_and_keeps_selection` (src/tests/render.rs).

#### Scenario: Every finding at every size
- **WHEN** each Home finding in turn is selected at 80×24, 60×24, 80×12, 50×16 and 38×10
- **THEN** the selected finding is drawn with its "▸" marker each time
