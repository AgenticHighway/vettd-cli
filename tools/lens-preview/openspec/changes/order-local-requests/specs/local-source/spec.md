# Spec Delta

## ADDED Requirements

### Requirement: Requests in a session are in time order
A session's requests SHALL be listed in time order, by the time that places
each request on a day (for Claude, its earliest line with usage). Requests at the same time SHALL be ordered by request
id for Claude, and in the order they were read (file, then line) for Codex.
The same logs SHALL give the same order on every run.

Covered by: `claude_requests_are_in_time_order_ties_by_request_id` and
`codex_subagent_requests_interleave_by_time` (src/local/tests.rs).

#### Scenario: Same logs, two runs
- **WHEN** `--source local --json` runs twice over the same logs
- **THEN** every session lists its requests in the same order both times

#### Scenario: Requests at the same time
- **WHEN** two Claude requests in a session have the same time, to the second
- **THEN** the one with the smaller request id comes first

#### Scenario: Codex subagent between root requests
- **WHEN** a Codex subagent request falls between two of its root session's requests
- **THEN** it is listed between them, although the subagent's rollout is read after the root's

#### Scenario: Claude request whose first line has no usage
- **WHEN** a Claude request's first line has no usage and a later line does
- **THEN** it is ordered by the later line's time, the same time that places it on a day
