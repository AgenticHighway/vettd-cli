# Spec Delta

## Purpose

Defines how the Lens preview's list rows separate figures from labels and
secondary columns, so a person can scan a screen for amounts and names.

## ADDED Requirements

### Requirement: Figures are drawn bold
On Week, Day, Session, Tokens and Find session rows, figures SHALL be drawn
bold: costs, request counts, token counts and shares. Labels, names and
dates SHALL not be bold. This SHALL hold in colour and under `NO_COLOR`, and
on the selected row. Flagged rows (partial, unpriced, unknown) SHALL keep
their flag colour, with their figures still bold.

#### Scenario: Day totals stand out
- **WHEN** the default Nov 16 list is drawn at 80×24
- **THEN** each row's total cells are bold, and its name cells are not

#### Scenario: Dates are not figures
- **WHEN** Week is drawn at 80×24
- **THEN** the "Nov 12" cells are not bold, and each day's amount cells are

#### Scenario: Bold without colour
- **WHEN** a Day list is drawn under `NO_COLOR`
- **THEN** total cells are bold and name cells are not

### Requirement: Secondary columns are muted
On Day rows and Find session results, the workspace and agent columns SHALL
be drawn muted, so the session name and the figures carry the most weight.
Under `NO_COLOR` muted cells SHALL be drawn dim. The selected row SHALL use
the selection style throughout, with figures still bold.

#### Scenario: Workspace and agent are muted
- **WHEN** the default Nov 16 list is drawn at 80×24
- **THEN** each unselected row's workspace and agent cells use the muted style, and its name cells do not

#### Scenario: Muted without colour
- **WHEN** a Day list is drawn under `NO_COLOR`
- **THEN** workspace and agent cells are dim

### Requirement: Emphasis keeps row text unchanged
Emphasis SHALL change only the style of cells, never the characters drawn.
Every screen's text SHALL be the same as without emphasis.

#### Scenario: Same text with emphasis
- **WHEN** any Week, Day, Session or Tokens screen from either corpus is drawn at 80×24 and 60×24
- **THEN** its characters equal those drawn before this change
