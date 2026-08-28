# vettd-cli#243 — gate rule DECIDED and IMPLEMENTED

Status: IMPLEMENTED (2026-08-28). The D4 ruling ("mechanism + recorded
additive leaning") is implemented on this branch and enforced in CI.

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

## The decision (D4 ruling, resolved 2026-08-28)

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

## What was implemented

1. **`scanner-field-gate.json`** (repo root) — the classification manifest. Records the
   pinned tag (`pinTag`) and one `surface`/`gate` decision with reasoning per
   `SkillScanResult` field. Current state at pin v0.1.4: `findings` → surface (already
   in the contract via `ExternalScannerResult.findings`); `has_skill_md`, `has_scripts`,
   `has_references`, `has_evals`, `file_count` → gate (internal-only, not in the public
   contract).
2. **`scripts/check-scanner-field-gate.sh`** — the gate. Resolves the *pinned* crate's
   real source via `cargo metadata` (works offline once the git dep is fetched), parses
   `SkillScanResult` from `result.rs` at that revision, and fails on:
   - pin mismatch (Cargo.toml tag ≠ manifest `pinTag`) — a bump must update the manifest;
   - unclassified fields — a crate field with no manifest entry;
   - `surface` field not mapped in `contract/skill_scan.rs` (the adapter);
   - `gate` field mapped in `contract/skill_scan.rs`.
   Stale manifest entries (classified fields no longer on the crate) are a warning.
3. **CI wiring** — the gate runs in the `check` job (before clippy/test), and
   `scanner-field-gate.json` + the script were added to the `rust` path filter so any
   pin/manifest/script change triggers it. A tag bump carrying unclassified fields fails
   the build.
4. **Pin documentation** — a comment at `crates/vettd-cli/Cargo.toml` (the dependency
   pin) states the gate rule and points at the manifest + script.

## What is NOT undecided

- The pin location (`crates/vettd-cli/Cargo.toml`, `tag = "v0.1.4"`).
- `scanner-data-contract.json` is unchanged by this issue (an AC of #243); the gate
  decides what may *enter* it at bump time, it does not edit it.
- `scanner-data-contract.json` has 26 `additionalProperties: false` sites and a drift
  alarm that auto-files a P0 on divergence — these are the enforcement surface the gate
  plugs into.

## Validation

- `cargo fmt --all --check`, `cargo clippy -- -D warnings`, `cargo test` — all green on
  this branch.
- `scripts/check-scanner-field-gate.sh` passes at the current pin (v0.1.4), and its
  failure modes (pin mismatch, unclassified field, surface-not-mapped, gate-mapped) were
  each verified to fail with a targeted error.
- Recorded on the issue: vettd-cli#243 (ruling + implementation). Release-order note for
  the additive `signals` shape: vettd#925 (both orders deploy safely).