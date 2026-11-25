# Proposal

## Why

`add-local-source` makes the Lens preview read local Claude Code and Codex
logs. Codex writes its 5-hour and weekly usage windows there. Claude Code
doesn't: its logs hold only rejections. Claude hands those windows to just
one place, the statusline command, through `rate_limits` on its stdin. So on
local data, Plan headroom's two Claude rows say "needs relay" and its weekly
history has no Claude figures.

This change adds the relay: an opt-in statusline mode that saves Claude's
window readings to a small local file, which `--source local` then reads.

This is the proposal only. It changes no code.

## What Changes

- **`--relay` mode.** Run as a Claude Code `statusLine.command`, the preview
  reads the stdin JSON. It appends only the `rate_limits.five_hour` and
  `seven_day` readings, with a timestamp, to `~/.vettd/lens/relay.jsonl`, and
  prints a short status line. It keeps no other field.
- **Opt-in, never automatic.** Lens never edits Claude's settings. The
  README shows the `statusLine` snippet to add by hand. An existing
  statusline can be kept with `--relay --then <command>`, which hands it the
  same stdin and prints its output.
- **Plan headroom reads the relay.** With `--source local`, the Claude
  5-hour and weekly rows use relay readings, with the same time-span
  grouping, running maximum, pace and reset rules as Codex. "Needs relay"
  splits into "relay not set up" and "no relay reading yet". Claude's weekly
  history comes from the last reading before each weekly reset.
- **Bounded and private.** One short line per changed reading. Readings
  older than 35 days are dropped when the relay next writes. Only this mode
  writes anything, and `--source local` stays read-only.

## Capabilities

### New Capabilities
- `statusline-relay`: the `--relay` mode, what it records, where, how it
  stays small and safe under concurrent sessions, and how Lens reads it back.

### Modified Capabilities
These are specs from the open changes `add-lens-tui-dashboard` and
`add-local-source`. Both must be archived first.
- `plan-headroom`: Claude rows and weekly history from the relay.
- `json-export`: Claude windows and `claude_weekly_ends_percent` on local
  data.

## Impact

- Code (for the apply step, not this change): argument parsing for
  `--relay` and `--then`, a relay writer, a relay reader beside the log
  readers from `add-local-source`, and README setup docs.
- One local file, `~/.vettd/lens/relay.jsonl`, written only by `--relay`.
- No network access and no new dependencies.
- Out of scope: a Relay setup screen in the TUI, editing Claude settings,
  `rate_limits.spend_limit` (gateway spend), and Codex, which needs no relay.
