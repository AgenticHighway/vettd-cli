# Tasks

Before applying: archive `add-lens-tui-dashboard` (see design.md,
"Ordering") and settle the open questions in design.md. Every existing test
must keep passing on `--source fixture` after each group.

## 1. Source option and model

- [ ] 1.1 Parse `--source fixture|local` (default `fixture`). Reject unknown values on stderr with a non-zero exit. Apply the `--corpus` combination rule if `add-stress-corpus` has landed
- [ ] 1.2 Let the model take a run-time period, a "now" and owned strings, in place of the fixed Nov 12–18, `fixture::NOW` and `&'static str`. Fixture output is unchanged
- [ ] 1.3 Pass the selected source to `plan.rs`, Home and the top bar, in place of direct `fixture::` reads

## 2. Local readers

- [ ] 2.1 Find log directories (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, else home), skip files modified before the period, and note missing directories on stderr
- [ ] 2.2 Claude reader: sessions with subagents, one request per `requestId` using its last line's usage, skipping `<synthetic>` and API-error lines
- [ ] 2.3 Codex reader: root-thread sessions, requests from `token_usage_record` by `response_id`, model from `turn_context`, and the token-part rule with its reconcile check
- [ ] 2.4 Codex windows from `token_count.rate_limits`, matched by `window_minutes`, with the latest `resets_at` and its readings
- [ ] 2.5 Limit hits over 28 days: Claude `five_hour` rejections by request, and Codex `usage_limit_exceeded` turns
- [ ] 2.6 Mark sessions partial on unparsable or truncated lines and missing usage, without stopping
- [ ] 2.7 Print "Reading local logs…" on stderr while reading

## 3. Pricing

- [ ] 3.1 Add `src/pricing.rs` with dated Claude entries (prefix, four rates, date checked, source URL) and a longest-prefix lookup
- [ ] 3.2 Leave unmatched models unpriced (no rate, never zero), and test that a session using one shows "unpriced"

## 4. Screens on local data

- [ ] 4.1 Top bar text per source (render test at 80×24 for both)
- [ ] 4.2 "synthetic" scope chip on inventory, component, copy, observation, change and apply views on local data
- [ ] 4.3 Home: the empty-period answer, the "(illustrative)" table title, and no review-kit finding on local data
- [ ] 4.4 Plan headroom: Claude rows "—" / "needs relay" and not selectable, the answer only counting windows with readings, the relay note, per-agent limit hits, and the weekly-history row needing the relay

## 5. Export and privacy

- [ ] 5.1 Export `source`, and on local data the `synthetic` section list, `now`, per-agent `limit_hits` and null `claude_weekly_ends_percent`
- [ ] 5.2 Test that a local export from hand-written logs contains none of their IDs, paths, titles or prompt text
- [ ] 5.3 Hand-written test logs under the preview's test data, covering streamed Claude lines, subagents, null `rate_limits` sides, swapped sides and a truncated last line. No real log lines
- [ ] 5.4 Verify by grep that the local path only opens files read-only and has no network or write use

## 6. Docs and checks

- [ ] 6.1 README: document `--source local`, what it reads, what stays synthetic, and that nothing leaves the machine
- [ ] 6.2 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` in `tools/lens-preview/`
- [ ] 6.3 Run `--source local` by hand on a real machine and check Home and Plan headroom against Codex's own `/status`, without recording any output in the repository
