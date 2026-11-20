# Spec Delta

## Purpose

Defines the synthetic datasets the Lens preview and its JSON export are built
from. A clean default fixture pins the documented numbers, and a noisy stress
corpus approximates real local logs at scale.

## ADDED Requirements

### Requirement: Corpus selection
The preview SHALL accept `--corpus <name>`, where the name is `default` or
`stress`, and `--json`, each anywhere on the command line. If `--corpus` is
omitted, the preview SHALL use `default`. Other arguments SHALL be ignored.

#### Scenario: Default when omitted
- **WHEN** the preview runs with `--json` and no `--corpus`
- **THEN** the export describes the default corpus and its `corpus` field is `"default"`

#### Scenario: Stress selected
- **WHEN** the preview runs with `--corpus stress --json`
- **THEN** the export describes the stress corpus and its `corpus` field is `"stress"`

#### Scenario: Flag order does not matter
- **WHEN** the preview runs with `--json --corpus stress`
- **THEN** stdout is byte-identical to a run with `--corpus stress --json`

#### Scenario: Unknown corpus rejected
- **WHEN** the preview runs with `--corpus other`
- **THEN** it prints `error: --corpus must be default or stress` to stderr, writes nothing to stdout and exits with code 2

#### Scenario: Missing corpus value rejected
- **WHEN** the preview runs with `--json --corpus` and nothing after it
- **THEN** it prints `error: --corpus must be default or stress` to stderr, writes nothing to stdout and exits with code 2

### Requirement: Deterministic export
The JSON export of either corpus SHALL be byte-identical on every run of the
same build.

#### Scenario: Repeated default export
- **WHEN** `--json` runs twice with the default corpus
- **THEN** the two stdout outputs are byte-identical

#### Scenario: Repeated stress export
- **WHEN** `--corpus stress --json` runs twice
- **THEN** the two stdout outputs are byte-identical

### Requirement: Default corpus unchanged
Apart from the added `corpus` field, the default corpus SHALL keep every
session, figure and field it had before this change.

#### Scenario: Export matches the pre-change capture
- **WHEN** the default corpus is exported with `--json`
- **THEN** the output, with the `corpus` field removed, equals as JSON the export captured from the branch head before this change, including `windows`, `token_sources`, `limit_hits`, `claude_weekly_ends_percent` and `now_minutes_from_nov_18`

#### Scenario: Pinned fixture figures hold
- **WHEN** the default corpus is loaded
- **THEN** the Nov 16 Index rebuild run costs $8.20, and every existing test of fixture figures still passes

### Requirement: Stress corpus shape
The stress corpus SHALL contain between 1,900 and 2,100 sessions over Nov 12–18,
spread across 6 workspaces. It SHALL include long paths and model IDs, and
session names that repeat across sessions.

#### Scenario: Scale and spread
- **WHEN** the stress corpus is loaded
- **THEN** it has 1,900–2,100 sessions, exactly 6 distinct workspaces, and sessions on each day from Nov 12 to Nov 18

#### Scenario: Long and duplicate identifiers
- **WHEN** the stress corpus is loaded
- **THEN** at least one workspace is longer than 80 characters, at least one model ID is longer than 40 characters, and at least one session name is shared by 10 or more sessions

### Requirement: Stress export workspaces and plan data
The stress export SHALL list the sorted, distinct workspaces of the stress
corpus. It SHALL carry the same plan-window and token-source data as the
default export, since neither corpus's sessions record that data.

#### Scenario: Workspaces from the selected corpus
- **WHEN** `--corpus stress --json` runs
- **THEN** `workspaces` lists the 6 stress workspaces in sorted order with no duplicates

#### Scenario: Plan data reused
- **WHEN** the stress and default exports are compared
- **THEN** their `windows`, `token_sources`, `limit_hits`, `claude_weekly_ends_percent` and `now_minutes_from_nov_18` values are equal, and `token_sources` basis points sum to 10,000

### Requirement: Low copy coverage
In the stress corpus, at most 15% of skill uses SHALL record which copy ran (an
artifact identity), and every recorded identity SHALL be one the inventory
lists for that component. A use without that record SHALL NOT have a version
label.

#### Scenario: Coverage ceiling
- **WHEN** the stress corpus is loaded
- **THEN** the share of skill uses that record an artifact is greater than 0% and at most 15%

#### Scenario: No version without identity
- **WHEN** a stress use has no artifact identity
- **THEN** its version is also absent

### Requirement: Unknowns stay unknown
Both corpora SHALL keep unpriced requests and incomplete sessions as their own
states. An unpriced request SHALL have no rate, never a zero rate.

#### Scenario: Unpriced in stress
- **WHEN** the stress corpus is exported
- **THEN** at least one request has a null rate, and no request has an all-zero rate standing in for "unpriced"

#### Scenario: Incomplete in stress
- **WHEN** the stress corpus is loaded
- **THEN** at least one session is marked incomplete

### Requirement: Stress load time
Building the stress corpus and writing its JSON export SHALL take less than one
second in a release build.

#### Scenario: Timed export
- **WHEN** a release build runs `--corpus stress --json` ten times
- **THEN** every run finishes in under one second of wall time

### Requirement: Interactive preview loads either corpus
The interactive preview SHALL start on the selected corpus without crashing.

#### Scenario: Stress content builds
- **WHEN** the screen content for Home, Plan headroom, Week, Find, Help, Inventory and each day of Nov 12–18 is built from the stress corpus
- **THEN** no build panics
