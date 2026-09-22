#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# Scanner field gate — vettd-cli#243 (epic #879)
#
# PURPOSE:
#   Enforce the D4 ruling ("mechanism + recorded additive leaning") at
#   tag-bump time: a routine bump of the vettd-skill-scanner pin cannot
#   silently widen this CLI's published contract surface.
#
#   The pinned crate's SkillScanResult is the crate-side output surface.
#   Every field on it must be classified in scanner-field-gate.json as
#   either:
#   surface — surfaced into scanner-data-contract.json, at the boundary
#               named by the field's contractPath:
#               - `skills[].externalScannerResults[].<leaf>` → the scanner
#                 adapter (contract/skill_scan.rs), which must map the
#                 field into ExternalScannerResult, and the camelCase
#                 leaf must EXIST at exactly that location in
#                 scanner-data-contract.json (structural walk — a typo,
#                 wrong parent, or missing property fails);
#               - `skills[].<leaf>` (skill level) → the skill builder
#                 (contract/skills.rs), which must reference the field by
#                 an assignment-shaped occurrence (struct-literal `field:`
#                 or access `.field`), and the camelCase leaf must EXIST
#                 at `skills.items.properties` in the contract JSON.
#   gate    — kept out of the contract: the camelCase leaf must NOT exist
#               at either contract boundary (skill level or inside
#               `externalScannerResults`), and no assignment-shaped
#               reference may appear in the adapter or skill builder.
#   Unclassified fields FAIL the bump; unknown decision values FAIL.
#
# CHECKS (exit non-zero on any failure):
#   1. Pin match      — the tag pinned in crates/vettd-cli/Cargo.toml must
#                       equal scanner-field-gate.json pinTag.
#   2. Completeness   — every SkillScanResult field on the pinned crate
#                       must have a manifest entry (no silent new fields).
#   3. Decision valid — every manifest decision must be exactly
#                       `surface` or `gate`.
#   4. Surface mapped — a `surface` field must actually reach the contract
#                       at the boundary its contractPath names:
#                       (a) the contractPath is resolved STRUCTURALLY
#                       against scanner-data-contract.json (the declared
#                       leaf must exist at exactly that location), and
#                       (b) the adapter / skill builder references the
#                       field by an assignment-shaped occurrence in real
#                       code — Rust comments and string literals are
#                       stripped before matching, and a bare word or
#                       declaration (`let x:`, `pub x:`) does not count.
#   5. Gate unmapped  — a `gate` field's camelCase leaf must not exist at
#                       either contract boundary, and no assignment-shaped
#                       reference may appear in the adapter or skill
#                       builder.
#   (stale manifest entries — classified fields no longer on the crate —
#   are a warning, not a failure.)
#
# HOW IT FINDS THE CRATE:
#   `cargo metadata` resolves the pinned git dependency to its checked-out
#   path, so the gate inspects the REAL source at the pinned revision, not
#   a copied schema. Works offline once the dependency is fetched.
#
# USAGE:
#   scripts/check-scanner-field-gate.sh          # run from repo root
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
MANIFEST="$REPO_ROOT/scanner-field-gate.json"
CLI_CARGO="$REPO_ROOT/crates/vettd-cli/Cargo.toml"
CONTRACT_JSON="$REPO_ROOT/scanner-data-contract.json"
# The scanner→contract mapping boundary for `externalScannerResults[]`
# fields: the adapter that threads SkillScanResult fields into
# ExternalScannerResult. The assignment-shaped consistency check is scoped
# here (not the whole contract/ dir) so unrelated CLI-side fields that happen
# to share a name (e.g. the Prompt/Agent `signals` fields) cannot produce
# false results.
ADAPTER="$REPO_ROOT/crates/vettd-cli/src/contract/skill_scan.rs"
# The skill builder that surfaces skill-level fields (`skills[].<field>`).
# The check here requires an assignment-shaped occurrence (`field:` struct
# literal or `.field` access, e.g. `scan_result.file_count`), not a bare
# word match — skills.rs is far larger than the adapter and a bare word
# would be too noisy.
SKILLS_RS="$REPO_ROOT/crates/vettd-cli/src/contract/skills.rs"

failures=0
warnings=0

fail() { echo "::error::scanner-field-gate: $1"; failures=$((failures + 1)); }
warn() { echo "::warning::scanner-field-gate: $1"; warnings=$((warnings + 1)); }

# ── 1. Pin match ─────────────────────────────────────────────────────
pinned_tag="$(grep -oP 'tag\s*=\s*"\K[^"]+' "$CLI_CARGO" | head -1 || true)"
manifest_tag="$(python3 -c "import json,sys; print(json.load(open('$MANIFEST'))['pinTag'])" 2>/dev/null || echo '__unreadable__')"

if [ -z "$pinned_tag" ]; then
    fail "no tag= found in $CLI_CARGO — cannot determine the pinned crate revision"
elif [ "$pinned_tag" != "$manifest_tag" ]; then
    fail "pin mismatch: Cargo.toml pins vettd-skill-scanner@$pinned_tag but scanner-field-gate.json records pinTag=$manifest_tag. A tag bump must update the manifest AND classify any new SkillScanResult fields (D4 ruling: unclassified fields fail the bump)."
fi

# ── Resolve the pinned crate checkout ────────────────────────────────
crate_manifest_path="$(
    cargo metadata --format-version 1 --locked 2>/dev/null \
        | python3 -c "
import json, sys
try:
    data = json.load(sys.stdin)
except json.JSONDecodeError:
    sys.exit(0)  # cargo metadata produced no parseable output
for p in data.get('packages', []):
    if p['name'] == 'vettd-skill-scanner':
        print(p['manifest_path'])
        break
" 2>/dev/null || true
)"

if [ -z "$crate_manifest_path" ]; then
    fail "could not resolve vettd-skill-scanner via cargo metadata — is the pin valid?"
else
    crate_result_rs="$(dirname "$crate_manifest_path")/src/result.rs"

    if [ ! -f "$crate_result_rs" ]; then
        fail "pinned crate source not found at $crate_result_rs — the gate cannot enumerate SkillScanResult fields"
    else
        # Extract `pub field_name:` lines inside `pub struct SkillScanResult { ... }`
        crate_fields="$(
            awk '/pub struct SkillScanResult \{/,/^\}/' "$crate_result_rs" \
                | grep -oP '^\s+pub \K[a-z_]+(?=:)' || true
        )"

        if [ -z "$crate_fields" ]; then
            fail "no fields extracted from SkillScanResult at $crate_result_rs — the parser may be stale"
        else
            # ── 2+3+4+5. Completeness, decision validity, structural
            # contractPath resolution, surface ⇔ mapped, gate ⇔ unmapped ──
            # One python3 pass over every crate field. It resolves each
            # surface contractPath STRUCTURALLY against
            # scanner-data-contract.json (the declared leaf must exist at
            # exactly that location), requires an assignment-shaped
            # reference in real code at the declared boundary (Rust
            # comments/strings and the test module are stripped first — a
            # comment, string, or test mention can no longer satisfy the
            # check), hard-fails unknown decisions, and rejects any gated
            # field that reaches either boundary.
            consistency_rc=0
            consistency_output="$(
                python3 - "$MANIFEST" "$CONTRACT_JSON" "$ADAPTER" "$SKILLS_RS" "$crate_fields" <<'PY' 2>&1
import json
import re
import sys

manifest_path, contract_path, adapter_path, skills_path, crate_fields_arg = sys.argv[1:6]
crate_fields = [f for f in crate_fields_arg.split("\n") if f]

with open(manifest_path, encoding="utf-8") as fh:
    manifest = json.load(fh)
with open(contract_path, encoding="utf-8") as fh:
    contract = json.load(fh)

pin_tag = manifest.get("pinTag", "?")


def skill_props():
    return contract.get("properties", {}).get("skills", {}).get("items", {}).get("properties", {})


def esr_props():
    return skill_props().get("externalScannerResults", {}).get("items", {}).get("properties", {})


def camel_case(name):
    head, *rest = name.split("_")
    return head + "".join(part.capitalize() for part in rest)


def strip_rust(src):
    """Remove Rust comments and string/char literal bodies so only real code
    (no comments, no string mentions) is matched."""
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            i += 2
            while i < n and src[i] != "\n":
                i += 1
            continue
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            i += 2
            while i < n:
                if src[i] == "*" and i + 1 < n and src[i + 1] == "/":
                    i += 2
                    break
                i += 1
            continue
        if c == '"':
            i += 1
            while i < n:
                if src[i] == "\\":
                    i += 2
                    continue
                if src[i] == '"':
                    i += 1
                    break
                i += 1
            continue
        if c == "'":
            j = i + 1
            if j < n and src[j] != "'":
                k = j
                while k < n and src[k] != "\n":
                    if src[k] == "\\":
                        k += 2
                        continue
                    if src[k] == "'":
                        break
                    k += 1
                if k < n and src[k] == "'":
                    i = k + 1
                    continue
        out.append(c)
        i += 1
    return "".join(out)


def strip_test_code(src):
    """Remove `#[cfg(test)]`-gated blocks (the `mod tests` module at the bottom
    of the adapter / skill builder) so test-only references cannot satisfy a
    boundary check. Runs after comment/string stripping, so braces are balanced."""
    out = []
    i, n = 0, len(src)
    while i < n:
        m = re.compile(r"#\[cfg\(test\)\]\s*\n\s*mod\s+\w+\s*\{").search(src, i)
        if m is None:
            out.append(src[i:])
            break
        out.append(src[i:m.start()])
        depth = 0
        j = m.end() - 1  # index of the '{' that opens the mod block
        while j < n:
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        i = j + 1
    return "".join(out)


adapter_src = strip_test_code(strip_rust(open(adapter_path, encoding="utf-8").read()))
skills_src = strip_test_code(strip_rust(open(skills_path, encoding="utf-8").read()))


def assigned_in(src, field):
    """Assignment-shaped occurrence: struct-literal field (`field:` /
    `field :`) or field access (`.field`) in real code. Declarations
    (`let field:`, `pub field:`), bare words, comments and strings do NOT
    count."""
    literal = re.compile(r"(?<!let )(?<!pub )\b" + re.escape(field) + r"\s*:")
    access = re.compile(r"\." + re.escape(field) + r"\b")
    return literal.search(src) is not None or access.search(src) is not None


errors = []

for field in crate_fields:
    entry = manifest.get("fields", {}).get(field)
    if entry is None:
        errors.append(
            "unclassified SkillScanResult field: '{}' (crate @ {}). Add it to "
            "scanner-field-gate.json with decision surface|gate and a reason. "
            "D4 default leaning: ADDITIVE surfacing for optional additively-shaped "
            "fields.".format(field, pin_tag)
        )
        continue

    decision = entry.get("decision")
    if decision not in ("surface", "gate"):
        errors.append(
            "field '{}' has unknown decision '{}' — must be exactly 'surface' or "
            "'gate'. Unknown decisions silently fell through before; now they fail "
            "the bump.".format(field, decision)
        )
        continue

    contract_path = entry.get("contractPath", "")
    leaf = camel_case(field)

    if decision == "gate":
        # Structural: the camelCase leaf must not exist at either contract
        # boundary (skill-level or inside externalScannerResults).
        if leaf in skill_props():
            errors.append(
                "field '{}' is classified gate but the camelCase leaf '{}' EXISTS in "
                "scanner-data-contract.json skills items properties — a gated field "
                "must not enter the contract surface.".format(field, leaf)
            )
        if leaf in esr_props():
            errors.append(
                "field '{}' is classified gate but the camelCase leaf '{}' EXISTS in "
                "scanner-data-contract.json externalScannerResults items properties — "
                "a gated field must not enter the contract surface.".format(field, leaf)
            )
        # Code-side: no assignment-shaped occurrence at either boundary.
        if assigned_in(adapter_src, field):
            errors.append(
                "field '{}' is classified gate but IS assigned/referenced in the "
                "adapter ({}) — a gated field must not enter the contract "
                "surface.".format(field, adapter_path)
            )
        if assigned_in(skills_src, field):
            errors.append(
                "field '{}' is classified gate but IS assigned/referenced in the "
                "skill builder ({}) — a gated field must not enter the contract "
                "surface.".format(field, skills_path)
            )
        continue

    # ── surface: resolve the contractPath structurally and require an
    # assignment-shaped code reference at the declared boundary ──
    esr_prefix = "skills[].externalScannerResults[]"
    if contract_path.startswith(esr_prefix + "."):
        leaf = contract_path[len(esr_prefix) + 1:]
        if not leaf or "." in leaf:
            errors.append(
                "field '{}' has malformed externalScannerResults contractPath '{}' "
                "(expected 'skills[].externalScannerResults[].<field>').".format(field, contract_path)
            )
            continue
        if leaf not in esr_props():
            errors.append(
                "field '{}' is classified surface (contractPath '{}') but the leaf "
                "'{}' does not exist at skills[].externalScannerResults[].* in "
                "scanner-data-contract.json — a typo, a wrong parent, or a missing "
                "property. The contract JSON and the manifest disagree.".format(field, contract_path, leaf)
            )
        if not assigned_in(adapter_src, field):
            errors.append(
                "field '{}' is classified surface (contractPath '{}') but has no "
                "assignment-shaped occurrence (struct-literal '{}:' or access '.{}') "
                "in {} — a comment or a bare word does not map the scanner value "
                "into the contract payload.".format(field, contract_path, field, field, adapter_path)
            )
    elif contract_path.startswith("skills[]."):
        leaf = contract_path[len("skills[]."):]
        if not leaf or "." in leaf:
            errors.append(
                "field '{}' has malformed skill-level contractPath '{}' "
                "(expected 'skills[].<field>').".format(field, contract_path)
            )
            continue
        if leaf not in skill_props():
            errors.append(
                "field '{}' is surfaced at the skill level ({}) but the leaf '{}' "
                "does not exist at skills[].* in scanner-data-contract.json — a typo, "
                "a wrong parent, or a missing property. The contract JSON and the "
                "manifest disagree.".format(field, contract_path, leaf)
            )
        if not assigned_in(skills_src, field):
            errors.append(
                "field '{}' is surfaced at the skill level ({}) but has no "
                "assignment-shaped occurrence (struct-literal '{}:' or access '.{}') "
                "in {} — a comment or a bare word does not map the scanner value "
                "into the skill payload.".format(field, contract_path, field, field, skills_path)
            )
    else:
        errors.append(
            "surface field '{}' has unrecognized contractPath '{}' (expected "
            "'skills[].externalScannerResults[].*' or 'skills[].<field>').".format(field, contract_path)
        )

for message in errors:
    print("::error::scanner-field-gate: " + message)
sys.exit(1 if errors else 0)
PY
            )" || consistency_rc=$?
            if [ -n "$consistency_output" ]; then
                printf '%s\n' "$consistency_output"
                error_lines="$(grep -c '^::error::' <<<"$consistency_output" || true)"
                warn_lines="$(grep -c '^::warning::' <<<"$consistency_output" || true)"
                failures=$((failures + error_lines))
                warnings=$((warnings + warn_lines))
                # A python crash (traceback, no ::error:: lines) must NOT pass
                # silently — the gate cannot be satisfied vacuously.
                if [ "${consistency_rc:-0}" -ne 0 ] && [ "$error_lines" -eq 0 ]; then
                    fail "internal error in the field-gate consistency check (python exited ${consistency_rc:-0}) — see output above."
                fi
            fi

            # ── Stale manifest entries (warning only) ──
            while IFS= read -r field; do
                if ! grep -qx "$field" <<<"$crate_fields"; then
                    warn "manifest classifies '$field' but it is not on SkillScanResult @ $pinned_tag — stale entry (remove it, or the pin is not what the manifest claims)."
                fi
            done <<<"$(python3 -c "
import json
manifest = json.load(open('$MANIFEST'))
print('\n'.join(manifest.get('fields', {}).keys()))
")"
        fi
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────
if [ "$failures" -gt 0 ]; then
    echo "::error::scanner-field-gate FAILED with $failures issue(s). See scanner-field-gate.json and vettd-cli#243."
    exit 1
fi

echo "scanner-field-gate OK: pin=$pinned_tag manifest=OK (${warnings} warning(s))"
exit 0