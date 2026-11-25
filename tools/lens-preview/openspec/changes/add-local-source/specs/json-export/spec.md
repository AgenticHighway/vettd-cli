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
`now_minutes_from_nov_18`. A field SHALL keep its JSON type on both sources,
and SHALL be null where the source can't supply it. Every number on Home and
Plan headroom SHALL be rebuildable from the export.

Covered by: `export_carries_home_and_headroom_data` (src/tests/render.rs),
plus a local export test to be added (tasks 5.1).

#### Scenario: Home and Plan headroom data present
- **WHEN** the export is built from the default corpus
- **THEN** `source` is `"fixture"`, `windows` has 4 entries, `limit_hits` is 3, `claude_weekly_ends_percent` has 4 entries, and `now_minutes_from_nov_18` equals the fixture's "now"

#### Scenario: Local export keeps the types
- **WHEN** the export is built with `--source local`
- **THEN** `synthetic` is false, `limit_hits` is a number, `token_sources` is a list of estimated shares, `claude_weekly_ends_percent` and `now_minutes_from_nov_18` are null, and Claude windows have no readings

## ADDED Requirements

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
