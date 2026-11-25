# Proposal

## Why

Every Lens preview screen is built from `src/fixture/`: 12 synthetic
sessions, 4 synthetic plan windows and an illustrative token-source split.
That was enough to settle layout and navigation, but it can't answer "is
this useful on my own machine?". The local logs Lens would read already hold
what Home and Plan headroom need. Claude Code and Codex both write
per-request token usage to their session logs, and Codex also writes its
5-hour and weekly `rate_limits` there.

This change makes the preview able to read that local data, read-only and
offline. The fixture stays the default and keeps every existing test.

This is the proposal only. It changes no code.

## What Changes

- **`--source fixture|local`.** `fixture` is the default and is exactly
  today's preview. `local` builds the sessions and Codex windows from local
  logs. An unknown value is rejected on stderr with a non-zero exit.
- **Claude Code sessions.** From `~/.claude/projects/*/*.jsonl`, with
  subagent files counted in their own session. One request per distinct
  `requestId` across all files, since resumed sessions copy history. Each
  request counts on its own UTC day.
- **Codex sessions.** From `~/.codex/sessions/**/rollout-*.jsonl`, with
  subagent threads counted in their root thread. One request per
  `token_usage_record` (per `response_id`).
- **Codex plan windows.** Plan headroom's two Codex rows come from
  `rate_limits.primary` / `secondary` in Codex `token_count` events. Each
  window is found by its `window_minutes` (300 or 10080) and by time, not by
  exact reset value.
- **Claude plan windows are not read in this change.** They reach Lens only
  through the statusline relay, which a later change adds. On local data,
  Claude rows say that they need the relay, and no number is shown or
  guessed.
- **Limit hits from logs.** "5-hour limit stopped work N×" counts the
  structured limit rejections that both agents record.
- **Pricing from a rate table in source.** It is dated, separates 5-minute
  and 1-hour cache writes, and only covers models it names. Any other model
  stays unpriced, never $0.
- **Labels.** The top bar names the source. Data that stays synthetic on
  local is labelled as synthetic where it is shown: the token-source split,
  the component inventory, copies and load order. The fixed review-kit
  finding is dropped on local.
- **Privacy.** Offline and read-only. The export, tests and docs hold
  aggregates only, never prompts, tool output, paths, folder names,
  titles, IDs or account data.

## Capabilities

### New Capabilities
- `local-source`: the source option, which files are read, how sessions,
  requests, Codex windows and limit hits are derived, pricing, failure
  handling and the privacy guarantees.

### Modified Capabilities
These are specs introduced by the open change `add-lens-tui-dashboard`. They
must be archived first, as described in design.md under "Ordering".
- `tui-shell`: the top bar names the data source.
- `home-dashboard`: the answer line covers the selected source, the
  token-source table is labelled illustrative on local data, and the
  review-kit finding is fixture-only.
- `plan-headroom`: window rows on local data, Claude rows without the relay,
  history rows from logs, and where window data comes from.
- `json-export`: a `source` field and an aggregates-only local export.

## Impact

- Code (for the apply step, not this change): a new reader module per agent,
  a pricing table, argument parsing in `src/main.rs`, and a source value
  passed to the views that read `fixture::` directly (`plan.rs`, Home, the
  top bar). The model's fixed Nov 12–18 period and `&'static str` fields
  must also accept a run-time period and owned strings.
- Test data: small hand-written JSONL files that mimic the log shapes. No
  real log line is copied into the repository.
- No network access and no new runtime dependencies beyond what parsing
  needs (`serde_json` is already a dependency).
- Out of scope: the statusline relay, real token-source attribution, a real
  component inventory and load order, Cache, Risky commands, Unused skills,
  Update every copy, Relay setup, and the main vettd-cli crate.
