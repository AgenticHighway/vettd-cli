# Proposal

## Why

PR #332 rebuilt the Lens preview as a ratatui dashboard. It added Home, Plan
headroom, scrolling that reaches every row, a unit toggle that keeps its
selection, and a larger `--json` export. The code and its tests are done, but
nothing in `openspec/specs/` describes the behavior. This change records it,
so later slices change a written contract and not only the tests.

This change documents behavior that already shipped. It changes no code.

## What Changes

- **TUI shell.** The preview draws with ratatui (crossterm backend) on stderr,
  so stdout stays free. It has a fixed frame layout, a too-small screen below
  38×10, NO_COLOR support and a preview pane at 76 columns and wider. It is
  its own Cargo workspace with its own CI workflow.
- **Navigation.** Esc goes back and restores the selection. Arrow and page
  keys move the selection, or scroll when there is no selectable row to move
  to. Find takes letters as text. `?` doesn't stack. `c` toggles the unit
  where it is offered and keeps the selection on the same target.
- **Home dashboard.** The first screen. It has an answer line, a daily chart
  with a "normal" level, a "where tokens went" table, and findings ordered by
  urgency that always get their rows first.
- **Plan headroom.** One row per provider window, with a straight-line pace
  to reset. The answer names the window that fills soonest. No plan is
  recommended.
- **JSON export.** `--json` prints the whole synthetic corpus, so every number
  on Home and Plan headroom can be rebuilt from it.

## Capabilities

### New Capabilities
- `tui-shell`: how the interactive preview is drawn and laid out, and how it
  is built and checked apart from vettd-cli.
- `navigation`: what each key does and how selection and scroll are kept.
- `home-dashboard`: the first screen and its findings.
- `plan-headroom`: provider usage windows, their pace, and what Lens does and
  doesn't claim about them.
- `json-export`: what `--json` contains.

### Modified Capabilities
None. `openspec/specs/` is empty. The open change `add-stress-corpus` also
describes `--json`. See design.md, "Overlap with add-stress-corpus".

## Impact

- Documentation only: `openspec/changes/add-lens-tui-dashboard/`.
- No source, test, workflow or README file changes.
- Out of scope: screens that exist only in the design proposal (Cache, Risky
  commands, Unused skills, Update every copy, Relay setup), reading real logs
  or the Claude statusline, and the main vettd-cli crate.
