#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# Telemetry field gate — vettd-cli#965 (spike #828)
#
# PURPOSE:
#   Keep the three descriptions of the passive-observer telemetry
#   payload from drifting apart. The payload surface is described in
#   three places, each of which enforces something the others cannot:
#
#     telemetry-field-gate.json     — the egress allowlist. Every leaf
#                                     path that may leave the machine,
#                                     its disclosure category, its
#                                     closed enum, and the value-level
#                                     rules a key-path walker cannot see.
#     telemetry-envelope.schema.json — the wire contract the ingest API
#                                     validates against (JSON Schema).
#     contract/disclosure.rs        — the categories the CLI shows a
#                                     user before anything is sent.
#
#   A field added to one and not the others is exactly the failure this
#   script exists to catch: a payload the cloud accepts but the gate
#   never authorised, or a category disclosed to no one.
#
# CHECKS (exit non-zero on any failure):
#   1. Versions       — both files parse; gate.envelopeVersion ==
#                       schema.properties.envelope_version.const ==
#                       observe/envelope.rs ENVELOPE_VERSION; and
#                       gate.gateVersion == 1.
#   2. Leaf parity    — the gate's `fields` keys and the schema's leaf
#                       paths are the same set, modulo the three known
#                       nullable/array container paths the gate lists
#                       once and the schema cannot express as a leaf.
#                       Every enum present in both files is identical.
#   3. Disclosure     — every gate disclosureCategories[].name is a
#                       variant line in contract/disclosure.rs.
#
#   4. Golden payload  — the built binary's `observe check` accepts the
#                       committed golden envelope (the byte-exact output
#                       of the Python prototype). Steps 1-3 only compare
#                       the artifacts to each other; this is the only
#                       check that the COMPILED gate agrees with them.
#   5. Rules fire     — each tests/fixtures/observe/gate-negative/<rule>
#                       fixture is the golden with one thing wrong, and
#                       must be refused with exit 1 naming <rule>. A gate
#                       that accepted everything would pass step 4.
#
# PORTABILITY:
#   bash + python3 only, no `grep -P` and no GNU-only flags, so this
#   runs unchanged on a macOS developer machine and on CI.
#
# USAGE:
#   scripts/check-telemetry-field-gate.sh          # run from anywhere
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
GATE="$REPO_ROOT/telemetry-field-gate.json"
SCHEMA="$REPO_ROOT/telemetry-envelope.schema.json"
# Phase 4 introduces this file. Until it exists the ENVELOPE_VERSION
# cross-check is a warning, not a failure (see step 1).
ENVELOPE_RS="$REPO_ROOT/crates/vettd-cli/src/observe/envelope.rs"
DISCLOSURE_RS="$REPO_ROOT/crates/vettd-cli/src/contract/disclosure.rs"

# The gate version this script knows how to read. A bump means the gate
# grew rules this script does not check, so it must be a deliberate edit
# here rather than a silent pass.
EXPECTED_GATE_VERSION=1

failures=0
warnings=0

fail() { echo "::error::telemetry-field-gate: $1"; failures=$((failures + 1)); }
warn() { echo "::warning::telemetry-field-gate: $1"; warnings=$((warnings + 1)); }

# ── 1. Versions ──────────────────────────────────────────────────────
for f in "$GATE" "$SCHEMA"; do
    if [ ! -f "$f" ]; then
        fail "missing $f — the telemetry gate and envelope schema live at the repo root."
        exit 1
    fi
    if ! python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$f" 2>/dev/null; then
        fail "$f is not valid JSON — nothing downstream can be checked."
        exit 1
    fi
done

gate_envelope_version="$(python3 -c "
import json, sys
print(json.load(open(sys.argv[1])).get('envelopeVersion', ''))
" "$GATE")"

gate_version="$(python3 -c "
import json, sys
print(json.load(open(sys.argv[1])).get('gateVersion', ''))
" "$GATE")"

schema_envelope_version="$(python3 -c "
import json, sys
schema = json.load(open(sys.argv[1]))
print(schema.get('properties', {}).get('envelope_version', {}).get('const', ''))
" "$SCHEMA")"

if [ "$gate_version" != "$EXPECTED_GATE_VERSION" ]; then
    fail "gateVersion is '$gate_version' but this script checks gate v$EXPECTED_GATE_VERSION. A gate bump adds rules; update this script deliberately instead of letting a new gate pass unchecked."
fi

if [ -z "$gate_envelope_version" ]; then
    fail "telemetry-field-gate.json has no envelopeVersion."
elif [ "$gate_envelope_version" != "$schema_envelope_version" ]; then
    fail "envelope version mismatch: gate.envelopeVersion='$gate_envelope_version' but schema.properties.envelope_version.const='$schema_envelope_version'. The gate and the wire contract must describe the same envelope."
fi

# `grep -o` + `cut` rather than `grep -P` so this works with BSD grep.
if [ ! -f "$ENVELOPE_RS" ]; then
    warn "$ENVELOPE_RS does not exist yet (it lands in Phase 4 of docs/vettd-observe-port-plan.md) — ENVELOPE_VERSION could not be cross-checked against the gate and schema."
else
    envelope_rs_version="$(
        grep -o 'ENVELOPE_VERSION: &str = "[^"]*"' "$ENVELOPE_RS" | cut -d'"' -f2 || true
    )"
    if [ -z "$envelope_rs_version" ]; then
        # The file exists, so the constant is expected to be in it: an
        # empty result means it was renamed or reshaped and this check
        # has gone blind, which is a failure, not a warning.
        fail "no ENVELOPE_VERSION constant found in $ENVELOPE_RS — the extractor is stale or the constant was renamed."
    elif [ "$envelope_rs_version" != "$gate_envelope_version" ]; then
        fail "envelope version mismatch: $ENVELOPE_RS declares ENVELOPE_VERSION='$envelope_rs_version' but the gate and schema say '$gate_envelope_version'."
    fi
fi

# ── 2. Leaf-path parity and shared enums ─────────────────────────────
# Derives the schema's leaf paths in the gate's own pathSyntax (dot-joined
# keys, array elements as `[]`) and diffs them against the gate's `fields`
# keys. Issue lines go to stdout, one per line; the human count note goes
# to stderr.
parity_status=0
parity_out="$(python3 - "$GATE" "$SCHEMA" <<'PY'
import json
import sys

# The gate lists these three once as containers (two nullable objects and
# one array of objects) and lists their children as leaves; JSON Schema
# has no leaf at these paths, so the difference is expected and permanent
# for envelope 0.2.0. Any other difference is a hard failure.
EXPECTED_GATE_ONLY = {
    "records[].assets[].signals.context_cost_est",
    "records[].assets[].signals.tokens_attributed",
    "records[].tokens_by_model[]",
}

gate = json.load(open(sys.argv[1], encoding="utf-8"))
schema = json.load(open(sys.argv[2], encoding="utf-8"))
issues = []


def resolve(node, root, depth=0):
    """Follow local `$ref`s until a concrete subschema is reached."""
    while isinstance(node, dict) and "$ref" in node:
        ref = node["$ref"]
        if not ref.startswith("#/"):
            raise ValueError("non-local $ref %r" % ref)
        depth += 1
        if depth > 32:
            raise ValueError("$ref cycle at %r" % ref)
        node = root
        for part in ref[2:].split("/"):
            node = node[part]
    return node


def walk(node, path, root, leaves, enums):
    """Collect leaf paths and the enum at each, in the gate's path syntax."""
    node = resolve(node, root)
    branches = node.get("oneOf") or node.get("anyOf")
    if branches:
        # A nullable object is `oneOf: [null, <object>]`; the null branch
        # carries no paths, so only the value branch contributes leaves.
        for branch in branches:
            branch = resolve(branch, root)
            if branch.get("type") == "null":
                continue
            walk(branch, path, root, leaves, enums)
        return
    if "properties" in node:
        for key, child in node["properties"].items():
            walk(child, "%s.%s" % (path, key) if path else key, root, leaves, enums)
        return
    if "items" in node:
        walk(node["items"], path + "[]", root, leaves, enums)
        return
    leaves.add(path)
    if "enum" in node:
        enums[path] = node["enum"]


schema_leaves = set()
schema_enums = {}
walk(schema, "", schema, schema_leaves, schema_enums)

gate_fields = gate.get("fields", {})
gate_paths = set(gate_fields)

gate_only = gate_paths - schema_leaves
schema_only = schema_leaves - gate_paths

for path in sorted(gate_only - EXPECTED_GATE_ONLY):
    issues.append(
        "leaf-path parity: '%s' is allowed by the gate but is not a leaf of "
        "telemetry-envelope.schema.json — the cloud would reject a payload the gate authorises."
        % path
    )
for path in sorted(schema_only):
    issues.append(
        "leaf-path parity: '%s' is a schema leaf with no telemetry-field-gate.json entry — "
        "a field the wire contract accepts but no disclosure category covers." % path
    )
for path in sorted(EXPECTED_GATE_ONLY - gate_only):
    issues.append(
        "leaf-path parity: '%s' is recorded as a known gate-only container but is no longer one. "
        "Update EXPECTED_GATE_ONLY in scripts/check-telemetry-field-gate.sh." % path
    )

gate_enums = gate.get("enums", {})
for path in sorted(gate_paths & schema_leaves):
    name = gate_fields[path].get("enum")
    in_schema = path in schema_enums
    if name is None:
        if in_schema:
            issues.append(
                "enum parity: '%s' is a closed enum in the schema but the gate does not name an "
                "enum for it — the gate would let an unlisted value through." % path
            )
        continue
    if name not in gate_enums:
        issues.append("enum parity: '%s' names enum '%s', which the gate does not define." % (path, name))
        continue
    if not in_schema:
        issues.append(
            "enum parity: '%s' is enum '%s' in the gate but is not a closed enum in the schema."
            % (path, name)
        )
        continue
    if json.dumps(gate_enums[name]) != json.dumps(schema_enums[path]):
        issues.append(
            "enum parity: enum '%s' at '%s' differs between the gate and the schema — the members "
            "and their order must be identical." % (name, path)
        )

sys.stderr.write(
    "telemetry-field-gate: gate fields=%d, schema leaves=%d, known gate-only containers=%d\n"
    % (len(gate_paths), len(schema_leaves), len(EXPECTED_GATE_ONLY))
)
for line in issues:
    print(line)
PY
)" || parity_status=$?

if [ "$parity_status" -ne 0 ]; then
    fail "leaf-path parity check could not run (python3 exited $parity_status) — see the traceback above."
elif [ -n "$parity_out" ]; then
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        fail "$line"
    done <<<"$parity_out"
fi

# ── 3. Disclosure categories ─────────────────────────────────────────
# Every category the gate assigns fields to must be a variant a user can
# actually be shown. Matching the variant line (not any mention) keeps a
# doc comment or an ALL_CATEGORIES entry from standing in for the variant.
if [ ! -f "$DISCLOSURE_RS" ]; then
    fail "missing $DISCLOSURE_RS — disclosure categories cannot be checked."
else
    category_names="$(python3 -c "
import json, sys
gate = json.load(open(sys.argv[1]))
print('\n'.join(c['name'] for c in gate.get('disclosureCategories', [])))
" "$GATE")"

    if [ -z "$category_names" ]; then
        fail "telemetry-field-gate.json declares no disclosureCategories."
    else
        while IFS= read -r name; do
            [ -n "$name" ] || continue
            if ! grep -qE "^[[:space:]]*${name},$" "$DISCLOSURE_RS"; then
                fail "disclosure category '$name' is in the gate but is not a DisclosureCategory variant in $DISCLOSURE_RS — telemetry would be sent under a category the CLI never discloses."
            fi
        done <<<"$category_names"
    fi
fi


# ── 4. The gate as the binary enforces it, on the golden payload ─────
# Steps 1-3 compare the two JSON artifacts and the disclosure enum to
# each other. None of them proves the compiled gate agrees: a Rust-side
# parsing bug, or a rule the loader silently drops, would leave all
# three files consistent and the enforcement broken. So run the real
# `observe check` over the committed golden envelope, which is the
# byte-exact output of the Python prototype (see the parity test in
# observe/envelope.rs) — the one payload we know to be correct.
FIXTURES="$REPO_ROOT/crates/vettd-cli/tests/fixtures/observe"
GOLDEN_ENVELOPE="$FIXTURES/golden/envelope.json"
GOLDEN_DYNAMIC="$FIXTURES/golden/dynamic.json"
NEGATIVE="$FIXTURES/gate-negative"

# `cargo run -q` so this works from a clean checkout; in CI the binary
# is already built by the preceding `cargo test --locked` step, so this
# is a no-op link. Cargo's own stderr is kept out of the rule matching.
VETTD=(cargo run --locked -q --manifest-path "$REPO_ROOT/Cargo.toml" -p vettd-cli --bin vettd --)

if ! command -v cargo >/dev/null 2>&1; then
    fail "cargo is not on PATH — steps 4 and 5 check the gate as the built binary enforces it and cannot be skipped silently."
else
    check_stderr="$(mktemp)"
    check_stdout="$(mktemp)"
    trap 'rm -f "$check_stderr" "$check_stdout"' EXIT

    run_check() {
        # run_check <payload> [dynamic]; sets $check_code, fills the temp files.
        local payload="$1" dynamic="${2:-}"
        set +e
        if [ -n "$dynamic" ]; then
            "${VETTD[@]}" observe check "$payload" --dynamic "$dynamic" \
                >"$check_stdout" 2>"$check_stderr"
        else
            "${VETTD[@]}" observe check "$payload" >"$check_stdout" 2>"$check_stderr"
        fi
        check_code=$?
        set -e
    }

    if [ ! -f "$GOLDEN_ENVELOPE" ]; then
        fail "missing $GOLDEN_ENVELOPE — the golden payload is what step 4 checks the compiled gate against."
    else
        run_check "$GOLDEN_ENVELOPE" "$GOLDEN_DYNAMIC"
        if [ "$check_code" -ne 0 ]; then
            fail "\`observe check\` rejected the golden envelope (exit $check_code). Either the gate file changed without the golden being regenerated, or the compiled gate no longer matches telemetry-field-gate.json. Output: $(tr '\n' ' ' <"$check_stderr")"
        fi
    fi

    # ── 5. Each rule actually fires ──────────────────────────────────
    # A gate that accepted everything would pass step 4. These fixtures
    # are the golden envelope with exactly one thing wrong; each must be
    # refused, naming its rule. Filename convention: the rule id, with
    # the one `-` standing in for the `:` in namespaced rules
    # (`pattern-url_scheme.json` → `pattern:url_scheme`). A fixture with
    # a `<name>.dynamic.json` sibling is passed that as --dynamic.
    if [ ! -d "$NEGATIVE" ]; then
        fail "missing $NEGATIVE — without negative fixtures nothing proves the gate rejects anything."
    else
        negative_seen=0
        for fixture in "$NEGATIVE"/*.json; do
            [ -f "$fixture" ] || continue
            base="$(basename "$fixture" .json)"
            # Sidecars are inputs, not fixtures.
            case "$base" in *.dynamic) continue ;; esac
            negative_seen=$((negative_seen + 1))

            # The first `-` becomes `:`; no plain rule id contains a dash.
            expected_rule="$(printf '%s' "$base" | sed 's/-/:/')"
            sidecar="$NEGATIVE/$base.dynamic.json"

            if [ -f "$sidecar" ]; then
                # The payload must be clean WITHOUT the sidecar, so the test
                # proves the dynamic set is what fired rather than something
                # else already wrong in the fixture.
                run_check "$fixture"
                if [ "$check_code" -ne 0 ]; then
                    fail "$base is meant to fail only against its dynamic sidecar, but it already fails on its own (exit $check_code): $(tr '\n' ' ' <"$check_stderr")"
                fi
                run_check "$fixture" "$sidecar"
            else
                run_check "$fixture"
            fi

            if [ "$check_code" -ne 1 ]; then
                fail "$base: expected \`observe check\` to exit 1 (violations found), got $check_code. A negative fixture the gate accepts means the '$expected_rule' rule is not being enforced. Output: $(tr '\n' ' ' <"$check_stderr")"
                continue
            fi
            # Match the rule in its `<path>: <rule>: <detail>` position, on the
            # violation lines only. Grepping the whole stream for the bare rule
            # id was tautological: the header line names the payload, whose
            # filename IS the rule id, so every fixture passed regardless of
            # which rule actually fired. Caught by renaming not_in_enum.json to
            # out_of_bounds.json and watching the script stay green.
            if ! grep -v 'violation(s) in' "$check_stderr" \
                | grep -q ": $expected_rule: "; then
                fail "$base: refused, but no violation line names '$expected_rule'. The fixture is exercising a different rule than its filename claims. Output: $(tr '\n' ' ' <"$check_stderr")"
            fi
            if grep -q "gate: OK" "$check_stdout"; then
                fail "$base: a refused payload must not also print a clean gate summary on stdout."
            fi
        done

        # The six rules the fixtures exist to pin. A fixture deleted rather
        # than replaced would otherwise shrink this check silently.
        if [ "$negative_seen" -lt 6 ]; then
            fail "only $negative_seen negative fixture(s) in $NEGATIVE; at least 6 are expected (unknown_key, not_in_enum, epoch_in_number, format_mismatch, pattern-url_scheme, dynamic-loaded_set_names). Deleting one removes a rule from enforcement testing."
        fi
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────
if [ "$failures" -gt 0 ]; then
    echo "::error::telemetry-field-gate FAILED with $failures issue(s). See telemetry-field-gate.json, telemetry-envelope.schema.json and docs/vettd-observe-port-plan.md."
    exit 1
fi

echo "telemetry-field-gate OK: gate=v${gate_version} envelope=${gate_envelope_version} (${warnings} warning(s))"
exit 0
