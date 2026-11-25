# Spec Delta

## Purpose

What `--json` prints. This delta covers Claude windows and weekly history on
local data once the statusline relay exists.

## MODIFIED Requirements

### Requirement: Whole corpus export
The export SHALL contain a `source` field (`"fixture"` or `"local"`) and
the data of that source: `synthetic`, `period`, `snapshot`, `workspaces`,
`agents`, `components`, `placements`, `scans`, `sessions`, `windows`,
`token_sources`, `limit_hits`, `claude_weekly_ends_percent` and
`now_minutes_from_nov_18`. A field SHALL keep its JSON type on both sources,
and SHALL be null where the source can't supply it. Every number on Home and
Plan headroom SHALL be rebuildable from the export.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs),
plus local export tests to be added (`add-local-source` tasks 5.1 and this
change's tasks 3.3).

#### Scenario: Home and Plan headroom data present
- **WHEN** the export is built from the default corpus
- **THEN** `source` is `"fixture"`, `windows` has 4 entries, `limit_hits` is 3, `claude_weekly_ends_percent` has 4 entries, and `now_minutes_from_nov_18` equals the fixture's "now"

#### Scenario: Local export keeps the types
- **WHEN** the export is built with `--source local` and no relay file
- **THEN** `synthetic` is false, `limit_hits` is a number, `token_sources` is a list of estimated shares, `claude_weekly_ends_percent` and `now_minutes_from_nov_18` are null, and Claude windows have no readings

#### Scenario: Local export with relay data
- **WHEN** the export is built with `--source local` and a relay file spanning two weekly resets
- **THEN** Claude windows hold their relay readings, and `claude_weekly_ends_percent` is a list of 2 numbers
