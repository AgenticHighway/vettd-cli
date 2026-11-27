# Spec Delta

## ADDED Requirements

### Requirement: Requests in a session are in time order
A session's requests SHALL be listed in time order, by the time of each
request's first line. Requests at the same time SHALL be ordered by request
id for Claude, and in the order they were read (file, then line) for Codex.
The same logs SHALL give the same order on every run.

Covered by: `claude_requests_are_in_time_order_ties_by_request_id` and
`codex_subagent_requests_interleave_by_time` (src/local/tests.rs).

#### Scenario: Same logs, two runs
- **WHEN** `--source local --json` runs twice over the same logs
- **THEN** every session lists its requests in the same order both times

#### Scenario: Requests at the same time
- **WHEN** two Claude requests in a session have first lines at the same second
- **THEN** the one with the smaller request id comes first

#### Scenario: Codex subagent between root requests
- **WHEN** a Codex subagent request falls between two of its root session's requests
- **THEN** it is listed between them, although the subagent's rollout is read after the root's
