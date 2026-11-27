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
The export SHALL contain a `source` field (`"fixture"` or `"local"`) and
the data of that source: `synthetic`, `period`, `snapshot`, `workspaces`,
`agents`, `components`, `placements`, `scans`, `sessions`, `windows`,
`token_sources`, `limit_hits`, `claude_weekly_ends_percent` and
`now_minutes_from_nov_18`. Every number on Home and Plan headroom SHALL be
rebuildable from the export.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs).

#### Scenario: Home and Plan headroom data present
- **WHEN** the export is built from the default corpus
- **THEN** `source` is `"fixture"`, `windows` has 4 entries, `limit_hits` is 3, `claude_weekly_ends_percent` has 4 entries, and `now_minutes_from_nov_18` equals the fixture's "now"

### Requirement: Field types match across sources
A field SHALL keep its JSON type on both sources, and SHALL be null where
the source can't supply it.

Covered by: `local_export_holds_no_identifiers` and
`relay_readings_feed_claude_windows_like_codex` (src/local/tests.rs).

#### Scenario: Local export keeps the types
- **WHEN** the export is built with `--source local` and no relay file
- **THEN** `synthetic` is false, `limit_hits` is a number, `token_sources` is a list of estimated shares, `claude_weekly_ends_percent` and `now_minutes_from_nov_18` are null, and Claude windows have no readings

#### Scenario: Local export with relay data
- **WHEN** the export is built with `--source local` and a relay file spanning two weekly resets
- **THEN** Claude windows hold their relay readings, and `claude_weekly_ends_percent` is a list of 2 numbers

### Requirement: Token sources in basis points
Each `token_sources` entry SHALL give a source and its `basis_points`, and the
basis points SHALL sum to 10,000.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs).

#### Scenario: Shares sum to the whole
- **WHEN** the export is built
- **THEN** the `basis_points` of all `token_sources` sum to 10,000

### Requirement: Local-only export fields
A local export SHALL add four fields that a fixture export doesn't have:
`now` (a UTC timestamp), `synthetic_sections` (sections still built from
the fixture), `estimated_sections` (sections derived rather than read) and
`limit_hits_by_agent` (an object keyed by agent). On local data,
`limit_hits` SHALL be the sum of `limit_hits_by_agent`.

Covered by: a local export test to be added (tasks 5.1).

#### Scenario: Synthetic sections named
- **WHEN** the export is built with `--source local`
- **THEN** `synthetic_sections` lists `components`, `placements` and `scans`, `estimated_sections` lists `token_sources`, and `limit_hits` equals the sum of `limit_hits_by_agent`

#### Scenario: Fixture export unchanged
- **WHEN** the export is built with `--source fixture`
- **THEN** it has no `now`, `synthetic_sections`, `estimated_sections` or `limit_hits_by_agent` field
