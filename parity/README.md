# Parity test harness

Cross-scanner parity harness for issue [#133](https://github.com/AgenticHighway/vettd-cli/issues/133).

Compares the vettd-cli Rust scanner (`vettd-skill-scanner`) against the vettd
TypeScript scanner (`analyzeSkillFiles`) for the same skill inputs.

**Expected to fail** while the Rust engine is stubbed. That failure is the
goal-driven signal — the harness passes once the engine reaches parity.

---

## Prerequisites

- Python 3.10+
- An nvm-activated shell with Node v24.15 (`nvm use 24.15`)
- Rust toolchain (for `cargo run`)

---

## Setup

No Python packages to install — the harness uses only the standard library.
For the test suite, `pytest` is required:

```sh
pip install pytest
```

---

## Running the harness

All commands are run from the **vettd-cli repo root**.

### Case-test (synthetic inputs)

```sh
python parity/case_test.py \
  --vettd-adapter "../vettd/node_modules/.bin/tsx ../vettd/tools/parity-adapter.ts"
```

Omit `--cli-adapter` to use the default (`cargo run -q -p vettd-cli --bin parity-adapter --`), or override it:

```sh
python parity/case_test.py \
  --vettd-adapter "../vettd/node_modules/.bin/tsx ../vettd/tools/parity-adapter.ts" \
  --cli-adapter "cargo run -q -p vettd-cli --bin parity-adapter --"
```

Run a single case:

```sh
python parity/case_test.py \
  --vettd-adapter "..." \
  --case missing-skill-md
```

### Fixture-test (real skill directories)

```sh
python parity/fixture_test.py \
  --fixtures /path/to/skills/malicious-test-0506/skills \
  --vettd-adapter "../vettd/node_modules/.bin/tsx ../vettd/tools/parity-adapter.ts"
```

Run a single skill:

```sh
python parity/fixture_test.py \
  --fixtures /path/to/skills \
  --vettd-adapter "..." \
  --skill ai-code-reviewer
```

### Harness unit tests

```sh
pytest parity/test_compare.py -v
```

---

## Adapter protocol

Both adapters share the same stdin/stdout protocol:

**stdin** (JSON):
```json
{
  "textFiles": { "<rel-path>": "<utf8-content>", ... },
  "allPaths":  ["<rel-path>", ...]
}
```

**stdout** (JSON):
```json
{ "findings": [ ... ] }
```

**stderr**: human-oriented diagnostics only.  
**exit code**: 0 on success, non-zero on error.

---

## Comparison spec

See [#133 spec comment](https://github.com/AgenticHighway/vettd-cli/issues/133#issuecomment-4770174535)
for the authoritative spec. Summary:

| Field | Rule |
|---|---|
| `category`, `ruleId`, `label` | Match key (used to pair findings across scanners) |
| `severity`, `intent`, `chainId`, `source` | Exact match |
| `detail` | Fuzzy — strip `"Detected in <path>:<line> — "` prefix, compare remainder |
| `filepath` | **Excluded** — path normalization not guaranteed to align |
| `owaspLlmCategory` | **Excluded** — deprecated |
| `id`, `skillAuditId`, `fingerprint`, `sources`, `index` | **Excluded** — server-derived |

Third-party findings (`source != "vettd"`) are silently dropped from both sides.

---

## Known limitations

**Loader parity is out of scope.** The harness builds one file-map (via
`loader.py`) and feeds it to both engines identically. This measures *engine*
parity only. Production loaders differ:
- vettd: zip-extract with `isBinaryPath` + 4 MB per-file cap
- vettd-cli: 8 KB-truncating directory walk (noted stub in `skill_scan.rs`)

Reconciling loader behavior is a separate follow-on concern.

**nvm.** Run the Python harness from a shell where `nvm use 24.15` (or
equivalent) has already activated the right Node version. The subprocess
inherits `node` from the environment; the harness never calls `nvm` directly.
