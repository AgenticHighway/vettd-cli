# `vettd observe` — passive observation of local agent sessions

`vettd observe` reads Claude Code session transcripts already on your disk and produces a
report about which of your loaded assets — skills, sub-agents, MCP servers, rules files — are
actually being invoked, and how they behave when they are.

It is **off by default**, reads nothing until you opt in, and sends nothing until you pass
`--submit`. Everything it can transmit is hashes, integer counts, and values drawn from closed
enumerations. No message text, path, file name, asset name, session id, or timestamp finer than
a UTC calendar day can leave the machine — not by policy, but because an egress allowlist is
checked against the built payload and refuses to write or send anything it does not recognise.

## Contents

- [Quick start](#quick-start)
- [What is read, what is derived, what is sent](#what-is-read-what-is-derived-what-is-sent)
- [The 14 disclosure categories](#the-14-disclosure-categories)
- [The field gate](#the-field-gate)
- [Command reference](#command-reference)
- [Subcommands](#subcommands)
- [Exit codes](#exit-codes)
- [Local state, cursors and the ledger](#local-state-cursors-and-the-ledger)
- [Reading the report](#reading-the-report)
- [Auditing a payload](#auditing-a-payload)

## Quick start

```bash
# 1. Opt in. This writes [telemetry] enabled = true to ~/.vettd/.vettd.toml.
vettd observe enable

# 2. Look at what an observation would contain. Writes a file; sends nothing.
vettd observe --dry-run

# 3. Read the report and inspect the payload it wrote.
vettd observe check vettd-observations.json

# 4. Only when you want to: send it.
vettd observe --submit
```

Nothing between steps 1 and 3 touches the network. Step 4 requires saved credentials
(`vettd auth`) or `--api-key`.

## What is read, what is derived, what is sent

**Read** (locally, read-only, never transmitted):

- `~/.claude/projects/**/*.jsonl` and `*.ndjson` — Claude Code session transcripts, including
  message text, tool inputs and results, file paths, and session ids
- Sub-agent transcripts and their `.meta.json` sidecars
- `~/.claude/skills/**`, `agents/**`, and MCP configuration, to identify assets by content

**Derived** (on this machine, from what was read):

- A per-run pseudonym: `run_id = HMAC(device_secret, "claude_code:<session_key>")`. The secret
  lives at `~/.vettd/observer_secret`, is never transmitted, and is never rotated automatically.
  Because the secret is device-local, run ids from two machines cannot be linked.
- Per-asset identity: a content hash for a skill or rules file, a canonical-descriptor hash for
  an MCP server, or `HMAC(device_secret, "<asset_type>:<name>")` where there is no content to
  hash. The `key_basis` field says which of the three was used.
- Integer counts, mergeable statistics (`n`, `sum`, `min`, `max`, `sumsq`), and closed-enum
  classifications. Envelope v0.2 carries `sumsq` as a bounded decimal string so values above
  JavaScript's exact integer range cannot be rounded in transit.

**Sent** (only with `--submit`, and only what the gate allows):

- Exactly the 85 leaf paths in `telemetry-field-gate.json`, grouped into the 14 categories below.

The disclosure of those categories is printed to stderr **before any session file is opened**,
on every invocation — including one that refuses to proceed.

## The 14 disclosure categories

| Category | Leaves | What it is |
| --- | ---: | --- |
| Telemetry bookkeeping | 5 | Envelope, extractor, gate, and collector versions |
| Observation day | 2 | UTC calendar day of emission and of each observed run start; no finer time resolution |
| Device identity | 3 | The persisted scanner device id and a per-run pseudonym; the harness session id itself is never transmitted |
| Harness identity | 3 | Which supported harness produced the run, its semantic version, and a coarse entrypoint class |
| Model identity | 2 | The allowlisted model identifier the harness reported, or `other` |
| Run shape | 5 | Closed-enum descriptors: effort, permission mode, task category, loaded-set basis, run outcome |
| Run outcome counts | 9 | Turns, tool calls, failures by class, denials, sub-agent runs, compactions, unpaired calls, repeated near-identical calls |
| Run token totals | 16 | Token totals by provider bucket and the basis they were read from; never a cost figure |
| Asset identity hashes | 3 | One hash per asset with its type and key basis; never a name or path |
| Loaded set | 6 | The loaded-set hash, its membership as asset hashes, and per-asset attribution tier and binding |
| Asset outcome counts | 7 | Per asset per run: invocations, failures by class, harness-native corroboration count |
| Asset timing stats | 5 | Per asset per run: mergeable latency stats in ms, from harness timestamps |
| Asset token stats | 9 | Per asset per run: mergeable token stats where attribution is exact, plus a local context-cost estimate |
| Coverage metadata | 10 | What the collector saw and did not see, so silence is distinguishable from nothing to report |

Coverage metadata exists so that an empty report is legible. A harness format change shows up as
`lines_unknown_type`, an unreadable session as `sessions_skipped_unparseable`, and a session still
being written as `truncated_sessions` — rather than as a silently smaller number.

## The field gate

`telemetry-field-gate.json` at the repository root is the egress allowlist. Every payload is
checked against it **before anything is written, stored, or sent**. A violation prints the rule
and the path, exits 2, and leaves no file, no database row, and nothing on stdout.

The gate enforces four kinds of rule:

1. **Structural.** Every leaf path must be in the allowlist. An unrecognised key is a violation —
   and the diagnostic reports its *length*, never the key itself, because an unknown key could be
   the very content the gate exists to withhold.
2. **Value-level.** Closed enums must hold a listed member; hashes must be exactly 64 lowercase
   hex characters; days must be real calendar dates; numbers must sit inside declared bounds and
   must not fall in a Unix-timestamp range; `sumsq` decimal strings must be canonical and at most
   1e21.
3. **Forbidden patterns.** Twenty patterns reject anything shaped like a path, URL, hostname,
   email, IP address, clock time, uuid, tool-use id, MCP tool name, git ref, JWT, or API token.
4. **Dynamic, fail-closed.** The emitter hands the gate its own local vocabulary — the skill,
   agent, MCP server and rules-file names on this machine, plus the username, hostname and home
   directory — and any string leaf containing one of them as a substring is refused.

Rule 4 is deliberately over-eager. If a machine has a skill called `3.4` and the harness version
is `3.4.5`, the payload is refused. That is not a bug: the gate cannot tell a coincidence from a
leak, so it refuses and says which rule fired without echoing the value. The local report is
unaffected — rerun without `--out` or `--submit` to see it.

CI runs `scripts/check-telemetry-field-gate.sh`, which keeps the gate, the JSON Schema wire
contract, and the CLI's disclosure enum from drifting apart, then runs the built binary over a
golden payload and over six negative fixtures that each violate exactly one rule.

## Command reference

```
vettd observe [OPTIONS]
vettd observe enable
vettd observe status [--json]
vettd observe check <payload> [--dynamic <sets>]
```

| Flag | Default | Effect |
| --- | --- | --- |
| `--harness <name>` | `claude_code` | Harness to read. Only `claude_code` is supported today. |
| `--root <path>` | `~/.claude` | Harness home to read. |
| `--task <text>` | pooled | The task this evidence is for; omitted means the pooled, unspecified view. |
| `--window-days <n>` | `30` | How far back a session file may have been modified to be considered. |
| `--model <id>` | all | Narrow the report to one model id. |
| `--dry-run` | off | Build and gate-check the payload, write it, send nothing. Implies `--out`. |
| `--out [path]` | `vettd-observations.json` | Write the canonical payload. Bare `--out` uses the default name, which the repo's `.gitignore` already covers. |
| `--scrub` | off | Replace asset names in the report with `<type>:<hash prefix>`. |
| `--public-names <path>` | none | Names that may still be shown when scrubbing, one per line. |
| `--prices <path>` | compiled-in | Price table for the display-time cost lines. |
| `--submit [url]` | off | Send the payload. Bare `--submit` derives the route from your saved endpoint; `--submit <url>` is used verbatim. |
| `--api-key <key>` | saved | Credential for submission. |
| `--allow-public-endpoint` | off | Permit a public endpoint. Plain HTTP to a public host is refused regardless. |
| `--resend` | off | Send records the ledger already recorded as delivered, ignoring cursors too. |

`--json` (the global flag) writes the canonical envelope to stdout instead of the report. As
everywhere else in `vettd`, machine-readable output owns stdout and human output goes to stderr,
so `vettd --json observe --dry-run > payload.json` yields a file that parses.

## Subcommands

**`vettd observe enable`** appends `[telemetry] enabled = true` to `~/.vettd/.vettd.toml`. If the
table already exists — even set to `false` — it prints the path and the line to change instead of
editing. A value you deliberately set to `false` is a decision, and nothing rewrites it for you.

**`vettd observe status [--json]`** reports whether observation is enabled and where every piece
of local state lives, without creating any of it.

**`vettd observe check <payload>`** runs the field gate over a payload file. It is the audit tool:
anyone handed a payload can check it with no access to the machine that produced it. Pass
`--dynamic <sets>` to supply the emitter's local vocabulary as well; without it every structural
and value rule still applies, just not the substring rule.

`check` refuses a file with a duplicate JSON key rather than checking it. Most parsers keep the
last value for a duplicated key, so a payload could carry a leak in the first copy and a clean
value in the second and pass every rule — which is precisely what this command exists to catch.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success, or nothing new to send |
| 1 | Runtime error — bad `--root`, write failure, refused endpoint, submission failed |
| 2 | The payload failed the field gate; nothing was written or sent |
| 3 | Not configured — `[telemetry]` is off, or `--submit` with no credential |

`vettd observe check` uses its own scale: 0 clean, 1 violations found, 2 the input could not be
read.

3 is distinct from 1 on purpose. "You have not set this up" is not a failure, and a script that
treats it as one would be wrong.

## Local state, cursors and the ledger

| Path | Purpose |
| --- | --- |
| `~/.vettd/.vettd.toml` | The `[telemetry]` opt-in. Per-user only — a `.vettd.toml` in a repository you cloned can never opt you in. |
| `~/.vettd/observer_secret` | 32 random bytes, `0600`. The HMAC key behind every pseudonym. Never transmitted, never auto-rotated. |
| `~/.vettd/observer/observer-v1.sqlite3` | Read cursors and the submission ledger. Created only by `--submit`. |

Set `VETTD_HOME` to relocate vettd's per-user state for an isolated run. Under that directory,
the paths above keep their `.vettd/` prefix and saved auth lives at `.config/vettd/config.json`.

Cursors record how far into each transcript has been read. They are **change detectors**, not
resume points: a run whose transcript grew is rebuilt from byte zero, because a record is the
cumulative state of that run and not a partial delta. An unchanged run is skipped entirely.

The ledger records `(run_id, endpoint_host, record_sha256)` for each delivered record. Keying on
the record hash — not just the run — is what lets a run that was truncated when first observed be
sent again once it completes: same `run_id`, different record, so the server replaces its row
instead of the CLI treating it as already sent.

Both are **submission state**. A `--dry-run` never opens the store, so running one twice gives
identical output and can never starve a later submit. Cursors advance only after the server has
confirmed it holds the record: a payload written locally and then lost to a failed POST is not
mistaken for one that was delivered. Large windows are split into requests of at most 500 records
and 1 MiB; completed batches are ledgered, but cursors wait until every batch is confirmed.

Rotating the secret (delete the file) clears both cursors and the ledger, because every pseudonym
in them refers to a key that no longer exists.

## Reading the report

The report ranks assets by the **lower** end of what the evidence supports, using a Wilson score
interval ordered by its upper bound, so a skill invoked twice with two successes does not outrank
one invoked ninety times with eighty-five.

Display floors keep thin evidence off the screen rather than dressing it up. Each signal has its
own, because they need different amounts of evidence to mean anything:

| Signal | Shown from | Ordered by from |
| --- | ---: | ---: |
| Invocation count | 1 | 1 |
| Attributed tokens | 3 | 3 |
| Latency | 5 | 5 |
| Non-success rate | 20 | 50 |

A non-success rate is the strictest because it is the easiest to over-read: two failures in three
calls is not a 67% failure rate, it is three calls. Below its floor the report says how much
evidence there is instead of implying a conclusion.

Cost figures are display-time only, computed locally from a dated price table
(`crates/vettd-cli/resources/observe-prices.json` or `--prices`). No cost figure is ever part of a
payload — only token counts are, and prices change.

## Auditing a payload

```bash
# What would be sent, byte for byte, and whether it passes the gate.
vettd observe --dry-run --out payload.json
vettd observe check payload.json

# The same check a recipient can run, with no access to this machine.
vettd observe check payload.json

# Every distinct string anywhere in the payload, to read for yourself.
python3 - payload.json <<'PY'
import json, sys
def strings(node):
    if isinstance(node, str):
        yield node
    elif isinstance(node, dict):
        for key, value in node.items():
            yield key
            yield from strings(value)
    elif isinstance(node, list):
        for item in node:
            yield from strings(item)
print("\n".join(sorted(set(strings(json.load(open(sys.argv[1])))))))
PY
```

The bytes `--dry-run` writes are canonical JSON — sorted keys, no insignificant whitespace,
ASCII-only — so two payloads describing the same observation are byte-identical. For a large
submission, the CLI sends canonical record subsets of that same gate-checked document to honor
the ingest route's record and byte limits; it does not transform any record.
