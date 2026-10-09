# Scope note for #965 — what the passive-observer production implementation inherits from spike #828

> ## Status: shipped
>
> **Superseded as a plan; kept as a record.** The production implementation landed in `vettd-cli`
> as `vettd observe` (see [`observe.md`](observe.md) for the shipped behaviour and
> [`vettd-observe-port-plan.md`](vettd-observe-port-plan.md) for the port's phase-by-phase
> record). This document is the spike's reasoning at the time it was written, moved here from
> `spikes/828-passive-observer/` when that directory was retired. It is not maintained against
> the code, and where the two disagree the code and `observe.md` win.
>
> Where the implementation deliberately diverged from what is described below:
>
> | This document says | What shipped | Why |
> | --- | --- | --- |
> | `run_id = HMAC(secret, harness session id)` | `HMAC(secret, "{harness}:{session_key}")` | The harness prefix namespaces the pseudonym, so the same session key under two harnesses cannot collide. |
> | Reuse the scan cache (`~/.vettd/scan-cache/scan-v2.sqlite3`) for cursors | A separate store, `~/.vettd/observer/observer-v1.sqlite3` | The scan cache opens without WAL or a busy timeout, and its `CACHE_SCHEMA_VERSION` orphaning would silently discard cursors on a version bump. |
> | `extractor_version` as `proto-0.1.0+taskcat-1` | `1+1` | Every string leaf is substring-checked against the machine's own asset names; a long producer-controlled string is a large fail-closed collision surface for no benefit. |
> | Cloud route built on `hub/signals` | Built on `dev` | `hub/signals` carries the emitter-credential work; the observations route shares nothing with it but a file pattern. |
>
> One thing this document identifies that is **not** yet closed: `harness_version` defaults to
> `"unknown"`, whose substrings include `now`, `own` and `know`, so a machine with a three-letter
> asset name of that shape is refused. Closing it means exempting producer-controlled leaves from
> the dynamic substring rule, as the gate already exempts closed enums. That is a gate contract
> change and is left for the owner.

> Written by spike [#828](https://github.com/AgenticHighway/vettd/issues/828) for the tracker
> [#965](https://github.com/AgenticHighway/vettd/issues/965) (second scanning pass, chat-log
> signals). Companion to the spike answer, now [`passive-observer-decision-828.md`](passive-observer-decision-828.md). Laptop tier only:
> read-only, opt-in, fail-open, one developer machine. Nothing here is a fleet-tier commitment;
> org-level consent, collector attestation and "not silently disableable" are a different feature
> with a different consent model.
>
> Repo facts below were verified against `vettd-cli` v0.9.3 (`dbd3872`) and `vettd`
> `hub/signals` (`9251b8d`) on 2026-09-02.

## Ruling this note assumes

#828 recommends **own epic**, and #965 moves to it wholesale. The observed modality answers
different questions from #824's controlled evaluation (personal usage, real environments, no
control over configuration), carries its own consent surface and its own route, and cannot be
a child of a static-scan tracker without inheriting acceptance criteria it cannot meet (see
"What v1 does not do" below). If the maintainer rules "fold into #824" instead, everything
below still applies; only the parent changes.

## What #965 inherits from the prototype (was `spikes/828-passive-observer/`, since deleted)

| Artifact | Status | How it transfers |
|---|---|---|
| `telemetry-field-gate.json` (promoted to the repo root) | **The one artifact meant to survive.** 85 leaf paths, 14 disclosure categories, closed enums (model ids are a closed, gate-versioned list), 20 value-level forbid patterns, dynamic forbid sets with path-component splitting | Becomes the CI-consumed manifest for a `scripts/check-telemetry-field-gate.sh` sibling of the scanner field gate; the categories become `DisclosureCategory` variants in `contract/disclosure.rs` |
| `telemetry-envelope.schema.json` (promoted to the repo root) | Draft 2020-12, `additionalProperties:false` at every object, no floats | The wire contract for the new route; served like `scanner-data-contract.json`; validated with Ajv on the cloud side |
| `prototype/check_field_gate.py` | Reference semantics of the gate: key-path walker + value-level checks + dynamic forbids; rejects duplicate JSON keys and non-identifier keys; never echoes a value or key | Port to Rust as a test over the serialized payload (the walker already exists; the value checks are new) and to a CI script over a golden payload |
| Asset key derivation (`attribute.py`) | content hash (tree hash for skill dirs), canonical-descriptor hash for MCP servers, HMAC name pseudonym otherwise; `key_basis` enum | Port as-is; the descriptor canonicalisation rules are the part to test hardest |
| Attribution tiers, `binding`, the delta settle rule, bom_version | Decisions D4 | Port as-is; the settle rule is specific to Claude Code's `deferred_tools_delta` semantics |
| Evidence-state floors and Wilson ordering (`rank.py`) | Decision D5 | The cloud display layer implements these; the CLI never ranks for the cloud |
| Task-category rule table (`taskcat.py`, `taskcat-1`) | Published, versioned into `extractor_version` | Port verbatim; a rule change is a new `extractor_version` |
| Test list (`prototype/tests/`) | Unit, gate-negative, non-blocking | Re-express in Rust; the non-blocking suite gains the Windows `share_mode` test the prototype cannot run |
| `prices.json` (promoted to `crates/vettd-cli/resources/observe-prices.json`) | Dated display-time price table | Cloud-side display concern; never stored with the observation |

Nothing from Splitrail is taken by file in the prototype. If the Rust port takes
`src/analyzers/claude_code.rs` / `codex_cli.rs` by file, pin the v3.7.2 commit, keep the MIT
notice, exclude the uploader and history reader, and say so in the fork's README. Know what you
are taking: Splitrail's parse stage folds each JSONL line into a per-message `Stats` struct (tool
*counts* by category, tokens, cost) and drops tool names, tool ids, `is_error` and MCP server
names at parse time. The deserialization structs and discovery globs transfer; the extraction
does not.

## What #965 must rebuild (CLI, `vettd-cli`, all under `crates/**` so CI gates it)

1. **`Source` trait + two readers.** Serde structs for the Claude Code line types the prototype
   consumes (`user`, `assistant`, `attachment`, `summary`) with a key allowlist applied at
   deserialisation (`#[serde(deny_unknown_fields)]` is the wrong tool — unknown line *types* must
   be counted, not rejected); Codex rollout structs from the protocol crate's public shapes,
   confirmed on a real file first. Both stream line by line from a byte cursor; both open
   read-only; on Windows open with `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`
   (`std::os::windows::fs::OpenOptionsExt::share_mode`) so archive-by-rename is never blocked.
2. **Device secret.** `identity.rs` gains `~/.vettd/observer_secret` (32 random bytes, 0600, same
   persistence helper as `scanner_uuid`, never egressed, never auto-rotated; rotation clears the
   emitted-session ledger).
3. **Disclosure and gate.** New `DisclosureCategory` variants; `validate_payload_coverage`
   generalised to the telemetry envelope; a golden-payload test that runs the value-level checks;
   `scripts/check-telemetry-field-gate.sh` wired into `ci.yml` next to the scanner field gate.
4. **Cursors.** Reuse the scan-cache SQLite (`~/.vettd/scan-cache/scan-v2.sqlite3`) with a new
   `observer_cursors` table. That database is opened with plain `Connection::open` today (no WAL,
   no `busy_timeout`); if an observer run can overlap a scan, add both or use a separate file.
5. **Opt-in and consent.** A new command (`vettd observe`) behind an explicit config flag
   (`telemetry.enabled = true`), a first-run disclosure rendered from the telemetry categories
   exactly as scans render theirs, shown on every path including non-interactive ones, and
   `--dry-run` that writes the payload locally without sending. Standing consent for *scans*
   does not imply consent for *observation*; the README's "network activity only when you
   explicitly opt into a submission flow" sentence must stay true for this feature too.
6. **Submit.** Reuse `submit.rs` mechanics (Bearer key, retries honouring `Retry-After`,
   endpoint guard) against the new route; batch runs; persist an emitted-session ledger so a
   re-run never double-sends.
7. **Detectors.** Add `~/.codex` (`config.toml`, `sessions/`) to `discovery.rs` and
   `mcp_configs.rs` so `--submit` inventories can evidence Codex presence at all.

## What #965 must build (cloud, `vettd`; built on `dev`, not `hub/signals` — see the status header)

The cloud already has a second ingest route built for out-of-process emitters:
`POST /api/signals/ingest` (#973) — emitter credential (`ah_emit_…`), provenance stamped at the
boundary, ≤500 signals per request, 1 MB body, 90 s deadline, durable 30/min rate limit, one line
in `proxy.ts::isApiKeyAuthPath`. **Telemetry cannot use it as-is**: that route is per-subject
(`{subjectType, subjectId, signals[]}`) and upserts current state under
`(subjectType, subjectId, ruleId, relatedType, relatedId, source, userId)`; a run spans many
subjects and the identity key cannot hold strata (harness, model, task category). So:

- **Route.** `POST /api/observations/ingest`, cloned from the signals route's pattern: user API
  key (`ah_`, the credential the CLI already holds), the `isApiKeyAuthPath` line, a
  `RATE_LIMIT_POLICY` row (durable, per user, ~10/min), `MAX_BODY_BYTES` 1 MB (a payload of
  ~50 runs × ~30 assets is ~200 KB), the same streamed body read and 90 s deadline, Ajv
  validation against `telemetry-envelope.schema.json`, idempotency on `run_id` (a resend is a
  200, never a second row).
- **Tables (retention in the same PR, per #881's rule).** `ObservationRun` (one row per
  `run_id`: resource fields, run-level enums and counts, `bomVersion`, `observedAt` = run day,
  `harnessId`) and `ObservationAssetStat` (one row per `(run_id, asset_id, signal)` holding the
  mergeable set `n, vMin, vMax, vSum, vSumSq`; never a percentile). Sweep by `observedAt` with the
  existing `pruneExpired*` hook in `scan-jobs/runner.ts`, 90 days like `AssetSignalEvent`.
  `bomVersion → asset_ids` in an `ObservationBom` table so co-occurrence is a join, not a payload.
- **Harness identity.** `harnessId String @default("")` on both observation tables **and on
  `AssetSignal`, added to `assetSignalIdentity`**. #965 said this needs a real column and must
  not be minted casually; this is the mint. The alternative, `referenceFrame` (decided in #957,
  column not added, outside the identity key), cannot hold two harness strata for one asset
  without colliding on the upsert.
- **Projection.** A job that folds `ObservationAssetStat` into `AssetSignal` rows:
  `sourceClass: "logs"`, `source: "vettd-cli"`, `userId` = the submitting user (never `""`, per
  #957), `sampleSize` = merged `n`, `observedAt` = latest run day, `derivation: "inferred"` while
  the CLI tier is inferred, `harnessId` set, and `ruleId`s like
  `reliability/observed-non-success-rate`, `performance/observed-invocation-latency`,
  `cost/observed-tokens-per-run`, `cost/observed-invocation-frequency`. `observedAt` is the
  retention key on `AssetSignalEvent`: never backdate it.
- **Display.** The existing signal drawer already renders "not yet measured" for absent
  verdicts (#920's ruling). D5's evidence-state floors, Wilson intervals and the separate
  "not enough evidence yet" list are new display logic in `verdicts.ts`/the drawer, keyed on
  `sampleSize`.
- **Asset key join.** Telemetry carries hashes only. The join to display names goes through the
  inventory the same device submitted under the scan disclosure; that inventory's ids are
  `{source_path}:{hash12}` today and MCP ids are `{name}-{sha12(config_path)}` — neither is the
  content/descriptor hash telemetry emits. The CLI leg therefore has to add the telemetry
  `asset_id` to the scan payload (a `scanner-data-contract.json` change, sequenced downstream of
  vettd-cli#243 and always vettd-first) before any telemetry row can be shown with a name.

## What v1 does not do

**Single-machine v1 cannot retire the public #916/#917 proxies.** Those proxies are directory-wide
public signals; an observation row is tenant-scoped (#957) and one developer's runs are not a
population. v1 *supplements* the proxies on the user's own dashboard ("on your machine, in your
runs, here is what was observed") and leaves the public tiles alone. Retirement needs the
fleet-tier aggregate, whose aggregation privacy (minimum cohort size, no per-device rows in org
views) is a separate feature. Record this on #916 and #917 when #828 rules so their "retired by
#965" acceptance criteria are amended rather than silently missed.

Also not in v1: any enforcement or blocking; public cross-org aggregation; eBPF; a resident
watcher process (Claude Code's in-band deltas remove the need for one; Codex's mid-session config
changes stay undetected and documented); ClickHouse or any store migration; the eval engine.

## Proposed children (replacing the list in #965's comment)

| # | Child | Repo | Depends on | Landed |
| --- | --- | --- | --- | --- |
| 1 | Observer secret + telemetry disclosure categories + walker generalisation + golden-payload gate test + CI script | vettd-cli | — | **Yes** — `identity.rs`, `contract/disclosure.rs` (14 variants), `observe/gate.rs`, `scripts/check-telemetry-field-gate.sh` |
| 2 | Claude Code source (structs, key allowlist, pairing, dedupe, in-band loaded set, settle rule, sub-agent linkage) + non-blocking tests incl. Windows share mode | vettd-cli | 1 | **Yes** — `observe/source.rs`, `observe/claude_code/`; the Windows share-mode test runs in CI on a Windows runner |
| 3 | Attribution: asset keys, binding, bom_version, tiers | vettd-cli | 2 | **Yes** — `observe/extract.rs`, `observe/attribute/`, `observe/taskcat.rs` |
| 4 | `vettd observe`: opt-in flag, disclosure on every path, cursors, ledger, dry-run, submit | vettd-cli | 1–3, 6 | **Yes** — `observe/{args,pipeline,store,subcommands,submit}.rs`; shipped ahead of child 6 rather than after it, since the CLI's contract is the schema, not the route |
| 5 | Codex source, confirmed on real files; `~/.codex` detectors | vettd-cli | 2 | No — deliberately out of scope; `--harness` rejects anything but `claude_code` at parse time rather than accepting it and finding nothing |
| 6 | `POST /api/observations/ingest` + tables + retention + `harnessId` column and identity-key migration | vettd | — | In progress on `dev`, not `hub/signals` |
| 7 | Projection into `AssetSignal` + display floors/intervals/"not enough evidence yet" | vettd | 6 | No — follow-up |
| 8 | Telemetry `asset_id` on the scan payload (contract bump through vettd-cli#243's gate) | vettd → vettd-cli | 3 | No — follow-up |
| 9 | Amend #916/#917 acceptance to "supplemented on the user's view; public retirement is fleet-tier" | vettd | ruling | No — needs the maintainer |

## Estimate against actual capacity

Capacity as recorded in the 2026-07-27 sprint planning and the git log: two-week sprints; one
engineer at ~10 days per sprint doing essentially all spike and signals work (191 of 296 `vettd`
commits and 35 of 43 `vettd-cli` commits since July), one part-time engineer at ~5 days per
sprint, product at ~2 engineering days. The July estimate for "log scanning" was "let's just say
one day"; this note supersedes it.

| Child | Engineer-days | Notes |
|---|---|---|
| 1 gate + secret + disclosure | 3 | mostly test surface |
| 2 Claude Code source + non-blocking suite | 6 | Windows share-mode test needs a Windows runner |
| 3 attribution | 3 | descriptor canonicalisation is the risk |
| 4 `vettd observe` + consent + submit | 4 | consent copy review included |
| 5 Codex source + detectors | 4 | blocked on access to a real Codex rollout file |
| 6 route + tables + migration | 5 | identity-key migration on `AssetSignal` needs a dry run |
| 7 projection + display | 5 | thresholds are config, per #886 |
| 8 contract bump | 2 | first deliberate walk through the #243 gate |
| **Total** | **32** | |

Serial on one engineer: about **7 weeks** (32 days ÷ 10 days per sprint, plus one sprint of
slack for the two blocked items). With the cloud children (6, 7) taken by the second engineer:
about **5 weeks**. Neither includes fleet-tier work.

## Open questions for the maintainer

1. New route (`/api/observations/ingest`, user key) versus an emitter credential on the existing
   `/api/signals/ingest`. Recommendation: new route — the existing one cannot carry strata or
   run idempotency, and the CLI already holds a user key. Say no here and children 6–7 collapse
   into a projection-only design that loses model/task stratification.
2. Whether `harnessId` joins `assetSignalIdentity` now (recommended) or waits until a second
   harness emits. Adding it later means rewriting the unique index under live rows.
3. Whether the ADR-001, Sol-review and adversarial-review documents the brief cites can be
   linked from #828. None was reachable from this spike's environment; the decisions were made
   against the brief's summary of them.
