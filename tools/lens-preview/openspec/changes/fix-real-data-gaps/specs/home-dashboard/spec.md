# Spec Delta

## MODIFIED Requirements

### Requirement: Findings in order of urgency
Home SHALL list its "Worth a look" findings most urgent first, each shown only
when it applies: (1) the plan window that fills soonest or, when none does,
a window that may be full ("<agent> <window>: <share>%, may be full"); (2)
the largest complete day, when it is more than 3× normal; (3) the known gap
in which copy of review-kit loads in Codex; (4) the first session with
partial input; (5) a link to every day and its sessions.

Covered by: `home_leads_with_the_filling_window`,
`home_dashboard_shows_usage_before_findings_at_80x24`,
`elapsed_forecast_says_may_be_full` and two `incomplete_*_called_unpriced`
tests.

#### Scenario: Order in USD
- **WHEN** Home is shown at 80×24 in USD
- **THEN** the findings are, in order: "Claude 5-hour: 62%, full by 13:22", "Nov 16: 10× a normal day", "review-kit: unknown load in Codex", "Nov 18: partial input, unpriced", and "Every day and its sessions"

#### Scenario: Partial input is not always unpriced
- **WHEN** the first session with partial input has every request priced
- **THEN** its finding reads "<day>: partial input" with no ", unpriced"; it says ", unpriced" only when a request has no rate

#### Scenario: A window may be full
- **WHEN** no window is on pace to fill, and Codex weekly's pace filled it before now with no newer reading
- **THEN** the first finding reads "Codex weekly: <share>%, may be full" and opens Plan headroom
