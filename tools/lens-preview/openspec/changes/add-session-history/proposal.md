# Proposal

## Why

When the owner checked the preview against their own logs (`--source local`),
they said they want to see historical sessions too. The preview shows only
the 7 UTC days ending today, and never opens a log file untouched for 28
days, so older sessions can't be seen at all. This is one person's request
after a by-eye check, not test evidence. The design below is expert review.

## What Changes

- A new History screen lists every session found in the logs on disk,
  however old, newest first, one row per session: its start date, agent,
  name, requests and API-equivalent dollars.
- `h` opens History from Home and Week. Esc returns to where it was opened.
- In History, typing filters the list, as Find does. It matches on a
  session's name, workspace, agent and date label.
- Enter on a History row opens that session's existing session view, built
  over all of the session's requests rather than only those in the period.
- On `--source local`, History is read after the dashboard is drawn, so
  startup stays as it is. Until reading finishes, History says it is still
  reading and how many files it has read.
- On `--source fixture`, History lists the corpus's sessions, with the
  "synthetic" chip. The stress corpus (1991 sessions) is the long-list case.
- The footer and `?` help name `h` on the screens where it works.

The 7-day dashboard, its figures and the 28-day limit-hit count don't change.
The JSON export doesn't change: History is not exported.

## Capabilities

### New Capabilities
- `session-history`: the History screen, what it lists, how it is reached,
  filtered and left, and how it reports that it is still reading.

### Modified Capabilities
- `local-source`: log files older than 28 days are now opened, but only for
  History, after the dashboard is drawn. The dashboard's period, usage and
  limit hits are unchanged.

## Impact

- `src/local/`: a second read over all files for History, run in the
  background, reusing the existing Claude and Codex readers.
- `src/data.rs` / `src/model.rs`: History sessions held apart from the
  period's sessions, and a History view.
- `src/app.rs`: `h`, filtering, Enter and Esc in History.
- `src/views.rs`, `src/screen/`: the History rows, the reading state, footer
  and help.
- `src/tests/`, `src/e2e/`, `tui-tests/`: new tests, including the stress
  corpus at 80×24 and 60×24.
- `README.md`: History in the walkthrough and the `--source local` section.
  `docs/screen-test.md` is unchanged.
- No new dependencies. `tests/golden/default-export.json`, the release and
  stress export hashes, and both corpora stay byte-stable.
- Out of scope: exporting History, sorting or grouping the list, date-range
  flags, stepping the dashboard back by week, and caching the read between
  runs.
