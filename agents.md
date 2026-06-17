# AGENTS.md

This file gives coding agents the minimum project context needed to work safely
in this repository.

## Project overview

- `vettd` is a Rust CLI scanner for AI execution artifacts.
- The scanner runs locally and can optionally submit results to compatible
  ingest APIs.
- Custom detection rules are declarative TOML files loaded from
  `~/.vettd/rules/`.

## Repo shape

- `crates/vettd-cli/src/` — main application code
- `docs/` — public architecture and usage docs
- `examples/rules/` — example custom detection rules
- `scripts/` — smoke and manual test helpers

## Working norms

- Keep changes small and focused.
- Preserve existing CLI behavior unless the task explicitly changes it.
- Prefer extending existing modules over adding parallel abstractions.
- Isolate side effects in the existing I/O-oriented modules.
- Add or update tests when behavior changes.

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
