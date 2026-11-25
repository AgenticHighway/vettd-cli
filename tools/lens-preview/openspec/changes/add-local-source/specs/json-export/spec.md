# Spec Delta

## Purpose

What `--json` prints. This delta adds the `source` field and the local
export.

## MODIFIED Requirements

### Requirement: Whole corpus export
The export SHALL contain a `source` field (`"fixture"` or `"local"`) and
the data of that source: `synthetic`, `period`, `snapshot`, `workspaces`,
`agents`, `components`, `placements`, `scans`, `sessions`, `windows`,
`token_sources`, `limit_hits`, `claude_weekly_ends_percent` and
`now_minutes_from_nov_18`. On local data, `synthetic` SHALL list the
sections still built from the fixture, and `now` (a UTC timestamp) SHALL
replace `now_minutes_from_nov_18`. Every number on Home and Plan headroom
SHALL be rebuildable from the export.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs),
plus a local export test to be added (tasks 5.1).

#### Scenario: Home and Plan headroom data present
- **WHEN** the export is built from the default corpus
- **THEN** `source` is `"fixture"`, `windows` has 4 entries, `limit_hits` is 3, `claude_weekly_ends_percent` has 4 entries, and `now_minutes_from_nov_18` equals the fixture's "now"

#### Scenario: Local export
- **WHEN** the export is built with `--source local`
- **THEN** `source` is `"local"`, `synthetic` lists `token_sources`, `components`, `placements` and `scans`, `limit_hits` is given per agent, Claude windows have no readings, and `claude_weekly_ends_percent` is null rather than an empty or made-up list
