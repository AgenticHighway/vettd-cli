# Next.js API spec: `directory`/`inventory` search endpoints

## Summary

This describes the API surface the **Next.js app** (the production host behind
`vettd.agentichighway.ai`, or whatever host serves `/api/directory` and
`/api/inventory`) needs to implement to support the CLI changes on this
branch. See [`SEARCH_INTERFACE.md`](SEARCH_INTERFACE.md) for the CLI-side
request/response contract this derives from.

**One Next.js app serves both flows on the same paths.** There is no separate
beta subdomain or path prefix — `--language`/`--agent-compatibility`/
`--rankings` and the GET→POST switch are gated entirely on the *client* side
by the CLI's `SEARCH_BETA_TESTING` env var
([`network.rs`](../crates/vettd-cli/src/network.rs)'s
`search_beta_testing_enabled()`). The same route handler must accept both a
plain `GET` (old shape, always available, unauthenticated-safe) and a `POST`
with a JSON body (new shape) against the identical URL:

```
{base}/api/directory
{base}/api/inventory
```

`{base}` itself *can* differ per client — the CLI derives it from whichever
ingest endpoint is configured (`derive_api_url` in `network.rs`), and beta
testers may point `VETTD_DIRECTORY_ENDPOINT`/`VETTD_INVENTORY_ENDPOINT` at a
different host entirely (e.g. a local mock or staging deploy) — but for a
given deployment, normal and beta traffic land on the same route, only
differing by HTTP method. The Next.js route handler must not assume method
implies environment.

## Route: `GET /api/directory` (and `/api/inventory`) — unchanged

Existing behavior. Query params:

| Param | Type | Notes |
|---|---|---|
| `search` | string | search query |
| `sort` | `newest \| rating \| alpha` | |
| `page` | number | |

Response body (`DirectoryListResponse` shape) is unchanged — do not add
`language`/`agentCompatibility`/`rankings` fields to GET responses. The CLI's
non-beta client deserializes with an allow-list; extra fields are silently
dropped, but adding them here defeats the point of gating and risks
confusing non-beta consumers that assume a stable contract.

## Route: `POST /api/directory` (and `/api/inventory`) — new

Needs to be added. No auth/method gating is required server-side beyond
normal API auth — the beta flag is a CLI-only concept, not a server concept.
The server should simply support both verbs on the same path indefinitely
(or until the beta filters graduate to the default GET shape).

**Request body:**

```ts
type SearchRequestBody = {
  search: string;
  page: number;
  sort: "newest" | "rating" | "alpha";
  reverse: boolean;
  assetType: "skill" | "mcp";        // always present, default "skill"
  languages: string[];              // always present, [] if unfiltered
  agentCompatibility: string[];     // always present, [] if unfiltered
  sources: string[];                // always present, [] if unfiltered
  rankFilters: Record<string, number>;  // always present, {} if unfiltered
  rankings: RankingsFilter | null;  // null if --rankings not passed
  // only present when assetType === "mcp" (always present then, [] if unset):
  mcpCategory?: string[];
  deployment?: string[];
  registryType?: string[];
};

type RankingsFilter = {
  stars?: number;                        // "at least this value"
  skillsShLeaderboardRank?: number;      // "at least this value"
  numberOfAggregators?: number;          // "at least this value"
  officialClaudeMarketplace?: boolean;   // "must equal this value"
  // additional numeric/boolean keys are omitted-key = unconstrained
};
```

`assetType`, `sources`, `rankFilters` map to the query service's
`asset_type`, `sources`, `rank_filters` (`QueryRequest`). When
`assetType === "mcp"` the response is `McpSearchListResponse`
(`{mcpServers, total, page, totalPages, mock: false, indexReady}`) instead
of the skill envelope below; `POST /api/inventory` rejects
`assetType: "mcp"` with a 400. See `vettd/docs/search-beta-api-spec.md`
"Phase 3".

Validation the handler owns:
- `languages`/`agentCompatibility` filtering semantics (AND vs OR across
  multiple values) — **open question**, not yet decided by the CLI side; see
  `SEARCH_INTERFACE.md`'s "Open questions". The CLI passes the raw arrays
  through unfiltered, so this is purely a server-side decision.
- Whether unknown `rankings` keys are rejected (400) or ignored. Also open;
  pick one and document it, since the CLI does no client-side validation of
  key names today.

**Response body** — same envelope as GET, with three additive fields per
result item:

```ts
type DirectoryCard = {
  // existing fields, unchanged
  slug: string;
  name: string;
  description: string;
  version: string;
  author: string;
  category: string;
  badgeStatus: string;
  overallGrade: string;
  sourceType: string;
  scannerRunCount: number;

  // new, only on POST responses
  docLanguage?: string;   // named `docLanguage` on the wire, not `language`
  agentCompatibility?: string[];
  rankings?: {
    stars: number;
    skillsShLeaderboardRank: number;
    numberOfAggregators: number;
    officialClaudeMarketplace: boolean;
  };
  // opaque scan-verdict passthroughs from the query service (snake_case,
  // object | null) — forwarded verbatim, not modelled by the CLI
  llm_scan?: object | null;
  cli_security?: object | null;
  vettd_scan?: object | null;
};

type DirectoryListResponse = {
  skills: DirectoryCard[];
  total: number;
  page: number;
  totalPages: number;
  mock: boolean;   // true when SEARCH_BETA_MOCK_DATA served this response
};
```

The CLI's `DirectoryCard` struct marks these three fields
`#[serde(skip_serializing_if = "Option::is_none")]` specifically so a GET
response (which omits them) round-trips cleanly through `--json`. **Do not
send `docLanguage`/`agentCompatibility`/`rankings` as explicit `null` on GET
responses** — omit the keys entirely, matching today's behavior, so old
clients see byte-identical output.

The CLI deserializes `docLanguage` via a serde alias but still surfaces it
under its own `language` key in `--json` output — don't rename the CLI's
output field to match.

## Summary of what's needed in the Next.js app

- [x] Add a `POST` handler alongside the existing `GET` handler on
      `/api/directory` and `/api/inventory`, same route file/path.
- [x] Parse and validate `SearchRequestBody` (above), incl. `assetType` /
      `sources` / `rankFilters` / the mcp-only filters.
- [x] `assetType: "mcp"` on `POST /api/directory` → proxy the query service's
      `mcp_servers` catalog, response `McpSearchListResponse`; 400 on
      `POST /api/inventory`.
- [x] Extend the search/query layer to filter on language, agent
      compatibility, ranking thresholds, `sources`, `rankFilters`.
- [x] Include `language`/`agentCompatibility`/`rankings` and the
      `llm_scan`/`cli_security`/`vettd_scan` passthroughs in POST responses
      only — never on GET responses.

All of the above are implemented on the `vettd` branch
`feat/directory-mcp-search-and-verdicts` (see `vettd-e2e/E2E_TEST_PLAN_05.md`).
The CLI side (this repo) consumes them behind `SEARCH_BETA_TESTING`.
