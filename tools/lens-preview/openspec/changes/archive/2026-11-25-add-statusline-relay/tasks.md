# Tasks

Before applying: archive `add-lens-tui-dashboard`, then `add-local-source`
(see design.md, "Ordering"), and settle the open questions in design.md.
Every existing test must keep passing on `--source fixture`.

## 0. Check the real feed

- [x] 0.1 With the person's consent, run one real Claude session with a temporary statusline command that writes only the stdin JSON's *key paths and value types* (no values) to a scratch file outside the repository. Compare them with design.md "What the statusline gives", and fix the design if they differ. Delete the scratch file afterwards (2026-11-25: the paths and types matched; `used_percentage` was an integer, which the relay accepts; the scratch file was deleted)

## 1. Relay writer

- [x] 1.1 Parse `--relay`, `--relay-file <path>` and `--then <command>`
- [x] 1.2 Read stdin, and append one whole-line record per present window with only `t`, `w`, `pct`, `resets_at`, skipping unchanged readings and `spend_limit`
- [x] 1.3 Default file `~/.vettd/lens/relay.jsonl`, creating the directory if missing
- [x] 1.4 Prune records older than 35 days at most once a day, via a temporary file and rename
- [x] 1.5 Print `5h <pct>% · 7d <pct>%` or nothing, or with `--then` the chained command's stdout. Always exit 0, with errors on stderr only
- [x] 1.6 Tests with hand-written stdin: both windows, absent `rate_limits`, bad JSON, identifying fields not stored, repeated reading, `--then` output, pruning

## 2. Relay reader

- [x] 2.1 Read the relay file with `--source local`, skipping unparsable lines, and honour `--relay-file`
- [x] 2.2 Build Claude windows with the Codex rules: time span, running maximum, pace, reset-since-last-reading
- [x] 2.3 Weekly history: the highest `pct` before each of the last 4 weekly resets, and the number of weeks seen

## 3. Screens and export

- [x] 3.1 Plan headroom Claude rows: relay readings, "relay not set up", and "no relay reading yet"
- [x] 3.2 Weekly history row from the relay, with a week count when fewer than 4
- [x] 3.3 Export Claude windows and `claude_weekly_ends_percent` from the relay, or null

## 4. Docs and checks

- [x] 4.1 README: build the binary, add the `statusLine` snippet by hand, chain an existing statusline with `--then`, and say what the file holds and where. No real readings in examples
- [x] 4.2 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` in `tools/lens-preview/`
- [x] 4.3 Check by grep that nothing outside `--relay` writes files, and that no mode writes Claude settings
