# Spec Delta

## Purpose

Home, the first screen of the Lens preview. This delta covers Home on local
data.

## MODIFIED Requirements

### Requirement: Answer line
Home's answer line SHALL read: sessions · requests · tokens · priced USD,
for every session from the selected source in the period. On local data
with no sessions, it SHALL read "No sessions in the last 7 days".

Covered by: `home_dashboard_shows_usage_before_findings_at_80x24`
(src/tests/render.rs), plus a local-data test to be added (tasks 4.3).

#### Scenario: Default corpus
- **WHEN** Home is shown with the default corpus
- **THEN** the answer starts "12 sessions" and contains "$12.36 priced"

#### Scenario: No local sessions
- **WHEN** Home is shown with `--source local` and no session in the period
- **THEN** the answer reads "No sessions in the last 7 days" and the preview keeps running

### Requirement: Where tokens went
Home SHALL show a "Where tokens went" table by source. Its shares SHALL add up
to 100%, and Hooks SHALL be listed with 0 tokens rather than left out. The
split is a fixture value, because no source records it. On local data the
title SHALL read "Where tokens went (illustrative)".

Covered by: `token_source_shares_sum_to_whole` and
`home_dashboard_shows_usage_before_findings_at_80x24` (src/tests/render.rs),
plus a local-data test to be added (tasks 4.3).

#### Scenario: Shares and Hooks
- **WHEN** Home is shown at 80×24
- **THEN** the table lists Hooks after File reads, and the source basis points sum to 10,000

#### Scenario: Labelled on local data
- **WHEN** Home is shown at 80×24 with `--source local`
- **THEN** the table title reads "Where tokens went (illustrative)"

### Requirement: Findings in order of urgency
Home SHALL list its "Worth a look" findings most urgent first, each shown only
when it applies: (1) the plan window that fills soonest; (2) the largest
complete day, when it is more than 3× normal; (3) on the fixture only, the
known gap in which copy of review-kit loads in Codex; (4) the first session
with partial input; (5) a link to every day and its sessions.

Covered by: `home_leads_with_the_filling_window` and
`home_dashboard_shows_usage_before_findings_at_80x24` (src/tests/render.rs),
plus a local-data test to be added (tasks 4.3).

#### Scenario: Order in USD
- **WHEN** Home is shown at 80×24 in USD
- **THEN** the findings are, in order: "Claude 5-hour: 62%, full by 13:22", "Nov 16: 10× a normal day", "review-kit: unknown load in Codex", "Nov 18: partial input, unpriced", and "Every day and its sessions"

#### Scenario: No review-kit finding on local data
- **WHEN** Home is shown with `--source local`
- **THEN** no finding mentions review-kit
