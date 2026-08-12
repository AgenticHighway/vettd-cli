# Search interface: old vs. new shape

## Summary

Adds `--language`, `--agent-compatibility`, and `--rankings` filters to
`vettd directory search` / `vettd inventory search`, gated behind
`SEARCH_BETA_TESTING` (see
[`crates/vettd-cli/src/network.rs`](../crates/vettd-cli/src/network.rs)'s
`search_beta_testing_enabled()`), the same flag that gates
`VETTD_DIRECTORY_ENDPOINT`/`VETTD_INVENTORY_ENDPOINT`.

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
      "slug": "pdf-summarizer",
      "name": "PDF Summarizer",
      "description": "Extracts and summarizes long PDF reports.",
      "version": "2.3.0",
      "author": "acme-labs",
      "category": "productivity",
      "badgeStatus": "verified",
      "overallGrade": "A",
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

## New shape (`SEARCH_BETA_TESTING=1` — always POST + JSON)

**Input:** three new filters, additive to the existing query/page/sort/reverse.
Using any of them without the beta flag is a hard CLI error (exit 1) — never
silently ignored:

```
vettd directory search <query> \
  [--language <lang>]... \
  [--agent-compatibility <agent>]... \
  [--rankings '<json>'] \
  [--page N] [--sort newest|rating|alpha] [--reverse] [--json]
```

| Flag | Repeatable | Behavior |
|---|---|---|
| `--language <lang>` | yes | Filter to skills whose implementation language matches, e.g. `--language python --language typescript`. |
| `--agent-compatibility <agent>` | yes | Filter to skills compatible with the named agent/runtime, e.g. `--agent-compatibility claude-code --agent-compatibility cursor`. |
| `--rankings '<json>'` | no | Minimum-threshold filter — same field names as the response's `rankings` object. Numeric fields mean "at least this value," booleans mean "must equal this value." Omitted keys are unconstrained. |

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
  "languages": ["python", "typescript"],
  "agentCompatibility": ["claude-code", "cursor"],
  "rankings": {"stars": 50, "officialClaudeMarketplace": true}
}
```

`languages`/`agentCompatibility` are always present as arrays (empty if no
`--language`/`--agent-compatibility` flags were given). `rankings` is `null`
if `--rankings` wasn't passed. This body shape is sent even when none of the
three new filters are used — the beta flag alone is what switches GET→POST,
so a beta tester always sees the new request shape.

**Output** — same envelope, with three additions per skill: `language`,
`agentCompatibility` (the skill's actual values, not a filter), and the
nested `rankings` object. (The server sends this field as `docLanguage` on
the wire; the CLI deserializes it via a serde alias and still surfaces it
under `language` in `--json` output.) The envelope itself also gains a
top-level `mock` flag.

```json
{
  "skills": [
    {
      "slug": "pdf-summarizer",
      "name": "PDF Summarizer",
      "description": "Extracts and summarizes long PDF reports.",
      "version": "2.3.0",
      "author": "acme-labs",
      "category": "productivity",
      "badgeStatus": "verified",
      "overallGrade": "A",
      "sourceType": "github",
      "scannerRunCount": 12,
      "language": "python",
      "agentCompatibility": ["claude-code", "cursor"],
      "rankings": {
        "stars": 812,
        "skillsShLeaderboardRank": 14,
        "numberOfAggregators": 3,
        "officialClaudeMarketplace": true
      }
    }
  ],
  "total": 1,
  "page": 1,
  "totalPages": 1,
  "mock": false
}
```

`language`/`agentCompatibility`/`rankings`/`mock` are additive `Option` fields on
`DirectoryCard` (`crates/vettd-cli/src/directory.rs`) — old clients (without
the beta flag, or on an older CLI version) simply don't have the field and
continue to ignore it, per the existing forward-compatibility contract.

**Dual dump behavior (unchanged):** with `SEARCH_BETA_TESTING=1` the CLI
still prints the raw JSON response (now including the new fields) followed
by the formatted table, regardless of `--json`. The human-readable table
does not yet render a `rankings` summary column — exact layout TBD; the
fields are visible today via the raw-JSON dump / `--json`.

## Gating

Both the `VETTD_DIRECTORY_ENDPOINT`/`VETTD_INVENTORY_ENDPOINT` overrides and
the new filters are strictly opt-in — with `SEARCH_BETA_TESTING` unset,
behavior (including `--json` output shape) is byte-identical to before this
change. `language`/`agentCompatibility`/`rankings` are marked
`#[serde(skip_serializing_if = "Option::is_none")]` on `DirectoryCard`
specifically so they don't leak into old-shape `--json` output as `null`
when the server doesn't send them.

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
