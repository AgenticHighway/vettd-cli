# Tasks

PR #332 built all of these. Each task names the test that covers it. Where
there is no automated test, it names the file that was read to verify it.
`cargo test --locked` at `aa37924`: 47 passed, 0 failed, 0 ignored.

## 1. TUI shell

- [x] 1.1 Draw with ratatui (crossterm) on stderr and restore the terminal on exit. No test; verified in `src/main.rs` (`run`, `Terminal` guard)
- [x] 1.2 Give the preview its own Cargo workspace and a path-filtered CI workflow. No test; verified in `Cargo.toml` and `.github/workflows/lens-preview.yml`
- [x] 1.3 Frame layout: top bar, breadcrumb that keeps Home and the current level, ◆ answer that wraps with a hanging indent, chips, body, Enter hint with n/total, footer. Covered by `deep_breadcrumb_keeps_home_and_current_level`, `answer_wraps_instead_of_cutting_its_numbers`, `wrapped_answer_keeps_its_indent` (src/tests/render.rs) and `plan_journey_goes_deep_and_comes_back_to_the_same_finding` (Enter hint); frame order, chip "…" and n/total verified in `src/screen/mod.rs`
- [x] 1.4 Too-small screen below 38×10 that accepts only q and Ctrl+C and keeps the place. Covered by `too_small_terminal_says_what_it_needs`, `too_small_window_keeps_the_place_and_only_quits`
- [x] 1.5 NO_COLOR: no colours, selection in reverse video. Covered by `no_color_marks_selection_with_reverse_video_only`, `no_color_journey_keeps_focus_visible_without_colour`
- [x] 1.6 Preview pane at 76+ columns. Plan headroom, copy, observation and change views use full width. Covered by `headroom_rows_fit_80_columns`; pane presence verified in `src/screen/list.rs`

## 2. Navigation

- [x] 2.1 Esc goes back and restores the selection, and does nothing at Home. Covered by `plan_journey_goes_deep_and_comes_back_to_the_same_finding`, `spend_journey_reaches_the_run_and_restores_selection`, `maintainer_journey_filters_clears_and_returns_to_the_row`, `esc_at_home_keeps_the_person_at_home`
- [x] 2.2 Arrow and page keys move between actions or scroll, with a viewport-follow flag. Covered by `token_detail_scrolls_to_its_last_row`, `help_scrolls_to_quit`, `exact_copy_reveals_rows_above_its_first_action`, `plan_headroom_reaches_its_trailing_explanation` (src/e2e/scrolling.rs)
- [x] 2.3 Find types letters, q included. Covered by `find_types_letters_instead_of_running_commands`
- [x] 2.4 `?` on Keys adds no second Keys screen. Covered by `repeated_help_takes_one_esc_to_leave`, `help_opens_and_returns`
- [x] 2.5 `c` toggles the unit only where offered and keeps the selected target. Covered by `unit_toggle_switches_home_between_spend_and_requests`, `unit_toggle_keeps_the_selected_target`, `unit_toggle_is_ignored_where_it_is_not_offered`

## 3. Home dashboard

- [x] 3.1 Home is the first screen, with the answer line sessions · requests · tokens · priced USD. Covered by `home_dashboard_shows_usage_before_findings_at_80x24`, `plan_journey_goes_deep_and_comes_back_to_the_same_finding`
- [x] 3.2 Daily chart with a median-of-complete-days normal, "·" for no input and "≥" for partial days. Covered by `home_dashboard_shows_usage_before_findings_at_80x24`, `partial_day_reads_as_floor_and_stays_out_of_normal`
- [x] 3.3 "Where tokens went" table summing to 100%, with Hooks at 0. Covered by `token_source_shares_sum_to_whole`, `home_dashboard_shows_usage_before_findings_at_80x24`
- [x] 3.4 Findings in order of urgency. Covered by `home_leads_with_the_filling_window`, `home_dashboard_shows_usage_before_findings_at_80x24`, `spend_journey_reaches_the_run_and_restores_selection`, `partial_unpriced_session_says_so_and_never_shows_zero`, `day_without_input_says_so`
- [x] 3.5 Findings get their rows first, and the selected one is always drawn. Covered by `selected_home_finding_is_drawn_at_every_size`, `home_dashboard_stacks_at_60x24_and_keeps_selection`

## 4. Plan headroom

- [x] 4.1 One row per window: used, at reset, resets, source. Covered by `headroom_answers_with_the_filling_window_and_cites_sources`, `headroom_rows_fit_80_columns`
- [x] 4.2 Straight-line pace, with "—" for fewer than 2 readings. Covered by `pace_needs_two_readings`, `pace_extrapolates_to_reset_and_names_when_it_fills`
- [x] 4.3 Answer names the soonest fill. Covered by `headroom_answers_with_the_filling_window_and_cites_sources`, `soonest_window_wins_over_the_largest_projection`
- [x] 4.4 No plan recommendation; history and source-of-truth rows. Covered by `plan_headroom_reaches_its_trailing_explanation`; history rows verified in `src/plan.rs`
- [x] 4.5 Window data is synthetic only. Covered by `export_carries_home_and_headroom_data`; no real reads verified by grep of `src/`

## 5. JSON export

- [x] 5.1 `--json` exports the whole corpus, including windows, token_sources, limit_hits, claude_weekly_ends_percent and now_minutes_from_nov_18. Covered by `export_carries_home_and_headroom_data`
