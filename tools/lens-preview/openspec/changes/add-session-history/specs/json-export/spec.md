# Spec Delta

## ADDED Requirements

### Requirement: History export (opt-in)
With `--json --history`, the export SHALL add a `history` array holding
every session History lists, newest first, ties broken by agent then name.
Each entry SHALL have the same fields and identifier rules as a `sessions`
entry. Without `--history`, the export SHALL hold no `history` key and SHALL
be byte-identical to what plain `--json` prints today. `--history` without
`--json` SHALL be rejected.

#### Scenario: Stress corpus history
- **WHEN** the preview runs with `--json --corpus stress --history`
- **THEN** `history` has 1991 entries, the first being the latest to start

#### Scenario: Plain export unchanged
- **WHEN** the preview runs with `--json` on the default corpus
- **THEN** stdout is byte-identical to `tests/golden/default-export.json` and has no `history` key

#### Scenario: Local history holds no identifiers
- **WHEN** the export is built with `--source local --json --history`
- **THEN** each `history` entry follows the same identifier rules as `sessions`, and the no-identifier test passes

#### Scenario: History flag without JSON
- **WHEN** the preview runs with `--history` and no `--json`
- **THEN** it prints `vettd-lens-preview: --history needs --json` to stderr, writes nothing to stdout and exits with code 2
