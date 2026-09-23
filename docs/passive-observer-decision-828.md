# Spike #828 — Passive observer: harness session logs as an observational asset signal (DECISION)

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

> Answer to [#828](https://github.com/AgenticHighway/vettd/issues/828). Laptop tier only:
> read-only, opt-in, fail-open, one developer machine. Fleet-tier concerns (org consent,
> collector attestation, "not silently disableable") are a different feature with a different
> consent model; nothing here is a fleet-tier commitment.
>
> The product claim this supports is narrow and stays narrow: *given this task, this harness and
> this model, here is what has been observed about these assets in real runs, and how confident we
> are.* It is not a comparative-effect claim of any kind. Nothing in the schema, the API shape, or the
> copy below says otherwise, and `prototype/lint_copy.py` fails on the phrases that would.
>
> Every repo claim was verified at `vettd-cli` v0.9.3 (`dbd3872`) and `vettd` `hub/signals`
> (`9251b8d`) on 2026-09-02; harness facts were verified on real Claude Code 2.1.258 session files
> and on the public `openai/codex` protocol crate. Where the repo contradicts the brief, the repo
> wins and the contradiction is listed in [§9](#9-where-the-repo-contradicts-the-brief).

Deliverables in this directory:

| File | What it is |
|---|---|
| `README.md` | This answer (also posted on #828) |
| `SCOPE-965.md` (now [`passive-observer-scope-965.md`](passive-observer-scope-965.md)) | Scope note for #965 (also posted there) |
| `telemetry-field-gate.json` (now at the repo root) | The egress allowlist in the form the gate consumes — **the artifact meant to survive into #965** |
| `telemetry-envelope.schema.json` (now at the repo root) | JSON schema of the payload, every object closed |
| `prices.json` (now `crates/vettd-cli/resources/observe-prices.json`) | Dated display-time price table; cost is derived, never stored |
| `prototype/` | Throwaway Python prototype: reads real local session state, extracts, attributes, aggregates, gate-checks the written payload, prints a ranked list. Never posts |
| `worked-example/` | Real, scrubbed output from the spike author's own session files |

## Recommendation: own epic

The observed modality is worth pursuing and should be its own epic, with #965 moving to it
wholesale. It answers questions #824's controlled evaluation cannot (what happens in real
environments the user configured), it is cheap per observation, and the loop closes end to end
with hand-written parsers and no new dependency. It is not a fold into #824: the consent surface,
the route, the identity key and the confounding story are all different. "Drop" would leave Cost
& Usage with no real-signal path (#885), but note [§5 D5](#d5--cold-start-and-the-empty-state): a
single machine cannot retire the public #916/#917 proxies either; that needs the fleet tier.

## 1. Verification findings

Confirmed at head, in the order the brief asked:

- **Cargo and toolchain** — Rust 2021, toolchain pinned 1.85.1, `ureq 3`, `rusqlite 0.39`
  (bundled), no `notify`, no `opentelemetry`. `tokio 1.53` is in `Cargo.lock` only as a
  transitive dev-dependency of `httpmock`; the shipped binary has none.
- **The disclosure walker** — `contract/disclosure.rs::validate_payload_coverage` walks every
  serialized key path of the payload and panics on any key without a `DisclosureCategory`; tests
  cover the maximal and minimal payloads and reject an injected unknown key. It is enforced on the
  payload, not the code, exactly as the brief says. It is a *key* walker: it cannot see a path,
  URL or name inside a string value, which is why the telemetry gate adds value-level checks.
- **Consent** — PR #228 shipped "configured auth = standing consent"; the disclosure prints on
  every path to stderr. One nuance the brief's summary misses: on `--submit`, the payload builder
  that reads application logs runs *before* the disclosure is printed (only the interactive path
  reads after consent). Standing consent makes that legal; "disclosure before read" holds only
  interactively. #196 is closed.
- **Emission** — `submit.rs` is a JSON POST with a Bearer key and retries honouring
  `Retry-After`. The endpoint guard blocks *public non-production* hosts unless
  `--allow-public-endpoint`; the production host and any saved config bypass it. The brief's "guard
  against non-local endpoints" is inverted in practice: production is always trusted.
- **Only log reader** — `network_evidence::scan_mcp_logs`: seven Claude/VS Code/Cursor log
  directories, at most 15 files, 7-day mtime cutoff, the whole file read into memory and the last
  32 KiB kept (a byte cap, not a tail read), URL-credential redaction and home-directory scrubbing.
  No session-transcript parser exists anywhere in the crate.
- **Asset ids** — `{source_path}:{artifact_hash[..12]}` for prompts, skills, agents and apps; only
  `Prompt.contentHash` is on the wire. **MCP server ids are `{name}-{sha256(config_path)[..12]}`**,
  not name plus command as the brief says. No path-free key exists for any type.
- **The scan cache** — `~/.vettd/scan-cache/scan-v2.sqlite3`, opened with plain
  `Connection::open` (no WAL, no busy timeout); `root_cursors` stores a one-shot FSEvents watermark
  on macOS, not a subscription. No daemon, no watcher thread. No `lib.rs`: the crate cannot be
  linked by a prototype.
- **Cloud** — `/api/scans/ingest` validates with Ajv against `scanner-data-contract.json`
  (28 `additionalProperties:false` sites); an extra top-level key is a 400. Telemetry cannot ride
  it. The team's local `vettd` checkout used by this spike was stale (2026-08-12); on
  `dev`/`hub/signals` the signals epic has landed: `AssetSignal` with `sourceClass` reserving
  `logs`, `sampleSize`/`observedAt`/`synthetic` as real columns, and a second ingest route
  `POST /api/signals/ingest` for out-of-process emitters (#973). Harness identity has no column
  anywhere; #965 and #885 route minting it here.
- **Splitrail v3.7.2** — seventeen analyzers, `notify 8.2`, `tokio` (full), `reqwest 0.13`,
  `rusqlite 0.38`; the sqlite link conflict with 0.39 is real. **Its parse stage folds as it
  reads**: `parse_jsonl_file` emits one `ConversationMessage` per JSONL line carrying a `Stats`
  struct of tool *counts* by category plus tokens and cost; tool names, tool ids, `is_error` and
  MCP server names are dropped at parse time. The Codex analyzer likewise keeps only
  `function_call` ids and subtracts cumulative `token_count` events. What transfers by file is
  the deserialization structs and the discovery globs, not the extraction.
- **Claude Code, on real files** — every line carries `cwd`, `gitBranch`, `sessionId`,
  `timestamp`, `isSidechain`; usage has four buckets plus thinking; `tool_use`/`tool_result` pair
  by id across lines (348 of 348 paired in the sample). Three things the brief did not know:
  (a) `attachment` lines record the **loaded set in-band** at session start — skill names, deferred
  tool names including `mcp__<server>__<tool>`, agent types, MCP instructions, and rules-file
  content — and record *deltas* thereafter; (b) sub-agent transcripts live under
  `<session>/subagents/agent-<id>.jsonl` with a `.meta.json` whose `toolUseId` equals the parent's
  `Agent` tool_use id (workflow-spawned agents sit one level deeper, under
  `subagents/workflows/<id>/`, which the first parser missed), and their assistant lines carry
  **harness-native `attributionAgent` / `attributionMcpServer` / `attributionMcpTool` fields**
  (the main transcript carries the MCP pair on every response an MCP tool produced); (c) one API response is split over many
  lines that repeat the same `message.id`, and the usage on those lines grows as the response
  streams, so the fullest line per id must be kept and naive summation overcounts output tokens
  several-fold. Backgrounded sub-agent spawns return in 1–2 s while the child runs for minutes:
  the parent-side latency says nothing about the sub-agent.
- **Codex, from the protocol crate** (no real file was available; must be confirmed on one) —
  rollout lines `{timestamp, type, payload}` with `session_meta`, `turn_context` (model,
  approval policy, effort), `response_item` (`function_call`/`_output`, `custom_tool_call`),
  `event_msg` (`token_count` cumulative with `cached_input_tokens` and `reasoning_output_tokens`;
  `mcp_tool_call_begin/end` with server, tool and `Failed`; `context_compacted`). MCP tools are
  named `<server>__<tool>`, optionally prefixed `mcp__`, hash-suffixed past 128 characters.
- **OpenClaw** — docs place runtime sessions in a per-agent SQLite file and archives under
  `sessions/`; `OPENCLAW_STATE_DIR` is confirmed; archive-by-rename and a native cost field are
  not confirmed by the docs. **Hermes** — docs confirm `~/.hermes/state.db`, WAL, "concurrent
  readers + one writer", a 1 s busy timeout with jittered retries, schema version 23 with
  declarative column reconciliation, and `estimated_cost_usd`/`actual_cost_usd` on `sessions`.
- **opentelemetry-rust** — `opentelemetry-otlp` 0.32 has MSRV 1.75, an `http-json` exporter and
  a `reqwest-blocking-client` feature with tokio optional. "OTel is tricky because the CLI is
  Rust" is not a reason either way, as the brief says.
- **Customer evidence** — the usability-test template asks "Which harness(es) do you run things
  in today? (Claude Code, Cursor, custom in-house, sandboxed VMs, none yet, other)". No recorded
  answers were reachable. No issue in either repo names Codex; the only harness in any scope
  line is Claude Code, plus "Claude Code, VS Code/Cursor" in a #196 comment. `--submit`
  inventories can evidence Claude Desktop, VS Code and Cursor config presence and **cannot
  evidence Codex at all** — no detector knows `~/.codex`.
- **Attachments** — ADR-001, the Sol review and the adversarial review of v1 were not reachable
  from this spike's environment (the two June-2026 OSS design documents were read in full and use
  none of that vocabulary). Decisions below were made against the brief's summary of them; please
  link them from #828.

## 2. The five decisions

### D1 — Harness scope for v1

**Claude Code and Codex for the parser trait. Cursor presence is tracked; a Cursor parser is
gated on a format contract. Hermes and OpenClaw are out.**

Why Claude Code: it is the only harness with customer evidence, and it has the richest log — the
loaded set and its deltas are in-band, sub-agents link by id, and the harness writes its own
attribution fields. Why Codex second: it is file-based JSONL with a public, typed protocol crate,
it has a Splitrail parser to compare against, and its token schema (cached-input, no
cache-creation, reasoning separate) and namespaced tool naming force the source trait to be real
rather than Claude-shaped. Codex is an engineering bet, not a customer-evidenced pick, and this
answer says so.

Why not the others, explicitly: **Cursor** stores transcripts as undocumented SQLite blobs in
`state.vscdb` that move between releases — a parser there demonstrates nothing about a file-based
trait and breaks on schedule; its *presence* is already detected. **Hermes** is a WAL,
multi-writer database with a migrating schema (v23 today); a reader is a database client with
lock and version coupling that the fail-open constraint forbids in v1. **OpenClaw** sessions
contain third parties' messages, runtime state is SQLite, and sessions never "complete".

What would change it: run the inventory query (latest `ScanEvent` per user with
`type='vettd-cli'`, count users whose `scanRoots` or asset id prefixes match `/.cursor` or
`Cursor/User` versus `/.claude` versus `/.codex`; expect zero for Codex by construction), add the
`~/.codex` detector so the next release can evidence Codex, and swap Codex for Cursor only if
Cursor dominates among design partners *and* ships a documented export.

What the trait normalises: session boundaries (resume, compaction, fork/parent, archive), token
buckets (nullable, tagged by provider), tool naming to asset key, failure classes, sub-agent
linkage, harness-clock timestamps, model id, harness version, and the *basis* of the loaded set.
A messaging-platform variant for sessions that never complete is reserved as an enum value and
not implemented.

### D2 — The egress allowlist

**`telemetry-field-gate.json`: 85 leaf paths across 14 disclosure categories, closed enums, and
20 value-level forbid patterns plus dynamic forbid sets (path-like sets are also split into their
components, so a branch leaf or a directory name alone is caught).** The categories are named as future
`DisclosureCategory` variants; the walker semantics are the existing ones; the value rules exist
because a key walker cannot see a path inside a string.

Decided rather than assumed:

- **Timestamps** egress at UTC day resolution only (`observed_day` = day of the session's first
  harness timestamp). **Session duration does not egress**: per-day sums of durations are an
  attendance log and nothing in D5 ranks on it. Per-invocation latency stays, as relative
  mergeable statistics.
- **No project-grouping key in v1.** `bom_version` already groups runs by loaded set. A
  pseudonym `HMAC(observer secret, canonical cwd)` is reserved for per-project baselines if they
  are ever needed, unimplemented.
- **Failure classes**, closed: `tool_error | timeout | user_denied | interrupted | unknown`.
  Only `tool_error` and `timeout` count toward it, and it is called the *observed non-success rate*
  because a harness's `is_error` conflates expected non-zero outcomes (a grep
  that matches nothing, an MCP 400) with faults. A user rejecting a permission prompt is not the
  asset failing and is counted separately.
- **Task category** is derived from a published local rule set over tool-mix shares
  (`prototype/taskcat.py`, version `taskcat-1`, folded into `extractor_version`): `code_edit |
  code_explore | shell_ops | mcp_heavy | mixed | unspecified`. No content is read and the shares
  themselves never egress.
- **Run confounders** are recorded as closed enums so D5 can stratify or visibly pool: `effort`,
  `permission_mode`, `entrypoint_class`.
- **Identity.** Device id is the existing `scanner_uuid`. A new device-local
  `~/.vettd/observer_secret` (32 random bytes, 0600, never egressed, never auto-rotated) keys
  `run_id = HMAC(secret, harness session id)` and every name pseudonym. `scanner_uuid` cannot be
  that key because it egresses, and a plain hash of a name is trivially reversible.
- **Model ids** are a closed list versioned with the gate (`enums.model`); anything else becomes
  `other`. A prefix pattern was tried first and rejected because a user-named model such as
  `claude-<org>-<project>` would pass it and carry a name. Harness versions must be semver or
  become `unknown`. Because sub-agents can run on a different model from the parent, token
  buckets egress **per model** (`tokens_by_model[]`) as well as in total, so cost is never rendered
  at the wrong price.

Never: message content, file paths, cwd, branches, repo or org names, tool arguments, error
strings, environment variable names, usernames, hostnames, URLs, asset names of any kind,
user-chosen identifiers (slugs, OpenClaw agent ids, Hermes session titles), harness session,
message, request or tool-use ids, wall-clock finer than a day, session duration, native cost.
Human-readable names live in the inventory the user already consented to; the join is server-side
on asset hashes.

### D3 — Emission format at the cloud boundary

**Vettd-native envelope with OTLP semantics, not OTLP encoding. Per-run records carrying
per-asset mergeable statistics; neither per-invocation rows nor cross-run aggregates.**

The shape: a `resource` block (device, harness, collector) plus typed `records[]`, which an
exporter can map to OTLP losslessly if an enterprise ever wants Collector routing. Literal OTLP
is rejected on a technical ground: its attribute-list encoding (`attributes[].key/value`) turns
every field into `attributes[].value` and defeats the key-path walker that enforces the allowlist.
The brief's reject condition also applies: the cloud exposes no OTLP route and #804's envelope
has not shipped, so the literal shape buys nothing now.

What egresses: **one record per run** with run-level enums, counts and token totals (in total and
per model), and a per-asset list of `{n, sum, min, max, sumsq}` per signal — never a percentile,
per #881's rollup rule. A loaded-set change inside a run is a count on the record
(`counts.loaded_set_changes`) plus every segment's set in `bom[]`, never a second record: the
first version emitted one record per segment and duplicated the run's tokens and counts, which the
adversarial review caught. Strata (harness, model, task category, effort, permission mode, day) survive for D5;
per-call timelines do not, because a timeline is a behavioural fingerprint and an event stream is
the observability product #804 names as do-not-build. Per-run rather than local per-day rollups
because `run_id` is the server-side idempotency key; records are stripped to a rollup's
information content (no duration, no segment index) and sorted by `(observed_day, run_id)` so
file order carries no time.

Cloud landing: **not** `/api/signals/ingest` as-is — it is per-subject and upserts current state
under an identity key that cannot hold strata. #965 clones that route's pattern as
`POST /api/observations/ingest`, stores runs and per-asset mergeable rollups with retention in
the same PR, projects `AssetSignal` rows (`sourceClass: logs`, `sampleSize`, `observedAt`), and
mints `harnessId` as a real column on the observation tables and on `AssetSignal` (in the
identity key). `referenceFrame` (#957) is outside the key and cannot hold two harness strata for
one asset. Details in [`passive-observer-scope-965.md`](passive-observer-scope-965.md).

### D4 — Attribution model

Three tiers; every emitted signal carries the one that produced it.

| Tier | Basis | Used for |
|---|---|---|
| Direct | The asset was explicitly invoked in the turn: a Skill call, an MCP tool call resolving to a known server, a sub-agent dispatch resolving to a known agent definition | Observed non-success rates, per-invocation latency, exact sub-agent token totals |
| Loaded | The asset was in the run's loaded set but not observably invoked | Context-cost accounting and co-occurrence only |
| Inferred | Heuristic match (name pseudonym, precedence rule, historical read with filesystem-now hashes) | Nothing user-facing. Stored, not displayed |

Decided:

- **Asset keys** (`key_basis`): content hash where there is content (skills = canonical tree
  hash of the skill directory; agents, rules files, prompts = file hash); MCP servers = sha256 of
  a canonical stripped descriptor `{transport, command basename or URL host class, args minus
  secret-shaped and path-shaped tokens, sorted env NAMES}` — unsalted on purpose, because
  cross-user identity for well-known servers is the join feature and the same fact already
  egresses under the scan disclosure; for a private server the preimage is a stable pseudonym,
  not a secret. Assets with no local content or descriptor (harness-provided or remote
  connectors, listed-but-uninvoked skills) get `HMAC(observer secret, "<type>:<name>")`, which has
  no cross-device meaning and is Inferred by construction. Harness built-in agent types are not
  assets and never enter the ranking.
- **Loaded-set capture.** Claude Code records the loaded set in-band at session start and
  records deltas; basis `harness_log`, no resident process. A **settle rule** decides segments:
  a delta folds into the current segment when it removes nothing and every added name belongs to
  an MCP server the harness had listed as pending (verified: the second delta 13 s into the
  sample session was an async MCP connect completing, not a configuration change); a new segment
  starts only on removals, re-adds or unexplained additions. The record carries the session-start
  set as `bom_version`, the number of settled changes as a count, and every segment's set in
  `bom[]`, so run-level totals are never duplicated and co-occurrence per segment survives. Codex has
  no in-band listing: basis `filesystem`, loaded set fixed at session start, mid-session change
  undetected — a documented gap, and a resident watcher is not proposed for v1 (its operational
  surface — daemon, watch limits, disk — is the thing this tier avoids).
- **The invariant that is actually keepable.** Names are exactly what the harness listed at
  `ts_listed`; hashes are of the file as it was at `hashed_at`; the record claims nothing about
  the file at `ts_listed` unless `binding = mtime_proven` (the asset directory's newest mtime is
  older than the listing). Two hashes are exact from the log alone (`binding =
  harness_log_exact`): rules files, whose content Claude Code writes in-band, and *invoked*
  skills, whose body is injected after the command marker.
- **Which asset types can ever be Direct**: skills, MCP servers, agent definitions. Rules files,
  prompts and container definitions are loaded, never invoked: they get Loaded-class metrics only
  (context-cost estimate, co-occurrence) and are shown in a separate list, never ordered by an
  observed rate they cannot have.
- **Failure classification** as in D2. Claude Code denial = `is_error` and (`interrupted` or a
  denial phrase), classified locally then discarded — version-fragile, and zero denials occurred
  in the sample. Codex: `Failed` status, `success:false`, approval decisions.
- **Session-level signals** (turns, token totals, repeated-call indicator) attach to `run_id` and
  `bom_version`, never to an asset.
- **Sub-agents.** Linked by the child's `meta.json.toolUseId` = the parent's `Agent` tool_use id
  (5 of 5 in the sample). Outcome and tokens come from the child transcript, deduplicated by
  `message.id` like the parent's; the parent-side result is a spawn acknowledgement and is
  excluded from latency. Harness-native attribution fields are recorded as corroboration counts.
- **Same-name MCP servers at user and project scope**: identical command → same descriptor hash;
  different → resolve by harness precedence, tag the observation Inferred.
- **What the prototype can claim.** It reads historical logs and hashes the filesystem as it is
  now, so every emitted row is `inferred`, with `direct_evidence_available` recording whether the
  log carries an explicit invocation (the production collector's path to Direct). On the sample
  machine 34 of the 49 listed skills have a local directory (32 bind `unproven` because their
  files are newer than the listing, one binds `mtime_proven`, one is hashed exactly from its
  in-band body), 15 are name pseudonyms, and the MCP servers are remote connectors with no local
  descriptor, so every connector row is a name pseudonym.

### D5 — Cold start and the empty state

**Explicit `evidence_state` per (asset, signal, stratum); per-class display floors; Wilson
intervals; strata never pooled silently; v1 is single-machine.**

`evidence_state ∈ {observed, early_evidence, insufficient_evidence, not_applicable,
no_coverage}`. Assets that were loaded but never invoked in the runs shown are collapsed into one
summary line by type rather than listed as fifty rows of "0 in 0 calls"; when the stated task's
category has no runs at all, the view pools every category in the harness and says so in its
header — pooling is visible or it does not happen. Floors, presented as display floors rather than statistics: counts at n ≥ 1;
token totals at n ≥ 3 runs; latency at n ≥ 5; an observed non-success *rate* is shown only at
n ≥ 20 with its 95 % Wilson interval (at k = 0, n = 20 the interval is 0–16 %, which is
informative; at ten observations it is not) and used for ordering only at n ≥ 50 (half-width
about ±10 points). Ordering is by the interval's **upper bound**, ascending, with tiebreak
`(upper, −n, asset_id)`: this is the conservative rule and it punishes low n, not high n
(0 of 50 ranks below 10 of 1000). Below the show floor the display is "k non-successes in n
calls" — a count is honest at n = 1.

Stratify by harness × model × task category; day pools over a display window; `effort`,
`permission_mode` and `entrypoint_class` are pooled with a visible caption in v1 (recorded, so a
later version can stratify). The display shows the stratum matching the stated task and lists
other strata as context, never merged into it. Unknowns live in a separate "not enough evidence
yet" list, sorted by n descending with "needs N more", never interleaved with the ranked list:
insufficient evidence is a state, not a low rank.

Cost is derived at display time from tokens, the model id and `prices.json` (dated); no money
figure is stored or transmitted and there is no "saved" figure.

**Scope: single-machine.** Consent is laptop-tier; #957 ruled every signal row tenant-scoped;
the personal claim ("in your runs, this was observed") is the one that is safe by construction.
The empty state is therefore most of the product and is designed as such above. Org aggregation
across an enrolled org's machines is the fleet-tier feature and would put aggregation privacy
(minimum cohort, no per-device rows in org views) on #965's critical path; public cross-org
aggregation is out either way. Consequence stated plainly in [`passive-observer-scope-965.md`](passive-observer-scope-965.md): single-machine data
cannot retire the public #916/#917 proxies; v1 supplements them on the user's own view.

## 3. Signals

| Signal | Tier | Claude Code | Codex | Informs |
|---|---|---|---|---|
| Observed non-success rate, per asset | Direct | `tool_result.is_error` paired by id; denial phrase separated | `Failed` status / `success:false`; approval decisions | Which asset to prefer for the task, with an interval |
| Turns per session; non-convergence indicator | Run | user prompt lines; repeated `(name, input hash)` triples ≥ 3, hash discarded | user messages; same rule | Whether a loaded set correlates with churn (never "turns to completion") |
| Token totals by bucket, model id | Run | four buckets + thinking, deduplicated by `message.id` | cumulative `token_count` deltas; cached-input and reasoning buckets | Cost rendering at display time; D5 model stratum |
| Session duration | local only | first/last harness timestamp | same | Not egressed (D2); used locally for truncation detection |
| Per-invocation latency | Direct | result timestamp − call timestamp; spawns excluded | output timestamp − call timestamp | Responsiveness, as mergeable stats |
| Invocation count and recency | Direct | counts; recency at day resolution | same | Usage frequency, observed (the usage rate no artifact carries) |
| Loaded-set context cost | Loaded | listing-line bytes ÷ 4 for skills (lazy body), in-band rules bytes, deferred tool-schema bytes | filesystem estimate | Portfolio footprint, tagged with method |
| Co-occurrence and collision | Loaded | `bom[]` membership; overlap from vettd's own metadata | same | Compatibility, server-side join |

## 4. The field list

`telemetry-field-gate.json` is the deliverable; `telemetry-envelope.schema.json` is its schema.
Posture: counts, observed rates derived server-side from counts, relative durations as mergeable statistics, token
totals by bucket, allowlisted model identifiers, asset hashes and pseudonyms, a coarse task
category, and bookkeeping (versions, device id, harness id and semver, run pseudonym, loaded-set
hash, tier, binding, evidence basis, coverage). Coverage fields (`sessions_seen`,
`lines_unknown_type`, `truncated_sessions`, `cursor_state`, …) exist so a collector that died a
week ago never looks like one that observed nothing worth reporting.

## 5. Prototype

`prototype/observe.py` reads real local session state for one harness, extracts the signal set,
attributes, aggregates into the envelope, **refuses to write the payload if the gate check
fails**, writes it, and prints the ranked list with `evidence_state` and tier per row. It posts
nothing. Run:

```
python3 prototype/observe.py --harness claude_code --task "<stated task>" \
  --secret-file <local secret> --out worked-example/observations.example.json --scrub --synthetic-demo
python3 prototype/check_field_gate.py worked-example/observations.example.json \
  --dynamic worked-example/observations.example.json.dynamic.json
python3 -m unittest discover -s prototype/tests -p 'test_*.py'
python3 prototype/lint_copy.py README.md SCOPE-965.md worked-example/ranking.example.txt
```

What the run produced, on the machine that produced this spike (Claude Code 2.1.258, one main
session plus its 19 sub-agent transcripts, read while the session was still being written):

- Tests: `python3 -m unittest discover prototype/tests` — 183 tests, 0 failures, 0 skipped, in
  about 8 s (the 200 MB streaming test ran; free space was above the 1 GB floor).
- Gate: 0 violations against 85 allowed leaf paths on the real payload and on the synthetic one;
  each injected fault (unknown key at any depth, path, URL, loaded-set name or path component,
  second-resolution or colon-less timestamp, uuid outside `device_id`, bad enum, non-hex or
  wrong-length id, off-list model, bearer-like or AWS-style value built at runtime, epoch in a
  count, duplicate JSON key, non-identifier key) makes the checker exit non-zero.
- Determinism: two consecutive runs with the same secret and day produced byte-identical
  payloads.
- Copy lint: 0 findings over this answer, the scope note and the printed ranking.
- Prose check: the gate's dynamic forbid sets (names, ids, paths, username, hostname, home
  directory) run over the same three documents with the four public connector names and the one
  invoked built-in skill allowlisted: 0 findings.

An adversarial review (four lenses: leaks, parser correctness against the raw files, statistics,
gate bypasses) ran against the first working build and changed it. What it found and what
changed: a `run` skill was a substring of the enum literal `truncated` (closed-enum fields are now
exempt from the substring rule); harness versions accepted prerelease and build text that can
carry a hostname (plain `MAJOR.MINOR.PATCH` only now); a loaded-set change produced one record per
segment and duplicated run totals (one record per run now); cursors were committed before the
gate check (after the write now); skill listing bytes matched by substring (exact `- name:` now);
token dedupe kept the first, smallest line of a streamed response (fullest now); workflow-spawned
sub-agents one directory deeper were never discovered (found now); the in-band skill body is the
*next* meta line, not the tail of the command line (hashed correctly now); harness-injected
notification lines counted as a person's turns (excluded now); MCP corroboration markers were
ignored (counted now); a bare "rejected" in any error text counted as a user denial (denial
phrases must name the user now); permission mode took the first line's value (most frequent now);
the checker echoed unknown key names, accepted duplicate JSON keys and non-identifier keys,
missed colon-less ISO times, IPv4, localhost, non-listed TLDs and AWS-style keys, and let a path
component through if the whole path was not present (all fixed); the ranking pooled categories
silently for an unspecified task, filtered the model stratum by the dominant model only,
floor-divided means, priced runs with no usage evidence at zero, crashed on a record with more
non-successes than calls, and counted loaded-but-uninvoked runs in "over N runs" (all fixed).
Accepted and documented rather than fixed: a live session's `run_outcome` depends on when it is
read (`truncated` while the file is still growing); a locally installed asset whose name is a
substring of a constant version string blocks emission until renamed (fail-closed by design);
Windows file-share semantics need the Rust test in #965.

## 6. Worked example

The author's own session (the one that produced this spike) is the input; it is the only real
session state on the machine. Names are scrubbed to `<type>:<asset_id prefix>` except the four
public, harness-provided MCP connectors and the one built-in skill that was invoked, which are
allowed through the `--public-names` allowlist. Every number below is what the harness log
recorded; every row is `inferred` because the prototype reads history and hashes the filesystem
as it is now.

Payload facts (`worked-example/observations.example.json`, 34 KB, gate-clean):

- 1 run record, `observed_day` 2026-09-02, `run_outcome` `truncated` (the session was
  live when read), `permission_mode` `plan`, `effort` `xhigh`, `entrypoint_class`
  `remote`, `task_category` `shell_ops` from the tool-mix rule set, `loaded_set_basis`
  `harness_log`, `loaded_set_changes` 0 (the two deferred-tool deltas were async MCP
  connects and folded into one segment under the settle rule).
- Counts: 2 person turns, 822 tool calls across the tree, 20 flagged errors, 0 user denials,
  19 sub-agent runs, 1 unpaired call (the in-flight one), 43 repeated near-identical calls.
- Tokens, deduplicated by response id and split by model because sub-agents ran on a different
  model: claude-fable-5-1: input 67,523, cache creation 4,440,238, cache read 72,679,594, output 270,901, thinking 87,049; claude-opus-5: input 244, cache creation 783,124, cache read 14,037,593, output 865, thinking 0.
- 58 assets in the loaded set: 49 skills (34 with a local directory, 1 hashed exactly from its
  in-band body, 15 name pseudonyms), 7 MCP connectors (all remote, no local descriptor, name
  pseudonyms), 2 rules files (hashed exactly from in-band content). 5 assets were invoked;
  the rest are loaded-only. The busiest connector's 135 calls are corroborated by 172
  harness-native attribution markers.
- Coverage: 1 session seen, 1 emitted, 2,635 lines read, 383 of an unconsumed type,
  1 truncated session, cursor state `fresh`, run pseudonyms keyed by a test secret.

Printed ranking (verbatim):

```
wrote worked-example/observations.example.json (34409 bytes, sha256 7e113ec3fdfab9cf...)
gate: OK (85 allowed leaf paths, 0 violations)
Observed asset evidence for task: close spike 828: verify the repo, decide the five questions, build the observer prototype
Stratum: harness=claude_code model=all task_category=shell_ops (1 runs over 1 observed days)
The task category was read from the stated task with a keyword table; other categories are listed as context, never merged in.
Pooled in this view (recorded, not stratified): effort, permission_mode, entrypoint_class, day.
Models pooled in this view: claude-fable-5-1 (1 runs), claude-opus-5 (1 runs)
Ranked by the upper bound of the 95% interval on the observed non-success rate, ascending (n >= 50 calls):
  1. mcp_server:github  tier=inferred state=observed  10 non-successes in 135 calls (95% interval 4.1%–13.1%) over 1 runs; latency mean 930 ms in 135 paired calls (observed)
Not enough evidence yet (sorted by calls seen; never interleaved with the ranked list):
  -  mcp_server:Notion  tier=inferred state=insufficient_evidence  1 non-successes in 15 calls; needs 5 more calls for an interval; latency mean 1447 ms in 15 paired calls (observed)
  -  mcp_server:Google_Drive  tier=inferred state=insufficient_evidence  0 non-successes in 3 calls; needs 17 more calls for an interval; latency insufficient_evidence (3 paired calls)
  -  mcp_server:Gmail  tier=inferred state=insufficient_evidence  0 non-successes in 1 calls; needs 19 more calls for an interval; latency insufficient_evidence (1 paired calls)
  -  skill:workflow-authoring  tier=inferred state=insufficient_evidence  0 non-successes in 1 calls; needs 19 more calls for an interval; latency insufficient_evidence (0 paired calls)
Loaded in these runs but never invoked (51 assets: 3 mcp_server, 48 skill): no invocation evidence; listed in the payload, not ranked.
Loaded-only assets (rules files, prompts): context-cost estimate only, no non-success figure applies:
  -  rules_file:336cc4fbf19b  tier=inferred state=observed  context cost est. 2 tokens (file_bytes_div4) in 1 runs
  -  rules_file:6495ed93579c  tier=inferred state=observed  context cost est. 1497 tokens (file_bytes_div4) in 1 runs
Cost (display-time derivation, not stored), from tokens in this stratum and the price table dated 2026-09-02:
  claude-fable-5-1: USD 213.60 over 1 runs
  claude-opus-5: USD 35.81 over 1 runs
Every figure above is an observation from harness logs on this machine, not a causal claim.
```

Reading it: the busiest connector has enough calls for an interval and is the only ranked row;
the second busiest has 15 calls and is told it needs 5 more before an observed rate is shown at
all; the invoked skill has one call and no latency because a skill injection is not a tool round
trip; 48 skills and 3 connectors were loaded and never touched, and they are one line, not
fifty. The cost lines are display-time derivations from the per-model token buckets and the
dated placeholder price table.

The labelled synthetic run (invented counts, written to a separate file) shows the populated
layout once:

```
==============================================================================
SYNTHETIC DEMO — invented counts, not observations; shown only so the populated
ranking layout can be seen. Written to worked-example/observations.example.json.synthetic.json
==============================================================================
Observed asset evidence for task: close spike 828: verify the repo, decide the five questions, build the observer prototype
Stratum: harness=claude_code model=all task_category=shell_ops (40 runs over 28 observed days)
The task category was read from the stated task with a keyword table; other categories are listed as context, never merged in.
Pooled in this view (recorded, not stratified): effort, permission_mode, entrypoint_class, day.
Ranked by the upper bound of the 95% interval on the observed non-success rate, ascending (n >= 50 calls):
  1. mcp_server:SYNTHETIC-server-alpha  tier=inferred state=observed  16 non-successes in 80 calls (95% interval 12.7%–30.0%) over 40 runs; latency mean 1047 ms in 80 paired calls (observed)
  2. mcp_server:SYNTHETIC-server-beta  tier=inferred state=observed  48 non-successes in 80 calls (95% interval 49.0%–70.0%) over 40 runs; latency mean 1547 ms in 80 paired calls (observed)
Not enough evidence yet (sorted by calls seen; never interleaved with the ranked list):
  -  mcp_server:SYNTHETIC-server-delta  tier=inferred state=insufficient_evidence  3 non-successes in 16 calls; needs 4 more calls for an interval; latency mean 2341 ms in 16 paired calls (observed)
  -  skill:SYNTHETIC-skill-gamma  tier=inferred state=insufficient_evidence  0 non-successes in 5 calls; needs 15 more calls for an interval; latency mean 420 ms in 5 paired calls (observed)
Loaded-only assets (rules files, prompts): context-cost estimate only, no non-success figure applies:
  -  rules_file:SYNTHETIC-rules-epsilon  tier=inferred state=observed  context cost est. 48000 tokens (file_bytes_div4) in 40 runs
Context, other task categories in this harness (not merged): mixed 20 runs
Cost (display-time derivation, not stored), from tokens in this stratum and the price table dated 2026-09-02:
  claude-sonnet-5: USD 1.02 over 40 runs
Every figure above is an observation from harness logs on this machine, not a causal claim.
```

## 7. Hard constraints and what the tests actually show

| Constraint | Test | Shows | Cannot show |
|---|---|---|---|
| Raw content never leaves | gate check on the written payload; sentinel content planted in every content position of the fixtures never appears in parsed facts; violation messages never echo a value or a key name | The payload has only allowlisted leaf paths and no forbidden values; parsing discards content | A future harness line type that carries content under an allowed key |
| Deterministic, versioned extraction | two runs → byte-identical payload; `extractor_version` on every payload | Same inputs, secret and day give the same bytes | Determinism across float formats (the envelope has no floats by design) |
| Non-blocking, fail-open | rename-while-open; kill mid-read with atomic cursor commits then resume equals a single pass; bounded memory on a 200 MB file; byte-offset resume with a partial trailing line; disk cap on the cursor store; unchanged groups emit nothing; a changed main or child rebuilds the cumulative run | POSIX inode semantics; atomic resume; streaming reads; per-file cursor orchestration | Windows share modes (needs the Rust `share_mode` test in #965); macOS; WAL, which is not applicable because both harnesses are file-based |
| Opt-in, visible silence | `coverage` block; `run_id_basis`, `device_id_source` | Silence is distinguishable from nothing observed | Anything about the production opt-in, which does not exist yet |
| Tier, count, evidence class on every signal | schema `required`; gate `required` | Every row carries them | — |
| No causal claims | `lint_copy.py` over the docs and the rank templates | The strings in this directory | Copy written elsewhere |
| Cost derived, never stored | schema has no cost field; `prices.json` is dated | — | — |

## 8. Splitrail

Nothing is taken from Splitrail in this prototype. If #965 takes `src/analyzers/claude_code.rs`
or `codex_cli.rs` by file: pin the v3.7.2 commit, keep the MIT notice, exclude the uploader and
the history reader, and say so in the fork's README — and know that the parse stage folds to
per-message counts, so only the structs and discovery globs are worth taking.

## 9. Where the repo contradicts the brief

| Brief | Repo |
|---|---|
| "no tokio" | `tokio 1.53` is in `Cargo.lock` via the `httpmock` dev-dependency; none in the binary |
| "MCP servers are identified by name plus command" | `{name}-{sha256(config_path)[..12]}` — name plus the *config file path* |
| "a guard against non-local endpoints" | The guard blocks public *non-production* hosts; production and saved config always pass |
| Consent resolves "before any log or user file is read" | True on the interactive path; on `--submit` the log-reading builder runs before the disclosure prints (standing consent) |
| `scan_mcp_logs` does "capped tail reads" | Whole file read into memory, last 32 KiB kept |
| "OpenClaw: JSONL sessions, archives by renaming" | Docs: runtime sessions in a per-agent SQLite file; `sessions/` holds archives and migration sources; rename not documented |
| "Hermes: SQLite, WAL, multi-writer" | Confirmed, and it also stores native cost columns (which the allowlist forbids) |
| Splitrail "emission layer produces aggregates" | Also the parse layer: per-message counts, no tool ids or errors |
| Claude Code "every line carries cwd and gitBranch" | Confirmed — and it also carries the loaded set, deltas, and sub-agent attribution fields, which change D4 |
| Customer evidence from `--submit` inventories | Inventories cannot evidence Codex; no detector knows `~/.codex` |
| README promise "network activity only when you explicitly opt into a submission flow" | Holds; but the README's submission list omits the log-derived network-evidence category the disclosure code transmits, and its Privacy section never mentions log reading |

## 10. Surfaced and kept out of the design

Enforcement and anything that can block an agent; public aggregation across organisations;
eBPF; any telemetry-store migration; the eval engine; fleet-tier consent and attestation. Two
adjacent defects noticed in passing, not acted on: `contract-drift-check.yml` compares version
strings, not content (#881 already records this), and the README's submission list is
incomplete as above.

## 11. Deferred, with triggers

| Deferred | Until |
|---|---|
| Cursor parser | A documented transcript export exists *and* the inventory query shows Cursor dominating |
| Codex mid-session loaded-set changes | A resident collector is accepted as an operational surface (not v1) |
| Project pseudonym | A stratification need `bom_version` cannot serve |
| Per-asset token attribution beyond sub-agents | A harness writes per-turn attribution (Claude Code's sub-agent fields are the first sign) |
| Org aggregation | Fleet tier: minimum cohort size and org consent designed first |
| Retiring the public #916/#917 proxies | The fleet-tier aggregate exists |
| Stratifying by `effort`/`permission_mode`/`entrypoint_class` | Enough runs that pooling with a caption is no longer honest |

## 12. Verification notes

- Rust facts: `Cargo.toml`, `Cargo.lock`, `crates/vettd-cli/src/{contract/disclosure.rs,
  submit.rs, network.rs, output.rs, cli.rs, network_evidence.rs, contract/helpers.rs,
  contract/mcp.rs, scan_cache.rs, scan_refresh.rs, identity.rs}`, `scanner-field-gate.json`,
  `scripts/check-scanner-field-gate.sh`, `.github/workflows/ci.yml`, `README.md`.
- Cloud facts: `vettd` `hub/signals` — `apps/web/app/api/signals/ingest/route.ts`,
  `apps/web/proxy.ts`, `packages/api/src/rate-limit/policy.ts`, `packages/db/prisma/schema.prisma`,
  `docs/spikes/*.md`.
- Harness facts: real Claude Code 2.1.258 session and sub-agent files on the machine that
  produced this spike; `openai/codex` `codex-rs/protocol/src/{protocol.rs, items.rs, tool_name.rs}`
  and `codex-rs/codex-mcp/src/tools.rs`; Splitrail v3.7.2 `Cargo.toml`, `src/analyzers/{mod.rs,
  claude_code.rs, codex_cli.rs}`, `src/types.rs`, `src/config.rs`; OpenClaw and Hermes public
  docs; crates.io metadata for `opentelemetry-otlp`.
- Not verified: Codex on a real rollout file; Claude Code compaction (`summary`) lines and
  denial text (none in the sample); Windows file sharing; any Cursor, Hermes or OpenClaw file.

## 13. Provenance and score eligibility (vettd#795, vettd#797)

Added 2026-09-03, when the CLI implementation landed. The spike predates the provenance work, so
this section says how an observation record answers the two issues that govern whether evidence
may affect a public score. It is a **ruling to be confirmed by the owner**, not a claim that the
issues are closed — both are open, and their acceptance criteria are quoted verbatim below.

### What an observation record says about its own provenance

| Dimension (#795) | For an observation record | Field |
| --- | --- | --- |
| Execution locus | The customer's own machine. Not hosted, not BYOC — a developer laptop running the harness. | `resource.device_id` (per-device, HMAC-free scanner uuid), `resource.harness` |
| Key provenance | **Customer-supplied.** The runs observed were driven by the user's own harness credentials. Vettd supplies no key and runs no model; the CLI reads logs after the fact. | Implicit in the locus; there is no Vettd-key path to observation |
| Method / scanner version | The extraction rules, the egress allowlist, and the derivation basis for every asset key and token total. | `extractor_version`, `gate_version`, `envelope_version`, plus per-asset `key_basis` and `binding`, and `tokens.basis` |

Method provenance is deliberately more granular here than #795 asks. A single "scanner version"
cannot distinguish an asset keyed by content hash from one keyed by a name pseudonym, or a token
total read from a provider's own accounting from one estimated locally — and those distinctions
decide how much weight a downstream consumer may put on the number. `key_basis`, `binding` and
`tokens.basis` carry them per record.

### Eligibility ruling

Under #797's line — "cloud-verified or Vettd-key results may be score-bearing; customer-key or
self-reported results remain provenance-labeled context" — an observation record is
**customer-key** and therefore **provenance-labelled context, never score-bearing**. Concretely:

- Labelled `source = vettd-cli`, `sourceClass = logs`, `derivation = inferred`, tenant-scoped to
  the submitting `userId`.
- Visible on **that user's own view only**. It does not enter a public grade, a directory tile, or
  the #916/#917 proxies.
- Display-gated by the evidence-state floors: a rate is shown from `sampleSize >= 20` and ordered
  by from `>= 50`. One developer's runs are not a population, and the floors are what stop the
  interface from implying otherwise.
- Enters `AssetSignal` only through child 7's projection, which is a separate piece of work with
  its own review — not as a side effect of this route existing.

This is the conservative reading, and it is the right one for a v1: a public score that moved
because one developer's laptop reported something would be indefensible to an auditor and trivial
to attack. Nothing here forecloses a fleet-tier aggregate later; that needs a different consent
model and a minimum cohort size, and is out of scope by the "What v1 does not do" section above.

### Acceptance criteria, quoted

[vettd#795 — *Execution locus, key provenance, and method provenance on evidence records*](https://github.com/AgenticHighway/vettd/issues/795)
(open; fetched 2026-09-03):

> - [ ] Evidence records persist execution locus, key provenance, and method version
> - [ ] Values populated for all current production scan paths (hosted suite scans)
> - [ ] Fields exposed in evidence-bearing API responses

The third criterion is a cloud concern. The first two are what the CLI can satisfy from its side:
the fields above are on every record the CLI emits, and the schema requires them. Note the second
criterion's scope — "all current production scan paths (hosted suite scans)" — does not name the
observation path, because #795 predates it; whether observation records must carry the same field
*names* as hosted scan evidence, or map onto them at the boundary, is the owner's call.

[vettd#797 — *Score-bearing eligibility rules (methodology-versioned)*](https://github.com/AgenticHighway/vettd/issues/797)
(open; depends on #795; fetched 2026-09-03):

> - [ ] Written eligibility policy, versioned, published on the methodology page
> - [ ] Score computation reads eligibility class; non-eligible evidence visibly excluded from the grade
> - [ ] Closed spec issues #542/#533/#544 reviewed for reusable scoring concepts (cite, don't rebuild)

All three are cloud-side. The ruling above is this record's input to the first: it states which
class observation evidence falls into and why, so the written policy has something concrete to
codify rather than deriving it after the fact.
