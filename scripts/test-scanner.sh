#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# test-scanner.sh — Exercise every non-interactive vettd subcommand.
#
# Runs via `cargo run` so no separate binary build step is needed.
# Designed for macOS dev machines. No API key required — all tests
# run locally and validate exit codes + output.
#
# Usage:
#   ./scripts/test-scanner.sh
#
# Environment variables (optional — enables submission tests):
#   AH_TEST_API_KEY         API key for testing submissions
#   AH_TEST_LOCAL_ENDPOINT  Local server URL  (default: http://localhost:3000/api/scans/ingest)
#   AH_TEST_REMOTE_ENDPOINT Remote server URL (default: https://vettd.agentichighway.ai/api/scans/ingest)
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Keep the suite non-interactive even when launched from a TTY.
exec </dev/null

# Load .env if present (values from environment always take precedence)
if [[ -f "$REPO_ROOT/.env" ]]; then
    while IFS='=' read -r key value; do
        # Skip blank lines and comments
        [[ -z "$key" || "$key" == \#* ]] && continue
        # Only set if not already set in the environment
        if [[ -z "${!key+x}" ]]; then
            export "$key"="$value"
        fi
    done < "$REPO_ROOT/.env"
fi

RUN="cargo run -p vettd-cli --"
OUT_DIR="test-runs"
TIMESTAMP="$(date -u +%Y-%m-%dT%H-%M-%SZ)"
PASS=0
FAIL=0
SKIP=0

# Submission test config (from env or .env defaults)
AH_TEST_API_KEY="${AH_TEST_API_KEY:-}"
AH_TEST_LOCAL_ENDPOINT="${AH_TEST_LOCAL_ENDPOINT:-http://localhost:3000/api/scans/ingest}"
AH_TEST_REMOTE_ENDPOINT="${AH_TEST_REMOTE_ENDPOINT:-https://vettd.agentichighway.ai/api/scans/ingest}"

mkdir -p "$OUT_DIR"

# ── Helpers ──────────────────────────────────────────────────────────

green()  { printf "\033[32m%s\033[0m\n" "$*"; }
red()    { printf "\033[31m%s\033[0m\n" "$*"; }
dim()    { printf "\033[2m%s\033[0m\n" "$*"; }
bold()   { printf "\033[1m%s\033[0m\n" "$*"; }

section() {
    echo ""
    bold "━━━ $1 ━━━"
}

pass() {
    green "  ✓ $1"
    PASS=$((PASS + 1))
}

fail() {
    red "  ✗ $1"
    FAIL=$((FAIL + 1))
}

skip() {
    dim "  ⊘ $1 (skipped)"
    SKIP=$((SKIP + 1))
}

# Run a command, expect exit code 0
expect_ok() {
    local label="$1"; shift
    if "$@" > /dev/null 2>&1; then
        pass "$label"
    else
        fail "$label (exit code $?)"
    fi
}

# Run a command, expect non-zero exit code
expect_fail() {
    local label="$1"; shift
    if "$@" > /dev/null 2>&1; then
        fail "$label (expected failure but got 0)"
    else
        pass "$label"
    fi
}

# Run a command, capture stdout, check it contains a string
expect_contains() {
    local label="$1"; shift
    local needle="$1"; shift
    local output
    output=$("$@" 2>/dev/null) || true
    if echo "$output" | grep -q "$needle"; then
        pass "$label"
    else
        fail "$label — expected '$needle' in output"
    fi
}

# Run a command and verify the output file exists and is valid JSON
expect_json_file() {
    local label="$1"
    local file="$2"
    if [ ! -f "$file" ]; then
        fail "$label — file not found: $file"
        return
    fi
    if python3 -m json.tool "$file" > /dev/null 2>&1; then
        local size
        size=$(wc -c < "$file" | tr -d ' ')
        pass "$label (${size} bytes)"
    else
        fail "$label — invalid JSON: $file"
    fi
}

# ── Banner ───────────────────────────────────────────────────────────

echo ""
bold "┌──────────────────────────────────────────┐"
bold "│  vettd test suite — $TIMESTAMP  │"
bold "└──────────────────────────────────────────┘"

# ── 0. Build ─────────────────────────────────────────────────────────

section "Build"
echo "  Building vettd..."
if cargo build -p vettd-cli 2>&1 | tail -1; then
    pass "cargo build -p vettd-cli"
else
    fail "cargo build -p vettd-cli"
    echo ""
    red "Build failed — cannot continue."
    exit 1
fi

# ── 1. Help / version ───────────────────────────────────────────────

section "Help & version"
expect_ok    "vettd --help"         $RUN --help
expect_ok    "vettd --version"      $RUN --version
expect_ok    "vettd scan --help"    $RUN scan --help
expect_ok    "vettd quick --help"   $RUN quick --help
expect_ok    "vettd full --help"    $RUN full --help
expect_ok    "vettd file --help"    $RUN file --help
expect_ok    "vettd folder --help"  $RUN folder --help
expect_ok    "vettd repo --help"    $RUN repo --help
expect_ok    "vettd rules --help"   $RUN rules --help
expect_ok    "vettd auth --help"    $RUN auth --help
expect_ok    "vettd setup --help"   $RUN setup --help
expect_ok    "vettd update --help"  $RUN update --help

# Version flag output contains version number
VERSION_OUT=$($RUN --version 2>&1) || true
if echo "$VERSION_OUT" | grep -qE '^vettd [0-9]+\.[0-9]+\.[0-9]+$'; then
    pass "--version prints semver"
else
    fail "--version output unexpected: $VERSION_OUT"
fi

# ── 2. Single file scan ─────────────────────────────────────────────

section "Single file scan"

# Scan the repo's agents.md — a known agentic artifact
AGENTS_FILE="$REPO_ROOT/agents.md"
FILE_JSON="$OUT_DIR/${TIMESTAMP}-file.json"

expect_ok       "file scan (agents.md, overview)"  $RUN file "$AGENTS_FILE"
expect_ok       "file scan (agents.md, --full)"     $RUN file "$AGENTS_FILE" --full
expect_ok       "file scan (agents.md, --summary)"  $RUN file "$AGENTS_FILE" --summary

# JSON output to stdout
expect_contains "file scan (--json has scanMeta)" "scanMeta" $RUN file "$AGENTS_FILE" --json

# JSON output to file
$RUN file "$AGENTS_FILE" --out "$FILE_JSON" > /dev/null 2>&1 || true
expect_json_file "file scan (--out writes valid JSON)" "$FILE_JSON"

# Contract flag
expect_contains "file scan (--contract has scanMeta)" "scanMeta" $RUN file "$AGENTS_FILE" --contract

# ── 3. Folder scan ──────────────────────────────────────────────────

section "Folder scan"
FOLDER_JSON="$OUT_DIR/${TIMESTAMP}-folder.json"

expect_ok       "folder scan (this repo)"           $RUN folder "$REPO_ROOT"
expect_ok       "folder scan (--summary)"            $RUN folder "$REPO_ROOT" --summary
$RUN folder "$REPO_ROOT" --out "$FOLDER_JSON" > /dev/null 2>&1 || true
expect_json_file "folder scan (--out writes valid JSON)" "$FOLDER_JSON"

# ── 4. Repo scan ────────────────────────────────────────────────────

section "Repo scan (deep)"
REPO_JSON="$OUT_DIR/${TIMESTAMP}-repo.json"

expect_ok       "repo scan (this repo)"             $RUN repo "$REPO_ROOT"
$RUN repo "$REPO_ROOT" --json > "$REPO_JSON" 2>/dev/null || true
expect_json_file "repo scan (--json writes valid JSON)" "$REPO_JSON"

# ── 5. Quick scan ───────────────────────────────────────────────────

section "Quick scan (agentic config areas)"
QUICK_JSON="$OUT_DIR/${TIMESTAMP}-quick.json"

expect_ok       "quick scan (overview)"              $RUN quick
expect_ok       "quick scan (--summary)"             $RUN quick --summary
$RUN quick --out "$QUICK_JSON" > /dev/null 2>&1 || true
expect_json_file "quick scan (--out writes valid JSON)" "$QUICK_JSON"

# ── 6. Default scan ─────────────────────────────────────────────────

section "Default scan (tiered user-space surfaces)"
DEFAULT_JSON="$OUT_DIR/${TIMESTAMP}-default.json"

# This walks critical host roots plus bounded user-space roots
echo "  (this scans critical host roots plus bounded user-space roots — may take a few seconds)"
expect_ok       "default scan (overview)"            $RUN scan --summary
$RUN scan --json > "$DEFAULT_JSON" 2>/dev/null || true
expect_json_file "default scan (--json writes valid JSON)" "$DEFAULT_JSON"

# ── 7. Severity filter ──────────────────────────────────────────────

section "Severity filtering"
expect_ok "quick --min-severity=critical"  $RUN quick --min-severity critical --summary
expect_ok "quick --min-severity=high"      $RUN quick --min-severity high --summary
expect_ok "quick --min-severity=medium"    $RUN quick --min-severity medium --summary
expect_ok "quick --min-severity=low"       $RUN quick --min-severity low --summary

# ── 8. Rules management ─────────────────────────────────────────────

section "Rules management"
expect_ok    "rules list"                  $RUN rules list

# ── 9. Contract payload validation ──────────────────────────────────

section "Contract payload validation"

# Validate that contract JSON has the expected top-level keys
CONTRACT_JSON="$OUT_DIR/${TIMESTAMP}-contract-validate.json"
$RUN repo "$REPO_ROOT" --contract > "$CONTRACT_JSON" 2>/dev/null || true

for key in scanMeta prompts skills mcpServers agents agenticApps; do
    if python3 -c "
import json, sys
data = json.load(open('$CONTRACT_JSON'))
assert '$key' in data, '$key not found'
" 2>/dev/null; then
        pass "contract has '$key'"
    else
        fail "contract missing '$key'"
    fi
done

# scanMeta should have scanId, scannerVersion
for field in scanId scannerVersion scannedAt scanDurationMs; do
    if python3 -c "
import json
data = json.load(open('$CONTRACT_JSON'))
assert '$field' in data['scanMeta'], '$field not in scanMeta'
" 2>/dev/null; then
        pass "scanMeta.$field present"
    else
        fail "scanMeta.$field missing"
    fi
done

# ── 10. Fixture corpus ──────────────────────────────────────────────

section "Fixture corpus"

FIXTURE_ROOT="$REPO_ROOT/crates/vettd-cli/tests/fixtures/docker"
PLAIN_FIXTURE="$FIXTURE_ROOT/plain-image-definition"
DIRECT_FIXTURE="$FIXTURE_ROOT/direct-agentic-compose"
COLOCATED_FIXTURE="$FIXTURE_ROOT/colocated-agent-project"

PLAIN_JSON="$OUT_DIR/${TIMESTAMP}-fixture-plain.json"
DIRECT_JSON="$OUT_DIR/${TIMESTAMP}-fixture-direct.json"
COLOCATED_CONTRACT="$OUT_DIR/${TIMESTAMP}-fixture-colocated-contract.json"

$RUN folder "$PLAIN_FIXTURE" --json > "$PLAIN_JSON" 2>/dev/null || true
expect_json_file "plain Docker fixture (--json writes valid JSON)" "$PLAIN_JSON"
if python3 -c "
import json
data = json.load(open('$PLAIN_JSON'))
assert data['agenticApps'] == [], data['agenticApps']
" 2>/dev/null; then
    pass "plain Docker fixture stays out of agentic app output"
else
    fail "plain Docker fixture classification changed"
fi

$RUN folder "$DIRECT_FIXTURE" --json > "$DIRECT_JSON" 2>/dev/null || true
expect_json_file "direct agentic fixture (--json writes valid JSON)" "$DIRECT_JSON"
if python3 -c "
import json
data = json.load(open('$DIRECT_JSON'))
assert len(data['agenticApps']) == 1, f\"expected 1 app, got {len(data['agenticApps'])}\"
app = data['agenticApps'][0]
assert app['agentCount'] == 0, app['agentCount']
assert app['framework'] == 'LangGraph', app['framework']
assert 'Service orchestration configuration' in app['description'], app['description']
" 2>/dev/null; then
    pass "direct agentic fixture builds one agentic app"
else
    fail "direct agentic fixture classification changed"
fi

$RUN folder "$COLOCATED_FIXTURE" --contract > "$COLOCATED_CONTRACT" 2>/dev/null || true
expect_json_file "co-located agent fixture (--contract writes valid JSON)" "$COLOCATED_CONTRACT"
if python3 -c "
import json
data = json.load(open('$COLOCATED_CONTRACT'))
assert len(data['agents']) == 1, f\"expected 1 agent, got {len(data['agents'])}\"
assert len(data['agenticApps']) == 1, f\"expected 1 app, got {len(data['agenticApps'])}\"
assert data['agenticApps'][0]['agentCount'] == 1, data['agenticApps'][0]['agentCount']
" 2>/dev/null; then
    pass "co-located agent fixture builds one agentic app"
else
    fail "co-located agent fixture contract changed"
fi

# ── 11. Data quality checks (quick scan) ────────────────────────────

section "Data quality (quick scan contract)"

# Use the quick scan output which has real MCP + agent data
QUICK_JSON="$OUT_DIR/${TIMESTAMP}-quick.json"

# 10a: No duplicate MCP servers by name
if python3 -c "
import json, sys
data = json.load(open('$QUICK_JSON'))
names = [s['name'] for s in data['mcpServers']]
dupes = [n for n in set(names) if names.count(n) > 1]
if dupes:
    print(f'  Duplicates: {dupes}', file=sys.stderr)
    sys.exit(1)
" 2>&1; then
    pass "no duplicate MCP server names"
else
    fail "duplicate MCP server names found"
fi

# 10b: No duplicate tools per agent
if python3 -c "
import json, sys
data = json.load(open('$QUICK_JSON'))
for a in data['agents']:
    tools = [t['name']+':'+t['type'] for t in a['tools']]
    if len(tools) != len(set(tools)):
        dupes = [t for t in set(tools) if tools.count(t) > 1]
        print(f'  Agent \"{a[\"name\"]}\" has duplicate tools: {dupes}', file=sys.stderr)
        sys.exit(1)
" 2>&1; then
    pass "no duplicate agent tools"
else
    fail "duplicate agent tools found"
fi

# 10c: No duplicate dependentAgents per MCP server
if python3 -c "
import json, sys
data = json.load(open('$QUICK_JSON'))
for s in data['mcpServers']:
    deps = s['dependentAgents']
    if len(deps) != len(set(deps)):
        dupes = [d for d in set(deps) if deps.count(d) > 1]
        print(f'  Server \"{s[\"name\"]}\" has dup deps: {dupes}', file=sys.stderr)
        sys.exit(1)
" 2>&1; then
    pass "no duplicate dependentAgents"
else
    fail "duplicate dependentAgents found"
fi

# 10d: MCP auth is not always the same value
if python3 -c "
import json, sys
data = json.load(open('$QUICK_JSON'))
servers = data['mcpServers']
if len(servers) > 1:
    auths = set(s['auth'] for s in servers)
    # With proper per-server inference, not every server should have the same auth
    # At minimum, servers without env vars should be 'None'
    if len(auths) == 1 and 'API Key' in auths:
        print(f'  All {len(servers)} servers show auth=API Key', file=sys.stderr)
        sys.exit(1)
" 2>&1; then
    pass "MCP auth varies per server"
else
    fail "MCP auth is 'API Key' for every server"
fi

# 10e: scanMeta.hostNetwork exists and has firewall fields
if python3 -c "
import json, sys
data = json.load(open('$QUICK_JSON'))
hn = data['scanMeta']['hostNetwork']
assert isinstance(hn['firewallEnabled'], bool), 'firewallEnabled not bool'
assert isinstance(hn['firewallMode'], str), 'firewallMode not string'
assert isinstance(hn['stealthMode'], bool), 'stealthMode not bool'
assert isinstance(hn['firewallRules'], list), 'firewallRules not list'
" 2>&1; then
    pass "scanMeta.hostNetwork has firewall fields"
else
    fail "scanMeta.hostNetwork missing or malformed"
fi

# 10f: MCP servers have transport field
if python3 -c "
import json, sys
data = json.load(open('$QUICK_JSON'))
for s in data['mcpServers']:
    t = s.get('transport', '')
    if t not in ('stdio', 'sse', 'streamable-http', 'unknown'):
        print(f'  Server \"{s[\"name\"]}\" has bad transport: {t!r}', file=sys.stderr)
        sys.exit(1)
" 2>&1; then
    pass "MCP servers have valid transport field"
else
    fail "MCP server transport field invalid"
fi

# 10g: MCP servers have networkEvidence array
if python3 -c "
import json, sys
data = json.load(open('$QUICK_JSON'))
for s in data['mcpServers']:
    ev = s.get('networkEvidence', None)
    if not isinstance(ev, list):
        print(f'  Server \"{s[\"name\"]}\" has no networkEvidence array', file=sys.stderr)
        sys.exit(1)
    # Every server should have at least a transport evidence entry
    if len(ev) == 0:
        print(f'  Server \"{s[\"name\"]}\" has empty networkEvidence', file=sys.stderr)
        sys.exit(1)
" 2>&1; then
    pass "MCP servers have networkEvidence arrays"
else
    fail "MCP server networkEvidence missing or empty"
fi

# 10h: MCP servers have envVars array
if python3 -c "
import json, sys
data = json.load(open('$QUICK_JSON'))
for s in data['mcpServers']:
    ev = s.get('envVars', None)
    if not isinstance(ev, list):
        print(f'  Server \"{s[\"name\"]}\" has no envVars array', file=sys.stderr)
        sys.exit(1)
" 2>&1; then
    pass "MCP servers have envVars arrays"
else
    fail "MCP server envVars missing"
fi

# ── 12. Error cases ─────────────────────────────────────────────────

section "Error cases"
expect_fail  "file scan (nonexistent file)"  $RUN file /tmp/vettd-no-such-file-12345.txt
expect_fail  "submit without API key"        $RUN quick --submit

# ── 13. Full scan (smoke test — quick bail) ──────────────────────────

section "Full scan (smoke test)"
echo "  (scans from / — capped at 500k files, will take longer)"
echo "  Running with --summary to keep output brief..."
expect_ok "full scan (--summary)" $RUN full --summary

# ── 14. Auth config ──────────────────────────────────────────────────

section "Auth configuration"

# Back up existing config if present
# On macOS, dirs::config_dir() returns ~/Library/Application Support
if [[ "$(uname)" == "Darwin" ]]; then
    AUTH_CONFIG_DIR="${HOME}/Library/Application Support/vettd"
else
    AUTH_CONFIG_DIR="${HOME}/.config/vettd"
fi
AUTH_CONFIG="${AUTH_CONFIG_DIR}/config.json"
AUTH_BACKUP=""
if [ -f "$AUTH_CONFIG" ]; then
    AUTH_BACKUP="${AUTH_CONFIG}.test-backup-${TIMESTAMP}"
    cp "$AUTH_CONFIG" "$AUTH_BACKUP"
    dim "  Backed up existing config to $AUTH_BACKUP"
fi

# Test auth --key saves config
$RUN auth --key "ah_test_dummy_key_12345" --endpoint "http://localhost:3000/api/scans/ingest" > /dev/null 2>&1
if [ -f "$AUTH_CONFIG" ]; then
    if grep -q "ah_test_dummy_key_12345" "$AUTH_CONFIG" 2>/dev/null; then
        pass "auth --key writes config.json"
    else
        fail "auth --key config.json missing key"
    fi
    if grep -q "localhost" "$AUTH_CONFIG" 2>/dev/null; then
        pass "auth --endpoint writes to config.json"
    else
        fail "auth --endpoint not in config.json"
    fi
else
    fail "auth --key did not create config.json"
fi

# Restore original config
if [ -n "$AUTH_BACKUP" ]; then
    mv "$AUTH_BACKUP" "$AUTH_CONFIG"
    dim "  Restored original config"
elif [ -f "$AUTH_CONFIG" ]; then
    rm "$AUTH_CONFIG"
    dim "  Cleaned up test config"
fi

# ── 15. Self-update system ────────────────────────────────────────────

section "Self-update system"

# update --help should work
expect_ok "vettd update --help" $RUN update --help

# update --check should fail gracefully if the network is unavailable or the
# current build doesn't include an embedded official verification key.
UPDATE_CHECK_OUT=$($RUN update --check 2>&1) && UPDATE_CHECK_OK=true || UPDATE_CHECK_OK=false
if [ "$UPDATE_CHECK_OK" = true ]; then
    pass "update --check succeeded"
elif echo "$UPDATE_CHECK_OUT" | grep -qE "latest version|Update available|Failed to fetch|verification key"; then
    pass "update --check reports status or network error correctly"
else
    fail "update --check unexpected output: $UPDATE_CHECK_OUT"
fi

# update without --force should not crash (it may fail on network or signature
# prerequisites in dev/source builds, which is ok)
UPDATE_OUT=$($RUN update 2>&1) || true
if echo "$UPDATE_OUT" | grep -qE "latest version|Update available|Failed to fetch|update manifest|verification key"; then
    pass "update reports status or error gracefully"
else
    fail "update unexpected output: $UPDATE_OUT"
fi

# ── 16. Local submission ──────────────────────────────────────────────

section "Local submission (localhost:3000)"

# Check if local server is running
LOCAL_AVAILABLE=false
LOCAL_STATUS=$(curl -s -o /dev/null --connect-timeout 2 -w "%{http_code}" "$AH_TEST_LOCAL_ENDPOINT" 2>/dev/null || echo "000")
if [[ "$LOCAL_STATUS" == "200" || "$LOCAL_STATUS" == "401" || "$LOCAL_STATUS" == "403" || "$LOCAL_STATUS" == "405" ]]; then
    LOCAL_AVAILABLE=true
    dim "  Local ingest endpoint detected at $AH_TEST_LOCAL_ENDPOINT (HTTP $LOCAL_STATUS)"
else
    dim "  No compatible local ingest endpoint at $AH_TEST_LOCAL_ENDPOINT (HTTP $LOCAL_STATUS)"
fi

if [ "$LOCAL_AVAILABLE" = true ] && [ -n "$AH_TEST_API_KEY" ]; then
    LOCAL_SUBMIT_JSON="$OUT_DIR/${TIMESTAMP}-local-submit.json"

    # Scan + submit to local
    if $RUN file "$AGENTS_FILE" \
        --contract \
        --out "$LOCAL_SUBMIT_JSON" \
        --submit "$AH_TEST_LOCAL_ENDPOINT" \
        --api-key "$AH_TEST_API_KEY" 2>&1 | tail -5; then
        pass "local submit (file scan → localhost)"
    else
        fail "local submit (file scan → localhost)"
    fi
    expect_json_file "local submit contract saved" "$LOCAL_SUBMIT_JSON"

    # Quick scan + submit to local
    if $RUN quick \
        --submit "$AH_TEST_LOCAL_ENDPOINT" \
        --api-key "$AH_TEST_API_KEY" > /dev/null 2>&1; then
        pass "local submit (quick scan → localhost)"
    else
        fail "local submit (quick scan → localhost)"
    fi

    # Test auth rejection with bad key
    if $RUN file "$AGENTS_FILE" \
        --submit "$AH_TEST_LOCAL_ENDPOINT" \
        --api-key "${AH_TEST_API_NO_KEY:-ah_invalid_key_000}" > /dev/null 2>&1; then
        fail "local submit (bad key should fail but didn't)"
    else
        pass "local submit (bad key rejected)"
    fi
elif [ -z "$AH_TEST_API_KEY" ]; then
    skip "local submit — set AH_TEST_API_KEY to enable"
else
    skip "local submit — no compatible local ingest endpoint"
fi

# ── 15. Remote submission ─────────────────────────────────────────────

section "Remote submission (vettd.agentichighway.ai)"

if [ -n "$AH_TEST_API_KEY" ]; then
    REMOTE_SUBMIT_JSON="$OUT_DIR/${TIMESTAMP}-remote-submit.json"

    # File scan + submit to production
    REMOTE_OUTPUT=""
    REMOTE_OUTPUT=$($RUN file "$AGENTS_FILE" \
        --contract \
        --out "$REMOTE_SUBMIT_JSON" \
        --submit "$AH_TEST_REMOTE_ENDPOINT" \
        --allow-public-endpoint \
        --api-key "$AH_TEST_API_KEY" 2>&1) && REMOTE_FILE_OK=true || REMOTE_FILE_OK=false
    echo "$REMOTE_OUTPUT" | tail -5
    if [ "$REMOTE_FILE_OK" = true ]; then
        pass "remote submit (file scan → production)"
    elif echo "$REMOTE_OUTPUT" | grep -q "401\|Authentication failed"; then
        skip "remote submit (file scan) — API key not provisioned on production (401)"
    else
        fail "remote submit (file scan → production)"
    fi
    expect_json_file "remote submit contract saved" "$REMOTE_SUBMIT_JSON"

    # Quick scan + submit to production
    REMOTE_OUTPUT=""
    REMOTE_OUTPUT=$($RUN quick \
        --submit "$AH_TEST_REMOTE_ENDPOINT" \
        --allow-public-endpoint \
        --api-key "$AH_TEST_API_KEY" 2>&1) && REMOTE_QUICK_OK=true || REMOTE_QUICK_OK=false
    if [ "$REMOTE_QUICK_OK" = true ]; then
        pass "remote submit (quick scan → production)"
    elif echo "$REMOTE_OUTPUT" | grep -q "401\|Authentication failed"; then
        skip "remote submit (quick scan) — API key not provisioned on production (401)"
    else
        fail "remote submit (quick scan → production)"
    fi

    # Repo scan + submit to production
    REMOTE_REPO_JSON="$OUT_DIR/${TIMESTAMP}-remote-repo.json"
    REMOTE_OUTPUT=""
    REMOTE_OUTPUT=$($RUN repo "$REPO_ROOT" \
        --contract \
        --out "$REMOTE_REPO_JSON" \
        --submit "$AH_TEST_REMOTE_ENDPOINT" \
        --allow-public-endpoint \
        --api-key "$AH_TEST_API_KEY" 2>&1) && REMOTE_REPO_OK=true || REMOTE_REPO_OK=false
    echo "$REMOTE_OUTPUT" | tail -5
    if [ "$REMOTE_REPO_OK" = true ]; then
        pass "remote submit (repo scan → production)"
    elif echo "$REMOTE_OUTPUT" | grep -q "401\|Authentication failed"; then
        skip "remote submit (repo scan) — API key not provisioned on production (401)"
    else
        fail "remote submit (repo scan → production)"
    fi
    expect_json_file "remote submit repo contract saved" "$REMOTE_REPO_JSON"

    # Test bad key against production
    if $RUN file "$AGENTS_FILE" \
        --submit "$AH_TEST_REMOTE_ENDPOINT" \
        --allow-public-endpoint \
        --api-key "${AH_TEST_API_NO_KEY:-ah_invalid_key_000}" > /dev/null 2>&1; then
        fail "remote submit (bad key should fail but didn't)"
    else
        pass "remote submit (bad key rejected)"
    fi
else
    skip "remote submit — set AH_TEST_API_KEY to enable"
fi

# ── Summary ──────────────────────────────────────────────────────────

echo ""
bold "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
TOTAL=$((PASS + FAIL + SKIP))
echo "  Total: $TOTAL  │  $(green "✓ $PASS passed")  │  $(red "✗ $FAIL failed")  │  $(dim "⊘ $SKIP skipped")"
bold "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Clean up test artifacts (keep the test-runs dir for inspection)
echo "  Test outputs in: $OUT_DIR/"
ls -lh "$OUT_DIR"/${TIMESTAMP}-* 2>/dev/null | while read -r line; do
    dim "    $line"
done
echo ""

if [ "$FAIL" -gt 0 ]; then
    red "Some tests failed!"
    exit 1
else
    green "All tests passed."
    exit 0
fi
