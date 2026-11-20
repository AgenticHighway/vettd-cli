# Tasks

Paths are relative to `tools/lens-preview/`. Run every check in Docker
`rust:1.97.1` with the source mounted read-only and
`CARGO_TARGET_DIR=/tmp/target`. Each render test must fail on the code from
before its task. Confirm this by running the new test against the previous
commit, or by reasoning recorded in the evidence.

## 1. Baseline

- [x] 1.1 Before any code change, record the sha256 of `--json` and `--corpus stress --json` from a release build, and the sha256 of `tests/golden/default-export.json`. Verify they match Slice 0's recorded figures (default `2f641dd0…16cf5f`, stress `d6b8d5c3…197469`)
  - Evidence: Docker `rust:1.97.1` (rustup applies the repo's `rust-toolchain.toml`, 1.88.0) at `44540b9`: default `2f641dd0…16cf5f` (with and without `--corpus default`), stress `d6b8d5c3…197469` (both flag orders), golden file `9e33c787…236ab99`. All match Slice 0.

## 2. Home chart

- [x] 2.1 In `src/screen/home.rs`, apply the raised-baseline rule (design decision 1) to the bar values in `draw_chart`, and expose the baseline from a pure function next to `daily` so tests can call it. Add unit tests: stress USD gets a baseline above zero; default USD gets zero; all-equal values get zero; a baseline is never negative. Record whether the default requests chart's baseline changes
  - Evidence: `baseline()` in `src/screen/home.rs`; bars draw `value − base`. Unit tests in `src/tests/readability.rs`: stress USD floor > 0, whole dollars, ≤ lowest day; default USD 0; equal, over-2×, no-data → 0; rounding (850/1050 → 600), exact 2× → 0, `u32::MAX` no overflow. Default requests chart: baseline 1 (min 2, peak 3), recorded in design decision 2. Sonnet review ACCEPT.
- [x] 2.2 Build the chart title from parts with full and short forms (design decision 2). Add render tests: stress Home at 80×24 has bars whose shortest column is at most two-thirds the tallest and a title naming `$<whole dollars>`; stress Home at 60×24 has a title with no cut word that says every day is partial; default Home at 80×24 has the same title as before. Verify that the existing Home render tests pass unedited
  - Evidence: `chart_title()` shortens unit and axis, then notes, then drops notes; it never drops the unit or the axis (a review found the first version dropped the axis at narrow widths, now fixed). Stress 80×24: shortest bar ≤ ⅔ of tallest, title `SPEND  ·  normal: no complete day  ·  every day partial (≥)  ·  from $1510`; 60×24 short form whole; 40×24 keeps `SPEND` and `from $1510`; default title unchanged. Existing render and corpus tests unedited. Docker: fmt, clippy clean, 90 passed. Old code fails these by reasoning: there is no floor or `from $` and the title is cut at 60 columns.

## 3. Shortening and Day rows

- [x] 3.1 Add `shorten(text, max)` to `src/model.rs` (design decision 3), with unit tests for text that fits, odd and even `max`, `max` of 1 and 2, and multi-byte characters such as "≥" and "…"
  - Evidence: `shorten()` in `src/model.rs`; 5 unit tests (fits, exact width odd/even, max 0/1/2, multi-byte, existing "…"). Sonnet review ACCEPT.
- [x] 3.2 Make `session_row` and `day_sessions` take the width and size the workspace column (design decision 4), and build the Day header in `src/screen/list.rs` from the same widths. Add render tests: on the stress Nov 16 list at 80×24 and 60×24, every visible row's total ends in the same column inside the list area, and a long workspace shows its start, "…" and its end. Verify the default Day render tests pass unedited
  - Evidence: `session_row` takes `ws`; `day_ws_width`/`day_header` in `src/views.rs`; header equals the old literal at ws 4 (test). Stress Nov 16: totals end in one column inside `list_width` at 80×24 and 60×24; the 94-char workspace fills the 11-char column at 80 (`platf…tress`) and shows `platform/`, `…`, `-stress` at 60. 38×10 stress Day 12–18 draws without panic. Default Day tests unedited.

## 4. Chips and worker rows

- [x] 4.1 In `src/screen/mod.rs::chips`, shorten an over-wide chip value before dropping it (design decision 5). Add render tests: the stress Session view for the 94-character workspace at 80×24 shows the day, a shortened workspace, the agent and the cost chips. Check both colour and `NO_COLOR` chip forms
  - Evidence: `chips()` shortens an over-wide value, reserving room for later chips, and is byte-identical when all fit. `long_scope_keeps_agent_and_cost_chips` checks day, "…", agent and cost at 80×24 in colour and mono.
- [x] 4.2 Make `cost_rows` and `tokens` in `src/views.rs` shorten the model to the width (design decision 6). Add render tests: on a stress Session view with the 63-character model at 80×24 and 60×24, that worker row ends with its cost; on a stress Day list at 80×24 that selects such a session, the preview pane shows the row on one line ending in its cost. Verify that the default Session and Tokens tests pass unedited
  - Evidence: cost and token rows shorten the model to `width − 3`; preview rows fit the pane width. Tests: the 63-char model row ends in its cost at 80×24/60×24; the stress Day preview row ends in its cost. Default Session/Tokens tests unedited. Docker: fmt, clippy clean, 102 passed.

## 5. Workspace filter

- [x] 5.1 Add `fixture::workspaces(&[Session])` (design decision 7), and use it in `src/components/inventory.rs` and for the `w` cycle length in `src/app.rs`. Add an end-to-end test: on the stress corpus, `w` pressed six times shows each stress workspace in sorted order, and a seventh press returns to "All workspaces". Verify the existing maintainer journey passes unedited
  - Evidence: `fixture::workspaces()` (the export now calls it, and the hashes are unchanged); inventory and the `w` cycle use it, and `contexts()` still uses `WORKSPACES`. `workspace_filter_follows_the_stress_corpus`: six sorted stress workspaces, then All. The default list is `["api","web"]`; the existing maintainer journey is unedited and passes. Sonnet review ACCEPT (nit: default cycle checked through `workspaces()` and the existing journey, not a new UI loop).

## 6. Long lists

- [x] 6.1 Add an end-to-end test on the stress corpus at 80×24: open Nov 16, press PgDn until the counter stops changing, check that the last position is selected and drawn, then press PgUp the same number of times and check `1/<n>`. Change `Viewport` only if the test fails (design decision 8), and record the outcome
  - Evidence: `long_list_pages_to_both_ends` passed first time: N = 295, 18 PgDn to `295/295` with the last row selected and drawn, 18 PgUp to `1/295`. `Viewport` unchanged. This guards the requirement; it is not a fix, because paging already worked.

## 7. NO_COLOR

- [x] 7.1 Add mono render tests for stress Home, the Nov 16 Day list and the long-workspace Session view at 80×24. They check the same title, alignment and chip rules as the colour tests, and that the selected row is reversed
  - Evidence: `no_color_home_title_names_the_floor_in_whole_dollars`, `no_color_day_totals_end_in_one_column`, `no_color_long_session_chips_stay_in_bracket_form`; each asserts REVERSED on the `▸` cell.

## 8. Verification

- [x] 8.1 Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and `cargo test --locked` in Docker. Verify all pass with no ignored tests, and record the counts
  - Evidence: Docker `rust:1.97.1` (toolchain 1.88.0 from `rust-toolchain.toml`): `cargo fmt --check` OK, `cargo clippy --locked --all-targets -- -D warnings` clean, `cargo test --locked` 107 passed, 0 failed, 0 ignored (79 before this change).
- [x] 8.2 Re-record the sha256 of the release `--json` for both corpora and of `tests/golden/default-export.json`. Verify they are unchanged from 1.1
  - Evidence: default `2f641dd0…16cf5f` (with and without `--corpus default`), stress `d6b8d5c3…197469` (both flag orders), golden `9e33c787…236ab99`. All match 1.1.
- [x] 8.3 Time ten release runs of `--corpus stress --json` in Docker, record the figures, and verify the worst run is under 1 s
  - Evidence: ten release runs (ms): 47, 44, 44, 68, 68, 57, 49, 43, 40, 46. Worst 68 ms.
- [x] 8.4 Run the tui-test journeys (`tui-tests/`) in Docker `node:22.17.0-bookworm` and verify 8/8 pass
  - Evidence: `node:22.17.0-bookworm` with rustup 1.88.0, the README command: `# pass 8 # fail 0`.
- [x] 8.5 Launch the TUI from the repo root with `--manifest-path tools/lens-preview/Cargo.toml`, on both corpora at 80×24 and 60×24 and under `NO_COLOR`, and check Home, Nov 16, a long-workspace Session and the inventory `w` cycle by eye. Record what was seen
  - Evidence: launched on the stress corpus from the repo root in the desktop Terminal panel. At about 120 columns, the Home title read `DAILY SPEND  ·  normal: no complete day  ·  every day partial (≥)  ·  axis from $1510` and bar heights differed visibly. The project owner then checked by eye and reported that the layout "looks good". The owner also said it "still feels hard to navigate". That is outside this change's readability scope; it is recorded as owner feedback, not screen-test evidence, for a later slice. Which sizes and corpora the owner covered was not itemised.
- [x] 8.6 Update `README.md` where it describes Home's chart or the inventory filter, and add the baseline and filter behavior. Verify that the documented commands still run
  - Evidence: README now describes the corpus-wide `w` cycle, the raised baseline and its `from $…` title, and middle shortening. The documented Docker test commands ran (8.1, 8.4).
