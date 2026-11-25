# Tasks

Before applying: archive `add-lens-tui-dashboard` (see design.md,
"Ordering") and settle the open questions in design.md. Every existing test
must keep passing on `--source fixture` after each group.

## 1. Source option and model

- [ ] 1.1 Parse `--source fixture|local` (default `fixture`). Reject unknown values, and `--source local` with any `--corpus` once that option exists, on stderr with a non-zero exit
- [ ] 1.2 Let the model take a run-time period, a "now" and owned strings, in place of the fixed Nov 12–18, `fixture::NOW` and `&'static str`. Day labels carry their month and sort by date. Fixture output is unchanged
- [ ] 1.3 Pass the selected source to `plan.rs`, Home and the top bar, in place of direct `fixture::` reads

## 2. Local readers

- [ ] 2.1 Find log directories (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, else home), skip files modified more than 28 days ago, count only period requests towards usage, put each request on its own UTC day, and note missing directories on stderr
- [ ] 2.2 Claude reader: sessions with subagents assigned by `sessionId`, one request per `requestId` across all files (owned by the earliest file), usage from the line with the most output tokens, skipping `<synthetic>` and API-error lines as requests, and workspace labels from `cwd` only
- [ ] 2.3 Codex reader: root-thread sessions, requests from `token_usage_record` by `response_id`, model from `turn_context`, and the token-part rule, and partial on `input + output != total` or any cache write
- [ ] 2.4 Codex windows from `token_count.rate_limits`, matched by `window_minutes`, current instance by time span (not exact `resets_at`), running-maximum readings, and the reset-since-last-reading state
- [ ] 2.5 Limit hits over 28 days: Claude `five_hour` rejections on placeholder lines by `requestId` across all files, and Codex `usage_limit_exceeded` turns by `turn_id`
- [ ] 2.6 Mark sessions partial on unparsable or truncated lines and missing usage, without stopping
- [ ] 2.7 Print "Reading local logs…" on stderr while reading

## 3. Pricing

- [ ] 3.1 Add `src/pricing.rs` with dated Claude entries (prefix; fresh input, cache read, 5-minute and 1-hour cache write, output rates; date checked; source URL) and a longest-prefix lookup
- [ ] 3.2 Leave requests unpriced (no rate, never zero) for unmatched models, a missing cache-write split, or non-standard `speed` / `service_tier`. Test that a session with only such requests shows "unpriced"

## 4. Screens on local data

- [ ] 4.1 Top bar text per source (render test at 80×24 for both)
- [ ] 4.2 "synthetic" scope chip on inventory, component, copy, observation, change and apply views on local data
- [ ] 4.3 Home: the empty-period answer, the "(illustrative)" table title, and no review-kit finding on local data
- [ ] 4.4 Plan headroom: Claude rows "—" / "needs relay" and not selectable, the answer only counting windows with readings, the relay note, per-agent limit hits, the weekly-history row needing the relay, and 5-hour rows opening the day of their latest reading

## 5. Export and privacy

- [ ] 5.1 Export `source`. On local data, keep shared field types (`synthetic` false, `limit_hits` a number, null fixture-only fields) and add `now`, `synthetic_sections` and `limit_hits_by_agent`
- [ ] 5.2 Test that a local export from hand-written logs contains none of their IDs, paths, project folder names, titles or prompt text
- [ ] 5.3 Hand-written test logs under the preview's test data, covering streamed Claude lines, a request copied into a resumed session, a session across midnight, a misfiled subagent, a drifting `resets_at`, an out-of-order reading, a reset window, null `rate_limits` sides, swapped sides and a truncated last line. No real log lines
- [ ] 5.4 Verify by grep that the local path only opens files read-only and has no network or write use

## 6. Docs and checks

- [ ] 6.1 README: document `--source local`, what it reads, what stays synthetic, and that nothing leaves the machine
- [ ] 6.2 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` in `tools/lens-preview/`
- [ ] 6.3 Run `--source local` by hand on a real machine and check Home and Plan headroom against Codex's own `/status`, without recording any output in the repository
