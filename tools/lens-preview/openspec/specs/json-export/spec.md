# json-export Specification

## Purpose
What `--json` prints: the whole synthetic corpus, so every number the
preview shows can be rebuilt from it.

## Requirements

### Requirement: JSON mode
When `--json` is the first argument, the preview SHALL print one JSON object
to stdout and exit without starting the TUI.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs) for
the object. Argument handling is verified in `src/main.rs` (`main`).

#### Scenario: Export instead of TUI
- **WHEN** the preview runs with `--json`
- **THEN** stdout holds one JSON object and no terminal UI is drawn

### Requirement: Whole corpus export
The export SHALL contain the whole synthetic corpus: `synthetic`, `period`,
`snapshot`, `workspaces`, `agents`, `components`, `placements`, `scans`,
`sessions`, `windows`, `token_sources`, `limit_hits`,
`claude_weekly_ends_percent` and `now_minutes_from_nov_18`. Every number on
Home and Plan headroom SHALL be rebuildable from it.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs).

#### Scenario: Home and Plan headroom data present
- **WHEN** the export is built from the default corpus
- **THEN** `windows` has 4 entries, `limit_hits` is 3, `claude_weekly_ends_percent` has 4 entries, and `now_minutes_from_nov_18` equals the fixture's "now"

### Requirement: Token sources in basis points
Each `token_sources` entry SHALL give a source and its `basis_points`, and the
basis points SHALL sum to 10,000.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs).

#### Scenario: Shares sum to the whole
- **WHEN** the export is built
- **THEN** the `basis_points` of all `token_sources` sum to 10,000
