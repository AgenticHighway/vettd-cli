# Proposal

## Why

While checking `stream-local-logs` (2026-11-27), two runs of the same build
at the same moment on the owner's logs listed a session's `requests[]` in
different orders in about 22 of 33 sessions. Only the order differed. That
makes run-to-run and build-to-build export comparisons noisy, and the order
carried no meaning.

Cause: the Claude reader collects each request's best line in a `HashMap`
and pushed requests by iterating it, an order that changes every run. The
period and History views keep the reader's order inside each session. The
Codex reader was already stable (file path, then line), but a session's
subagent rollouts came after its root's, not in time order.

## What Changes

- Inside a session, requests are listed in time order (the time of the
  request's first line). Ties go by request id for Claude and by file and
  line order for Codex.
- This holds for both agents, so a Codex subagent request sits between the
  root requests made before and after it.

The owner chose this order on 2026-11-27 over two alternatives (time order
for Claude only, or reading order for Claude).

## Capabilities

### Modified Capabilities
- `local-source`: a session's requests have a stable, time-based order.

## Impact

- `src/local/claude.rs`: pushes requests sorted by (first-line time, request id).
- `src/local/mod.rs`: `assemble` stable-sorts the requests by time before
  placing them in sessions.
- `src/local/tests.rs`: two tests of the order.
- Only `--source local` output changes, and only the order of requests in a
  session. The fixture exports and golden file are byte-identical. No new
  dependencies.
