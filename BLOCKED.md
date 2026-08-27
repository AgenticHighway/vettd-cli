# BLOCKED — vettd-cli#243 gate rule undecided

Status: BLOCKED (undecided design point — do not improvise)

## What this branch is for

Issue [AgenticHighway/vettd-cli#243]("Gate whether new scanner-crate fields reach the
CLI contract on tag bump") — a dependency of the #879 release train. It is the CLI-side
counterpart of [AgenticHighway/vettd#925]("Signal emission path: scanner crate to vettd ingest").

`vettd-skill-scanner` is consumed here as a tag-pinned Git dependency
(`crates/vettd-cli/Cargo.toml:14` → `tag = "v0.1.4"`). The crate's `main` has new
`Signal` output fields (`crates/vettd-skill-scanner/src/signal.rs`, carried on
`SkillScanResult.signals` and the HTTP shim response). Those fields are inert for this
CLI until the pin is bumped; when it is bumped they arrive at `scanner-data-contract.json`
and its 26 `additionalProperties: false` sites.

## Why this is blocked

Issue #243's **gate rule is unspecified** in the issue body and its sole comment. The issue
asks for a decision (surface vs gate) and a mechanism that makes the answer hold at
tag-bump time, but does not state:

- **which fields** the gate applies to (only `signals` today, or any future crate-side
  output field), and
- **under what condition** the CLI's published contract either surfaces them
  (additively into `scanner-data-contract.json`) or gates them (keeps them out).

Per the working constraint "do not invent design", this rule must not be guessed. The
issue ACs ("Decision recorded: new crate output fields are surfaced, or gated, with the
reasoning"; "A routine tag bump cannot silently widen this CLI's published contract
surface"; "The constraint is documented where the dependency pin is declared, not only in
an issue") are all contingent on that decision.

## What is NOT undecided

- The pin location (`crates/vettd-cli/Cargo.toml:14`, `tag = "v0.1.4"`).
- `scanner-data-contract.json` must remain unchanged by the gate decision itself (an AC
  of #243; the file currently exists at the repo root and is served publicly).
- `scanner-data-contract.json` has 26 `additionalProperties: false` sites and a drift
  alarm that auto-files a P0 on divergence — these are the enforcement surface the
  decision must plug into, but they do not choose surface-vs-gate.

## To unblock

Decide and record the field-level gate rule (which fields, under what condition, with
reasoning), then implement:
1. The mechanism that makes the answer hold at tag-bump time (not depending on the bump
   author knowing the rule).
2. Documentation at the dependency pin (`crates/vettd-cli/Cargo.toml:14`).

Until then this branch carries only this file.