## 1. Pricing

- [x] 1.1 Give OpenAI entries their published long-context rates over 272K input tokens. Verify: `prompt_limits_change_the_rate`, and the owner's logs have 0 unpriced requests in the period (was 599).

## 2. Partial days

- [x] 2.1 Day's title, chip and rows call a day or session partial for incomplete input or, in USD, a request with no rate. Verify: `unpriced_day_is_partial_on_day_as_on_week`, `partly_priced_session_row_ends_in_floor_mark`.

## 3. Plan headroom

- [x] 3.1 A window whose pace filled it before now, unconfirmed, is named "may be full" in the answer and Home's first finding. Verify: `elapsed_forecast_says_may_be_full`, and on the owner's logs the answer names Codex weekly.

## 4. 80 columns

- [x] 4.1 Shorten Day's comparison and partial chips so every chip shows whole. Verify: `day_scope_chips_show_whole_at_80_columns`, and on the owner's logs no Day scope line contains "…".

## 5. Docs and checks

- [x] 5.1 README: long-context rates, partial days, "may be full", and the relay only running in terminal Claude Code.
- [x] 5.2 `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` in Docker; plain `--json` export hashes unchanged.
