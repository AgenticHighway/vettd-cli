#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# test-submit.sh — Build vettd and submit a test scan to the server.
#
# Usage:
#   ./scripts/test-submit.sh [API_KEY] [SCAN_TARGET] [ENDPOINT]
#
# Examples:
#   ./scripts/test-submit.sh your-api-key
#   ./scripts/test-submit.sh your-api-key ~/projects/my-app
#   ./scripts/test-submit.sh your-api-key . http://localhost:3000/api/scans/ingest
#   AH_TEST_API_KEY=your-api-key ./scripts/test-submit.sh
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

API_KEY="${AH_TEST_API_KEY:-}"
if [ $# -gt 0 ]; then
    API_KEY="$1"
    shift
fi

if [ -z "$API_KEY" ] && [ -t 0 ]; then
    read -rsp "API key: " API_KEY
    echo ""
fi

if [ -z "$API_KEY" ]; then
    echo "Usage: $0 [API_KEY] [SCAN_TARGET] [ENDPOINT]" >&2
    echo "Set AH_TEST_API_KEY or provide the API key as the first argument." >&2
    exit 1
fi

SCAN_TARGET="${1:-.}"
ENDPOINT="${2:-https://vettd.agentichighway.ai/api/scans/ingest}"

TIMESTAMP="$(date -u +%Y-%m-%dT%H-%M-%SZ)"
OUT_DIR="test-runs"
OUT_FILE="${OUT_DIR}/${TIMESTAMP}-test.json"

mkdir -p "$OUT_DIR"

echo "═══════════════════════════════════════════════════════"
echo "  vettd test-submit"
echo "═══════════════════════════════════════════════════════"
echo "  Target:   $SCAN_TARGET"
echo "  Endpoint: $ENDPOINT"
echo "  Output:   $OUT_FILE"
echo "  Time:     $TIMESTAMP"
echo "═══════════════════════════════════════════════════════"
echo ""

echo "→ Building vettd..."
cargo build -p vettd-cli 2>&1 | tail -1
echo ""

echo "→ Running scan + submit..."
cargo run -p vettd-cli -- repo "$SCAN_TARGET" \
    --contract \
    --out "$OUT_FILE" \
    --submit "$ENDPOINT" \
    --api-key "$API_KEY"

EXIT_CODE=$?

echo ""
if [ $EXIT_CODE -eq 0 ]; then
    echo "✓ Done. Contract saved to $OUT_FILE"
    echo "  Size: $(wc -c < "$OUT_FILE" | tr -d ' ') bytes"
else
    echo "✗ Submission failed (exit $EXIT_CODE)"
fi

exit $EXIT_CODE
