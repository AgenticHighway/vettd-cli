# Prototype module contracts (spike #828)

Pipeline: `sources/*.read()` → `SessionFacts` → `extract.extract()` → `RunFacts` →
`attribute.attribute()` → `AttributedRun` → `aggregate.build_envelope()` → wire envelope (dict) →
`check_field_gate.check()` → `rank.rank()` → text.

Invariants every module upholds:

- No free text from a session survives parsing. Names are the only session-derived strings kept, and
  only locally. Content, tool inputs, tool results, prompts, summaries, file bodies are hashed or
  counted and discarded.
- Deterministic: same inputs + same secret + same `today` → byte-identical envelope.
- Fail-open: a malformed line is counted (`parse_errors`, `lines_unknown_type`) and skipped, never
  raised past the source.
- Durations are differences of harness timestamps.

## sources/claude_code.py

`ClaudeCodeSource(root: str)`; `harness = "claude_code"`.

- `discover(root, window_days, now_ms) -> List[SessionRef]`: `<root>/projects/*/*.jsonl` are main
  sessions (`session_key` = file stem); `<root>/projects/*/<stem>/subagents/agent-*.jsonl` are
  children (`kind="child"`, `parent_key` = stem, `child_meta` from the sibling `.meta.json`:
  `agentType`, `toolUseId`, `spawnDepth`). Window filter on file mtime.
- `read(ref, cursor=None) -> (SessionFacts, Cursor)`: streams with `sources.base.iter_lines`.
  Consumes line types `user`, `assistant`, `attachment`, `summary`; counts every other type in
  `lines_unknown_type` without parsing it. Per kept line, projects to a key allowlist before any
  other use: `type uuid parentUuid timestamp sessionId isSidechain agentId version entrypoint
  permissionMode effort message.id message.model message.stop_reason message.usage.*
  message.content[].{type,id,name,tool_use_id,is_error} toolUseResult.{interrupted,isAsync,agentId,status}
  sourceToolAssistantUUID isMeta`. Never keeps `slug`, `cwd`, `gitBranch`, `mcpMeta`, `input`,
  text, `toolUseResult` bodies — but DOES record `slug`, `cwd`, `gitBranch`, `sessionId`, `agentId`,
  tool_use ids, message ids, `mcpMeta` server names into `facts.forbids` buckets
  (`slugs`, `cwd_and_branches`, `harness_session_ids`, `agent_ids`, `tool_use_ids`, `message_ids`,
  `loaded_set_names`) so the gate checker can forbid them.
- Tool calls: `tool_use` blocks (assistant lines) create `ToolCall(tool_use_id, name, ts_ms,
  message_id, input_fingerprint=sha256(canonical json of input))`; for `name.startswith("mcp__")`
  set `server` = second `__`-segment; `name == "Skill"` → `skill` = input.skill or input.name;
  `name == "Agent"` → `agent_type` = input.subagent_type; `is_async` when the paired user line has
  `toolUseResult.isAsync` true or the result text starts with "Async agent launched". `tool_result`
  blocks (user lines) pair by `tool_use_id`, set `result_ts_ms`, `is_error`, `interrupted`
  (`toolUseResult.interrupted`), and `child_key` = `toolUseResult.agentId` for Agent spawns.
  Failure class: `user_denied` if `is_error` and (`interrupted` or result text matches
  `doesn't want to proceed|rejected|Request interrupted by user`); else `tool_error` if `is_error`;
  else None. Result text is examined for that classification and then discarded.
- Turns: user lines whose content is a string or contains a `text` block, not `isMeta`, not
  tool_result-only.
- Usage: `message.usage` on assistant lines → `Usage` keyed by `message.id` (dedupe); `model` from
  `message.model`; `models[model] += 1` once per message id.
- Attachments: `skill_listing` → `LoadedSetEvent(kind="initial", skills=names, listing_bytes={name:
  len(line)} from `content` lines matched by name)`; `deferred_tools_delta` → `LoadedSetEvent(kind=
  "initial" if first else "delta", tool_names=addedNames, pending_mcp=pendingMcpServers,
  failed_mcp=failedMcpServers, removed=removedNames, readded=readdedNames, tool_schema_bytes={server:
  sum(len(line) for addedLines of that server)})`; `agent_listing_delta` → agent_types=addedTypes;
  `mcp_instructions_delta` → tool_names unchanged but pending resolution noted; `nested_memory` →
  `InBandAsset(kind="rules_file", name=basename(path), content_sha256=sha256(content), byte_len)`
  and `rules_files=[basename]` on the initial event. `summary` lines → `compactions += 1`.
- Invoked skill bodies: a user line with `isMeta` true whose text contains `<command-name>X</command-name>`
  → `InBandAsset(kind="skill_body", name=X, content_sha256 of the text after the closing tag)` and a
  synthetic `ToolCall(name="Skill", skill=X)` paired with itself (latency None).
- Children: `read()` on a child ref works the same; `SessionFacts.ref.kind == "child"`.
- Truncation: `facts.truncated` when file mtime is within 120 s of `now_ms` and the last assistant
  `stop_reason` is not `end_turn`.

## sources/codex.py

`CodexSource(root)`; `harness = "codex"`. `discover`: `<root>/sessions/**/*.jsonl` and
`<root>/archived_sessions/**/*.jsonl`, `session_key` from `session_meta.payload.id` (fallback file
stem). `read`: rollout lines `{timestamp,type,payload}`; `session_meta` → harness_version =
`cli_version`, entrypoint from `originator`; `turn_context` → model, effort, `approval_policy` →
permission_mode; `response_item` `function_call`/`custom_tool_call` → ToolCall (name; MCP tools are
`<server>__<tool>` optionally prefixed `mcp__`; server = namespace before the last `__`),
`function_call_output`/`custom_tool_call_output` pairs by `call_id`; `event_msg` `token_count` is
cumulative → per-turn deltas (backwards counters mark `truncated`), `mcp_tool_call_begin/end` give
server identity and `Failed` status, `context_compacted` → compactions. No in-band loaded set:
`loaded_events` empty; attribute falls back to filesystem basis.

## extract.py

`extract(facts: SessionFacts, now_ms: int) -> RunFacts`. Merges children: `subagent_runs`, child
usages into token totals (deduped by message id across the whole tree), child outcomes into
`InvocationObs(asset_type="agent", child_tokens_total=...)` for the parent's Agent spawn, child tool
calls into the parent's counts. Tokens: sum over `usages` values; `basis="harness_usage"` when any
usage exists. `repeated_tool_calls` = number of ToolCalls whose `(name, input_fingerprint)` occurs
≥3 times. `tool_class_shares`: classify each ToolCall name into `edit` (Edit/Write/MultiEdit/
NotebookEdit/apply_patch), `read` (Read/Glob/Grep/LS/WebFetch/WebSearch), `shell` (Bash/shell/exec),
`mcp` (mcp__* or server set), `other`; shares = count/total. `model`: most frequent model, passed
through `taskcat.allowlist_model`. `entrypoint_class`: contains "remote" → remote; contains
"vscode|jetbrains|ide" → ide; contains "sdk" → sdk; "cli" → cli; else unknown. `permission_mode`:
values already in the closed enum pass through (Codex pre-maps); otherwise map `acceptEdits→accept_edits`, `bypassPermissions→bypass`, `dontAsk→dont_ask`, `plan/default/auto`
as-is, else unknown. `run_outcome`: `truncated` if facts.truncated; `compacted` if compactions>0
and last_stop_reason != end_turn; `interrupted` if any tool call unpaired or interrupted at the
end; `completed` if last_stop_reason == end_turn; else unknown. Invocations: from tool calls with
`skill` (asset_type skill), `server` (mcp_server), `agent_type` (agent; corroborated when the child
transcript's `attributionAgent` matched, which the source records as `child_meta["corroborated"]`).

## attribute.py

`attribute(run: RunFacts, fs_index: FsIndex, secret: bytes) -> AttributedRun`.
- `FsIndex(claude_home=None, codex_home=None)`: lazily hashes local assets: skills under
  `<claude_home>/skills/**/SKILL.md` (tree hash of the skill dir: sorted relative paths + sha256 of
  each file), agents under `<claude_home>/agents/*.md`, MCP descriptors from `<claude_home>/.claude.json`
  `mcpServers` and `<claude_home>/settings.json`, and for Codex from `<codex_home>/config.toml`
  `[mcp_servers.*]`. Records max mtime per asset dir for the binding rule.
- Keys: skill with local tree → `content_hash` + binding by mtime rule (`mtime_proven` if max mtime
  < listing ts, else `unproven`); invoked skill with in-band body → `content_hash` of the body,
  binding `harness_log_exact`; rules file in-band → `content_hash`, `harness_log_exact`; MCP server
  with descriptor → `descriptor_hash` = sha256(canonical json of {transport, command basename or
  url host class, args minus secret-shaped/path-shaped tokens, sorted env NAMES}); anything else →
  `name_hash` = HMAC-SHA256(secret, f"{asset_type}:{name}"), binding `not_applicable`.
- Settle rule for `deferred_tools_delta` events: fold into the current segment when `removed ==
  readded == []` and every added name is `mcp__<S>__*` for an `S` in the union of prior
  `pending_mcp`; otherwise start a new segment. `skill_listing`/`agent_listing_delta` with
  `isInitial` never split.
- `bom_version` = sha256(",".join(sorted(asset_ids))).
- Tiers in this prototype: every observation is `inferred` (historical read, filesystem-now hashes),
  with `direct_evidence_available = True` for assets that had explicit invocations in the log.
  `context_cost_est`: skills → (listing_bytes//4, listing_bytes_div4); rules files → (byte_len//4,
  file_bytes_div4); MCP servers → (tool_schema_bytes//4, tool_schema_bytes_div4); else None.
- Builtin agent types (Explore, Plan, general-purpose, claude, Bash, statusline-setup,
  claude-code-guide, output-style-setup) are NOT assets: their spawns count in run counts only.
- `name_map[asset_id] = f"{asset_type}:{name}"` (local only).

## taskcat.py

`RULES_VERSION = "taskcat-1"`. `categorize(shares: Dict[str, float]) -> str` (pure): total==0 →
`unspecified`; `mcp >= 0.5` → `mcp_heavy`; `edit >= 0.25` → `code_edit`; `shell >= 0.5` →
`shell_ops`; `read >= 0.5` → `code_explore`; else `mixed`. Boundaries are the published rule set.
`allowlist_model(raw) -> str`: `raw` if it is in `KNOWN_MODELS` (identical to the gate's `enums.model`, a closed list versioned with the gate), else `"other"`. `KNOWN_MODELS` exported.

## aggregate.py

`Stats` helper: `from_values(ints) -> {n,sum,min,max,sumsq}` (empty → n=0, sum=0, min=0, max=0,
sumsq=0); `merge(a,b)` associative + commutative. Records also carry `tokens_by_model[]` (one entry per allowlisted model id, same buckets) because sub-agents may run on a different model than the parent; `records[].model` is the dominant model of the MAIN transcript. `build_envelope(runs: List[AttributedRun],
resource: dict, coverage: dict, today: str, secret: bytes, run_id_basis: str) -> dict`: one record
per (run, segment); `run_id` = HMAC-SHA256(secret, f"{harness}:{session_key}:{segment.index}");
records sorted by `(observed_day, run_id)`; `assets[]` sorted by asset_id; `bom[]` unique by
bom_version, sorted; all keys as in `../telemetry-envelope.schema.json`. `to_json_bytes(env)` →
`json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n"`. Returns also the
merged `name_map` and merged `forbids` for the checker (as a separate function
`collect_dynamic(runs) -> Dict[str, set]`).

## rank.py

`wilson(k, n, z=1.96) -> (lo, hi)`. Floors: `FLOORS = {"count":1, "tokens":3, "latency":5,
"rate_show":20, "rate_order":50}`. `evidence_state(signal, n) -> "observed"|"early_evidence"|
"insufficient_evidence"|"not_applicable"|"no_coverage"`. `rank(envelope, name_map, task, harness,
model=None) -> RankResult` (when the matched task-category stratum has no runs, every category in the harness is pooled and `pooled_categories` is set so render() says so; rows with zero invocations are collapsed into one "loaded but never invoked" summary line per render) with `ranked` (n ≥ rate_order; key `(hi, -n, asset_id)`), `early`
(rate_show ≤ n < rate_order, shown with interval, unordered), `insufficient` (sorted by n desc,
with `needs = rate_show - n`), `loaded_only` (rules/prompts: context-cost only). `render(result,
scrub: bool, public_names: set) -> str`: text table; every row shows tier, evidence_state, and for
rates "k non-successes in n calls (95% interval lo–hi)"; copy templates live in `COPY` dict so
lint_copy can test them; never a causal verb. Cost line at the end: tokens × prices.json with the
table date named, per run model; "(display-time derivation, not stored)".

## check_field_gate.py

`check(payload: dict, gate: dict, dynamic: Dict[str, set] | None) -> List[str]` (empty = pass).
Walk every leaf; unknown leaf path (or unknown intermediate object) → violation; nullable objects
allowed null; enum/format/bounds per field; hash paths exact hex64; day paths exact; allowed uuid
paths exact; all other string leaves must pass every forbidden pattern and every dynamic set
(substring, case-insensitive). Numeric leaves outside `ms2`/`tokens2` also fail if in
[1.5e9, 2.5e9] or [1.5e12, 2.5e12] (`epoch_in_number`). CLI: `python3 check_field_gate.py
<payload.json> [--gate path] [--dynamic names.json]`, exit 1 on violations, prints each.

## lint_copy.py

`lint(text) -> List[str]`: forbidden phrases (case-insensitive): `causes?`, `because of`,
`improves?`, `makes? (you|your|it) `, `faster than`, `better than`, `worse than`, `% (better|worse)`,
`saves?`, `proves?`, `guarantee`, `\$\d`, bare `reliable|unreliable` not preceded by "observed";
required hedge: any line containing "rate" must contain "observed" or "in \d+ calls". CLI over files,
exit 1 on findings.

## cursor_store.py

`CursorStore(path)`: JSON file mapping session path → {byte_offset, inode}; `save()` writes
temp+rename (atomic); `cap_bytes` evicts oldest entries beyond the cap.

## observe.py

CLI: `--harness claude_code|codex --root <home dir> --task "<text>" --secret-file <path>
--out <payload.json> [--today YYYY-MM-DD] [--window-days N] [--scrub] [--public-names file]
[--cursor-store path] [--synthetic-demo]`. Runs the pipeline, writes the payload with
`to_json_bytes`, writes `<out>.dynamic.json` (the dynamic forbids, local only, for the checker),
runs the checker in-process and refuses to write if it fails, prints `rank.render`. `--synthetic-demo`
appends a clearly labelled synthetic AttributedRun set (invented counts) so a populated ranking can
be shown; the label appears in the output header and the payload is written to a separate
`<out>.synthetic.json`.
