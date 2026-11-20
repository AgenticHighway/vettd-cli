# Tasks

Paths are relative to `tools/lens-preview/`.

## 1. Baseline

- [x] 1.1 Capture `--json` output of the unchanged branch head (built in Docker from the unchanged source) to `tests/golden/default-export.json`, and verify the file parses as JSON, contains 12 sessions, and has the `windows`, `token_sources`, `limit_hits`, `claude_weekly_ends_percent` and `now_minutes_from_nov_18` keys
  - Evidence: Captured in Docker `rust:1.97.1` from unchanged `aa37924`; 12 sessions, all 5 plan keys present; sha256 `9e33c787…236ab99`, 9,726 bytes.

## 2. Stress generator

- [x] 2.1 Add `src/fixture/stress.rs` with a splitmix64 PRNG, a seed constant and `pub fn stress_sessions() -> Vec<Session>` per design decisions 1–8, re-export it from `src/fixture/mod.rs`, and leave `src/fixture/sessions.rs` untouched; verify with `cargo build`
  - Evidence: `src/fixture/stress.rs` + re-export in `src/fixture/mod.rs`; `sessions.rs` unchanged; build, clippy clean.
- [x] 2.2 Add unit tests in `src/tests/stress.rs` (registered in `src/tests/mod.rs`) for the stress shape: session count 1,900–2,100, exactly 6 workspaces, all of days 12–18 present, a workspace over 80 chars, a model over 40 chars, a name repeated 10+ times, skill-use artifact share above 0% and at most 15%, no version without an artifact, every artifact one that `PLACEMENTS` lists for that component, at least one `None` rate and no all-zero rate, at least one incomplete session, every `component` below 10, every token part at most 1,000,000 and at most 4 requests per session. Verify that `cargo test` passes
  - Evidence: `src/tests/stress.rs` shape tests pass (2,011 sessions, 6 workspaces, longest 94 chars, model 63 chars, names repeat 220–278×, 9.4% coverage). Sonnet review ACCEPT; fixes for version match, skill-only artifacts, cost maths and char counts applied.
- [x] 2.3 In `src/tests/stress.rs`, add a test that two `stress_sessions()` calls serialize to identical JSON, and a test that building the corpus takes under 1 s; verify with `cargo test`
  - Evidence: Determinism, fingerprint (2,011 sessions, 1,345,621 cents) and <1 s build tests pass.

## 3. Selection and export

- [x] 3.1 In `src/main.rs`, replace the `args().nth(1)` check with parsing for `--json` and `--corpus default|stress` anywhere on the command line, per design decision 9, and pass the selected corpus to `run`. An unknown or missing value prints `error: --corpus must be default or stress` to stderr and exits 2. Verify manually that `--corpus other` and a trailing `--corpus` exit 2 with empty stdout, and that `--json --corpus stress` and `--corpus stress --json` give the same output
  - Evidence: `--corpus other` and `--json --corpus` → exit 2, 0 stdout bytes, exact stderr message; both flag orders hash `d6b8d5c3…197469`.
- [x] 3.2 In `src/main.rs`, make `export` take the corpus name, add `corpus`, and take `workspaces` from the selected corpus (sorted and distinct), keeping the plan and source fields unchanged, per design decisions 10–11. Update the existing caller in `src/tests/render.rs`. Verify with `--corpus stress --json` showing 6 workspaces and the same `windows` as the default export
  - Evidence: Stress export lists 6 sorted workspaces; plan fields equal default (test). `render.rs` caller updated.
- [x] 3.3 Add a golden test: the default export, minus `corpus`, equals `tests/golden/default-export.json` as a `serde_json::Value`. Add a test that the export function gives identical strings on repeated calls for both corpora, and a test that the stress export's plan and source fields equal the default export's. Verify with `cargo test`
  - Evidence: Golden, repeat-export and plan-reuse tests in `src/tests/corpus.rs` pass. Sonnet review ACCEPT.
- [x] 3.4 Add a test that builds `content(...)` for each top-level view (Home, Headroom, Week, Find, Help, Inventory, and Day for each of days 12–18) from the stress corpus without panicking; verify with `cargo test`
  - Evidence: `content()` and an 80×24 draw for Home, Headroom, Week, Find, Help, Inventory, Day 12–18 from stress: no panic.
- [x] 3.5 Document `--corpus` in `README.md` next to `--json`, and verify the documented commands run
  - Evidence: README paragraph added; `--json` and `--corpus stress --json` run in Docker release build. Interactive TUI not launched (no TTY in Docker).

## 4. Verification

- [x] 4.1 Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` in Docker, and verify all three pass with no ignored tests
  - Evidence: Docker `rust:1.97.1`: fmt OK, clippy -D warnings clean, `cargo test` 75 passed, 0 failed, 0 ignored.
- [x] 4.2 In Docker, run the release build's `--json` twice per corpus and compare sha256 hashes, and verify they match
  - Evidence: default `2f641dd0…16cf5f` ×2 (and no `--corpus`); stress `d6b8d5c3…197469` ×2.
- [x] 4.3 Time ten release runs of `--corpus stress --json` in Docker, and verify the worst run is under 1 s. Record the figures
  - Evidence: Ten release runs (ms): 46, 41, 50, 50, 46, 48, 43, 45, 44, 43; worst 50 ms.
- [x] 4.4 Write `docs/coverage-note.md`, one page using aggregate figures only. Record the real figures and their source, state that copy coverage is unmeasured, map each stress parameter to a real figure or an assumption (including the reused plan fields), and include the measured timings. Verify that no raw record, path or identifier from the local logs appears in it
  - Evidence: `docs/coverage-note.md` written; figures checked against the aggregate report; no paths or identifiers from local logs.
