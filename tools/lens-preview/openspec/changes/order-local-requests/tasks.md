# Tasks

## 1. Find the cause

- [x] 1.1 Find where a session's request order comes from. Verify: each `HashMap`/`HashSet` iteration in `src/local/` checked for an effect on output order.
  - Evidence: `claude.rs` pushed requests with `for (key, b) in &best` (a `HashMap`, random order per run); `assemble` keeps the reader's order. Codex pushes from a `Vec` in file and line order. The other iterations (`sources`, Codex `threads` metadata, uses counted through a `BTreeMap`) don't affect order. Fixture exports don't use the local readers.
- [x] 1.2 Ask the owner which order they want. Verify: a recorded choice.
  - Evidence: 2026-11-27, the owner chose time order with ties by request id, applied to both agents.

## 2. Make the order stable

- [x] 2.1 Sort Claude requests by (first-line time, request id) before pushing them, and stable-sort all requests by time in `assemble`. Verify: `claude_requests_are_in_time_order_ties_by_request_id` and `codex_subagent_requests_interleave_by_time`.
  - Evidence: both pass. With the old `claude.rs` and `mod.rs` restored in a scratch copy, both fail (Docker, toolchain 1.88.0).

## 3. Check output

- [x] 3.1 Fixture exports byte-identical. Verify: release `--json`, `--corpus default --json` (both flag orders), `--corpus stress --json` (both flag orders), golden file.
  - Evidence: Docker release build: default `33a1743f…2c60c2ef4` (all three forms), stress `c8ea73d4…853762f68` (both orders), golden `9e33c787…236ab99`.
- [x] 3.2 Owner's logs, counts only. Verify: two runs of the new build give the same sessions; old and new builds differ only in request order.
  - Evidence: two simultaneous `--source local --json` runs: 34 of 34 sessions identical; two `--json --history` runs: 183 of 183 History sessions identical (only `now` differed, as the runs were back to back). Old build against new build, run at the same moment: 29 of 34 sessions differ as stored, 0 differ once request order is ignored.
- [x] 3.3 Docker checks (`rust:1.97.1`, repo root read-only, toolchain 1.88.0 from `rust-toolchain.toml`). Verify: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`.
  - Evidence: fmt clean, clippy `-D warnings` clean, `cargo test --locked` 240 + 3 passed, 0 ignored.
