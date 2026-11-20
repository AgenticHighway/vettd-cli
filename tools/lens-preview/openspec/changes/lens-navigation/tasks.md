# Tasks

Paths are relative to `tools/lens-preview/`. Run every check in Docker
`rust:1.97.1` with the source mounted read-only and
`CARGO_TARGET_DIR=/tmp/target`. Each new test must fail on the code from
before its task. Confirm this by running it against the previous commit, or
by reasoning recorded in the evidence. Label design judgments as expert
review.

## 1. Baseline

- [x] 1.1 Before any code change, record the sha256 of `--json` and `--corpus stress --json` from a release build, and of `tests/golden/default-export.json`. Verify they match Slice 1's recorded figures. Count and record the presses for Home → Nov 13 sessions (expected 7) and Home → Index rebuild session on the default corpus
  - Evidence: Docker `rust:1.97.1` at `3f86b26`: default `2f641dd0…16cf5f` (with and without `--corpus default`), stress `d6b8d5c3…197469` (both flag orders), golden `9e33c787…236ab99`. All match Slice 1. Presses (from the existing journeys `day_without_input_says_so` and `spend_journey_reaches_the_run_and_restores_selection`): Home → Nov 13 sessions 7 (↓×4, Enter, ↓, Enter); Home → Index rebuild 3 (↓, Enter, Enter).

## 2. Jump keys

- [x] 2.1 Add `Viewport::jump` and map Home/End always and g/G outside Find in `src/app.rs` (design decision 4). Unit-test `jump` with actions, without actions, and with leading and trailing non-selectable rows. Add a journey test: on the stress Nov 16 list at 80×24, End selects the last session with the last row visible, and g returns to the first. Add a test that g in Find types "g"
  - Evidence: `Viewport::jump` in `src/app.rs`; Home/End always, g/G outside Find. Unit tests in `src/tests/navigation.rs` (with actions, without actions, short list). Journeys in `src/e2e/navigation.rs`: stress Nov 16 End gives `n/n` with the last row visible, g gives `1/n`, then G and Home; End/Home on Home's findings; g and G typed in Find. Old code fails by reasoning: Home, End and g were ignored, so the counter stayed `1/n`.

## 3. Home bars and number keys

- [x] 3.1 Add `Page::day` and the ←/→ rules (design decision 1). Make `frame` return the bar's day as the active target and make ↑/↓/Home/End clear it. Journey tests: →, →, Enter opens Nov 13; Esc returns with Nov 13 still selected; ← then ↓ returns to the earlier finding; → at Nov 18 stays there; a no-data day opens and shows "No session input."
  - Evidence: `Page::day`; ←/→ in `edit_page`; `frame` swaps the active target for `View::Day(day)` with a static hint; the first vertical key only clears the bar; `c` keeps the finding while a bar is chosen. Journeys: →, →, Enter opens Nov 13 in 3 presses (was 7) and shows "No session input."; Esc keeps Nov 13 chosen; ← starts at Nov 18 and → stops there; ↓ after ← returns to the Nov 16 finding, and the `2/5` counter is hidden while the bar is chosen. Old code fails: ←/→ did nothing.
- [x] 3.2 Draw the selected bar in the selection style with the `▸` caption marker in `src/screen/home.rs`. Render tests at 80×24 and 60×24 on both corpora, and under `NO_COLOR`: the marker is on the selected day only and no caption is cut. Verify the existing Home render tests pass unedited with no bar selected
  - Evidence: The chosen caption replaces the space after the day with `▸` (`14▸$0.82`, `13▸·`), or prefixes it when only the day fits (`▸14`), drawn in the selection style. A test chooses every day on both corpora at 80×24 and 60×24, in colour and under `NO_COLOR`. Screens inspected at stress 60×24 and default 80×24 and 60×24: no caption cut (expert review). Existing Home render tests pass unedited.
- [x] 3.3 Map 1–7 on Home and Week in `global` (design decision 2). Journey tests: 5 on Home opens Nov 16 and Esc returns to Home; 1 on Week opens Nov 12; 1 on a Session view does nothing; 1 and 6 in Find give the search "16"
  - Evidence: `1`–`7` in `global` on Home and Week. Journeys: 5 on Home opens Nov 16 and Esc returns; 1 on Week opens `Home › Daily activity › Nov 12`; 1 on a Session view leaves the crumb unchanged; `16` is typed in Find. Docker: 120 passed.

## 4. Find

- [x] 4.1 Extend `find` in `src/views.rs` to days, sessions and components with counted dividers (design decision 3), and change the title. Tests: "index" lists Index rebuild under Sessions and Enter opens it; "nov 16" lists the day and Enter opens its sessions; Esc after opening keeps the search and selection; an empty search gives the same rows as before (compare against the old component list); a workspace name matches its sessions; stress one-letter search draws at 80×24 and 60×24 without panic. Update tests and tui-tests that assert the old title, and list each one in the evidence
  - Evidence: `find` in `src/views.rs` takes the corpus, measure and width. Days match "Nov dd"; sessions match name or workspace, ordered by day, then measure largest first, as `Nov dd` + the Day row; components are unchanged. Each group sits under a `── Name (n)` divider; the title is "Find a day, session or component". Tests: "index" lists 6 Index rebuild sessions (it runs daily) and the Nov 16 one opens, with Esc keeping the selection; "nov 16" opens the day; "api" counts every api session and Esc keeps search and row; empty search equals the old component list; day-then-size order; stress "e" draws at 80×24 and 60×24 and End reaches the last row. No test or tui-test asserted the old title, so none changed (grep). Docker: 126 passed. Old code fails: Find matched components only.

## 5. Emphasis

- [x] 5.1 Add `Row::spans` and `Emphasis`, and draw them in `src/screen/list.rs::lines` (design decision 5). Unit-test span splitting: empty spans, spans clipped at the width, multi-byte text, and the selected row keeping its background
  - Evidence: `Row::spans`, `Emphasis` and the `Cells` builder in `src/model.rs`; `emphasised()` in `src/screen/list.rs` splits each row into runs. Figure adds bold to the row style; Secondary is muted (dim under `NO_COLOR`) except on the selected and flagged rows. `Theme` is private to `screen`, so the cases are tested on drawn cells in `src/e2e/emphasis.rs`: unmarked help rows have no bold; stress rows clipped at 60×24 and shortened with `…` keep bold totals and muted workspaces; the selected row keeps its background with a bold total.
- [x] 5.2 Mark figure spans on Week, Day, Session, Tokens and Find session rows, and secondary spans on Day and Find session rows. Render tests on the default Nov 16 list and Week at 80×24, in colour and under `NO_COLOR`: totals are bold, names and dates are not, and workspace and agent cells are muted or dim. A shared test checks that every span on every row of both corpora's Week, Day, Session and Tokens screens is inside the row's text
  - Evidence: Marked: Week amounts; Day and Find session totals (not `unpriced`), workspace and agent; Session part costs and request costs; Tokens counts. Tests on drawn cells: default Nov 16 totals bold, names plain, workspace and agent muted; Week `Nov 14` plain and `$0.82` bold, `No input` plain; `NO_COLOR` bold and dim; flagged Nov 18 row stays amber; Session and Tokens figures bold; Find results the same. `every_span_marks_the_cell_it_names` checks every span on Week, Day, Session and Tokens for both corpora, both widths and both units: inside the text, figures hold a digit, secondary cells are that session's agent or workspace. Old code fails: no cell was bold or muted.
- [x] 5.3 Verify the row text is unchanged: every existing text-only render test passes unedited, and the buffers' text for Week, Day, Session and Tokens on both corpora at 80×24 and 60×24 equals the text from before section 5
  - Evidence: Screen text for Week, Days 12–18, and Session and Tokens (all 12 default sessions, the first 60 stress sessions) on both corpora at 80×24 and 60×24 (320 screens) was dumped before section 5 and after it with a temporary test: byte-identical. Every existing text-only render test passed unedited. Docker: 136 passed.

## 6. Discoverability and docs

- [x] 6.1 Add the help lines and the Home and Week footer items (design decision 6). Render tests: Home at 80×24 names `←→`; Week at 80×24 names `1–7`; footers at 60×24 are unchanged; help lists every new key
  - Evidence: Help gains `Home/End First or last row (g / G too)`, `← / →   Home: choose a day's bar` and `1–7     Home, Week: open that day`, and its `/` line now reads "Find a day, session or component". At 65+ columns, Home's footer puts `←→ Day` in place of `q Quit` (q still quits and is in help) and Week gets `1–7 Day`. Tests: Home at 80×24 names `←→ Day`; Week names `1–7 Day`; at 60×24 Home and Week footers equal their old text; help lists every new key. `help_scrolls_to_quit` still passes at 38×10. Docker: 139 passed.
- [x] 6.2 Update `README.md` walkthroughs to name the shorter paths (→ to a day on Home, `/` for a session). Check that `docs/screen-test.md` tasks still read the same and add no key names to them
  - Evidence: README developer walkthrough gains a "Shorter paths" note (←/→ and Enter, 1–7, `/` for days and sessions, Home/End and g/G, bold figures and muted columns). `docs/screen-test.md` names no keys in its tasks (grep), so it is unchanged and the test still checks unprompted discovery.

## 7. Final checks

- [x] 7.1 Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked` and the tui-tests suite in Docker. Recheck the three sha256 figures against 1.1. Record presses for the two paths from 1.1 after the change (expected Home → Nov 13 in 3 or fewer). Record any check that was skipped, and why
  - Evidence: Docker `rust:1.97.1` (toolchain 1.88.0 from `rust-toolchain.toml`), source read-only: `cargo fmt --check` clean (after one `cargo fmt` pass on the new code), `cargo clippy --locked --all-targets -- -D warnings` clean, `cargo test --locked` 139 passed, 0 ignored. tui-tests in `node:22.17.0-bookworm` per the README: 13 of 13 pass, 0 skipped, including five new journeys (9 Right, Right, Enter opens Nov 13 and Esc keeps the bar; 10 `5` opens Nov 16; 11 `/ index` opens the Nov 16 run and Esc keeps search and row; 12 stress End and g reach both ends; 13 footer and help name the new keys). sha256 unchanged from 1.1: default `2f641dd0…16cf5f` (both forms), stress `d6b8d5c3…197469` (both flag orders), golden `9e33c787…236ab99`. Presses after the change: Home → Nov 13 sessions 3 (→, →, Enter), was 7; Home → Index rebuild 2 (5, Enter), was 3. Known edge: `c` while a bar is chosen keeps the findings selection by position, so ↓ can then land on a neighbouring finding.

## Workflow follow-up

- Run the moderated screen test (`docs/screen-test.md`) before the next slice. This change is expert review only, and the owner's feedback is one person.
- Archive `add-stress-corpus` and `stress-readability` before this change so the capability specs land in order.
