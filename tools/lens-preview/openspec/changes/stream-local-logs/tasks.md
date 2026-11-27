# Tasks

## 1. Find the cause

- [x] 1.1 Measure `--source local --json` on the owner's logs (release, `PeakWorkingSet64` sampled every 20 ms) and count log file sizes, printing numbers only. Verify: the baseline peak is reproduced.
  - Evidence: 1,840–1,841 MB, 8.9–11.5 s (the earlier 6.9 s run was on a quieter machine). Of 287 Claude files modified in the last 28 days (1,391 MB), the largest is 652 MB and the next 33 MB. Of 42 Codex rollouts (147 MB), the largest is 44 MB.
- [x] 1.2 Separate the whole-file byte copy from the parsed lines. Verify: a build that reads through a buffer but still collects every parsed line.
  - Evidence: 1,197 MB, so about 640 MB was the byte copy and the rest was every parsed `Value` of the 652 MB file, held at once.

## 2. Stream the lines

- [x] 2.1 Add `lines_of`, a line iterator over a buffered reader with the old line rules, and build `read_lines` on it. Verify: `streamed_lines_match_the_whole_file_rules`.
  - Evidence: the test covers "\r\n", blank and whitespace-only lines, invalid UTF-8, a non-JSON line, a cut-off last line and a missing file.
- [x] 2.2 Have the Claude reader iterate `lines_of` and mark the session partial after the file is read. Codex keeps `read_lines` (largest file 44 MB; it looks for `session_meta` before walking the lines). Verify: peak on the owner's logs.
  - Evidence: `--source local --json` 128 MB (was 1,841 MB); `--json --history` 172 MB (was 1,854 MB).

## 3. Same output

- [x] 3.1 Fixture exports byte-identical. Verify: release `--json`, `--corpus default --json`, both stress flag orders, golden file.
  - Evidence: host and Docker builds: release `33a1743f1ffd5efc00b67b3cd850261cc1cd039ef564f165ec3e8928c60c2ef4`, stress `c8ea73d4ee398028babe292d19f3280da632aec2a46dd207ac97eef853762f68`, golden `9e33c78797c6cf3e0e7404228f28ac43e4e4ed3b375e6427ce72eefe0236ab99`.
- [x] 3.2 Same export on the owner's logs. The real export carries the time it ran, so hashes differ run to run. Verify: old and new binaries run at the same moment, compared field by field, printing counts only.
  - Evidence: plain `--json`: same byte length; only `sessions` differs, and only in the order of requests inside a session. `--json --history`: 33 period and 182 History sessions in both, 0 content differences. The old binary run against itself shows the same order-only differences (22 sessions), so the request order was not deterministic before this change. That is a separate, existing issue, not fixed here.
- [x] 3.3 Docker checks (`rust:1.97.1`, repo root read-only, toolchain 1.88.0 from `rust-toolchain.toml`). Verify: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`.
  - Evidence: cargo/rustc 1.88.0, source read-only: `cargo fmt --check` clean (after one `cargo fmt` pass on the new test), clippy `-D warnings` clean, `cargo test --locked` 238 + 3 passed, 0 ignored. `openspec validate --strict --all` 12/12. No dependency added (`Cargo.toml` and `Cargo.lock` unchanged).
