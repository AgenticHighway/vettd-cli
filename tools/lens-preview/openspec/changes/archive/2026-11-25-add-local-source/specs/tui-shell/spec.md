# Spec Delta

## Purpose

How the interactive Lens preview is drawn and laid out. This delta adds how
the top bar names the data source.

## ADDED Requirements

### Requirement: Top bar names the source
The top bar SHALL say where the data comes from. On the fixture it SHALL
read "synthetic data · nothing leaves this machine". On local data it SHALL
read "local logs · read-only · nothing leaves this machine".

Covered by: a render test to be added (tasks 4.1).

#### Scenario: Fixture top bar
- **WHEN** the preview is drawn at 80×24 with `--source fixture`
- **THEN** line 1 contains "synthetic data"

#### Scenario: Local top bar
- **WHEN** the preview is drawn at 80×24 with `--source local`
- **THEN** line 1 contains "local logs · read-only" and not "synthetic data"

### Requirement: Synthetic views are labelled on local data
On local data, every view still built from the fixture SHALL show a
"synthetic" scope chip: the component inventory, component, copy,
observation, change and apply views.

Covered by: a render test to be added (tasks 4.2).

#### Scenario: Inventory on local data
- **WHEN** the component inventory is opened with `--source local`
- **THEN** its scope chips include "synthetic"
