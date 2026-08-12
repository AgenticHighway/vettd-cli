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
  languages: string[];              // always present, [] if unfiltered
  agentCompatibility: string[];     // always present, [] if unfiltered
  rankings: RankingsFilter | null;  // null if --rankings not passed
};

type RankingsFilter = {
  stars?: number;                        // "at least this value"
  skillsShLeaderboardRank?: number;      // "at least this value"
  numberOfAggregators?: number;          // "at least this value"
  officialClaudeMarketplace?: boolean;   // "must equal this value"
  // additional numeric/boolean keys are omitted-key = unconstrained
};
```

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

- [ ] Add a `POST` handler alongside the existing `GET` handler on
      `/api/directory` and `/api/inventory`, same route file/path.
- [ ] Parse and validate `SearchRequestBody` (above).
- [ ] Decide + implement AND/OR semantics for `languages`/`agentCompatibility`.
- [ ] Decide + implement handling of unknown `rankings` keys.
- [ ] Extend the search/query layer to filter on language, agent
      compatibility, and ranking thresholds.
- [ ] Include `language`/`agentCompatibility`/`rankings` in POST responses
      only — never on GET responses.
