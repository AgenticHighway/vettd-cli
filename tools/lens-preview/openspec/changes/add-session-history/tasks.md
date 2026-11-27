# Tasks

## 1. Read a span instead of a fixed period

- [ ] 1.1 Make the local read take its span (the dashboard's 7 days with the 28-day file cutoff, or "all" with no cutoff), with the dashboard passing exactly what it passes today. Verify: `cargo test --locked` passes unchanged, and the release `--json` (`33a1743f…`), `--corpus stress --json` (`c8ea73d4…`) and golden (`9e33c787…`) hashes are byte-identical.
- [ ] 1.2 Add reader tests on hand-written logs under `tests/data/`: a session whose file is 90 days old is found by the "all" span and not by the dashboard span, and a session spanning the period's start counts all its requests under "all". Each test's comment states why the two spans must differ. Verify: the new tests fail when the span is ignored.

## 2. History data apart from the dashboard

- [ ] 2.1 Hold History's sessions in their own `Data`, and let the session and token views say whether their index is a period or a History session. Fixture and stress History are the corpus's sessions. Verify: unit tests show the dashboard `Data` and export unchanged after History is loaded, and stress History has 1991 sessions, newest first, ties broken by agent then name.
- [ ] 2.2 Run the "all" read on a background thread after the first screen is drawn, sending progress (files read, files found) and the result over a channel. `--json` never starts it. Verify: a test shows `--source local --json` opens no file older than 28 days, and `tests/startup.rs` still passes.
- [ ] 2.3 While History is reading, poll for events with a short timeout so progress redraws and the list appears without a key press. Block on events again once the result arrives. Verify: an e2e test opens History before reading finishes and sees the rows appear with no further key.

## 3. History screen

- [ ] 3.1 Add the History view: rows of date, agent, name, requests and dollars, newest first, using `screen-emphasis` spans. Local dollars are API-equivalent, "unpriced" when no request has a rate, and incomplete sessions carry the partial flag. Verify: render tests at 80×24 and 60×24 on both corpora show long names cut with "…" and date, requests and dollars in full. Check the widths against the 48-cell list pane before settling the layout.
- [ ] 3.2 Show the reading state: "reading" with files read out of files found, and no rows. Verify: render test on a History that hasn't finished.
- [ ] 3.3 `h` opens History from Home and Week, and Esc returns with the previous selection. `h` is typed in Find. Verify: journey tests for Home → `h` → Esc with the second finding kept, and for typing `h` in Find.
- [ ] 3.4 Filter by typing, using Find's matcher over name, workspace, agent and date label. Backspace removes a character, the title shows the filter and the match count, and an empty result says no session matches. Verify: tests for filtering by month and for no match.
- [ ] 3.5 Enter opens the session view over all of the session's requests. Esc returns to the same filter and row. Verify: journey test filter → third row → Enter → Esc, and a test that a session spanning the period's start shows all its requests.
- [ ] 3.6 Footer and `?` help name `h` on Home and Week. Update the tests that assert footer or help text. Verify: `cargo test --locked`.

## 4. Docs and checks

- [ ] 4.1 Update `README.md`: History in the walkthrough, and in the `--source local` section that History reads every log file after the first screen, while the export doesn't. Verify: by reading. Every key named exists in help.
- [ ] 4.2 Add History journeys to `tui-tests/` (open, filter, open a session, back) on the stress corpus. Verify: the README's `node:22.17.0-bookworm` journey command passes all journeys.
- [ ] 4.3 Run the full checks in Docker: `cargo fmt --check`, `cargo clippy --locked --all-targets --quiet -- -D warnings`, `cargo test --locked`, the journeys, `openspec validate --strict --all`, the three export hashes, and 10 runs of `--corpus stress --json` timed against the last 24–47 ms. Verify: all pass and the hashes are byte-identical. Report any skipped check.
- [ ] 4.4 Ask the owner to run History on their own logs and report, without paths or names, how long until the list appears and whether it looks right. Record it in `docs/screen-test-results.md` as an owner by-eye check (expert review, not test evidence), with read time and peak memory. Verify: the entry exists, and the owner has seen it.

## Workflow follow-up

- Archive `add-session-history` after it's implemented and the owner agrees.
- Don't request reviews until the owner has checked History on their own logs.
