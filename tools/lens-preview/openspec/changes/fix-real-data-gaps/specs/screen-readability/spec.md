# Spec Delta

## ADDED Requirements

### Requirement: Day partial matches Week
A Day SHALL be partial exactly when Week marks it partial: some session's
input is incomplete or, in USD mode only, some request has no rate. A
partial Day's title SHALL end in "≥ partial" and its last scope chip SHALL
read "≥ means at least". A session row whose total is a floor SHALL end in
"≥"; a session with no rate at all reads "unpriced" instead.

Covered by: `unpriced_day_is_partial_on_day_as_on_week` and
`partly_priced_session_row_ends_in_floor_mark` (src/local/tests.rs).

#### Scenario: Unpriced day
- **WHEN** a local Day has complete input and a request with no rate, in USD
- **THEN** its title ends in "≥ partial" and its last chip reads "≥ means at least"; in request counts it is not partial

### Requirement: Day scope chips fit at 80 columns
A Day's scope chips SHALL show whole at 80 columns, in colour and under
NO_COLOR, with a total up to $999.99. A comparable day's last chip SHALL
read "<+ or -><difference> vs prior day"; a day that can't be compared
SHALL read "No comparison".

Covered by: `day_scope_chips_show_whole_at_80_columns`
(src/tests/readability.rs).

#### Scenario: Comparable day at 80 columns
- **WHEN** Nov 16 is shown at 80×24 on the default corpus
- **THEN** its chips include "All workspaces" and "+$7.69 vs prior day" whole, with no "…"
