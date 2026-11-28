# Spec Delta

## MODIFIED Requirements

### Requirement: Rates cover both agents
The rate table SHALL hold the published API rates for the Claude and Codex
(OpenAI) models it names. A Codex request SHALL use its fresh input, cache
read and output rates. A Codex prompt over 272K input tokens SHALL use the
model's published long-context rates. A model with no published API rate,
such as `codex-auto`, SHALL have no entry.

Covered by: `prompt_limits_change_the_rate` (src/pricing.rs).

#### Scenario: Codex priced
- **WHEN** a Codex request's model matches a table entry
- **THEN** the request is priced from that entry, the same as a Claude request

#### Scenario: Long Codex prompt
- **WHEN** a `gpt-6.1-sol` request has 300,000 fresh input tokens
- **THEN** it is priced at the long-context input rate, $4 per million, not left unpriced
