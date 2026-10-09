# Negative fixtures for the telemetry field gate

Each `<rule>.json` here is the committed golden envelope
(`../golden/envelope.json`) with **exactly one thing wrong**, so
`scripts/check-telemetry-field-gate.sh` step 5 can prove the gate actually
refuses what it claims to refuse. Step 4 only proves the gate accepts a correct
payload; a gate that accepted everything would pass it.

`vettd observe check <fixture>` must exit 1 and name the rule in a
`<path>: <rule>: <detail>` line. The filename is the rule id with the single
`-` standing in for the `:` of a namespaced rule — `pattern-url_scheme.json`
pins `pattern:url_scheme`. A `<name>.dynamic.json` sibling is passed as
`--dynamic`.

| Fixture | Mutation | Fires |
|---|---|---|
| `unknown_key.json` | `records[0].leaked_field` added | `unknown_key` |
| `not_in_enum.json` | `records[0].run_outcome = "invented_outcome"` | `not_in_enum` |
| `epoch_in_number.json` | `coverage.bytes_read = 1700000000` | `epoch_in_number` |
| `format_mismatch.json` | `records[0].run_id` = 64 `z`s | `format_mismatch` |
| `pattern-url_scheme.json` | `resource.collector_version = "x-vettd://a"` | `format_mismatch` **and** `pattern:url_scheme` |
| `dynamic-loaded_set_names.json` | unchanged; the sidecar declares `3.4`, a substring of the golden's `harness_version` `3.4.5` | `dynamic:loaded_set_names` |

Two notes on why the table is not uniform:

- **`pattern-url_scheme` fires two rules.** Envelope 0.1.0 has no free-string
  leaf: every string is a closed enum, an exact format (hash, day, uuid), or a
  version. So a URL planted anywhere also fails its leaf's format. The pattern
  rules are defence in depth *behind* the formats, not the first line, and this
  fixture records that rather than hiding it. Step 5 asserts the named rule is
  among the violations.
- **`dynamic-loaded_set_names.json` is byte-identical to the golden.** The
  violation lives entirely in the sidecar, which is the point: the fail-closed
  substring rule refuses a payload whose *values are all legitimate* because a
  local-only name collides with one. Step 5 checks the payload is clean without
  the sidecar first, so the fixture cannot pass for the wrong reason.

## Regenerating

Only the mutations above are hand-authored; everything else is inherited from
the golden. Copy `../golden/envelope.json`, apply the one change from the table,
and rewrite it canonically — sorted keys, `(",", ":")` separators, ASCII-only,
one trailing newline, matching `observe::canonical::to_json_bytes`. A fixture
that is not canonical still works (the gate parses it), but a diff against the
golden then shows noise instead of the one changed value.
