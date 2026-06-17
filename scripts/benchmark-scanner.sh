#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

exec </dev/null

BIN="${VETTD_BENCH_BIN:-$REPO_ROOT/target/release/vettd}"
TARGET_PATH="${VETTD_BENCH_TARGET:-$REPO_ROOT}"
FILE_TARGET="${VETTD_BENCH_FILE:-$REPO_ROOT/agents.md}"
SKIP_BUILD="${VETTD_BENCH_SKIP_BUILD:-0}"
TIME_BIN="${VETTD_BENCH_TIME_BIN:-/usr/bin/time}"

if [[ "$SKIP_BUILD" != "1" ]]; then
    cargo build --release -p vettd-cli >/dev/null
fi

if [[ ! -x "$BIN" ]]; then
    echo "benchmark error: binary not found at $BIN" >&2
    exit 1
fi

if [[ ! -d "$TARGET_PATH" ]]; then
    echo "benchmark error: target directory not found at $TARGET_PATH" >&2
    exit 1
fi

if [[ ! -f "$FILE_TARGET" ]]; then
    echo "benchmark error: file target not found at $FILE_TARGET" >&2
    exit 1
fi

TIME_ARGS=(-p)
probe_file="$(mktemp)"
if "$TIME_BIN" -lp true >/dev/null 2>"$probe_file"; then
    TIME_ARGS=(-lp)
elif "$TIME_BIN" -v true >/dev/null 2>"$probe_file"; then
    TIME_ARGS=(-v)
fi
rm -f "$probe_file"

print_header() {
    printf '\n[%s]\n' "$1"
}

bench() {
    local label="$1"
    shift
    local timing_file
    timing_file="$(mktemp)"

    print_header "$label"
    if "$TIME_BIN" "${TIME_ARGS[@]}" "$@" >/dev/null 2>"$timing_file"; then
        cat "$timing_file"
    else
        cat "$timing_file" >&2
        rm -f "$timing_file"
        return 1
    fi
    rm -f "$timing_file"
}

echo "Benchmark binary: $BIN"
echo "Benchmark target: $TARGET_PATH"
echo "Benchmark file: $FILE_TARGET"
echo "Timing mode: ${TIME_ARGS[*]}"
echo "Tip: set VETTD_TIMINGS=1 on an individual scan command to print stage timings to stderr."

bench "file --json" "$BIN" file "$FILE_TARGET" --json
bench "folder --json" "$BIN" folder "$TARGET_PATH" --json
bench "repo --json" "$BIN" repo "$TARGET_PATH" --json
bench "quick --json" "$BIN" quick --json
bench "scan --summary" "$BIN" scan --summary