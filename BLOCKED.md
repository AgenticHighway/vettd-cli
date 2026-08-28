# vettd-cli#243 — gate rule DECIDED; implementation pending

Status: DECIDED (recorded 2026-08-28) — mechanism not yet implemented (deferred by
follow-up scope: no new implementation outside the #925 signal-emission branches).

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

## The decision (resolved 2026-08-28)

**Mechanism + recorded additive leaning.** Chosen for low regret + high agility.

- **Rule:** a bump-time gate (CI/script check) forces an explicit surface-or-gate
  decision per new crate output field before the pin can be bumped. Nothing is surfaced
  silently.
- **Default leaning:** optional, additively-shaped fields (per the epic's own convention
  — `AssetSignal` in `suite-contract.json` is additive, optional, open strings, no closed
  enums, `required` unchanged, byte-identical when empty) are surfaced additively into
  `scanner-data-contract.json` when the bump author explicitly classifies them.
- **Ungated fields fail the bump** — the mechanism, not the bump author's awareness, is
  what makes the answer hold.
- **Rationale:** no permanent public-contract commitment before real signal data exists
  (the crate's `signals` vector is always-empty until #915/#916); the additive path is
  sanctioned and low-friction when real signals arrive.

## Why it was blocked, and the resolution path

The issue body asked for the decision but did not state which fields / under what
condition. Per "do not invent design", the rule was raised as an open question; the
decision above was chosen by the dispatcher.

## What is NOT undecided

- The pin location (`crates/vettd-cli/Cargo.toml:14`, `tag = "v0.1.4"`).
- `scanner-data-contract.json` must remain unchanged by the gate decision itself (an AC
  of #243; the file currently exists at the repo root and is served publicly).
- `scanner-data-contract.json` has 26 `additionalProperties: false` sites and a drift
  alarm that auto-files a P0 on divergence — these are the enforcement surface the
  mechanism must plug into.

## Remaining work (NOT started — deferred by follow-up scope)

1. Implement the bump-time gate mechanism (CI/script) that fails a bump carrying
   unclassified new crate output fields.
2. Document the rule at the dependency pin (`crates/vettd-cli/Cargo.toml:14`), not only
   in this issue/file.
3. Record the decision on the issue itself (#243) with the reasoning above.

Until then this branch carries only this record.