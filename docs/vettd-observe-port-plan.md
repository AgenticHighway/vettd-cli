> Status: PLAN — written 2026-09-03 by the planning session for AgenticHighway/vettd-cli#250. Executed on branch `claude/vettd-cli-250-product-ready-tb6w2w` in both repos.
>
> ## Executed: phases 0-8 landed (2026-09-03)
>
> Kept per the repo convention for plan docs. What shipped, and where it differs from what is
> written below:
>
> | Phase | Landed | Note |
> | --- | --- | --- |
> | 0 Branch, baseline, artifact promotion | Yes | `cargo deny` was already failing at the branch point (RUSTSEC-2026-0258 via `httpmock`); fixed as its own commit. |
> | 1 Gate, disclosure, secret, CI script | Yes | Gate script steps 4-5 deferred to phase 6 as planned; they need the built binary. |
> | 2 Source, Claude Code reader, goldens | Yes | The Windows share-mode test runs in CI on a Windows runner (the plan's default ruling). |
> | 3 Extract, FsIndex, attribute, taskcat | Yes | |
> | 4 Envelope, canonical bytes, golden parity | Yes | `extractor_version` is `1+1`, not `proto-0.1.0+taskcat-1` — see below. |
> | 5 Rank, render, copy lint | Yes | |
> | 6 `vettd observe` command | Yes | `--out` is `Option<PathBuf>` with `default_missing_value`, not `Option<Option<PathBuf>>`. |
> | 7 Submit with ledger | Yes | `--resend` correction, recorded at step 9 of the pipeline section. |
> | 8 Docs and spike disposition | Yes | `git grep spikes/828` still hits ~40 source doc comments; see below. |
> | 9 Cloud route in `vettd` | Pending | |
>
> Three deliberate deviations, each argued where it lives:
>
> 1. **`extractor_version` is `1+1`.** Every string leaf is substring-checked against the machine's
>    own asset names, so a long producer-controlled string is a large fail-closed collision surface
>    for no benefit. One related issue is open and escalated rather than fixed: `harness_version`
>    defaults to `"unknown"`, whose substrings include `now`, `own` and `know`, so a machine with a
>    three-letter asset name of that shape is refused. Closing it means exempting
>    producer-controlled leaves from the dynamic rule, as the gate already exempts closed enums —
>    a gate contract change, and the owner's call.
> 2. **`--resend` bypasses the cursor probe**, not only the ledger. See the correction at step 9.
> 3. **Source doc comments keep their `spikes/828-passive-observer/prototype/…` citations.** This
>    plan expected `git grep spikes/828` to hit only the docs. Rewriting ~40 comments across 35
>    files would have churned the diff for a cosmetic grep result and lost the one useful thing
>    those citations carry: which Python function each Rust one came from. `observe/mod.rs`
>    instead carries a single note saying the directory was deleted and how to retrieve any file
>    from git — an instruction that is identical for every citation, and that a per-file rewrite
>    could not have carried, since it needs the deleting commit's sha.
>
> One factual error in this plan, corrected in place where it appears: the claim that canonical
> JSON leaves `0x7f` raw. Under `ensure_ascii=True` CPython's `ESCAPE_ASCII` is `([\\"]|[^ -~])`,
> so only `0x20`-`0x7e` survive and DEL is escaped as `\u007f`.

# Make vettd-cli#250 product-ready: port the passive observer to Rust (`vettd observe`) and land its cloud ingest route

## Context

PR [AgenticHighway/vettd-cli#250](https://github.com/AgenticHighway/vettd-cli/pull/250) answers spike vettd#828 with a ~10.5k-line **throwaway Python prototype** under `spikes/828-passive-observer/`. It reads local Claude Code session logs, projects every line to hashes/counts, attributes invocations to assets by content hash, aggregates one privacy-preserving record per run, gate-checks the payload against an 85-leaf-path egress allowlist (`telemetry-field-gate.json`), and prints a ranked, confidence-tagged asset list. Its own scope note (`SCOPE-965.md`) says production must be a Rust port under `crates/**` so CI gates it; today the three Rust CI jobs report *skipped* on this PR because `paths-filter` only watches `crates/**`, and the 183 Python tests are green only when run by hand.

The owner wants the PR product-ready before merge. Executor: an Opus-class agent in a fresh session with no access to the planning conversation. This plan is self-contained; when it says "port X", the executor reads the named Python file on the branch. This file is committed on both designated branches as `docs/vettd-observe-port-plan.md`; the vettd-cli copy is canonical and stays in the repo (the repo keeps plan docs such as `docs/performance-scan-plan.md`); the vettd copy is deleted before the vettd PR opens.

## Rulings (final, from the owner — do not relitigate)

| # | Decision | Consequence |
|---|---|---|
| 1 | **Rust port** as `vettd observe` in `crates/vettd-cli` | Python prototype is the reference semantics; Rust is the product |
| 2 | **Claude Code only** in v1 | `Source` trait stays harness-neutral; `sources/codex.py` and `~/.codex` detectors are NOT ported (follow-up) |
| 3 | **Cloud route included**: `POST /api/observations/ingest` in `vettd` | Built on `dev` (not `main`, not `hub/signals`); SCOPE-965 child 6 only; child 7 + `harnessId` on `AssetSignal` are follow-ups |
| 4 | **Delete the prototype** once Rust parity is proven; promote gate + schema to repo root; keep the two decision documents under `docs/` | Add the missing AC5 (vettd#795/#797 provenance) section the PR review flagged |

## Branch and PR mechanics (fixed)

**vettd-cli** — branch `claude/vettd-cli-250-product-ready-tb6w2w`. **Already prepared by the planning session**: fast-forwarded onto PR #250's head (`6328b63` = `main` dbd3872 + 2 spike commits) plus one commit adding this plan. Verify, do not redo:

```bash
cd /home/user/vettd-cli && git fetch origin && git checkout claude/vettd-cli-250-product-ready-tb6w2w
git log --oneline -4          # expect: <plan commit>, 6328b63 fix(spike)…, 4ec8c96 spike(#828)…, dbd3872 Merge pull request #246…
```
PR targets `main` and supersedes #250 (GitHub cannot re-point #250's head branch; close #250 with a pointer when the new PR opens). Commit subjects < 100 chars, Conventional Commits.

**vettd** — same branch name. `AssetSignal` and the signals module exist only from `dev` (36ce5b7, 68 ahead of main); the `/api/signals/ingest` pattern exists only on `hub/signals` (66 further, unmerged). **Already prepared by the planning session**: the branch was reset onto `origin/dev` (it previously held only harness-created history at `main`) plus one commit adding this plan. Verify, do not redo:

```bash
cd /home/user/vettd && git fetch origin && git checkout claude/vettd-cli-250-product-ready-tb6w2w
git log --oneline -2          # expect: <plan commit>, 36ce5b7 (origin/dev head at planning time)
```
PR targets `dev` (vettd's feature→dev rule). Never push to any other branch. Do not open PRs unless asked.

## Validation gates the executor must pass before every push

- vettd-cli: `cargo fmt --check && scripts/check-scanner-field-gate.sh && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && scripts/check-telemetry-field-gate.sh` (mirrors the `ci.yml` `check` job order once the new step is added). Toolchain is pinned `1.85.1`; any new crate must build there; `cargo deny check` must stay clean.
- vettd: `pnpm lint && pnpm typecheck && pnpm test` from repo root; Docker smoke of the new route per AGENTS.md "Quick local flow"; Prisma migration applied with `prisma migrate deploy` (never `migrate dev`/`reset`; never `--shadow-database-url` against a live DB — AGENTS.md gotchas 17/18).

## Reference semantics: where the truth lives

The Python prototype stays on the branch until the final disposition commit, so the executor reads it directly. Authoritative order when documents disagree: **code > tests > `prototype/CONTRACTS.md` > README §5**. Known drift the port must NOT copy from CONTRACTS.md: `run_id` is `HMAC(secret, "<harness>:<session_key>")` with **no segment index** (`aggregate.py:78-85`); there is **one record per run** with `counts.loaded_set_changes = segments-1`; the denial regex is `doesn't want to proceed|rejected by the user|denied by the user|permission (?:was )?denied|Request interrupted by user` (`claude_code.py:48`).

| Python (under `spikes/828-passive-observer/`) | What the Rust port takes from it |
|---|---|
| `sources/base.py` | data model, `iter_lines` byte-cursor contract (never yields a partial trailing line; offset = one past `\n`) |
| `sources/claude_code.py` | discovery (`os.listdir` + suffix match on `.jsonl`/`.ndjson`; children under `<stem>/subagents/agent-*` and one level deeper under `subagents/workflows/<id>/`; `.meta.json` → `agentType`,`toolUseId`,`spawnDepth`), `_project` key allowlist, pairing, attachments (`skill_listing`, `deferred_tools_delta`, `agent_listing_delta`, `mcp_instructions_delta`, `nested_memory`), in-band skill body (`<command-name>` + `<skill-format>true` next-meta-line rule), denial regex, truncation (`abs(now−mtime) ≤ 120 s` and last `stop_reason ≠ end_turn`), forbids buckets, `attributionAgent`/`attributionMcpServer` corroboration |
| `extract.py` | tool-class tables (`EDIT={Edit,Write,MultiEdit,NotebookEdit,apply_patch}`, `READ={Read,Glob,Grep,LS,WebFetch,WebSearch}`, `SHELL={Bash,shell,exec}`, mcp first), `REPEAT_THRESHOLD=3` (members counted), `run_outcome` order truncated→compacted→interrupted→completed→unknown, entrypoint/permission/effort maps, tree merge, `dedupe_usages` (largest `output_tokens` wins across the tree; ties keep parent-first order), `dominant_model` `(-count, name)` then allowlist |
| `attribute.py` | `FsIndex`, tree hash, descriptor canonicalisation (`{args, command, env_names, transport}`), `SECRET_FLAGS`, settle rule (`folds` iff no removed/readded/skills/agents/rules and every added tool's server ∈ prior pending), `_key_for` precedence (in-band → local file → descriptor → name), `_mtime_binding` (strict `<`), context-cost `//4` rules, builtin agent types |
| `taskcat.py` | `RULES_VERSION="taskcat-1"`; total 0 → unspecified; mcp≥0.5 → edit≥0.25 → shell≥0.5 → read≥0.5 → mixed; `KNOWN_MODELS` == gate `enums.model` |
| `aggregate.py` | envelope layout, sort keys `(observed_day, run_id)` / `asset_id` / `bom_version`, `Stats` merge, `to_json_bytes`, `collect_dynamic` |
| `check_field_gate.py` | walker + value rules (see "Field gate") |
| `rank.py` | Wilson (z=1.96), `FLOORS {count:1,tokens:3,latency:5,rate_show:20,rate_order:50}`, stratum/pooling, ordering `(hi, -n, asset_id)`, `COPY` templates verbatim (U+2013 en dash in intervals), cost lines |
| `cursor_store.py`, `observe.py` | control flow (probe-then-full-reread for changed groups; failed child abandons the group; gate check before any write) |
| `tests/*.py` | the invariant list to re-express in Rust (see "Tests") |
| `worked-example/*` | `observations.example.json` is a real, gate-clean sample; `ranking.example.txt` is the float-formatting baseline (`4.1%–13.1%`, `USD 213.60`) |

Three confirmed prototype defects to fix in the port, not replicate: (1) `_permission_modes` scratch state is stored inside `facts.forbids` and leaks into the gate's dynamic sets (`claude_code.py:376-382`) — keep `mode_counts` as a real field; (2) the per-file "keep fullest usage" branch is unreachable (`claude_code.py:395-403`) — per file first-line-per-`message.id` wins, tree-wide `dedupe_usages` is the real rule; (3) permission-mode tie-break is alphabetical-smallest, not "earlier" as the docstring says — keep the code behaviour and say so in a comment.

## Architecture (Rust)

New module tree `crates/vettd-cli/src/observe/` (each file ≤ 400 lines, fns ≤ 50 lines per CONTRIBUTING.md:146; `mod observe;` added to `main.rs` alphabetically after `mod network_evidence;`):

```
observe/
  mod.rs               pub(crate) fn run(args:&ObserveArgs, action:Option<&ObserveSubcommand>, json:bool) -> i32; re-exports
  args.rs              clap ObserveArgs (flattened) + ObserveSubcommand { Enable, Status, Check{payload, dynamic} }
  types.rs             data types + closed-enum &str consts (mirror sources/base.py + model.py; BTreeMap/BTreeSet everywhere)
  source.rs            trait Source; LineReader (byte-cursor streaming, MAX_LINE_BYTES, #[cfg(windows)] share_mode); resume_offset
  claude_code/mod.rs   ClaudeCodeSource { now_ms: Option<i64> } impl Source; link_children
  claude_code/discover.rs   listdir + suffix walk incl. subagents/ and subagents/workflows/<wf>/; .meta.json
  claude_code/project.rs    key-allowlist projection (TOP_KEYS, message/usage/block/attachment/toolUseResult) + harvest_names
  claude_code/apply.rs      ReadState: pairing, usage dedupe, turns, env, attachments, in-band skill bodies, compactions
  extract.rs           SessionFacts tree -> RunFacts
  taskcat.rs           RULES_VERSION, categorize, KNOWN_MODELS, allowlist_model
  canonical.rs         canonical_json(&Value)->Result<String> (Python ensure_ascii byte-compatible), hex_sha256, hmac_sha256_hex
  attribute/mod.rs     attribute(run,&FsIndex,secret)->AttributedRun; key precedence; observations; context cost
  attribute/fs_index.rs     FsIndex (skills tree hash, agents, MCP descriptors), canonical_descriptor, strip_args, is_secret_shaped
  attribute/segments.rs     SegState, folds, settle, segment_for, bom_version
  envelope.rs          Stats, EnvelopeMeta, build_envelope, to_json_bytes, collect_dynamic, filter_records, version consts
  gate.rs              Gate (include_str!("../../../../telemetry-field-gate.json"), LazyLock), Dynamic, check(&Value,&Dynamic)->Vec<String>
  disclosure.rs        gate-category ↔ DisclosureCategory parity; render_observe_disclosure(destination)
  store.rs             SQLite ~/.vettd/observer/observer-v1.sqlite3: observer_meta / observer_cursors / observer_ledger
  rank.rs              wilson, evidence_state, task_category_for, AssetRow, RankResult, rank (pure)
  render.rs            COPY templates, render, prices (include_str!("../../resources/observe-prices.json"))
  lint_copy.rs         #[cfg(test)] port of lint_copy.py, used by render tests only
  pipeline.rs          orchestration (below)
  submit.rs            submit_envelope(bytes,&AuthConfig)->Result<SubmitOutcome,String>
```

`cli.rs`: one variant and one dispatch arm, following the existing `if let Commands::X {..} = &cmd { …; return; }` shape in `run()` (`cli.rs:897+`):

```rust
/// Observe local Claude Code sessions and report per-asset evidence (opt-in)
Observe {
    #[command(flatten)] args: crate::observe::ObserveArgs,
    #[command(subcommand)] action: Option<crate::observe::ObserveSubcommand>,
},
// in run():
if let Commands::Observe { args, action } = &cmd {
    std::process::exit(crate::observe::run(args, action.as_ref(), json));
}
```

### `Source` trait and reader

```rust
pub trait Source {
    fn harness(&self) -> &'static str;                                                     // "claude_code"
    fn discover(&self, root: &Path, window_days: u32, now_ms: i64) -> Result<Vec<SessionRef>, String>;
    fn read(&self, r: &SessionRef, cursor: Option<&Cursor>) -> Result<(SessionFacts, Cursor), String>;
}
```
`LineReader::open(path)`: `OpenOptions::new().read(true)` + `#[cfg(windows)] .share_mode(FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE = 0x1|0x2|0x4)` (constants defined locally); seek to `start`; `BufReader::read_until(b'\n')`; stops at the first buffer not ending in `\n` (partial line never yielded, offset not advanced past it); yields `(end_offset, Vec<u8>)`. `MAX_LINE_BYTES = 64 MiB`: a longer line is drained to its newline in chunks, counted as `parse_errors += 1`, offset advanced. `resume_offset(cursor, path, meta) -> u64`: 0 if no cursor, path differs, `byte_offset > len`, or `inode` is `Some` and differs (`#[cfg(unix)] MetadataExt::ino()`, `None` elsewhere); else `byte_offset`. Never read a whole session file into memory (`network_evidence::read_file_tail` does; do not copy it).

### Data types (`types.rs`)

Mirror `prototype/CONTRACTS.md`/`sources/base.py`/`model.py` field-for-field: `SessionRef{path, harness, session_key, kind: Main|Child, parent_key, child_meta: BTreeMap}`, `Cursor{path, byte_offset: u64, inode: Option<u64>}`, `ToolCall{tool_use_id, name, ts_ms, message_id, result_ts_ms, is_error, interrupted, is_async, failure_class, input_fingerprint, server, skill, agent_type, child_key}` (+ `paired()`, `latency_ms()` = `max(0, result−ts)`), `Usage{message_id, model, ts_ms, input_tokens, output_tokens, cache_creation, cache_read, cached_input, thinking, reasoning}`, `LoadedSetEvent{ts_ms, kind: Initial|Delta, skills, tool_names, agent_types, rules_files, pending_mcp, failed_mcp, removed, readded, listing_bytes, tool_schema_bytes}`, `InBandAsset{kind: RulesFile|SkillBody, name, content_sha256, byte_len, ts_ms}`, `SessionFacts{…, mode_counts: BTreeMap<String,u64> /* explicit field, never a forbids bucket */, forbids: BTreeMap<String,BTreeSet<String>>}`, `InvocationObs`, `RunFacts`, `AssetKey{asset_id, asset_type, key_basis, name, binding}`, `Segment`, `AssetObservation`, `AttributedRun{run, segments, observations: BTreeMap<usize,Vec<_>>, name_map}`, `Stats{n: u64, sum: i64, min: i64, max: i64, sumsq: u128}` with `from_values`/`merge` (associative + commutative). Consts: `FAILURE_CLASSES`, `RATE_BEARING_FAILURES=[tool_error,timeout]`, `ASSET_TYPES`, `DIRECT_CAPABLE_TYPES=[skill,mcp_server,agent]`, `BUILTIN_AGENT_TYPES=[Explore,Plan,general-purpose,claude,Bash,statusline-setup,claude-code-guide,output-style-setup]`.

Timestamps: `chrono::DateTime::parse_from_rfc3339` (accepts `Z`/`±HH:MM`), fallback naive `%Y-%m-%dT%H:%M:%S%.f` as UTC, else `None`; ms = `timestamp_millis()` (truncation like `microsecond // 1000`); `utc_day` = `%Y-%m-%d` of `ts_ms.div_euclid(1000)` in UTC, never local.

### Pipeline (`pipeline.rs`), in this order

1. `telemetry_enabled_from_config()` false → print the disclosure anyway, then guidance + exit **3**.
2. Render the data/source disclosure (stderr) — **before any session log or user file is opened**, on every path. A saved submit destination necessarily comes from user config, so print its resolved host immediately after that lookup and still before any session read.
3. Resolve `secret` (`identity::resolve_observer_secret`), `device_id` (`identity::resolve_scanner_uuid(None)`), `now_ms`, `today`, `root` (default `~/.claude`).
4. Open the store **only when `--submit`** (cursors + ledger are submit-mode state); clear both if `observer_meta.secret_fingerprint` ≠ sha256(secret).
5. `discover` → group mains with children (both sorted by path) → seed coverage (`sessions_seen`, `cursor_state = resumed|fresh`, `run_id_basis = device_secret`, or `test_secret` when `--secret-file` was given).
6. Per group — submit mode: if every file has a cursor, probe-read from cursors; unchanged → stage cursors, skip; changed → add probe counts to coverage **and** re-read from byte 0 (the deliberate double count, `test_observe.py:146`). Non-submit modes read every group from byte 0 and never touch cursors. Failures: main → `sessions_skipped_unparseable += 1`, skip; child → same and abandon the whole group's staged cursors.
7. `extract` → `attribute`; `sessions_emitted += 1`.
8. `build_envelope(runs, EnvelopeMeta{resource, coverage, today, secret, run_id_basis, extractor_version})` → `collect_dynamic(runs)` + `current_username` (`$USER`/`$USERNAME`), `hostname` (`hostname::get()`), `home_dir`.
9. Ledger filter (submit mode only): drop records whose `(run_id, endpoint_host, record_sha256)` is already in `observer_ledger` unless `--resend`; `filter_records` rebuilds `bom[]` from survivors; zero survivors → stderr `nothing new to send`, exit 0.
   - **Corrected 2026-09-03 during Phase 7**: `--resend` must bypass the step-6 cursor probe as well as this ledger check. As written, an unchanged group short-circuits at step 6 before a record exists, so `--resend` could never reach step 9 and would print `nothing new to send` — in exactly the case a user reaches for the flag. This document's own acceptance line ("Cross-repo", below) expects `--resend` to report `0 new, 0 replaced, 1 duplicate` on an unchanged machine, which is only reachable with the probe bypassed. The prototype has no `--resend`, so there was no parity to break. Cursors are still staged and still advance only after a 2xx.
10. `gate::check(&envelope, &dynamic)`; any violation → stderr `REFUSING TO WRITE: payload fails the telemetry field gate:` + one line per violation, exit **2**, nothing written, no state mutated, nothing sent.
11. Write `--out` (canonical bytes) when given or when `--dry-run` (default `vettd-observations.json`, deliberately matched by the repo's `vettd-*.json` gitignore so users never commit payloads); `--json` prints the canonical bytes to stdout instead of the report.
12. `rank`/`render` → stdout (unless `--json`); `wrote …` and `gate: OK (85 allowed leaf paths, 0 violations)` → stderr.
13. Submit (`--submit` without `--dry-run`): interactive → `wizard::confirm("Send N run record(s) to <host>?", false)`; non-interactive → proceed (config opt-in + configured auth = standing consent, the PR #228 model). On 2xx: **one SQLite transaction** upserting ledger rows for run_ids the server reported `accepted|duplicate|replaced` plus all staged cursors. Deliberate deviation from the prototype (which committed cursors after the local write): cursors advance only once the server holds the record, so a dry-run can never starve a later submit.

Where each SCOPE-965 CLI item lands: (1) Source trait + reader → `source.rs`, `claude_code/*`; (2) device secret → `identity.rs`; (3) disclosure + gate + CI script → `contract/disclosure.rs` (14 variants), `observe/disclosure.rs`, `observe/gate.rs`, `scripts/check-telemetry-field-gate.sh`; (4) cursors → `store.rs`; (5) opt-in + consent + `--dry-run` → `cli.rs` config, `pipeline.rs`; (6) submit + ledger → `observe/submit.rs`, `store.rs`; (7) Codex detectors → follow-up.

Design decisions the executor should not reopen:
- **Separate SQLite file**, not the scan cache: `scan_cache.rs` opens with plain `Connection::open` (no WAL, no busy_timeout, `scan_cache.rs:70-79`) and its `CACHE_SCHEMA_VERSION`/`CARGO_PKG_VERSION` orphaning semantics must not apply to cursors. `store.rs::open_at`: `create_dir_all`, `Connection::open`, `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA synchronous=NORMAL;`, then
  ```sql
  CREATE TABLE IF NOT EXISTS observer_meta    (key TEXT PRIMARY KEY, value TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS observer_cursors (path TEXT PRIMARY KEY, harness TEXT NOT NULL, byte_offset INTEGER NOT NULL, inode INTEGER, updated_at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS observer_ledger  (run_id TEXT NOT NULL, endpoint_host TEXT NOT NULL, harness TEXT NOT NULL, record_sha256 TEXT NOT NULL, emitted_day TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY (run_id, endpoint_host));
  ```
  API: `open_default()`, `open_at(&Path)`, `ensure_secret_fingerprint(&[u8])`, `load_cursor(&Path)`, `has_any_cursor()`, `ledger_has(run_id, host, sha)`, `commit(cursors, ledger_rows)` (one `transaction()`, `MAX_CURSOR_ROWS = 10_000` eviction by oldest `updated_at`). Corrupt DB on open → rename to `observer-v1.sqlite3.corrupt-<unix>` and recreate (fail-open like `cursor_store._load`). `updated_at` = `Utc::now().to_rfc3339()` as in `scan_cache.rs:143`.
- **Disclosure categories extend the existing enum** (SCOPE-965 §3; `contract/disclosure.rs` documents itself as "the single source of truth for what the disclosure can mention"): add the 14 variants `TelemetryBookkeeping, ObservationDay, DeviceIdentity, HarnessIdentity, ModelIdentity, RunShape, RunOutcomeCounts, RunTokenTotals, AssetIdentityHash, AssetLoadedSet, AssetOutcomeCounts, AssetTimingStats, AssetTokenStats, CoverageMetadata` with `label()`/`description()` arms copied verbatim from `telemetry-field-gate.json.disclosureCategories[]`. **No walker generalisation**: the gate walker already rejects any leaf outside `fields`, and every `fields[*].category` names a variant (unit test `every_gate_category_is_a_disclosure_variant` asserts name/label/description parity). `disclosure_categories(&ContractPayload)` and its tests are untouched.
- **Gate and prices are compiled in** via `include_str!` and parsed once (`LazyLock`). Repo-root `telemetry-field-gate.json`/`telemetry-envelope.schema.json` are the source of truth; `crates/vettd-cli/resources/observe-prices.json` is a display resource (`--prices FILE` overrides).
- **Secret** (`identity.rs`): generalise `persist_uuid` (`identity.rs:27-74`) into `persist_secret_bytes(path, field_name, bytes)` (same 0700 dir / 0600 file path; `#[cfg(not(unix))]` plain `fs::write`) and have `persist_uuid` call it. Add `default_observer_secret_path() → ~/.vettd/observer_secret` and `resolve_observer_secret(explicit: Option<&Path>) -> Result<(Vec<u8>, &'static str /*run_id_basis*/), String>`: explicit file → raw bytes, `< 16` → `Err("observer secret must hold at least 16 bytes")`, basis `test_secret`; else read or create 32 bytes via `getrandom::fill` and persist, basis `device_secret`; never regenerate an existing file. Dependencies: `getrandom = "0.4"` and `hmac = "0.12"` (both already in `Cargo.lock` at 0.4.2/0.12.1 → zero new lock entries; MIT/Apache). Do not derive the secret from `Uuid::new_v4()` bytes.
- **Version constants** (`envelope.rs`): `ENVELOPE_VERSION="0.1.0"`, `GATE_VERSION=1`, `EXTRACTOR_VERSION="1+taskcat-1"` — deliberately minimal alphabetic content because every free-string leaf is a dynamic-forbid collision surface (an installed skill named `vettd` would otherwise block emission against `"vettd-cli-…"`). `resource = {device_id: scanner_uuid, device_id_source: "scanner_uuid", harness: "claude_code", harness_version: semver_or_unknown(last non-unknown), collector: "vettd-cli", collector_version: CARGO_PKG_VERSION}`.
- **`FsIndex` descriptor sources**: `<root>/.claude.json`, then `dirs::home_dir()/.claude.json` (where Claude Code actually writes `mcpServers`; the prototype only looked under `<root>` and therefore never found a real descriptor), then `<root>/settings.json`; first-wins. Skills `<root>/skills/**/SKILL.md` (tree hash), agents `<root>/agents/*.md`.

## CLI surface

```
vettd observe [--harness claude_code] [--root DIR] [--task TEXT] [--window-days N=30] [--model ID]
              [--dry-run] [--out [FILE]] [--scrub] [--public-names FILE] [--prices FILE]
              [--submit [URL]] [--api-key KEY] [--allow-public-endpoint] [--resend]
              [--secret-file FILE] [--now-ms MS] [--today YYYY-MM-DD]      (hidden test hooks, #[arg(hide = true)])
vettd observe enable | status [--json] | check <payload.json> [--dynamic <json>]
```
- `--harness` value_parser fixed list (only `claude_code` today); `--task` optional (empty → `unspecified`, pooled view with the visible caption); `--out` is `Option<Option<PathBuf>>` (bare → `vettd-observations.json`).
- **Opt-in**: `~/.vettd/.vettd.toml` `[telemetry] enabled = true`, read by extending `AccessConfig` (`cli.rs:346-449`) with `telemetry_enabled: bool` — restructure the loader so `[telemetry]` is read even when `[access]` is absent; add `pub(crate) fn telemetry_enabled_from_config()` beside `search_beta_testing_from_config` (`cli.rs:447`). No env override (consent must be a file the user wrote). Per-user file only, never the cwd file (issue #198). `vettd observe enable`: append `"\n[telemetry]\nenabled = true\n"` when the file has no `[telemetry]` table (create if absent); if the table exists, print the path and the exact line to change (never rewrite a user-authored file).
- **Disclosure on every path, before any session file is opened**: `observe::disclosure::render_observe_disclosure(destination: Option<&str>)` mirrors `render_disclosure` (`disclosure.rs:490-518`): `  This observation will include:` + one `    • {label} — {description}` per category (all 14, structural) + `  Source: Claude Code session logs under <root>/projects (read-only; message text, paths, names and ids never leave this machine)` + blank line. stderr only; twin of `disclosure_rendering_does_not_write_to_stdout` (`cli.rs:2775`). The pipeline prints `  Destination: {host}` after resolving saved auth because the host cannot be known before reading that config, but still before any session read.
- **stdout/stderr** (AGENTS.md rule): report text or `--json` bytes on stdout; disclosure, `wrote …`, `gate: OK …`, retries, violations, submit status on stderr. The prototype printed the first two on stdout — do not copy.
- **Exit codes** (existing conventions): 0 ok / nothing new; 1 runtime error (bad root, write failure, submit failure after retries, 400/401 from the server); 2 gate violation or non-interactive missing input; 3 not configured (`[telemetry]` off, or no auth for `--submit`). `observe check`: 0 clean / 1 violations / 2 unreadable input (incl. duplicate JSON keys).
- **Submit**: `output::resolve_submit_auth(&args.submit, args.api_key.as_deref(), args.allow_public_endpoint)` (`output.rs:75`); endpoint = explicit `--submit URL` verbatim, else `network::derive_api_url(&auth.endpoint, "observations/ingest")` (`network.rs:133`). Do **not** call `output::preflight_submission` (it gates on the *scanner* contract version). `observe/submit.rs::submit_envelope` copies the loop shape of `submit_contract_payload` (`submit.rs:146-243`: `BACKOFF_SECONDS=[5,30,120]`, `MAX_ATTEMPTS=3`, `is_retryable(429|500|502|503|504)`, `Retry-After` integer seconds on 429, `http_status_as_error(false)`, `User-Agent: updater::user_agent_string()`) — make those three items `pub(crate)` rather than duplicating them — adds `.timeout_global(Some(120 s))`, and maps `200` → parse `{results:[{run_id,status}]}` into `SubmitOutcome{accepted, duplicate, replaced, deadline_exceeded}`; `400` → `Err("Server rejected payload (400): {body}\nThis is likely a vettd bug — the envelope doesn't match telemetry-envelope.schema.json")`; `401` → the existing auth-failed text; `413` → `Err("Payload too large (413): ~{kb} KB. Reduce --window-days.")`. Success line to stderr: `Observations accepted: {a} new, {r} replaced, {d} duplicate`.

## Field gate, schema, and CI

- `git mv` the gate and schema to the **repo root** (next to `scanner-field-gate.json`); `prices.json` → `crates/vettd-cli/resources/observe-prices.json`.
- `observe/gate.rs` ports `check_field_gate.py` exactly: `Gate::from_json` precomputes `fields`, `enums`, compiled `formats`, `bounds`, `exact_paths` (hex64/day/uuid), `patterns`, `object_paths`, `required_children`; `Dynamic::normalize` (lowercase, `DYNAMIC_MIN_LEN=3`, `COMPONENT_SETS=[cwd_and_branches,slugs,home_dir]` split on `[/\\:._-]+` with `COMPONENT_MIN_LEN=4`); `check` walks like `_Checker` (`check_field_gate.py:133-250`): `bad_key_name` (`^[A-Za-z_][A-Za-z0-9_]*$`), `unknown_key` (reports key **length** only), `missing_required` (names the key — gate names are public), `type_mismatch`, `null_not_allowed`, `out_of_bounds` (echoes the integer), `epoch_in_number` (`[1.5e9,2.5e9]∪[1.5e12,2.5e12]`, exempt units `ms2`/`tokens2`), `format_mismatch` (+ calendar validity via `NaiveDate::parse_from_str`), `not_in_enum` (enum members then run patterns but **skip** the dynamic-substring rule), `pattern:<id>`, `dynamic:<set>`. Booleans are never integers; integers must be `is_i64()/is_u64()`. Duplicate JSON keys (serde_json keeps the last): `observe check` re-scans the raw text with a small streaming duplicate-key detector → exit 2 `duplicate key in JSON object`.
- **Regex-crate compatibility** (`regex` has no lookaround): 19 of the 20 `forbiddenValuePatterns` compile unchanged (inline `(?i)` and `\b` are supported). `epoch_in_string` (`(?<![0-9])1[5-9][0-9]{8}(?:[0-9]{3})?(?![0-9])`) cannot → `enum Pattern { Regex(regex::Regex), Fn(fn(&str)->bool) }` with `epoch_in_string` = "some maximal ASCII-digit run has length 10 or 13, starts with `1`, second digit `5`–`9`" (exactly equivalent). The JSON keeps the Python regex; `Gate::from_json` maps that `id` to the fn; test `gate_has_no_uncompilable_regex_except_epoch`. Also hand-rewritten: `attribute._OPAQUE_RE` (two lookaheads) → `len ≥ 32 && all in [A-Za-z0-9_+/=-] && any_alpha && any_digit`; test-only lint `bare_reliable` `(?<!observed )` and `_RATE_RE` `(?![ _-]limit)` → find the word, inspect the neighbouring slice.
- **Canonical bytes** (`canonical.rs`): `serde_json::Map` is a `BTreeMap` in this workspace (no `preserve_order`; `Cargo.lock` serde_json 1.0.149 has no `indexmap` edge), so key order is UTF-8 byte order == code-point order == Python `sort_keys`. `canonical_json(&Value) -> Result<String, String>`: `,`/`:` separators; integers decimal; **floats → `Err`** (the envelope has none; tool `input` fingerprints are local-only and may use `serde_json` default formatting — document the non-parity); strings escaped like Python `ensure_ascii=True`: `\"`, `\\`, `\n \r \t \b \f`, other `< 0x20` → `\u00xx` (lowercase hex), `0x7f` → `\u007f` (CORRECTED 2026-09-03: the plan previously said `0x7f` raw, which is the `ensure_ascii=False` rule; CPython's `ESCAPE_ASCII` is `([\\"]|[^ -~])`, so only `0x20`-`0x7e` survive — both the C and pure-Python encoders escape DEL), non-ASCII BMP → `\uxxxx`, astral → UTF-16 surrogate pair. `to_json_bytes(env) = canonical_json + "\n"`, assert `is_ascii()`. Hash preimages (verbatim from `attribute.py:74-84`, `aggregate.py:78-91`): `run_id = HMAC-SHA256(secret, "{harness}:{session_key}")`; `name_hash = HMAC-SHA256(secret, "{asset_type}:{name}")`; `bom_version = SHA256(sorted_unique(asset_ids).join(","))`; skill tree = `SHA256(canonical_json([[relpath_posix, sha256hex(file)], …] sorted))` with `max_mtime_ms` over files **and** directories; agent = `SHA256(file bytes)`; in-band = `SHA256(text)`; descriptor = `SHA256(canonical_json({"args","command","env_names","transport"}))`. HMAC via `hmac::Hmac<sha2::Sha256>`; hex via `format!("{:x}")` like `scan_cache::hex_sha256`. Float formatting in rank: `{:.1}`/`{:.2}`; means use `round_ties_even` (Python `round()` is banker's) — baseline `4.1%–13.1%`, `USD 213.60`.
- `scripts/check-telemetry-field-gate.sh` (sibling of the scanner gate script: bash + python3, `set -euo pipefail`, `fail()`/`warn()` with `::error::`/`::warning::`, exit 1 on failure, **no `grep -P`** so it runs on macOS): (1) both JSON files parse; `gate.envelopeVersion == schema.properties.envelope_version.const == ENVELOPE_VERSION` grepped from `envelope.rs` (`grep -o 'ENVELOPE_VERSION: &str = "[^"]*"' | cut -d'"' -f2`); `gate.gateVersion == GATE_VERSION`; (2) leaf-path parity gate↔schema — expected difference exactly `{records[].assets[].signals.context_cost_est, records[].assets[].signals.tokens_attributed, records[].tokens_by_model[]}`; every shared enum byte-identical; (3) every `disclosureCategories[].name` appears as a variant line in `crates/vettd-cli/src/contract/disclosure.rs`; (4) `cargo run --locked -q -p vettd-cli --bin vettd -- observe check crates/vettd-cli/tests/fixtures/observe/golden/envelope.json --dynamic …/golden/dynamic.json` exits 0; (5) each `crates/vettd-cli/tests/fixtures/observe/gate-negative/<rule>.json` (`unknown_key.json`, `not_in_enum.json`, `epoch_in_number.json`, `format_mismatch.json`, `pattern-url_scheme.json`, `dynamic-loaded_set_names.json` + sibling `.dynamic.json`) makes `observe check` exit 1 with the rule id on stderr. Steps 4–5 land in Phase 6 (they need the binary). Success line `telemetry-field-gate OK: gate=v1 envelope=0.1.0 (N warning(s))`.
- `ci.yml`: add `telemetry-field-gate.json`, `telemetry-envelope.schema.json`, `scripts/check-telemetry-field-gate.sh`, `crates/vettd-cli/resources/**` to the `rust` paths-filter (`ci.yml:78-88`); add step `Telemetry field gate` → `run: scripts/check-telemetry-field-gate.sh` **after** `cargo test --locked` (the binary exists by then).
- Fixture naming traps: root `.gitignore` ignores `*.jsonl` (line 23) and `vettd-*.json` (lines 20-21); keep session fixtures `.ndjson` (discovery accepts both suffixes, as the Python does); goldens live under `tests/fixtures/observe/golden/` with plain names. The fixture project dir is literally `-fixture-project` (leading dash): pass roots as `--root=` or `PathBuf`, never positionally in shell tests.

## Cloud route spec (`vettd`, on `dev`)

Pattern to clone: `apps/web/app/api/scans/ingest/route.ts` (auth **before** body read, streamed `readBodyWithLimit`, `checkDurableRateLimit(db, getUserRateLimitKey(...))`), not the emitter-credential signals route (hub/signals only).

**Files** — `packages/api/src/observations/`: `telemetry-envelope.schema.json` (byte-identical copy of the vettd-cli root file), `schema.ts` (`ENVELOPE_VERSION = "0.1.0"` + default re-export of the JSON), `types.ts` (hand-written `TelemetryEnvelope`, `ObservationRecord`, `ObservationAsset`), `validate.ts`, `persist.ts`, `retention.ts`, `config.ts`, `index.ts` (client-safe), `server.ts` (window guard like `signals/server.ts:1-9`; exports `persistObservations`, `pruneExpiredObservations`), `__tests__/`. Route `apps/web/app/api/observations/ingest/route.ts`. Docs `docs/observations-ingest.md`.

**Ajv** (`validate.ts`): the schema declares draft 2020-12, so mirror `scan/ingest.ts:23-25` **with the 2020 class**: `import Ajv2020 from "ajv/dist/2020"; const ajv = new Ajv2020({allErrors: false, strict: false}); addFormats(ajv); const validate = ajv.compile(telemetryEnvelopeSchema);` (the default `Ajv` class is draft-07 and throws on the `$schema` URI — do not "fix" by editing `$schema`). After Ajv enforce `MAX_RECORDS = 500`, `MAX_ASSETS_PER_RECORD = 2000` (schema has no `maxItems`). `validateEnvelope(body) → {valid:true; envelope} | {valid:false; error}` with the same `logger` lines as `validatePayload`; error text names the failing path, never a value.

**Prisma** (`packages/db/prisma/schema.prisma`, after `AssetSignalEvent`; hand-written migration `20260903000000_add_observation_tables/migration.sql` with the branch's prose preamble):
```prisma
// ─── Passive-observer telemetry (vettd#965 child 6; spike #828) ───
// One row per (user, run_id). run_id is an HMAC pseudonym minted on the device; a resend with the
// same run_id REPLACES the row (cumulative record) — never a second row. harnessId is the harness
// stratum; it is NOT yet on AssetSignal (follow-up gated on hub/signals).
model ObservationRun {
  id               String   @id @default(cuid())
  userId           String
  runId            String   // hex64
  harnessId        String   @default("")
  deviceId         String
  deviceIdSource   String
  harnessVersion   String
  collector        String
  collectorVersion String
  envelopeVersion  String
  extractorVersion String
  gateVersion      Int
  observedAt       DateTime // observed_day at 00:00:00Z — retention key, never backdated by the server
  emittedDay       String
  model            String
  entrypointClass  String
  effort           String
  permissionMode   String
  taskCategory     String
  bomVersion       String
  loadedSetBasis   String
  runOutcome       String
  turns            Int
  toolCalls        Int
  toolFailures     Int
  userDenials      Int
  subagentRuns     Int
  compactions      Int
  unpairedToolUses Int
  repeatedToolCalls Int
  loadedSetChanges Int
  tokensBasis      String
  tokInput         Int
  tokOutput        Int
  tokCacheCreation Int?
  tokCacheRead     Int?
  tokCachedInput   Int?
  tokThinking      Int?
  tokReasoning     Int?
  tokensByModel    Json     // records[].tokens_by_model verbatim
  recordHash       String   // sha256 of the canonical record — duplicate detection
  createdAt        DateTime @default(now())
  updatedAt        DateTime @updatedAt
  assetStats       ObservationAssetStat[]
  @@unique([userId, runId], name: "observationRunIdentity", map: "ObservationRun_identity_key")
  @@index([userId, harnessId, observedAt])
  @@index([observedAt], type: Brin)
}
// One row per (run, asset, signal); n/vSum/vMin/vMax/vSumSq is the mergeable set, never a percentile.
// signal ∈ invocations | failures.tool_error | failures.timeout | failures.user_denied | failures.interrupted |
//          failures.unknown | harness_corroborations | latency_ms | tokens_attributed | context_cost_est
// (count-only signals leave vSum..vSumSq null; zero-count failure rows and null signals are not written)
model ObservationAssetStat {
  id             String   @id @default(cuid())
  runRowId       String
  run            ObservationRun @relation(fields: [runRowId], references: [id], onDelete: Cascade)
  userId         String
  runId          String
  harnessId      String   @default("")
  assetId        String   // hex64
  assetType      String
  keyBasis       String
  tier           String
  binding        String
  directEvidence Boolean
  signal         String
  n              Int
  vSum           Int?
  vMin           Int?
  vMax           Int?
  vSumSq         Decimal? @db.Decimal(24, 0)   // gate bound 1e21 exceeds BIGINT
  method         String   @default("")         // context_cost_est.method only
  @@unique([runRowId, assetId, signal])
  @@index([userId, assetId, signal])
  @@index([userId, harnessId, assetId])
}
model ObservationBom {
  id          String   @id @default(cuid())
  userId      String
  bomVersion  String   // hex64
  assetIds    String[]
  firstSeenAt DateTime @default(now())
  lastSeenAt  DateTime
  @@unique([userId, bomVersion])
  @@index([lastSeenAt])
}
```
Bounds justify types: `ms` ≤ 604,800,000 and `tokens`/`count` ≤ 1e9 fit `Int`; `sumsq` ≤ 1e21 needs `Decimal(24,0)`. Migration generation per gotcha 17: `pnpm exec prisma migrate diff --from-migrations packages/db/prisma/migrations --to-schema-datamodel packages/db/prisma/schema.prisma --script` (no shadow URL); if Prisma demands a shadow DB for `--from-migrations`, diff `--from-schema-datamodel <old schema from origin/dev> --to-schema-datamodel <new>` instead; hand-review (only `CREATE TABLE/INDEX` + one FK); apply with `migrate deploy`; confirm with `migrate status`; then `pnpm --dir apps/web prisma:generate`.

**Persistence** (`persist.ts`): `persistObservations(db, userId, envelope, {deadlineAt}) → {accepted, duplicate, replaced, deadlineExceeded: string[], results: [{run_id, status}]}`. Per record in `records` order: past `deadlineAt` → `deadline_exceeded` for it and the rest; `recordHash = sha256(sortedKeyJson(record))` (local helper; need not match the CLI); in one `$transaction`: `findUnique` on `observationRunIdentity` → same hash → `duplicate`; else upsert the run row (`harnessId = envelope.resource.harness`, `observedAt = new Date(observed_day + "T00:00:00Z")`), `deleteMany` old stats, `createMany` new stats in chunks of 1000, upsert `ObservationBom` for the record's `bom_version` (`lastSeenAt = now`) → `replaced` if existing else `accepted`. Every nullable column written with explicit `?? null` (the signals `write.ts` convention). Stats mapping: `invocations` → `n=invocations.n`; `failures.<class>` → `n=count` (skip zero); `harness_corroborations` → `n=value` (skip null); `latency_ms`/`tokens_attributed` → the five columns (skip null); `context_cost_est` → `n=1, vSum=tokens, method` (skip null).

**Retention** (`retention.ts`, `config.ts`): `observationsConfig.OBSERVATION_RETENTION_DAYS` (zod, default 90, same shape as `signals/config.ts`); `pruneExpiredObservations(client, now?)` deletes `ObservationRun` where `observedAt < cutoff` (stats cascade) and `ObservationBom` where `lastSeenAt < cutoff`. Wire as a third guarded `try/catch` step in `packages/api/src/signals/sweep.ts::runSignalSweeps` (the only boot-started TTL driver on `dev`; `instrumentation.ts` already kicks it). Retention ships in the same PR (vettd#881 rule).

**Route** (`apps/web/app/api/observations/ingest/route.ts`; imports only `@vettd/api/*` and `@/lib/*`): `MAX_BODY_BYTES = 1 MiB`, `INGEST_DEADLINE_MS = 90_000`, `readBodyWithLimit` copied from `scans/ingest/route.ts:26-52`. Order: Content-Length > limit → 413 `{error: "Payload too large. Maximum size is 1 MB."}` → `authenticateApiKey(db, authorization)` (`@vettd/api/scan`) null → 401 `{error: "Unauthorized"}` → `policyFor("observations-ingest")` + `checkDurableRateLimit(db, getUserRateLimitKey("observations-ingest", userId), …)` fail → 429 `{error: "Rate limit exceeded. Max 10 submissions per minute."}` → body read (413 on overflow) → `JSON.parse` fail → 400 `{error: "Invalid JSON"}` → `validateEnvelope` fail → 400 `{error}` → `envelope_version !== ENVELOPE_VERSION` → 400 `{error: "Unsupported envelope_version"}` → `persistObservations(..., {deadlineAt: now + 90 s})` → 200 with the result object (a resend is 200 `duplicate`/`replaced`, never a new row) → thrown → `logger.error` + 500.

**Wiring**: `packages/api/src/rate-limit/policy.ts` durable section after `"scans-ingest"`: `"observations-ingest": {key: "observations-ingest", limit: 10, windowMs: 60_000, tier: "durable", scope: "user"}`. `apps/web/proxy.ts::isApiKeyAuthPath` (`:161+`): `if (pathname === "/api/observations/ingest") { return method === "POST"; }` before the admin branch (the hub/signals precedent). No `CSRF_EXEMPT_PATHS` entry: the origin check (`proxy.ts:283-299`) only rejects when an `Origin` header is present and disallowed, and the CLI sends none; the CLI must send `Content-Type: application/json` (it does) to pass the M5 check. No `MAX_BODY_BYTES_OVERRIDES` entry (1 MiB < 5 MiB). `packages/api/package.json` `exports` add `./observations` + `./observations/server` (copy the `./signals` pair); `packages/api/vitest.config.ts` aliases for both (server first). `apps/web/__tests__/openapi-drift.test.ts` `INTENTIONALLY_UNDOCUMENTED`: `["/api/observations/ingest", "vettd-cli passive-observer ingestion (vettd#828/#965 child 6); envelope governed by telemetry-envelope.schema.json; spec entry lands with the #965 display work"]`. `apps/web/README.md` API reference: a `### Observations` entry.

**Schema sync across repos**: `.github/workflows/contract-drift-check.yml` — add `packages/api/src/observations/telemetry-envelope.schema.json` to `on.push.paths` and a second job `telemetry` cloning the `check` job: vettd hash = `jq -S -c . <file> | sha256sum`, version = `jq -r .properties.envelope_version.const`; vettd-cli side fetched the way the workflow already fetches `contract_sync.rs` (root `telemetry-envelope.schema.json` hash + `ENVELOPE_VERSION` grepped from `crates/vettd-cli/src/observe/envelope.rs`, no `grep -P`); same dedupe/issue-creation/project steps with title `Telemetry envelope drift detected: …` and label `contract-drift`. Document the rule in `docs/contract-governance.md`: the CLI repo's root file is the source of truth; the cloud copy changes in the same PR as any CLI bump.

**Tests (vitest)**: `apps/web/__tests__/observations-ingest-route.test.ts` (mock `@/lib/db`, `@vettd/api/scan`, `@vettd/api/observations/server`, `@vettd/api/rate-limit/server`, `@/lib/rate-limit`; import `POST` after mocks as `ingest.test.ts` does): `rejects an oversized Content-Length with 413 before auth`, `rejects an oversized chunked body with 413`, `returns 401 when the api key is invalid and persists nothing`, `returns 429 when the durable limit is exhausted`, `returns 400 for malformed JSON`, `returns 400 for a schema violation and names no field value`, `returns 400 for an unsupported envelope_version`, `returns 200 with per-run statuses`, `passes a 90s deadline to persistObservations`. `packages/api/src/observations/__tests__/validate.test.ts`: golden envelope valid; `additional property`, `bad enum`, `float in count`, `501 records` invalid; `schema-parity.test.ts`: `envelope_version.const === ENVELOPE_VERSION`, `additionalProperties:false` on all 15 objects, `Ajv2020` compiles it. `persist.test.ts` (fake Prisma like `signals/__tests__/write.test.ts`): `accepted then duplicate on identical resend`, `replaced when the record hash changes and old stats are deleted`, `zero-count failure rows and null signals are not written`, `deadline marks remaining runs`. `persist.integration.test.ts` + `retention.integration.test.ts` (DB-gated `hasDb` preamble from `sweep.integration.test.ts:15-29`, ids namespaced by `randomUUID()`, cleanup in `afterEach`): `resend never creates a second ObservationRun row`, `pruneExpiredObservations deletes runs older than 90 days and cascades stats`, `bom rows age out by lastSeenAt`. `signals/__tests__/sweep.test.ts`: the other sweeps still run when the observation prune throws. `apps/web/__tests__/proxy.test.ts`: bearer POST reaches the route; bearer-less POST gets 401.

## Phased execution (commit boundaries and success criteria)

Serial on the CLI: 0 → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8. Phase 9 (cloud) is independent once Phase 1 has fixed the schema file; run it in parallel if a second session is available (Phase 7 needs only the response shape above). Every phase ends with the validation gates green and one commit.

### Phase 0 — Baseline and branches
1. Verify both branches match "Branch and PR mechanics" above (already prepared). If either has moved, stop and reconcile before coding.
2. Prove the oracle is green before touching it: `cd spikes/828-passive-observer && python3 -m unittest discover -s prototype/tests -p 'test_*.py'` → **183 tests, 0 failures** (the 200 MiB streaming test skips loudly below 1 GiB free). If it is not green, stop and report.
3. `cargo build --locked` succeeds on the untouched tree.

### Phase 1 — Artifacts, gate, disclosure, secret, CI script
- `git mv` gate + schema to repo root; `git mv spikes/828-passive-observer/prices.json crates/vettd-cli/resources/observe-prices.json`; fix the relative links in the spike `README.md`/`SCOPE-965.md`.
- `crates/vettd-cli/Cargo.toml`: `hmac = "0.12"`, `getrandom = "0.4"`; verify `cargo build --locked` (commit `Cargo.lock` only if the two direct edges change it) and `cargo deny check`.
- `observe/{mod.rs (skeleton), types.rs (consts), canonical.rs, gate.rs, disclosure.rs}`, `contract/disclosure.rs` (+14 variants), `identity.rs` (secret), `main.rs` `mod observe;`, `scripts/check-telemetry-field-gate.sh` steps 1–3, `ci.yml` filter + step.
- Tests: port `test_gate.py` (27) against a hand-built `minimal_valid_payload()` (from `test_gate.py:26-66`); `every_gate_category_is_a_disclosure_variant`; `observer_secret_is_generated_once_with_0600` (unix), `observer_secret_rejects_short_file`, `explicit_secret_file_bytes_are_loaded_exactly`; `epoch_in_string_fn_matches_lookaround_semantics`, `gate_has_no_uncompilable_regex_except_epoch`; `canonical_json_matches_python_ensure_ascii` (`"é"`→`é`, `"😀"`→`😀`, `"\u{1}"`→``, `"` `\` and controls); `schema_and_gate_leaf_paths_agree`.
- Commit: `feat(observe): telemetry field gate, disclosure categories, observer secret (#828)`.

### Phase 2 — Source trait + Claude Code reader + fixtures + goldens
- `observe/source.rs`, `observe/claude_code/{mod,discover,project,apply}.rs`.
- Copy `prototype/fixtures/claude_home/**` → `crates/vettd-cli/tests/fixtures/observe/claude_home/**` (keep `.ndjson`). Add `claude_home_gate_violation/` (a copy whose `skill_listing` names a skill `taskcat`, see Phase 6).
- **Generate goldens now with the Python prototype** (from `spikes/828-passive-observer/`; record the exact command in the parity test's doc comment):
  ```bash
  printf 'invented-observer-secret-material' > ../../crates/vettd-cli/tests/fixtures/observe/golden/secret.bin   # 33 bytes, same as test_observe.py:34
  python3 prototype/observe.py --harness claude_code --root prototype/fixtures/claude_home \
    --task "exercise passive observer resume" --secret-file ../../crates/vettd-cli/tests/fixtures/observe/golden/secret.bin \
    --out /tmp/golden.json --today 2027-01-15 --now-ms 1800000000000 --window-days 3650 --scrub \
    --gate ../../telemetry-field-gate.json --prices ../../crates/vettd-cli/resources/observe-prices.json > /tmp/golden.stdout
  cp /tmp/golden.json ../../crates/vettd-cli/tests/fixtures/observe/golden/envelope.json
  cp /tmp/golden.json.dynamic.json ../../crates/vettd-cli/tests/fixtures/observe/golden/dynamic.json
  tail -n +3 /tmp/golden.stdout > ../../crates/vettd-cli/tests/fixtures/observe/golden/ranking.txt   # drop the two progress lines
  ```
  Pinned: fixture timestamps are 2026-08-15T10:00:00Z–10:01:02Z → `observed_day=2026-08-15`; `--now-ms 1800000000000` (2027-01-15T08:00Z) is far from any checkout mtime so nothing is `truncated`; `--window-days 3650` keeps every fixture inside the window; `--scrub` with no public names makes every display name `type:asset_id[:12]`, so the ranking golden is reproducible from the envelope alone. Also copy `worked-example/{observations.example.json,ranking.example.txt,public-names.txt}` into `tests/fixtures/observe/worked-example/`.
- Tests: port `test_claude_code_source.py` (16; `no_content_string_survives_parse` plants `ZQXSENTINEL` in every content position and asserts `format!("{:?}", facts)` has 0 hits; `cursor_resume_and_partial_trailing_line` asserts the inode under `#[cfg(unix)]` and `inode == None` elsewhere) + from `test_nonblocking.py`: `byte_offset_resume_reads_only_new_complete_lines`, `iter_lines_is_deterministic`, `oversized_line_is_counted_and_skipped`, `#[cfg(unix)] rename_while_open_keeps_reading_and_offsets`, `#[cfg(windows)] rename_while_open_succeeds_with_share_mode`, `#[ignore] bounded_memory_large_file` (Linux `/proc/self/status` VmHWM delta < 64 MiB over a generated 200 MiB file; `cargo test -- --ignored`).
- **Windows CI decision** (owner-visible; default = do it): add a `check-windows` job to `ci.yml` on `blacksmith-4vcpu-windows-2025`, gated by the same `rust` filter, running only `cargo test --locked -p vettd-cli observe::source::` so the share-mode test executes (the release matrix already builds this target). If the owner declines, the test stays `#[cfg(windows)]` and the PR body says it is unexecuted in CI.
- Commit: `feat(observe): Claude Code session source with byte-cursor streaming`.

### Phase 3 — Extract, FsIndex, attribute, taskcat
- `observe/{extract.rs, taskcat.rs, attribute/{mod,fs_index,segments}.rs}`.
- Tests: port `test_extract.py` (23; the `TZ` test becomes a pure UTC assertion), `test_attribute.py` (20; set mtimes with `File::set_modified`, stable since 1.75; `tree_hash_matches_independent_recomputation`), `test_taskcat.py` (11; `known_models_equal_gate_enum` parses the compiled-in gate).
- Commit: `feat(observe): extract run facts and attribute assets by hash`.

### Phase 4 — Envelope, canonical bytes, golden parity
- `observe/envelope.rs` (`build_envelope(runs, &EnvelopeMeta)`, `to_json_bytes`, `collect_dynamic` — never emits `_`-prefixed buckets, `filter_records`).
- Tests: port `test_aggregate.py` (19; the associativity property test uses an in-test xorshift64 seeded 828 — no `rand`; `validates_against_envelope_schema` is replaced by `schema_and_gate_leaf_paths_agree` + the vettd Ajv test); `golden_envelope_bytes_match_prototype` (run the Rust pipeline on the fixture home with `EnvelopeMeta{resource: {device_id: "00000000-0000-4000-8000-000000000000", device_id_source: "placeholder", harness: "claude_code", harness_version, collector: "prototype", collector_version: "0.1.0"}, extractor_version: "proto-0.1.0+taskcat-1", run_id_basis: "test_secret", today: "2027-01-15"}`, `now_ms = 1_800_000_000_000`, `window_days = 3650`, secret from `golden/secret.bin`; assert `to_json_bytes(env) == fs::read(golden/envelope.json)` **byte-identical**); `golden_envelope_passes_the_gate_with_its_dynamic_set` (the Python sidecar contains `_permission_modes`; extra sets only make the gate stricter); `worked_example_envelope_is_gate_clean` (no dynamic sets).
- Commit: `feat(observe): per-run envelope, canonical bytes, golden parity`.

### Phase 5 — Rank, render, copy lint
- `observe/{rank.rs, render.rs, lint_copy.rs}`; `--json` output = `RankResult` serialised.
- Tests: port `test_rank.py` (22) incl. `render_uses_only_copy_templates` (regexes built from `COPY` with `\{[^}]*\}` → `.*`); `golden_ranking_matches_prototype` (`render(rank(golden env, names from the run, task, "claude_code", None), scrub=true, public=∅) == golden/ranking.txt` byte-for-byte); `worked_example_render_is_structurally_stable` (the example's five public names came from the author's machine and are not in the payload — assert the ranked row `10 non-successes in 135 calls (95% interval 4.1%–13.1%)`, the `USD 213.60`/`USD 35.81` lines and every section header appear verbatim with `scrub=true`); port `test_lint.py` (11) as `#[cfg(test)]` and run the lint over every `COPY` template and the disclosure text.
- Commit: `feat(observe): ranked evidence report with evidence-state floors`.

### Phase 6 — `vettd observe` command: args, opt-in, disclosure, store, dry-run, `enable|status|check`
- `observe/{args.rs, pipeline.rs, store.rs}`, `cli.rs` (`Commands::Observe`, dispatch, `[telemetry]` loader), `main.rs`; CI script steps 4–5 + `gate-negative/` fixtures.
- Tests: config (`access_config_parses_telemetry_enabled_true/false/absent`, `telemetry_flag_is_read_without_access_table`, per-user path not cwd); store (`store_open_tolerates_missing_and_corrupt_db`, `cursor_store_evicts_oldest_beyond_cap`, `secret_rotation_clears_cursors_and_ledger`, `commit_is_atomic_across_cursors_and_ledger`, `wal_and_busy_timeout_are_set`); pipeline (in-process, temp store; ports of `test_observe.py`: `unchanged_resume_emits_silence_and_stages_cursors_every_file`, `changed_child_rebuilds_the_complete_parent_run`, `changed_main_rebuilds_the_complete_run_and_double_counts_probe_bytes`, `failed_rebuild_does_not_advance_the_probe_cursor`, `failed_child_rebuild_preserves_the_complete_parent_record`, `explicit_zero_now_is_not_replaced_by_wall_clock`); `tests/observe_integration.rs` via `env!("CARGO_BIN_EXE_vettd")` with a seeded temp `$HOME` (copy `seed_home()` from `tests/search_integration.rs:292`, plus `.vettd/.vettd.toml`, `.vettd/observer_secret`, and a copy of the fixture home so reads never touch `tests/fixtures`): `observe_exits_3_when_telemetry_disabled` (stdout empty, disclosure + TOML snippet on stderr), `observe_prints_disclosure_to_stderr_before_reading`, `observe_dry_run_writes_canonical_file_and_touches_no_store`, `observe_json_prints_envelope_to_stdout_only`, `observe_second_dry_run_still_emits_the_session` (no cursors outside submit mode), `observe_check_exit_codes` (0/1/2 incl. duplicate keys), `observe_status_json`, `observe_enable_appends_telemetry_table`, `observe_gate_refusal_exits_2_and_writes_nothing` (the `claude_home_gate_violation` fixture: `taskcat` lands in `loaded_set_names` and is a substring of the free-string leaf `extractor_version` = `1+taskcat-1` → the documented fail-closed behaviour; assert exit 2, no `--out` file, stderr names `dynamic:loaded_set_names` and never the value), `observe_disclosure_rendering_does_not_write_to_stdout`, `observe_help_lists_flags`.
- Commit: `feat(observe): vettd observe command with opt-in, disclosure, cursors, dry-run`.

### Phase 7 — Submit
- `observe/submit.rs`; `submit.rs` constants/`is_retryable` → `pub(crate)`.
- Tests (httpmock, `tests/observe_integration.rs`): `observe_submit_posts_envelope_and_updates_ledger` (asserts `Authorization: Bearer …`, `Content-Type: application/json`, body `envelope_version`), `observe_second_submit_sends_nothing_new` (no request made), `observe_changed_run_is_resent_under_the_same_run_id`, `observe_resend_ignores_ledger`, `observe_submit_400_exits_1_without_ledger_write`, `observe_submit_429_honours_retry_after` (429 + `Retry-After: 0` then 200), `observe_submit_refuses_public_http_endpoint`, `observe_explicit_submit_url_is_used_verbatim`, `observe_derived_url_ends_with_observations_ingest`.
- Commit: `feat(observe): submit envelopes to /api/observations/ingest with ledger`.

### Phase 8 — Docs and spike disposition
- Docs (same PR): README `## How it works` line 21 (add `vettd observe --submit`), `### What is included in a submission` (7th bullet: observation telemetry — hashes, counts, closed enums; link `docs/observe.md`), `## Non-interactive use / automation` table row, `## Privacy` bullet (opt-in session-log reading, projection-to-hash, nothing sent without `--submit`), `## Configuration reference` rows (`[telemetry]`, `~/.vettd/observer_secret`, `~/.vettd/observer/observer-v1.sqlite3`), extend the Windows no-ACL sentence to the secret; new `docs/observe.md` (command reference, the 14 categories, what is read/derived/sent, exit codes, ledger/rotation, `enable|status|check`); `docs/user-flows.md` (entry-path branch + "Observe and submit" sequence); `docs/architecture.md` (pure `extract/attribute/envelope/gate/rank` rows; I/O `source/claude_code/store/submit` rows; config files); `docs/output-spec.md` `## Known non-goals` carve-out ("`vettd observe` reads session transcripts by explicit opt-in and emits hashes and counts only; it is not process inspection"); `scripts/test-scanner.sh` section (`observe --dry-run` against the fixture root with the flag seeded) and `scripts/test-json-output.sh` section (`observe status --json`, `observe --json`).
- Decision records (flat `docs/`, matching the repo): `git mv spikes/828-passive-observer/README.md docs/passive-observer-decision-828.md`, `git mv …/SCOPE-965.md docs/passive-observer-scope-965.md` (fix the two stale statements: no segment index in `run_id`; one record per run; update the status table with what this PR landed). Add to the decision record **`## Provenance and score eligibility (vettd#795, vettd#797)`**: execution locus = customer machine (`resource.device_id`, `harness`); key provenance = customer-supplied (the harness's own credentials — Vettd supplies none); method provenance = `extractor_version` + `gate_version` + `binding`/`key_basis`/`tokens.basis`; eligibility ruling = **provenance-labelled context on the submitting user's own view (`source=vettd-cli`, `sourceClass=logs`, `derivation=inferred`, tenant `userId`), never score-bearing for public grades or the #916/#917 proxies; display-gated by `sampleSize` floors (show ≥ 20, order ≥ 50); enters `AssetSignal` only through child 7's projection**. Cite the two issues' acceptance criteria verbatim (fetch them if network is available; otherwise mark the citations pending).
- Delete `spikes/828-passive-observer/` (prototype, tests, fixtures, remaining worked-example files, `.gitignore`). `git ls-files spikes | wc -l` → 0; `git grep -n "spikes/828"` → hits only inside the two decision docs' history notes and this plan.
- Update the status header of `docs/vettd-observe-port-plan.md` (this file) with what landed; keep it (repo convention for plan docs).
- Commits: `docs(observe): command reference, privacy model, decision records`; `chore(observe): retire the Python spike prototype`.

### Phase 9 — Cloud route (`vettd`, on `dev`)
- Implement the spec above in this order: schema copy → Prisma models + migration → `observations/*` + exports/aliases → route + proxy + policy + openapi allowlist → tests → drift-check job + `docs/contract-governance.md` + `docs/observations-ingest.md` + `apps/web/README.md`.
- Local validation: `pnpm install && pnpm --dir apps/web prisma:generate`, `docker compose up -d --build`, `migrate deploy` from `apps/web` with `.env.local` sourced, `pnpm lint && pnpm typecheck && pnpm test`, then the curl smoke below.
- Remove `docs/vettd-observe-port-plan.md` from the vettd branch before opening the vettd PR (the vettd-cli copy is canonical).
- Commits: `feat(observations): ingest route, tables, retention for vettd-cli observe (#828/#965)`; `chore: telemetry envelope drift check`.

## Tests: Python → Rust map (names are the contract; keep them)

| Python module | Rust location | Count | Notes |
|---|---|---|---|
| `test_gate.py` | `observe/gate.rs` | 27 + 3 | + regex-equivalence, golden-with-dynamic, leaf-path parity |
| `test_claude_code_source.py` | `observe/claude_code/mod.rs` | 16 | sentinel test; inode assertion `#[cfg(unix)]` |
| `test_nonblocking.py` | `observe/source.rs`, `observe/store.rs` | 6 + 5 | rename-while-open `#[cfg(unix)]`; share-mode `#[cfg(windows)]`; large-file `#[ignore]`; disk-cap → row-cap eviction; kill/resume replaced by `commit_is_atomic_across_cursors_and_ledger` (no per-line commits in the product) |
| `test_extract.py` | `observe/extract.rs` | 23 | `run_outcome_decision_table` (10 cases) |
| `test_attribute.py` | `observe/attribute/*` | 20 | `tree_hash_matches_independent_recomputation` |
| `test_taskcat.py` | `observe/taskcat.rs` | 11 | `known_models_equal_gate_enum` |
| `test_aggregate.py` | `observe/envelope.rs` | 19 + 3 | byte-identical golden parity; worked-example gate; ASCII escaping (in `canonical.rs`) |
| `test_rank.py` | `observe/rank.rs`, `observe/render.rs` | 22 + 2 | ranking golden byte-equal; worked-example structural |
| `test_lint.py` | `observe/lint_copy.rs` (`#[cfg(test)]`) | 11 | runs over `COPY` and the disclosure text |
| `test_observe.py` | `observe/pipeline.rs` + `tests/observe_integration.rs` | 6 + 11 + 9 | integration via `CARGO_BIN_EXE_vettd`; httpmock for submit |
| `test_codex_source.py` | — | 0 | deferred with the Codex reader (fixtures recoverable at `6328b63`) |

Every test doc-comment states the invariant it protects (AGENTS.md Rule 9), e.g. "cursors advance only after the server holds the record, otherwise a dry-run could silently starve the next submit".

## Verification (end to end)

**vettd-cli**
```bash
cd /home/user/vettd-cli
cargo fmt --check && scripts/check-scanner-field-gate.sh
cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && scripts/check-telemetry-field-gate.sh
cargo test --locked -p vettd-cli golden_ -- --nocapture                          # parity tests by name
cargo build --locked --release
H=$(mktemp -d); mkdir -p "$H/.vettd"; printf '[telemetry]\nenabled = true\n' > "$H/.vettd/.vettd.toml"
cp -r crates/vettd-cli/tests/fixtures/observe/claude_home "$H/claude_home"
HOME=$H ./target/release/vettd observe --root "$H/claude_home" --window-days 3650 --task "fixture run" --dry-run --out "$H/observations.json"; echo "exit=$?"   # 0; disclosure+wrote on stderr; report on stdout
./target/release/vettd observe check "$H/observations.json"; echo "exit=$?"    # 0
python3 -c 'import json,sys; e=json.load(open(sys.argv[1])); assert e["resource"]["collector"]=="vettd-cli" and e["coverage"]["sessions_emitted"]==1; print("ok")' "$H/observations.json"
HOME=$H ./target/release/vettd observe --root "$H/claude_home" --window-days 3650 --json | python3 -m json.tool >/dev/null
HOME=$H ./target/release/vettd observe --root /nonexistent --dry-run; echo "exit=$?"                       # 1, nothing written
rm "$H/.vettd/.vettd.toml"; HOME=$H ./target/release/vettd observe --dry-run; echo "exit=$?"                # 3, snippet on stderr
[ -d ~/.claude/projects ] && HOME=$HOME ./target/release/vettd observe --dry-run --scrub --out /tmp/obs.json && ! grep -q "$USER" /tmp/obs.json && echo "real-log run ok"   # if the executor machine has real sessions; enable the flag first
./scripts/test-scanner.sh && ./scripts/test-json-output.sh                        # new observe sections PASS
git ls-files spikes | wc -l                                                       # 0 after Phase 8
```

**vettd**
```bash
cd /home/user/vettd && pnpm lint && pnpm typecheck && pnpm test
docker compose up -d --build && (cd apps/web && set -a && source .env.local && set +a && pnpm exec prisma migrate deploy --schema=../../packages/db/prisma/schema.prisma && pnpm exec prisma migrate status --schema=../../packages/db/prisma/schema.prisma)
KEY=<an ah_ key minted via the dashboard>          # never commit
PAYLOAD="$H/observations.json"                       # the CLI dry-run output above (collector=vettd-cli, real scanner_uuid)
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://localhost:3000/api/observations/ingest -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' --data-binary @"$PAYLOAD"   # 200
# repeat → 200 with duplicate:1 ; extra top-level key → 400 ; no Authorization → 401 ; 2 MiB body → 413 ; 11th POST in a minute → 429
docker compose logs web | grep -i observation      # sweep line after boot
```
**Cross-repo**: `HOME=$H vettd auth --key $KEY --endpoint http://localhost:3000/api/scans/ingest`, then `HOME=$H vettd observe --root "$H/claude_home" --window-days 3650 --submit` → stderr `Observations accepted: 1 new, 0 replaced, 0 duplicate`; rerun → `nothing new to send`; `--resend` → `0 new, 0 replaced, 1 duplicate`; inspect `ObservationRun`/`ObservationAssetStat` rows via `docker compose exec db psql`.

## Risks and gotchas (read before coding)

- **Draft 2020-12 vs Ajv default**: `new Ajv()` compiles draft-07; use `Ajv2020`. Do not edit `$schema`.
- **`observedAt` is the retention key**: derive from `records[].observed_day` (UTC midnight), never `now()`.
- **BIGINT overflow**: `sumsq` bounds are 1e21 → `Decimal(24,0)`; Prisma returns `Decimal` — convert explicitly.
- **Fail-closed substring rule**: any asset name ≥ 3 chars that is a substring of a non-enum free string (`extractor_version`, `harness_version`, `collector_version`, `envelope_version`) blocks emission (README §5). Mitigations: `EXTRACTOR_VERSION="1+taskcat-1"`; the refusal names the set, never the value, and says the local report is still available.
- **`_permission_modes` leak**: never write `_`-prefixed buckets into `forbids`; the Python golden sidecar still contains that set (stricter gate only).
- **Cursor/ledger commit only after a 2xx**; a refused or failed submit changes no state; a live session emits `run_outcome=truncated` and is later **replaced** under the same `run_id` — the server's replace semantics exist for this.
- **Streaming**: memory bounded by the longest line; `MAX_LINE_BYTES` guard; never `read_to_end` a session file.
- **Windows**: `share_mode` on open; inode `None` (size-only cursor validity); no ACL hardening on the secret (document, as README already does for config.json); the `check` CI job is Ubuntu-only — see Phase 2.
- **`derive_api_url` shape**: strips `/scans/ingest` from the saved endpoint; an explicit `--submit URL` is used verbatim — test both.
- **`hmac`/`getrandom` MSRV**: already in the lock at versions that build on 1.85.1; pin with `=` if a newer minor raises MSRV; `cargo deny check` stays clean.
- **`.gitignore` traps**: `*.jsonl`, `vettd-*.json`, and the spike dir's `*.dynamic.json` — see "Field gate".
- **`serde_json` recursion limit (128)** on pathological tool inputs → count as `parse_errors`.
- **Prisma migration generation** may demand a shadow DB — fallback recipe above; never a live URL (gotcha 17).
- **Coverage block** is validated but not persisted server-side in v1 (follow-up).
- **Do not widen**: no Codex reader, no `~/.codex` detectors, no projection into `AssetSignal`, no `harnessId` on `AssetSignal`, no OpenAPI path (allowlist entry instead), no new always-on service, no interactive "offer to submit" after a plain run.

## Follow-ups (explicitly out of scope; list in both PR bodies)

1. Codex `Source` + `~/.codex` detectors (`discovery.rs:85` `AI_CLI_CONFIG_DIRS`), gated on a real rollout file (SCOPE-965 child 5).
2. vettd child 7: projection of `ObservationAssetStat` into `AssetSignal` (`sourceClass:"logs"`, `source:"vettd-cli"`, `derivation:"inferred"`, `sampleSize` = merged n, ruleIds `reliability/observed-non-success-rate`, `performance/observed-invocation-latency`, `cost/observed-tokens-per-run`, `cost/observed-invocation-frequency`) + D5 display floors/Wilson in `verdicts.ts`/drawer — gated on `hub/signals` landing on `dev`.
3. `harnessId` on `AssetSignal`/`AssetSignalEvent` and its addition to `assetSignalIdentity` (needs a dry run on live rows).
4. Telemetry `asset_id` on the scan payload through the vettd-cli#243 gate (child 8), so telemetry rows can be shown with names.
5. Persist envelope `coverage` per submission; serve `GET /api/observations/contract`; OpenAPI entry.
6. Amend vettd#916/#917 acceptance ("supplemented on the user's view; public retirement is fleet-tier") and record the #828 ruling on both (child 9); fleet tier (org consent, cohort minimums).
7. Per-project pseudonym `HMAC(secret, cwd)` (reserved, unimplemented).

## PR bodies (both repos)

State: what changed and why scope is limited (rulings table); validation actually run (paste the last lines of each command); migration sequencing (vettd: additive tables, no backfill; apply before deploying the route); rollback (revert the PR; tables can stay); deviations from SCOPE-965 (separate SQLite file; cursors commit after 2xx; replace-on-resend server semantics; `EXTRACTOR_VERSION` format); the Windows CI decision; follow-ups list. End with the Claude Code attribution footer required by the session.
