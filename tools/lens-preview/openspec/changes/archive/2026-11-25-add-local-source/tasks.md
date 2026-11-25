# Tasks

Before applying: archive `add-lens-tui-dashboard` (see design.md,
"Ordering") and read the resolved questions in design.md. Every existing test
must keep passing on `--source fixture` after each group.

## 1. Source option and model

- [x] 1.1 Parse `--source fixture|local` (default `fixture`). Reject unknown values, and `--source local` with any `--corpus` once that option exists, on stderr with a non-zero exit
- [x] 1.2 Let the model take a run-time period, a "now" and owned strings, in place of the fixed Nov 12–18, `fixture::NOW` and `&'static str`. Day labels carry their month and sort by date. Fixture output is unchanged
- [x] 1.3 Pass the selected source to `plan.rs`, Home and the top bar, in place of direct `fixture::` reads

## 2. Local readers

- [x] 2.1 Find log directories (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, else home), skip files modified more than 28 days ago, count only period requests towards usage, put each request on its own UTC day, and note missing directories on stderr
- [x] 2.2 Claude reader: sessions with subagents assigned by `sessionId`, one request per `requestId` across all files (owned by the earliest file), usage from the line with the most output tokens, skipping `<synthetic>` and API-error lines as requests, and workspace labels from `cwd` only
- [x] 2.3 Codex reader: root-thread sessions, requests from `token_usage_record` by `response_id`, model from `turn_context`, and the token-part rule, and partial on `input + output != total` or any cache write
- [x] 2.4 Codex windows from `token_count.rate_limits`, matched by `window_minutes`, current instance by time span (not exact `resets_at`), running-maximum readings, and the reset-since-last-reading state
- [x] 2.5 Limit hits over 28 days: Claude `five_hour` rejections on placeholder lines by `requestId` across all files, and Codex `usage_limit_exceeded` turns by `turn_id`
- [x] 2.6 Mark sessions partial on unparsable or truncated lines and missing usage, without stopping
- [x] 2.7 Read `~/.codex/archived_sessions/` too, counting each session once by `session_meta.payload.id`
- [x] 2.8 Session titles for the TUI: Claude's latest `custom-title` / `ai-title`, Codex's latest `session_index.jsonl` entry for the root thread, else the start time. Never exported
- [x] 2.9 Print "Reading local logs…" on stderr while reading
- [x] 2.10 Estimate token sources: split each request's context growth across new blocks by characters, with the Claude and Codex buckets in design.md Decision 13. Leave out unverified attachment types. Test with hand-written blocks of known sizes
- [x] 2.11 Skills and MCP uses per session: Claude `Skill` calls and `invoked_skills` once per request, and `mcp__*` calls by server. Codex `exec` reads of `SKILL.md` as estimated uses with a path class. Ignore `cloud_skills`

## 3. Pricing

- [x] 3.1 Add `src/pricing.rs` with dated Claude entries (prefix; fresh input, cache read, 5-minute and 1-hour cache write, output rates; date checked; source URL) and a longest-prefix lookup
- [x] 3.2 Add Codex (OpenAI) entries at published API rates. `codex-auto` and other models with no published rate get none
- [x] 3.3 Leave requests unpriced (no rate, never zero) for unmatched models, a missing cache-write split, or non-standard `speed` / `service_tier`. Test that a session with only such requests shows "unpriced"

## 4. Screens on local data

- [x] 4.1 Top bar text per source (render test at 80×24 for both)
- [x] 4.2 "synthetic" scope chip on inventory, component, copy, observation, change and apply views on local data
- [x] 4.5 Session, day and token views: "API-equivalent" on dollar figures and titles as session names on local data
- [x] 4.3 Home: the empty-period answer, "Where tokens went (estimated)", "API-equivalent" on every local dollar figure, and no review-kit finding on local data
- [x] 4.6 Token detail: "estimated" on source shares, the method, and the list of attachment types not counted (unverified). Session view: skills and MCP servers used, Codex uses and copy classes marked "estimated", Claude copy "unknown"
- [x] 4.4 Plan headroom: Claude rows "—" / "needs relay" and not selectable, the answer only counting windows with readings, the relay note, per-agent limit hits, the weekly-history row needing the relay, and 5-hour rows opening the day of their latest reading

## 5. Export and privacy

- [x] 5.1 Export `source`. On local data, keep shared field types (`synthetic` false, `limit_hits` a number, null fixture-only fields) and add `now`, `synthetic_sections`, `estimated_sections` and `limit_hits_by_agent`. `token_sources` holds the estimated shares, and sessions hold skill and MCP use counts without names
- [x] 5.2 Test that a local export from hand-written logs contains none of their IDs, paths, project folder names, titles, skill or MCP server names, or prompt text
- [x] 5.3 Hand-written test logs under the preview's test data, covering streamed Claude lines, a request copied into a resumed session, a session across midnight, a misfiled subagent, a drifting `resets_at`, an out-of-order reading, a reset window, null `rate_limits` sides, swapped sides and a truncated last line. No real log lines
- [x] 5.4 Verify by grep that the local path only opens files read-only and has no network or write use

## 6. Docs and checks

- [x] 6.1 README: document `--source local`, what it reads, what stays synthetic, and that nothing leaves the machine
- [x] 6.2 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` in `tools/lens-preview/`
- [x] 6.3 Run `--source local` by hand on a real machine and check Home and Plan headroom against Codex's own `/status`, without recording any output in the repository (2026-11-25: the TUI was run on real logs and the journeys are covered by `src/e2e/local.rs` on hand-written logs; the Codex 5-hour and weekly used shares and reset times matched `/status`)
