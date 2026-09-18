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
#               - `skills[].externalScannerResults[].*` → the scanner
#                 adapter (contract/skill_scan.rs), which must map the
#                 field into ExternalScannerResult;
#               - `skills[].<field>` (skill level) → the skill builder
#                 (contract/skills.rs), which must reference the field
#                 via its access form (`.<field>`), and the camelCase
#                 leaf must be a property of `skills.items` in
#                 scanner-data-contract.json.
#   gate    — kept out of the contract (must NOT be referenced at either
#               boundary).
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
#                       at the boundary its contractPath names (adapter
#                       reference, or skill-builder access form + contract
#                       JSON property).
#   5. Gate unmapped  — a `gate` field must not be referenced at either
#                       boundary.
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
# ExternalScannerResult. The word-based consistency check is scoped here
# (not the whole contract/ dir) so unrelated CLI-side fields that happen to
# share a name (e.g. the Prompt/Agent `signals` fields) cannot produce
# false results.
ADAPTER="$REPO_ROOT/crates/vettd-cli/src/contract/skill_scan.rs"
# The skill builder that surfaces skill-level fields (`skills[].<field>`).
# The check here is access-form based (`.<field>`, e.g. `scan_result.file_count`),
# not a bare word match — skills.rs is far larger than the adapter and a
# bare word would be too noisy.
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
            # ── 2. Completeness: every crate field must be classified ──
            while IFS= read -r field; do
                if ! python3 -c "
import json, sys
manifest = json.load(open('$MANIFEST'))
sys.exit(0 if '$field' in manifest.get('fields', {}) else 1)
"; then
                    fail "unclassified SkillScanResult field: '$field' (crate @ $pinned_tag). Add it to scanner-field-gate.json with decision surface|gate and a reason. D4 default leaning: ADDITIVE surfacing for optional additively-shaped fields."
                fi
            done <<<"$crate_fields"

            # ── 3+4+5. Consistency: decision validity, surface ⇔ mapped, gate ⇔ unmapped ──
            while IFS= read -r field; do
                decision="$(python3 -c "
import json
manifest = json.load(open('$MANIFEST'))
print(manifest['fields']['$field']['decision'])
" 2>/dev/null || echo '__unreadable__')"

                if [ "$decision" != "surface" ] && [ "$decision" != "gate" ]; then
                    fail "field '$field' has unknown decision '$decision' — must be exactly 'surface' or 'gate'. Unknown decisions silently fell through before; now they fail the bump."
                    continue
                fi

                contract_path="$(python3 -c "
import json
manifest = json.load(open('$MANIFEST'))
print(manifest['fields']['$field'].get('contractPath', ''))
" 2>/dev/null || echo '')"

                if [ "$decision" = "gate" ]; then
                    # Must not be referenced at either boundary.
                    if grep -q "\b${field}\b" "$ADAPTER" || grep -qP "\.${field}\b" "$SKILLS_RS"; then
                        fail "field '$field' is classified gate but IS referenced at a contract boundary (adapter $ADAPTER or skill builder $SKILLS_RS) — a gated field must not enter the contract surface."
                    fi
                    continue
                fi

                # ── surface: resolve the boundary from contractPath ──
                if [[ "$contract_path" == "skills[].externalScannerResults"* ]]; then
                    # Adapter boundary: mapped into ExternalScannerResult.
                    if grep -q "\b${field}\b" "$ADAPTER"; then
                        mapped=1
                    else
                        mapped=0
                    fi
                    if [ "$mapped" -eq 0 ]; then
                        fail "field '$field' is classified surface (contractPath '$contract_path') but is not mapped in $ADAPTER — a surface field must actually reach the contract payload."
                    fi
                elif [[ "$contract_path" == "skills[]."* ]]; then
                    leaf="${contract_path#skills[].}"
                    if [ -z "$leaf" ] || [[ "$leaf" == *.* ]]; then
                        fail "field '$field' has malformed skill-level contractPath '$contract_path' (expected 'skills[].<field>')."
                        continue
                    fi
                    # Skill-level boundary: (a) access-form reference in the
                    # skill builder, (b) camelCase property in the contract JSON.
                    if ! grep -qP "\.${field}\b" "$SKILLS_RS"; then
                        fail "field '$field' is surfaced at the skill level ($contract_path) but is not referenced in $SKILLS_RS via its access form ('.$field') — a surface field must actually reach the skill payload."
                    fi
                    if ! python3 - "$CONTRACT_JSON" "$leaf" <<'PY'
import json, sys
contract = json.load(open(sys.argv[1]))
props = contract.get('properties', {}).get('skills', {}).get('items', {}).get('properties', {})
sys.exit(0 if sys.argv[2] in props else 1)
PY
                    then
                        fail "field '$field' is surfaced at the skill level ($contract_path) but '$leaf' is missing from scanner-data-contract.json skills.items.properties — the contract JSON and the manifest disagree."
                    fi
                else
                    fail "surface field '$field' has unrecognized contractPath '$contract_path' (expected 'skills[].externalScannerResults[].*' or 'skills[].<field>')."
                fi
            done <<<"$crate_fields"

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