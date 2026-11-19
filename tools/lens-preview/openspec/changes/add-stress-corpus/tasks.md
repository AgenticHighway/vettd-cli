# Tasks

## 1. Baseline

- [ ] 1.1 Capture today's `--json` output (built in Docker from the unchanged source) to `tests/golden/default-export.json`, and verify the file parses as JSON and contains 12 sessions

## 2. Stress generator

- [ ] 2.1 Add a splitmix64 PRNG, a seed constant and `pub fn stress_sessions() -> Vec<Session>` to `src/fixture.rs` per design decisions 1–7, leaving `sessions()` untouched; verify with `cargo build`
- [ ] 2.2 Add unit tests for the stress shape: session count 1,900–2,100, exactly 6 workspaces, all of days 12–18 present, a workspace or path over 80 chars, a model over 40 chars, a name repeated 10+ times, skill-use artifact share above 0% and at most 15%, no version without an artifact, at least one `None` rate and no all-zero rate, at least one incomplete session, every `component` below 10. Verify that `cargo test` passes
- [ ] 2.3 Add a test that two `stress_sessions()` calls serialize to identical JSON, and a test that building the corpus takes under 1 s; verify with `cargo test`

## 3. Selection and export

- [ ] 3.1 Replace the `--json` check in `main()` with parsing for `--json` and `--corpus default|stress` per design decision 8. An unknown or missing value prints to stderr and exits 2. Verify manually that `--corpus other` exits 2 with empty stdout
- [ ] 3.2 Export `corpus`, and take `workspaces` from the selected corpus (sorted and distinct), per design decision 9; verify with `--corpus stress --json` showing 6 workspaces
- [ ] 3.3 Add a golden test: the default export, minus `corpus`, equals `tests/golden/default-export.json` as a `serde_json::Value`. Add a test that the export function gives identical strings on repeated calls for both corpora. Verify with `cargo test`
- [ ] 3.4 Add a test that builds `content(...)` for each top-level view from the stress corpus without panicking; verify with `cargo test`
- [ ] 3.5 Document `--corpus` in README.md next to `--json`, and verify the documented commands run

## 4. Verification

- [ ] 4.1 Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` in Docker, and verify all three pass with no ignored tests
- [ ] 4.2 In Docker, run the release build's `--json` twice per corpus and compare sha256 hashes, and verify they match
- [ ] 4.3 Time ten release runs of `--corpus stress --json` in Docker, and verify the worst run is under 1 s. Record the figures
- [ ] 4.4 Write `docs/coverage-note.md`, one page using aggregate figures only. Record the real figures and their source, state that copy coverage is unmeasured, map each stress parameter to a real figure or an assumption, and include the measured timings. Verify that no raw record, path or identifier from the local logs appears in it
