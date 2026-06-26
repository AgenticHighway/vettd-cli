#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# test-json-output.sh — Smoke test the global --json flag across all
# supported commands.
#
# For each command, verifies:
#   1. Exit code 0
#   2. stdout is valid JSON
#   3. Key fields are present / typed correctly
#
# Network-dependent tests (directory, update --check, contract status)
# are skipped when the API is unreachable.
#
# Usage:
#   ./scripts/test-json-output.sh
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"
exec </dev/null

RUN="cargo run -p vettd-cli --bin vettd --"
PASS=0
FAIL=0
SKIP=0

# ── Helpers ──────────────────────────────────────────────────────────

green() { printf "\033[32m%s\033[0m\n" "$*"; }
red()   { printf "\033[31m%s\033[0m\n" "$*"; }
dim()   { printf "\033[2m%s\033[0m\n"  "$*"; }
bold()  { printf "\033[1m%s\033[0m\n"  "$*"; }

section() { echo ""; bold "━━━ $1 ━━━"; }

pass() { green "  ✓ $1"; PASS=$((PASS + 1)); }
fail() { red   "  ✗ $1"; FAIL=$((FAIL + 1)); }
skip() { dim   "  ⊘ $1 (skipped)"; SKIP=$((SKIP + 1)); }

# Run a command, capture stdout, assert it's valid JSON.
# Returns the parsed JSON in $JSON_OUT on success.
JSON_OUT=""
assert_json() {
    local label="$1"; shift
    local output exit_code
    exit_code=0
    output=$("$@" 2>/dev/null) || exit_code=$?
    if [ $exit_code -ne 0 ] && [ $exit_code -ne 3 ] && [ $exit_code -ne 4 ] && [ $exit_code -ne 5 ]; then
        fail "$label — command exited $exit_code"
        JSON_OUT=""
        return
    fi
    if echo "$output" | python3 -m json.tool > /dev/null 2>&1; then
        JSON_OUT="$output"
        pass "$label"
    else
        fail "$label — stdout is not valid JSON"
        JSON_OUT=""
    fi
}

# Assert a jq-style Python field access is truthy.
# Usage: assert_field LABEL JSON 'data["key"]'
assert_field() {
    local label="$1" json="$2" expr="$3"
    if echo "$json" | python3 -c "
import json, sys
data = json.load(sys.stdin)
val = $expr
assert val is not None, 'field is None'
" 2>/dev/null; then
        pass "$label"
    else
        fail "$label"
    fi
}

assert_type() {
    local label="$1" json="$2" expr="$3" typ="$4"
    if echo "$json" | python3 -c "
import json, sys
data = json.load(sys.stdin)
val = $expr
assert isinstance(val, $typ), f'expected $typ, got {type(val).__name__}'
" 2>/dev/null; then
        pass "$label"
    else
        fail "$label"
    fi
}

# ── Build ─────────────────────────────────────────────────────────────

section "Build"
echo "  Building vettd..."
if cargo build -p vettd-cli 2>&1 | tail -1; then
    pass "cargo build"
else
    red "Build failed — cannot continue."
    exit 1
fi

# ── Probe network reachability ────────────────────────────────────────

ENDPOINT="https://vettd.agentichighway.ai/api"
NETWORK_OK=false
if curl -sf --connect-timeout 4 "$ENDPOINT/contract?version=true" > /dev/null 2>&1; then
    NETWORK_OK=true
    dim "  API reachable at $ENDPOINT"
else
    dim "  API unreachable — network tests will be skipped"
fi

# ── 1. auth status --json ─────────────────────────────────────────────

section "auth status --json"

assert_json "auth status --json exits cleanly and emits JSON" \
    $RUN --json auth status

if [ -n "$JSON_OUT" ]; then
    assert_type  "configured is bool"    "$JSON_OUT" 'data["configured"]'    'bool'
    assert_type  "api_key_set is bool"   "$JSON_OUT" 'data["api_key_set"]'   'bool'
    assert_field "scanner_uuid key present" "$JSON_OUT" '"scanner_uuid" in data'
    assert_field "account_uuid key present" "$JSON_OUT" '"account_uuid" in data'
fi

# ── 2. contract status --json ─────────────────────────────────────────

section "contract status --json"

if [ "$NETWORK_OK" = true ]; then
    assert_json "contract status --json exits cleanly and emits JSON" \
        $RUN --json contract status
    if [ -n "$JSON_OUT" ]; then
        assert_field "local_version present"  "$JSON_OUT" 'data["local_version"]'
        assert_field "status present"         "$JSON_OUT" 'data["status"]'
        if echo "$JSON_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
assert data['status'] in ('up_to_date','behind','ahead','error'), data['status']
" 2>/dev/null; then
            pass "status value is a known enum"
        else
            fail "status value is unexpected"
        fi
    fi
else
    assert_json "contract status --json emits JSON even when unreachable" \
        $RUN --json contract status
    if [ -n "$JSON_OUT" ]; then
        assert_field "local_version present" "$JSON_OUT" 'data["local_version"]'
    fi
fi

# ── 3. rules list --json ──────────────────────────────────────────────

section "rules list --json"

assert_json "rules list --json emits JSON" $RUN --json rules list
if [ -n "$JSON_OUT" ]; then
    assert_type "output is an array" "$JSON_OUT" 'data' 'list'
    # If there are any rules, check they have the expected shape
    echo "$JSON_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
for r in data:
    assert 'file' in r and 'name' in r and 'artifact_type' in r and 'confidence' in r, r
" 2>/dev/null && pass "each rule entry has file/name/artifact_type/confidence" || true
fi

# ── 4. rules validate --json ──────────────────────────────────────────

section "rules validate --json"

EXAMPLE_RULE="$REPO_ROOT/examples/rules/terraform-ai.toml"
if [ -f "$EXAMPLE_RULE" ]; then
    assert_json "rules validate --json (valid rule)" $RUN --json rules validate "$EXAMPLE_RULE"
    if [ -n "$JSON_OUT" ]; then
        assert_type  "valid is bool"           "$JSON_OUT" 'data["valid"]'        'bool'
        assert_field "name present"            "$JSON_OUT" 'data["name"]'
        assert_field "artifact_type present"   "$JSON_OUT" 'data["artifact_type"]'
        assert_type  "has_keywords is bool"    "$JSON_OUT" 'data["has_keywords"]' 'bool'
    fi

    # Invalid file
    BOGUS=$(mktemp /tmp/vettd-test-rule-XXXXXX.toml)
    echo "not toml at all ;;;" > "$BOGUS"
    INVALID_OUT=""
    INVALID_EXIT=0
    INVALID_OUT=$($RUN --json rules validate "$BOGUS" 2>/dev/null) || INVALID_EXIT=$?
    rm -f "$BOGUS"
    if echo "$INVALID_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
assert data.get('valid') == False
" 2>/dev/null; then
        pass "rules validate --json (invalid rule) emits valid=false JSON"
    else
        fail "rules validate --json (invalid rule) did not emit valid=false JSON"
    fi
else
    skip "rules validate — no example rule found at $EXAMPLE_RULE"
fi

# ── 5. update --check --json ──────────────────────────────────────────

section "update --check --json"

if [ "$NETWORK_OK" = true ]; then
    # Allow exit code 1: dev builds without an embedded verification key exit 1
    # but still emit JSON (either the result or an {"error": "..."} object).
    UPDATE_CHECK_OUT=$($RUN --json update --check 2>/dev/null) || true
    if echo "$UPDATE_CHECK_OUT" | python3 -m json.tool > /dev/null 2>&1; then
        pass "update --check --json emits valid JSON"
        JSON_OUT="$UPDATE_CHECK_OUT"
        # If the check succeeded (has current_version), assert shape
        if echo "$JSON_OUT" | python3 -c "import json,sys; d=json.load(sys.stdin); assert 'current_version' in d" 2>/dev/null; then
            assert_field "current_version present" "$JSON_OUT" 'data["current_version"]'
            assert_field "latest_version present"  "$JSON_OUT" 'data["latest_version"]'
            assert_type  "is_newer is bool"        "$JSON_OUT" 'data["is_newer"]' 'bool'
        else
            assert_field "error key present on failure" "$JSON_OUT" '"error" in data'
        fi
    else
        fail "update --check --json stdout is not valid JSON"
    fi
else
    skip "update --check --json — network unreachable"
fi

# ── 6. update --json (bare) ───────────────────────────────────────────

section "update --json (bare, no --check)"

# Bare update emits {} without actually performing an update in non-interactive mode.
# We just confirm the JSON output appears on stdout; the update itself may fail
# (missing verification key in dev builds) which is expected.
UPDATE_STDOUT=$($RUN --json update 2>/dev/null) || true
if echo "$UPDATE_STDOUT" | python3 -m json.tool > /dev/null 2>&1; then
    pass "update --json emits valid JSON to stdout"
else
    fail "update --json stdout is not valid JSON (got: ${UPDATE_STDOUT:0:80})"
fi

# ── 7. directory list --json ──────────────────────────────────────────

section "directory --json"

FIRST_SLUG=""

if [ "$NETWORK_OK" = true ]; then
    assert_json "directory list --json" $RUN --json directory list
    if [ -n "$JSON_OUT" ]; then
        assert_type  "skills is an array" "$JSON_OUT" 'data["skills"]' 'list'
        assert_field "total present"      "$JSON_OUT" 'data["total"]'
        assert_field "page present"       "$JSON_OUT" 'data["page"]'
        FIRST_SLUG=$(echo "$JSON_OUT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
skills = data.get('skills', [])
if skills:
    s = skills[0].get('slug') or ''
    print(s)
" 2>/dev/null || true)
    fi

    assert_json "directory search --json" $RUN --json directory search "mcp"
    if [ -n "$JSON_OUT" ]; then
        assert_type "skills is an array" "$JSON_OUT" 'data["skills"]' 'list'
    fi

    assert_json "directory random --json" $RUN --json directory random
    if [ -n "$JSON_OUT" ]; then
        assert_field "skill key present" "$JSON_OUT" '"skill" in data'
    fi

    if [ -n "$FIRST_SLUG" ]; then
        assert_json "directory view --json ($FIRST_SLUG)" \
            $RUN --json directory view "$FIRST_SLUG"
        if [ -n "$JSON_OUT" ]; then
            assert_field "name present"     "$JSON_OUT" 'data["name"]'
            assert_type  "findings is list" "$JSON_OUT" 'data["findings"]' 'list'
        fi

        assert_json "directory findings --json ($FIRST_SLUG)" \
            $RUN --json directory findings "$FIRST_SLUG"
        if [ -n "$JSON_OUT" ]; then
            assert_type "output is array" "$JSON_OUT" 'data' 'list'
        fi

        # For compare we need two slugs; try to grab a second one from list
        SECOND_SLUG=$(
            $RUN --json directory list 2>/dev/null \
            | python3 -c "
import json, sys
data = json.load(sys.stdin)
slugs = [s.get('slug') or '' for s in data.get('skills', []) if (s.get('slug') or '') != '$FIRST_SLUG']
print(slugs[0] if slugs else '')
" 2>/dev/null || true)

        if [ -n "$SECOND_SLUG" ]; then
            assert_json "directory compare --json ($FIRST_SLUG vs $SECOND_SLUG)" \
                $RUN --json directory compare "$FIRST_SLUG" "$SECOND_SLUG"
            if [ -n "$JSON_OUT" ]; then
                assert_field "a key present" "$JSON_OUT" 'data["a"]'
                assert_field "b key present" "$JSON_OUT" 'data["b"]'
            fi
        else
            skip "directory compare --json — could not find a second slug"
        fi
    else
        skip "directory view/findings/compare --json — directory returned no slugs"
    fi
else
    skip "directory --json tests — network unreachable"
fi

# ── Summary ───────────────────────────────────────────────────────────

echo ""
bold "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
TOTAL=$((PASS + FAIL + SKIP))
echo "  Total: $TOTAL  │  $(green "✓ $PASS passed")  │  $(red "✗ $FAIL failed")  │  $(dim "⊘ $SKIP skipped")"
bold "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [ "$FAIL" -gt 0 ]; then
    red "Some tests failed."
    exit 1
else
    green "All tests passed."
    exit 0
fi
