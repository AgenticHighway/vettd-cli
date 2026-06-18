# AGENTS.md

This file gives coding agents the minimum project context needed to work safely
in this repository.

## Project overview

- `proov` is a Rust CLI scanner for AI execution artifacts.
- The scanner runs locally and can optionally submit results to compatible
  ingest APIs.
- Custom detection rules are declarative TOML files loaded from
  `~/.ahscan/rules/`.

## Repo shape

- `crates/proov/src/` — main application code
- `docs/` — public architecture and usage docs
- `examples/rules/` — example custom detection rules
- `scripts/` — smoke and manual test helpers

## Working norms

- Keep changes small and focused.
- Preserve existing CLI behavior unless the task explicitly changes it.
- Prefer extending existing modules over adding parallel abstractions.
- Isolate side effects in the existing I/O-oriented modules.
- Add or update tests when behavior changes.

## Required behavior for agents

These rules apply to every task in this project unless explicitly overridden.
Bias: caution over speed on non-trivial work. Use judgment on trivial tasks.

## Rule 1 — Think Before Coding

State assumptions explicitly. If uncertain, ask rather than guess.
Present multiple interpretations when ambiguity exists.
Push back when a simpler approach exists.
Stop when confused. Name what's unclear.

## Rule 2 — Simplicity First

Minimum code that solves the problem. Nothing speculative.
No features beyond what was asked. No abstractions for single-use code.
Test: would a senior engineer say this is overcomplicated? If yes, simplify.

## Rule 3 — Surgical Changes

Touch only what you must. Clean up only your own mess.
Don't "improve" adjacent code, comments, or formatting.
Don't refactor what isn't broken. Match existing style.

## Rule 4 — Goal-Driven Execution

Define success criteria. Loop until verified.
Don't follow steps. Define success and iterate.
Strong success criteria let you loop independently.

## Rule 5 — Use the model only for judgment calls

Use me for: classification, drafting, summarization, extraction.
Do NOT use me for: routing, retries, deterministic transforms.
If code can answer, code answers.

## Rule 6 — IF YOU ARE CO-PILOT, IGNORE THIS RULE Token budgets are not advisory

Per-task: 4,000 tokens. Per-session: 30,000 tokens.
If approaching budget, summarize and start fresh.
Surface the breach. Do not silently overrun.

## Rule 7 — Surface conflicts, don't average them

If two patterns contradict, pick one (more recent / more tested).
Explain why. Flag the other for cleanup.
Don't blend conflicting patterns.

## Rule 8 — Read before you write

Before adding code, read exports, immediate callers, shared utilities.
"Looks orthogonal" is dangerous. If unsure why code is structured a way, ask.

## Rule 9 — Tests verify intent, not just behavior

Tests must encode WHY behavior matters, not just WHAT it does.
A test that can't fail when business logic changes is wrong.

## Rule 10 — Checkpoint after every significant step

Summarize what was done, what's verified, what's left.
Don't continue from a state you can't describe back.
If you lose track, stop and restate.

## Rule 11 — Match the codebase's conventions, even if you disagree

Conformance > taste inside the codebase.
If you genuinely think a convention is harmful, surface it. Don't fork silently.

## Rule 12 — Fail loud

"Completed" is wrong if anything was skipped silently.
"Tests pass" is wrong if any were skipped.
Default to surfacing uncertainty, not hiding it.

1. Secrets and passwords must **never** be committed to git. Use `.gitignore` and environment variables.
2. Use local Docker (`docker compose`) for build/test/runtime checks.
3. All AWS resources **must** be tagged (see Terraform `default_tags`).
4. **$500/mo hard budget cap** — do not add resources without checking cost impact.
5. ECS task size and RDS storage are intentionally capped. Do not increase without discussion.

## Expected checks

Run the standard Rust checks before finishing code changes:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

For broader CLI smoke coverage, run:

```bash
./scripts/test-scanner.sh
```

## Notes for agents

- Machine-readable output belongs on stdout.
- Human-oriented logs and progress output belong on stderr.
- Update public docs when behavior or CLI flows change.
