# vettd-cli#243 — gate rule DECIDED and IMPLEMENTED

Status: IMPLEMENTED (2026-08-28). The D4 ruling ("mechanism + recorded
additive leaning") is implemented on this branch and enforced in CI.

## What this branch is for

Issue [AgenticHighway/vettd-cli#243]("Gate whether new scanner-crate fields reach the
CLI contract on tag bump") — a dependency of the #879 release train. It is the CLI-side
counterpart of [AgenticHighway/vettd#925]("Signal emission path: scanner crate to vettd ingest").

`vettd-skill-scanner` is consumed here as a tag-pinned Git dependency
(`crates/vettd-cli/Cargo.toml` → `tag = "v0.2.0"`). The crate's `main` carries
`Signal` output fields (`crates/vettd-skill-scanner/src/signal.rs`) plus the
structural facts on `SkillScanResult` (`has_skill_md`, `has_scripts`,
`has_references`, `has_evals`, `has_assets`, `file_count`). Every field on
`SkillScanResult` must be classified before it reaches `scanner-data-contract.json`
and its `additionalProperties: false` sites — that is what this gate enforces.

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
- **Rationale:** no permanent public-contract commitment before real signal data exists;
  the additive path is sanctioned and low-friction when real signals arrive.

## What was implemented

1. **`scanner-field-gate.json`** (repo root) — the classification manifest. Records the
   pinned tag (`pinTag`) and one `surface`/`gate` decision with reasoning per
   `SkillScanResult` field, plus a `contractPath` naming where a surface field is
   actually emitted:
   - `skills[].externalScannerResults[].*` — mapped into `ExternalScannerResult` by the
     adapter (`contract/skill_scan.rs`). Current: `findings`, `signals`, `coverage`.
   - `skills[].<field>` — surfaced directly on the skill record by the skill builder
     (`contract/skills.rs`) and present in `scanner-data-contract.json`
     `skills.items.properties`. Current (v2.6.0): `has_skill_md` → `hasSkillMd`,
     `has_scripts` → `hasScripts`, `has_references` → `hasReferences`,
     `has_evals` → `hasEvals`, `has_assets` → `hasAssets`, `file_count` → `fileCount`.
2. **`scripts/check-scanner-field-gate.sh`** — the gate. Resolves the *pinned* crate's
   real source via `cargo metadata` (works offline once the git dep is fetched), parses
   `SkillScanResult` from `result.rs` at that revision, and fails on:
   - pin mismatch (Cargo.toml tag ≠ manifest `pinTag`) — a bump must update the manifest;
   - unclassified fields — a crate field with no manifest entry;
   - unknown decision values — anything other than exactly `surface` or `gate`;
   - a `surface` field not actually reaching the contract at the boundary its
     `contractPath` names (adapter reference, or skill-builder access form + structural
     presence in `scanner-data-contract.json` `skills.items.properties`);
   - a `gate` field referenced at either boundary.
   Stale manifest entries (classified fields no longer on the crate) are a warning.
3. **CI wiring** — the gate runs in the `check` job (before clippy/test), and
   `scanner-field-gate.json` + `scripts/**` are in the `rust` path filter so any
   pin/manifest/script change triggers it. A tag bump carrying unclassified fields fails
   the build. (Correction: this wiring was claimed in the original #243 write-up but was
   actually absent from `.github/workflows/ci.yml`; the `feat/879-cli-signals` branch
   delivers it.)
4. **Pin documentation** — a comment at `crates/vettd-cli/Cargo.toml` (the dependency
   pin) states the gate rule and points at the manifest + script.

## What is NOT undecided

- The pin location (`crates/vettd-cli/Cargo.toml`, `tag = "v0.2.0"`).
- `scanner-data-contract.json` changes go through the gate: a field may only enter it
  when the manifest classifies it as `surface` at the matching boundary.
- `scanner-data-contract.json` has `additionalProperties: false` at every nested site;
  divergence between the vettd and vettd-cli copies is watched by the drift alarm in the
  **vettd** repo (`.github/workflows/contract-drift-check.yml`), which compares both
  copies' version + canonical content hash and auto-files a P0 `contract-drift` issue on
  the vettd-cli repo when they diverge. That alarm lives in vettd, not here.

## Validation

- `cargo fmt --all --check`, `cargo clippy -- -D warnings`, `cargo test` — all green on
  this branch.
- `scripts/check-scanner-field-gate.sh` passes at the current pin (v0.2.0), and its
  failure modes (pin mismatch, unclassified field, unknown decision, surface-not-mapped,
  gate-mapped, skill-level field missing from the contract JSON) were each verified to
  fail with a targeted error.
- Recorded on the issue: vettd-cli#243 (ruling + implementation). Release-order note for
  the additive `signals` shape: vettd#925 (both orders deploy safely).