# Search interface: old vs. new shape

## Summary

Adds these filters to `vettd directory search` / `vettd inventory search`,
all gated behind `SEARCH_BETA_TESTING` (see
[`crates/vettd-cli/src/network.rs`](../crates/vettd-cli/src/network.rs)'s
`search_beta_testing_enabled()`), the same flag that gates
`VETTD_DIRECTORY_ENDPOINT`/`VETTD_INVENTORY_ENDPOINT`:

- `--language`, `--agent-compatibility`, `--rankings` — catalog filters (skill).
- `--source` (repeatable), `--rank-filter <key=N>` (repeatable) — discovery-source
  and per-source search-rank push-downs (skill + mcp for `--source`).
- `--asset-type skill|mcp` — which catalog to search. `mcp` switches the
  response envelope to `mcpServers` (`McpHit`-shaped cards). `directory` only
  — `inventory search --asset-type mcp` is rejected client-side (the MCP
  catalog is not user-scoped).
- `--mcp-category`, `--deployment`, `--registry-type` (all repeatable) —
  mcp-only filters (ignored server-side for `--asset-type skill`).

Skill search responses also now carry three opaque scan-verdict passthroughs
per card — `llm_scan`, `cli_security`, `vettd_scan` (`object | null`,
snake_case) — surfaced in `--json` / the raw dump.

The new filters don't fit cleanly into a query string, so enabling the flag
also switches the request from `GET ?query-string` to `POST` with a JSON
body — there is no GET-with-new-filters hybrid. This is the only way search
is issued once the beta flag is on.

Implemented in `crates/vettd-cli/src/directory.rs` (`handle_search`,
`validate_search_filters`, `build_search_body`), reused by
`crates/vettd-cli/src/inventory.rs`, over `read_client::post_json` /
`inventory_client::post_json`.

## Old shape (`SEARCH_BETA_TESTING` unset — unchanged, always GET)

**Input:**

```
vettd directory search <query> [--page N] [--sort newest|rating|alpha] [--reverse] [--json]
vettd inventory search <query> [--page N] [--sort newest|rating|alpha] [--reverse] [--json]
```

Translates to:

```
GET {base}/directory?search=<query>&sort=<sort>&page=<page>
```

**Output** (`DirectoryListResponse`, see `crates/vettd-cli/src/directory.rs`):

```json
{
  "skills": [
    {
      "slug": "e2e-testing",
      "name": "e2e-testing",
      "description": "Drives end-to-end browser tests with Playwright: installs the runner, generates specs from a user story, runs them headless with retries, and triages failures.",
      "version": "1.4.0",
      "author": "affaan-m",
      "category": "Testing",
      "badgeStatus": "verified",
      "overallGrade": "B",
      "sourceType": "github",
      "scannerRunCount": 12
    }
  ],
  "total": 1,
  "page": 1,
  "totalPages": 1
}
```

Deserialization is allow-list only — unknown response fields are silently
dropped (never printed), so any *new* field the server adds is invisible
until the CLI's struct is updated to include it.

> The `e2e-testing` skill (`github.com/affaan-m/ECC`, `skills/e2e-testing`) is
> the shared example subject across the rendered API docs
> (`vettd/docs/VETTD_API.md`, `ah-skills/search-demo/docs/QUERY_SERVICE_API.md`)
> — it's the one skill that carries all three catalog scan verdicts. The MCP
> example subject is `github:upstash/context7` (see "MCP search" below).

## New shape (`SEARCH_BETA_TESTING=1` — always POST + JSON)

**Input:** the beta filter surface, additive to the existing
query/page/sort/reverse. Using any of it — or `--asset-type mcp` — without
the beta flag is a hard CLI error (exit 1), never silently ignored:

```
vettd directory search <query> \
  [--language <lang>]... \
  [--agent-compatibility <agent>]... \
  [--rankings '<json>'] \
  [--source <s>]... \
  [--rank-filter <key=N>]... \
  [--asset-type skill|mcp] \
  [--mcp-category <c>]... [--deployment <d>]... [--registry-type <t>]... \
  [--page N] [--sort newest|rating|alpha] [--reverse] [--json]
```

| Flag | Repeatable | Behavior |
|---|---|---|
| `--language <lang>` | yes | Filter to skills whose implementation language matches, e.g. `--language python --language typescript`. |
| `--agent-compatibility <agent>` | yes | Filter to skills compatible with the named agent/runtime, e.g. `--agent-compatibility claude-code --agent-compatibility cursor`. |
| `--rankings '<json>'` | no | Minimum-threshold filter — same field names as the response's `rankings` object. Numeric fields mean "at least this value," booleans mean "must equal this value." Omitted keys are unconstrained. |
| `--source <s>` | yes | Filter by discovery source, e.g. `--source marketplace --source seed`. Real push-down (`sources` → query-service `sources`). Applies to both `--asset-type skill` and `mcp`. |
| `--rank-filter <key=N>` | yes | Per-source search-rank ceiling, e.g. `--rank-filter search_rank_skills_sh_rank=100`. Repeated flags merge into a `{key: N}` map (`rankFilters` → query-service `rank_filters`). Skill only. A malformed value (no `=`, non-integer `N`, empty key) is a CLI error (exit 1) reported before any request. |
| `--asset-type <skill\|mcp>` | no | Which catalog to search. Default `skill`. `mcp` (directory only) changes the response envelope — see "MCP search" below. |
| `--mcp-category <c>` | yes | mcp-only: `server`/`client`/`framework`/`tooling`. Sent but ignored server-side for `--asset-type skill`. |
| `--deployment <d>` | yes | mcp-only: `local`/`remote`/`hybrid`. |
| `--registry-type <t>` | yes | mcp-only: `npm`/`pypi`/`oci`/… |

Any of these (or `--asset-type mcp`) without `SEARCH_BETA_TESTING` is a hard
CLI error (exit 1), never a silent no-op.

`--rankings` example:

```json
{
  "stars": 50,
  "skillsShLeaderboardRank": 100,
  "numberOfAggregators": 2,
  "officialClaudeMarketplace": true
}
```

`--rankings '{"stars": 50, "officialClaudeMarketplace": true}'` returns only
skills with 50+ stars that are listed in the official Claude marketplace,
ignoring `skillsShLeaderboardRank`/`numberOfAggregators`. Invalid JSON is a
CLI error (exit 1) reported before any request is sent.

**Request** — with the beta flag on, the CLI always issues:

```
POST {base}/directory
Content-Type: application/json

{
  "search": "<query>",
  "page": <page>,
  "sort": "<sort>",
  "reverse": <reverse>,
  "assetType": "skill",
  "languages": ["python", "typescript"],
  "agentCompatibility": ["claude-code", "cursor"],
  "sources": ["marketplace"],
  "rankFilters": {"search_rank_skills_sh_rank": 100},
  "rankings": {"stars": 50, "officialClaudeMarketplace": true}
}
```

`assetType` is always present (`"skill"` default). `languages` /
`agentCompatibility` / `sources` are always present as arrays (empty when the
corresponding flag wasn't given); `rankFilters` is always present as an
object (`{}` when no `--rank-filter`). `rankings` is `null` if `--rankings`
wasn't passed. When `assetType` is `"mcp"` the body additionally carries
`mcpCategory` / `deployment` / `registryType` arrays (always present, `[]`
when unset); they are omitted entirely for `"skill"`. This body shape is
sent even when no filter flags are used — the beta flag alone is what
switches GET→POST.

### Exactly what goes on the wire

**The gate.** `network::search_beta_testing_enabled()` reads
`SEARCH_BETA_TESTING`, trims and lower-cases it; only `"1"` or `"true"` count
as on. That single flag picks the method:

- **off** → `GET {base}/directory?search=<q>&<sort params>&page=<n>`
  (`--reverse` folds into the sort params). Byte-identical to every prior
  release; no filters reachable on this path.
- **on** → **always** `POST {base}/directory` with the JSON body above, even
  with zero filter flags.

**Where `{base}` comes from.** `directory_base_url()` chooses an ingest
endpoint, then `network::derive_api_url(endpoint, "directory")` rewrites it:

1. `VETTD_DIRECTORY_ENDPOINT` — **only** read when the beta flag is on;
2. else the endpoint saved by `vettd auth` (`~/.config/vettd/config.json`);
3. else the built-in production endpoint (`submit::DEFAULT_PRODUCTION_ENDPOINT`).

`derive_api_url` strips a trailing `/scans/ingest` (or `/ingest`, or
truncates at `/api/`) and appends the resource:

```
http://localhost:3001/api/scans/ingest
  → strip "/scans/ingest", append "/directory"
http://localhost:3001/api/directory
```

**The request itself** (`read_client::post_json`, a bare `ureq` POST):

- **no `Authorization` header** — directory search is public. Only
  `User-Agent` is set. `content-type: application/json` via `send_json`.
- 10-second global timeout (`REQUEST_TIMEOUT_SECS`).
- `429` → message + `exit 1`; `404` → `ReadError::NotFound`; any other
  non-200 → `ReadError::ServerError(status)`; transport failure →
  `ReadError::Unreachable`.
- `inventory search` uses the authed twin `inventory_client::post_json`
  (adds the bearer token from `vettd auth`) and `VETTD_INVENTORY_ENDPOINT`.

Built by `directory::build_search_body()` in
`crates/vettd-cli/src/directory.rs`; the `--asset-type mcp` path routes to
`handle_mcp_search()` and deserializes `McpListResponse` instead.

**Output** — same envelope, with these additions per skill: `language`,
`agentCompatibility` (the skill's actual values, not a filter), the nested
`rankings` object, and three opaque scan-verdict passthroughs `llm_scan` /
`cli_security` / `vettd_scan` (`object | null`, snake_case keys, forwarded
verbatim from the query service — the CLI does *not* model their internals).
(The server sends the language field as `docLanguage` on the wire; the CLI
deserializes it via a serde alias and still surfaces it under `language` in
`--json` output.) The envelope itself also gains a top-level `mock` flag.

```json
{
  "skills": [
    {
      "slug": "e2e-testing",
      "name": "e2e-testing",
      "description": "Drives end-to-end browser tests with Playwright: installs the runner, generates specs from a user story, runs them headless with retries, and triages failures.",
      "version": "1.4.0",
      "author": "affaan-m",
      "category": "Testing",
      "badgeStatus": "verified",
      "overallGrade": "B",
      "sourceType": "github",
      "scannerRunCount": 12,
      "language": "en",
      "agentCompatibility": ["claude-code"],
      "rankings": {
        "stars": 240095,
        "skillsShLeaderboardRank": 142,
        "numberOfAggregators": 2,
        "officialClaudeMarketplace": true
      },
      "llm_scan": {
        "model": "openrouter/deepseek/deepseek-v3.2",
        "prompt_version": "37243f9d5700",
        "scanned_at": "2026-08-30T21:41:26.512874+00:00",
        "content_sha256": "b0e00e7e17cb259101139900816c5528aed18dd10bcf5f9cb42cfc35baf8a755",
        "max_severity": "LOW",
        "finding_count": 1,
        "primary_threats": ["unpinned-dependency-install"],
        "overall_assessment": "Benign testing skill. One low-severity note ...",
        "findings": [
          {"severity": "LOW", "aitech": "AITech-4.3", "title": "Unpinned global CLI install", "description": "...", "remediation": "Pin the version."}
        ]
      },
      "cli_security": {
        "grade": "C",
        "scanned_at": "2026-08-30T21:40:47.315190+00:00",
        "osv_snapshot_date": "2026-08-30",
        "packages": [
          {"package": "playwright", "ecosystem": "npm", "classification": "cli",
           "install_command": "npx playwright test tests/search.spec.ts --repeat-each=10",
           "vuln_count": 1, "max_severity": "HIGH", "advisory_ids": ["GHSA-7mvr-c777-76hp"]}
        ]
      },
      "vettd_scan": {
        "scan_id": "scn_01J9Z7Q3K8V2M4N6P8R0T2W4Y6",
        "overall_grade": "B",
        "trust_level": "cautious",
        "has_malicious_findings": false,
        "finding_count": 4,
        "severity_counts": {"critical": 0, "high": 0, "medium": 1, "low": 2, "info": 1},
        "categories_flagged": ["scripts", "best-practices"],
        "top_findings": [
          {"rule_id": "shell-exec-unpinned-install", "category": "scripts", "severity": "medium", "label": "..."}
        ]
      }
    }
  ],
  "total": 1,
  "page": 1,
  "totalPages": 1,
  "mock": false
}
```

`llm_scan` is the non-deterministic LLM threat scan (can revert to `null` on
a catalog re-index); `cli_security` is the OSV security-history grade for the
CLI tools the skill installs; `vettd_scan` is the deterministic Vettd scan
rollup for the matched repo. All `object | null` — `null` in mock mode, when
the skill has no catalog match, or when the query service is unreachable.
The CLI carries them as `Option<serde_json::Value>` on `DirectoryCard`
(`#[serde(skip_serializing_if = "Option::is_none")]`), so a GET / non-beta
response that omits them round-trips byte-identically. The human table does
not render a verdict column yet (follow-up) — they show in the raw dump /
`--json`.

Note: `docLanguage` here is the SKILL.md's *spoken/content* language (`"en"`),
not a programming language — the CLI surfaces it under `language` via a serde
alias. See `vettd/docs/search-beta-api-spec.md`.

`language`/`agentCompatibility`/`rankings`/`mock`/`llm_scan`/`cli_security`/
`vettd_scan` are additive `Option` fields on `DirectoryCard`
(`crates/vettd-cli/src/directory.rs`) — old clients (without the beta flag,
or on an older CLI version) simply don't have the field and continue to
ignore it, per the existing forward-compatibility contract.

**Dual dump behavior (unchanged):** with `SEARCH_BETA_TESTING=1` the CLI
still prints the raw JSON response (now including the new fields) followed
by the formatted table, regardless of `--json`. The human-readable table
does not yet render a `rankings` / verdict summary column — exact layout
TBD; the fields are visible today via the raw-JSON dump / `--json`.

## MCP search — `--asset-type mcp`

`vettd directory search <query> --asset-type mcp` sends
`{"assetType": "mcp", ...}` to the same `POST {base}/directory` and gets back
a **different response envelope** (`mcpServers`, not `skills`) of
`McpHit`-shaped cards. It is a thin fail-open proxy of the query service's
`mcp_servers` catalog — no Postgres spine, no `mock` fabrication.

- Only on `directory search`. `inventory search --asset-type mcp` is rejected
  client-side (exit 1) — the MCP catalog is not user-scoped and the backend
  400s it.
- Always beta (requires `SEARCH_BETA_TESTING`), so always `POST` + the dual
  raw/formatted dump.
- `--source` is forwarded; `--mcp-category` / `--deployment` /
  `--registry-type` are the mcp-only filters. `--rank-filter` /
  `--language` / `--agent-compatibility` are still sent but ignored
  server-side for `mcp`.
- `indexReady: false` (empty/absent `mcp_servers` collection, or query
  service unreachable) is reported **distinctly** from "no results for this
  query" — it is an onboarding/outage signal.
- The CLI's `McpCard` is an allow-list struct mirroring `McpHit` (snake_case
  passthrough, every field `Option`); unknown fields are dropped. The
  compact table shows id / category / registry / stars / dependency-vuln
  count — the full shape (OSV `security_*` block included) is in the raw
  dump / `--json`.

The catalog subject is `github:upstash/context7`:

```json
{
  "mcpServers": [
    {
      "score": 0.75,
      "rank": 1,
      "mcp_id": "github:upstash/context7",
      "name": "io.github.upstash/context7",
      "description": "A Model Context Protocol server that fetches up-to-date, version-specific documentation and code examples ...",
      "repo_url": "https://github.com/upstash/context7",
      "status": "active",
      "mcp_category": "server",
      "sources": ["repo_scan", "official_registry", "glama"],
      "registry_type": "npm",
      "package_identifier": "@upstash/context7-mcp",
      "deployment": "hybrid",
      "transport": "stdio",
      "has_installable_package": true,
      "has_remote": true,
      "license": "MIT License",
      "stars": 61421,
      "language": "TypeScript",
      "weekly_downloads": 867314,
      "security_source": "osv",
      "security_vuln_count": 0,
      "security_max_severity": null,
      "security_direct_deps_scanned": 8,
      "security_direct_deps_vuln_count": 44,
      "security_direct_deps_with_vulns": ["zod", "jose", "undici", "express"],
      "security_direct_deps_max_severity": "HIGH"
    }
  ],
  "total": 1,
  "page": 1,
  "totalPages": 1,
  "mock": false,
  "indexReady": true
}
```

`total` / `totalPages` reflect only the first page of catalog matches the
query service returned (it takes `limit` but no offset), not the whole
`mcp_servers` collection. `security_max_severity: null` (and any other wire
`null`) deserializes to an absent key in the CLI's `--json` output, per the
`skip_serializing_if` rule. Full contract: `vettd/docs/VETTD_API.md`,
`vettd/docs/search-beta-api-spec.md` "Phase 3".

## Schema → CLI output (the serde mapping)

The decode structs are in `crates/vettd-cli/src/directory.rs`:
`DirectoryListResponse` / `DirectoryCard` / `SkillRankings` /
`McpListResponse` / `McpCard`. They are narrow **allow-lists** — a wire field
that no struct names is dropped silently, never printed (no
`deny_unknown_fields`). That is the forward-compat contract: the server adds
fields freely; the CLI stays quiet until a struct opts in.

**Field-name mapping**

| Wire key (JSON) | Rust field | Rule |
|---|---|---|
| `totalPages`, `mcpServers`, `indexReady`, … | `total_pages`, `mcp_servers`, `index_ready` | `#[serde(rename_all = "camelCase")]` on the envelopes + the skill structs |
| `docLanguage` | `language` | `#[serde(alias = "docLanguage")]` — still accepts a bare `language` |
| `llm_scan` / `cli_security` / `vettd_scan` | same, `Option<serde_json::Value>` | explicit `#[serde(rename = "…")]` to opt out of camelCase; the value is **never modeled** — held as raw JSON and echoed verbatim |
| `security_max_severity`, `security_direct_deps_*`, … | same names | `McpCard` has **no** `rename_all` — `McpHit` is already snake_case, so field names map 1:1 |
| `"weeklyDownloads": null` / absent | `None` | wire `null` or missing → `None` → key omitted from the re-serialized `--json` |
| anything else | — | dropped |

**Why beta fields don't leak into non-beta `--json`.** Every beta field is
`Option<T>` + `#[serde(skip_serializing_if = "Option::is_none")]`. When the
server didn't send it the field is `None`, and re-serializing for `--json`
omits it — so a GET / non-beta response round-trips byte-identical to the
pre-beta shape. Same mechanism drops a wire `null`.

**Building the output** (`handle_search` / `handle_mcp_search`):

- **Beta always dual-dumps**, regardless of `--json`: it prints
  `--- SEARCH_BETA_TESTING: raw json ---` + the pretty-printed
  *re-serialized struct*, then `--- SEARCH_BETA_TESTING: formatted ---` +
  the table.
- `--json` only takes effect on the **non-beta** path (`if json && !beta` →
  print the pretty JSON, no table).
- **Skill table** (`print_cards`): columns
  `rating · name · source · scanned by · description`. Grade → a color-coded
  `[A]` badge; `sourceType` → `display_source_type`; `scannerRunCount + 1` →
  "N scanners".
- **MCP table** (`print_mcp_cards`): columns
  `mcp · category · registry · stars · dep vulns · description`. "dep vulns"
  is `security_direct_deps_vuln_count`.
- **The three verdict objects are not rendered in either table** — raw dump /
  `--json` only (a verdict column is a follow-up).
- `mcpServers` empty **and** `indexReady == false` → an explicit "catalog is
  not ready yet (indexReady=false)" message, never `No MCP servers for "…"`.
- Pagination hint (`use --page N to see more`) when `page < totalPages`.

## Worked examples — observed 2026-08-31 e2e run

Real output. Chain: isolated Qdrant `:6350` → fresh query-service `:8010` →
local `next dev` `:3001` (vettd branch
`feat/directory-mcp-search-and-verdicts`) → the CLI binary built on
`feat/directory-mcp-search-and-verdicts`. Setup for every command:

```bash
export HOME=/tmp/cli-home                 # scratch $HOME
export SEARCH_BETA_TESTING=1
export VETTD_DIRECTORY_ENDPOINT=http://localhost:3001/api/scans/ingest
BIN=./target/debug/vettd
```

### A — skill card with `llm_scan` + `cli_security`

```
$ $BIN directory search "e2e-testing" --json

--- SEARCH_BETA_TESTING: raw json ---
{
  "skills": [
    {
      "slug": "e2e-testing",
      "name": "e2e-testing",
      "description": "Playwright E2E testing patterns ...",
      "badgeStatus": "vettd",
      "overallGrade": "A",
      "sourceType": "github",
      "scannerRunCount": 0,
      "language": "en",                        # <- wire "docLanguage", aliased
      "agentCompatibility": [],
      "rankings": { "stars": 240095, "skillsShLeaderboardRank": 1,
                    "numberOfAggregators": 1, "officialClaudeMarketplace": false },
      "llm_scan": {
        "model": "openrouter/deepseek/deepseek-v3.2",
        "prompt_version": "37243f9d5700",
        "max_severity": "NONE",
        "finding_count": 0,
        "primary_threats": [],
        "overall_assessment": "... legitimate, well-documented E2E testing patterns ...",
        "scanned_at": "2026-08-30T21:47:35.882498+00:00"
      },
      "cli_security": {
        "grade": "C",
        "osv_snapshot_date": "2026-08-30",
        "packages": [
          { "package": "playwright", "ecosystem": "npm", "classification": "cli",
            "vuln_count": 1, "max_severity": "HIGH",
            "advisory_ids": ["GHSA-7mvr-c777-76hp"] }
        ]
      }
    }
  ],
  "total": 1, "page": 1, "totalPages": 1, "mock": false
}
--- SEARCH_BETA_TESTING: formatted ---
rating  name         source      scanned by    description
────────────────────────────────────────────────────────────────
[A]     e2e-testing  GitHub      1 scanner     Playwright E2E testing patterns, Page Object Model…
```

The verdict objects never reach the table — only the raw dump.
`vettd_scan` is absent here: this skill's catalog entry carried no
`vettd_scan_findings` in the test store.

### B — `vettd_scan` on a skill that has it (`daytona`, verdict block only)

```
      "vettd_scan": {
        "scan_id": "4cbdd8eb-9f69-45ea-a2a0-963abff76572",
        "overall_grade": "B",
        "trust_level": "Conditional",
        "has_malicious_findings": false,
        "finding_count": 13,
        "severity_counts": { "critical": 0, "high": 0, "medium": 3, "low": 8, "info": 2 },
        "categories_flagged": ["scripts", "best-practices"],
        "top_findings": [ { "rule_id": "…", "category": "scripts", "severity": "medium" } ]
      }
```

### C — MCP search, the `mcpServers` envelope

```
$ $BIN directory search "context7" --asset-type mcp --json

--- SEARCH_BETA_TESTING: raw json ---
{
  "mcpServers": [
    {
      "score": 1.0,
      "rank": 1,
      "mcp_id": "github:upstash/context7",
      "name": "io.github.upstash/context7",
      "description": "A Model Context Protocol server that fetches up-to-date ... docs ...",
      "readme": "![Cover](...)\n\n# Context7 ...",     # full readme, trimmed here
      "repo_url": "https://github.com/upstash/context7",
      "status": "active",
      "mcp_category": "server",
      "mcp_category_source": "rule",
      "sources": ["repo_scan", "official_registry", "glama"],
      "registry_type": "npm",
      "package_identifier": "@upstash/context7-mcp",
      "package_url": "https://www.npmjs.com/package/@upstash/context7-mcp",
      "deployment": "hybrid",
      "transport": "stdio",
      "has_installable_package": true,
      "has_remote": true,
      "attributes": ["hosting:remote-capable"],
      "license": "MIT License",
      "added": "2026-08-17",
      "stars": 61393,
      "language": "TypeScript",
      "weekly_downloads": 918346,
      "monthly_downloads": 3853157,
      "security_source": "osv",
      "security_vuln_count": 0,
      "security_vuln_ids": [],
      "security_direct_deps_scanned": 8,
      "security_direct_deps_vuln_count": 44,
      "security_direct_deps_with_vulns": ["zod", "jose", "undici", "express"],
      "security_direct_deps_max_severity": "HIGH"
      # security_max_severity: null  -> omitted (skip_serializing_if)
    }
  ],
  "total": 1, "page": 1, "totalPages": 1, "mock": false, "indexReady": true
}
--- SEARCH_BETA_TESTING: formatted ---
mcp                      category    registry     stars  dep vulns  description
───────────────────────────────────────────────────────────────────────────────
github:upstash/context7  server      npm          61393         44  A Model Context Protocol server that fetches u…

Showing 1 of 1 MCP servers for "context7".
```

### D — guard rails (all exit 1, nothing sent)

```
$ $BIN directory search "x" --rank-filter bogus
Error: --rank-filter 'bogus' must be in key=N form

$ unset SEARCH_BETA_TESTING; $BIN directory search "x" --asset-type mcp
Error: search filters (--language/--agent-compatibility/--rankings/--source/--rank-filter/--asset-type mcp/--mcp-category/--deployment/--registry-type) require SEARCH_BETA_TESTING=1.

$ $BIN inventory search "x" --asset-type mcp          # even with the beta flag on
Error: --asset-type mcp is not supported for `inventory search` — the MCP catalog is not user-scoped. Use `vettd directory search --asset-type mcp` instead.
```

### E — non-beta `--json` is untouched

```
$ unset SEARCH_BETA_TESTING; $BIN directory search "e2e-testing" --json
{
  "skills": [
    {
      "slug": "e2e-testing",
      "name": "e2e-testing",
      "description": "Mock skill fixture ...",
      "badgeStatus": "vettd",
      "overallGrade": "A",
      "sourceType": "github",
      "scannerRunCount": 0
    }
  ],
  "total": 1, "page": 1, "totalPages": 1
}

# no language / agentCompatibility / rankings / llm_scan / cli_security /
# vettd_scan / mock — skip_serializing_if keeps it byte-identical to pre-beta.
```

## Gating

Both the `VETTD_DIRECTORY_ENDPOINT`/`VETTD_INVENTORY_ENDPOINT` overrides and
the new filters are strictly opt-in — with `SEARCH_BETA_TESTING` unset,
behavior (including `--json` output shape) is byte-identical to before this
change. `language`/`agentCompatibility`/`rankings`/`llm_scan`/`cli_security`/
`vettd_scan` are marked `#[serde(skip_serializing_if = "Option::is_none")]`
on `DirectoryCard` specifically so they don't leak into old-shape `--json`
output as `null` when the server doesn't send them. `--asset-type mcp` and
every new filter flag (`--source`, `--rank-filter`, `--mcp-category`,
`--deployment`, `--registry-type`) are a hard exit-1 error without the beta
flag, matching `--language`/`--rankings`.

## Manual testing

`crates/vettd-cli/tests/search_integration.rs` has an `#[ignore]`d test,
`manual_mock_server_for_local_testing`, that stands up the same `httpmock`
server the automated tests use and blocks for 10 minutes so you can drive
the real `vettd` binary against it from another terminal:

```bash
cargo build -p vettd-cli --bin vettd   # make sure ./target/debug/vettd is fresh
cargo test -p vettd-cli --test search_integration \
    manual_mock_server_for_local_testing -- --ignored --nocapture
```

It prints the mock's base URL and the exact commands below (port varies per
run) — every line here was copy-pasted from that output and run for real.
Two things the printed instructions call out that are easy to get wrong:

- Use **`./target/debug/vettd`**, not `vettd` — there's nothing on `PATH`.
- `vettd auth` writes to your real `~/Library/Application Support/vettd/config.json`
  (or `~/.config/vettd/` on Linux). Point `$HOME` at a scratch directory
  first so it never touches real saved credentials.

```bash
export HOME=/tmp/vettd-manual-home

# Old shape (SEARCH_BETA_TESTING unset), against a saved config endpoint:
./target/debug/vettd auth --endpoint http://127.0.0.1:<port>/api/scans/ingest --key mock-api-key-123
./target/debug/vettd directory search pdf --json

# New shape (SEARCH_BETA_TESTING=1), via the env var override — no config needed:
export SEARCH_BETA_TESTING=1
export VETTD_DIRECTORY_ENDPOINT=http://127.0.0.1:<port>/api/scans/ingest
./target/debug/vettd directory search pdf --json
./target/debug/vettd directory search pdf --language python --agent-compatibility claude-code --rankings '{"stars": 50}' --json
./target/debug/vettd directory search pdf --source marketplace --rank-filter search_rank_skills_sh_rank=100 --json
./target/debug/vettd directory search context7 --asset-type mcp --json

# inventory also needs the `auth` step above for its api key
export VETTD_INVENTORY_ENDPOINT=http://127.0.0.1:<port>/api/scans/ingest
./target/debug/vettd inventory search notes --json
```

httpmock logs every request it receives, so you can see the exact POST body
the CLI sent. **Note:** if you test the old (no-flag) shape without running
the `vettd auth` step first, the CLI falls through to the real production
endpoint instead — the env var override is correctly inert without the beta
flag, so there's no accidental mock in that path either (confirmed live: an
un-authed run during testing this doc made a real, harmless GET to
production before the fix to always `auth` first was written up above).

## Open questions

- Is `--language`/`--agent-compatibility` an AND (skill must match all
  given values) or an OR (skill must match any)? Precedent: `--rankings` is
  AND-of-thresholds, so probably AND for consistency, but worth confirming
  against the real API contract. The CLI currently passes the raw list
  through unfiltered — the AND/OR semantics are the server's to define.
- Should `--rankings` reject unknown keys server-side, or ignore them the
  same way response deserialization does client-side?
