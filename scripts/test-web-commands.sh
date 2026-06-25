#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# test-web-commands.sh — Smoke tests for the new web-facing read commands.
#
# Requires a running local dev server (default: http://localhost:3000).
# Runs against the compiled binary (cargo build before running, or set
# VETTD_BIN to point at a pre-built binary).
#
# Usage:
#   ./scripts/test-web-commands.sh
#   VETTD_BIN=./target/release/vettd ./scripts/test-web-commands.sh
#
# Deliberately violates test-isolation conventions: slug values are
# extracted from `directory list` output and reused in view/findings/compare.
#
# Tests are written to surface failures, not hide them. Known prod bugs
# (e.g. decode errors on detail endpoints) will cause FAILs here. That's
# intentional — this is a diagnostic tool, not a gate.
# ──────────────────────────────────────────────────────────────────────
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

VETTD_BIN="${VETTD_BIN:-./target/debug/vettd}"

PASS=0
FAIL=0

pass() { echo "  PASS  $1"; ((PASS++)); }
fail() { echo "  FAIL  $1"; ((FAIL++)); }

# Run a command; pass if exit code is 0.
run_test() {
    local name="$1"; shift
    local output exit_code
    output=$("$@" 2>&1) && exit_code=$? || exit_code=$?
    if [[ $exit_code -eq 0 ]]; then
        pass "$name"
    else
        fail "$name (exit $exit_code)"
        echo "        output: $(echo "$output" | head -3)"
    fi
}

# Run a command; pass if exit code is in the given space-separated list.
run_test_exits() {
    local name="$1"; local valid="$2"; shift 2
    local output exit_code
    output=$("$@" 2>&1) && exit_code=$? || exit_code=$?
    if echo " $valid " | grep -qF " $exit_code "; then
        pass "$name (exit $exit_code)"
    else
        fail "$name (exit $exit_code, expected one of: $valid)"
        echo "        output: $(echo "$output" | head -3)"
    fi
}

# Run a command; pass if exit code is NOT 0 (i.e. the error case worked).
run_test_fails() {
    local name="$1"; shift
    local output exit_code
    output=$("$@" 2>&1) && exit_code=$? || exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        pass "$name (exit $exit_code)"
    else
        fail "$name (expected non-zero exit, got 0)"
        echo "        output: $(echo "$output" | head -3)"
    fi
}

# Assert the captured output matches an extended-regex pattern.
check_content() {
    local name="$1"; local pattern="$2"; local output="$3"
    if echo "$output" | grep -qE "$pattern"; then
        pass "$name"
    else
        fail "$name (pattern not matched: $pattern)"
        echo "        output: $(echo "$output" | head -5)"
    fi
}

echo
echo "vettd web command smoke tests"
echo "Binary: $VETTD_BIN"
echo "────────────────────────────────────────"

# ── auth status ────────────────────────────────────────────────────────
echo
echo "auth"

# Valid exits: 0 (ok), 3 (not configured / key invalid), 5 (unreachable).
# Capture output for content checks.
auth_output=$("$VETTD_BIN" auth status 2>&1); auth_exit=$?

case $auth_exit in
    0)   pass "auth status (exit 0 — configured, reachable, key valid)" ;;
    3)   pass "auth status (exit 3 — not configured or key invalid)" ;;
    5)   fail "auth status (exit 5 — unreachable)" ;;
    *)   fail "auth status (unexpected exit $auth_exit)" ;;
esac
echo "        output: $(echo "$auth_output" | head -5)"

# Output always includes the endpoint line or the not-configured message.
check_content \
    "auth status prints endpoint or not-configured message" \
    "Endpoint:|Not configured" \
    "$auth_output"

# If exit 0, whoami enrichment (Account/Email/Role) must be present.
if [[ $auth_exit -eq 0 ]]; then
    check_content \
        "auth status (exit 0) includes whoami identity lines" \
        "Account:|Email:|Role:" \
        "$auth_output"
fi

# Reachability line should appear whenever the server was tried (exit 0 or 5).
if [[ $auth_exit -eq 0 || $auth_exit -eq 5 ]]; then
    check_content \
        "auth status shows reachability line" \
        "Reachability:" \
        "$auth_output"
fi

# ── contract status ────────────────────────────────────────────────────
echo
echo "contract"

# Valid exits: 0 (match), 3 (behind), 4 (ahead). Only 5 = server down.
contract_output=$("$VETTD_BIN" contract status 2>&1); cs_exit=$?

case $cs_exit in
    0|3|4) pass "contract status (exit $cs_exit)" ;;
    5)     fail "contract status (exit 5 — unreachable)" ;;
    *)     fail "contract status (unexpected exit $cs_exit)" ;;
esac
echo "        output: $(echo "$contract_output" | head -2)"

check_content \
    "contract status output contains Contract: line" \
    "Contract:" \
    "$contract_output"

# ── directory list ─────────────────────────────────────────────────────
echo
echo "directory"

LIST_OUTPUT=$("$VETTD_BIN" directory list 2>&1); list_exit=$?
if [[ $list_exit -ne 0 ]]; then
    fail "directory list (exit $list_exit)"
    echo "        output: $(echo "$LIST_OUTPUT" | head -3)"
    echo
    echo "  Cannot extract slugs — skipping slug-dependent tests."
    echo
    echo "────────────────────────────────────────"
    echo "  $PASS passed  $FAIL failed"
    echo
    [[ $FAIL -eq 0 ]] && exit 0 || exit 1
fi
pass "directory list"
check_content "directory list output has skill count line" \
    "[0-9]+ skills" "$LIST_OUTPUT"

# Extract slugs from list output. Grade badges are ANSI-colored — strip escape
# codes before pattern matching so the regex sees plain "[X] slug-name ..." text.
LIST_CLEAN=$(printf '%s\n' "$LIST_OUTPUT" | sed $'s/\033\[[0-9;]*m//g')
SLUGS=()
while IFS= read -r line; do
    if [[ "$line" =~ ^[[:space:]]*\[[A-Z?]\][[:space:]]+([^[:space:]]+) ]]; then
        SLUGS+=("${BASH_REMATCH[1]}")
    fi
done <<< "$LIST_CLEAN"

if [[ ${#SLUGS[@]} -lt 2 ]]; then
    echo "  WARNING: fewer than 2 slugs returned — using what we have."
fi

SLUG1="${SLUGS[0]:-}"
SLUG2="${SLUGS[1]:-$SLUG1}"  # fallback: same slug twice
echo "  Using slugs: ${SLUG1:-<none>}, ${SLUG2:-<none>}"

# ── directory search ─────────────────────────────────────────────────
search_out=$("$VETTD_BIN" directory search "code" 2>&1); s_exit=$?
if [[ $s_exit -eq 0 ]]; then pass "directory search (query: 'code')"; else
    fail "directory search (query: 'code') (exit $s_exit)"
    echo "        output: $(echo "$search_out" | head -3)"
fi
check_content "directory search output has results or no-results line" \
    "results|No results" "$search_out"

run_test "directory search (query with space: 'code review')" \
    "$VETTD_BIN" directory search "code review"

# No-results case — exits 0 with a "No results" message (not an error).
no_results_out=$("$VETTD_BIN" directory search "xyzzy_no_results_expected_9q3" 2>&1); nr_exit=$?
if [[ $nr_exit -eq 0 ]]; then pass "directory search (no-results exits 0)"; else
    fail "directory search (no-results exits $nr_exit, expected 0)"
    echo "        output: $(echo "$no_results_out" | head -3)"
fi
check_content "directory search (no-results output says No results)" \
    "No results" "$no_results_out"

# ── directory random ─────────────────────────────────────────────────
random_out=$("$VETTD_BIN" directory random 2>&1); r_exit=$?
if [[ $r_exit -eq 0 ]]; then pass "directory random"; else
    fail "directory random (exit $r_exit)"
    echo "        output: $(echo "$random_out" | head -3)"
fi
# Either a card line or the empty-pool message.
check_content "directory random output has card or no-skills message" \
    "\[|\]|No public skills" "$random_out"

# ── directory view ──────────────────────────────────────────────────
if [[ -n "$SLUG1" ]]; then
    view_out=$("$VETTD_BIN" directory view "$SLUG1" 2>&1); v_exit=$?
    if [[ $v_exit -eq 0 ]]; then pass "directory view ($SLUG1)"; else
        fail "directory view ($SLUG1) (exit $v_exit)"
        echo "        output: $(echo "$view_out" | head -3)"
    fi
    check_content "directory view shows Grade: line" \
        "Grade:" "$view_out"
    check_content "directory view shows Findings: line" \
        "Findings:" "$view_out"
else
    fail "directory view (no slug available)"
fi

# ── directory findings ───────────────────────────────────────────────
if [[ -n "$SLUG1" ]]; then
    findings_out=$("$VETTD_BIN" directory findings "$SLUG1" 2>&1); f_exit=$?
    if [[ $f_exit -eq 0 ]]; then pass "directory findings ($SLUG1, default)"; else
        fail "directory findings ($SLUG1, default) (exit $f_exit)"
        echo "        output: $(echo "$findings_out" | head -3)"
    fi
    check_content "directory findings output has Findings for header" \
        "Findings for" "$findings_out"

    run_test "directory findings ($SLUG1, --min-severity high)" \
        "$VETTD_BIN" directory findings "$SLUG1" --min-severity high

    run_test "directory findings ($SLUG1, --min-severity medium)" \
        "$VETTD_BIN" directory findings "$SLUG1" --min-severity medium

    run_test "directory findings ($SLUG1, --min-severity low)" \
        "$VETTD_BIN" directory findings "$SLUG1" --min-severity low

    run_test "directory findings ($SLUG1, --min-severity critical)" \
        "$VETTD_BIN" directory findings "$SLUG1" --min-severity critical
else
    fail "directory findings (no slug available)"
fi

# ── directory compare ────────────────────────────────────────────────
if [[ -n "$SLUG1" && -n "$SLUG2" ]]; then
    compare_out=$("$VETTD_BIN" directory compare "$SLUG1" "$SLUG2" 2>&1); cmp_exit=$?
    if [[ $cmp_exit -eq 0 ]]; then pass "directory compare ($SLUG1 vs $SLUG2)"; else
        fail "directory compare ($SLUG1 vs $SLUG2) (exit $cmp_exit)"
        echo "        output: $(echo "$compare_out" | head -3)"
    fi
    check_content "directory compare output has Findings: row" \
        "Findings:" "$compare_out"

    # Same slug twice — dedup path.
    same_out=$("$VETTD_BIN" directory compare "$SLUG1" "$SLUG1" 2>&1); same_exit=$?
    if [[ $same_exit -eq 0 ]]; then pass "directory compare (same slug both sides: $SLUG1)"; else
        fail "directory compare (same slug both sides: $SLUG1) (exit $same_exit)"
        echo "        output: $(echo "$same_out" | head -3)"
    fi
else
    fail "directory compare (no slugs available)"
fi

# ── error handling ──────────────────────────────────────────────────
echo
echo "error handling"

run_test_fails "directory view (404 on unknown slug)" \
    "$VETTD_BIN" directory view "this-slug-does-not-exist-xyzzy"

run_test_fails "directory findings (404 on unknown slug)" \
    "$VETTD_BIN" directory findings "this-slug-does-not-exist-xyzzy"

run_test_fails "directory compare (404 on first slug)" \
    "$VETTD_BIN" directory compare "this-slug-does-not-exist-xyzzy" "$SLUG1"

run_test_fails "directory compare (404 on second slug)" \
    "$VETTD_BIN" directory compare "$SLUG1" "this-slug-does-not-exist-xyzzy"

# ── summary ─────────────────────────────────────────────────────────
echo
echo "────────────────────────────────────────"
echo "  $PASS passed  $FAIL failed"
echo

[[ $FAIL -eq 0 ]] && exit 0 || exit 1
