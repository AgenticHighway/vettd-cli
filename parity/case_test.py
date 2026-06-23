"""
case_test.py — Parity case-test runner.

Runs both adapters against each hard-coded synthetic case and prints a
deterministic pass/fail diff per case. Designed to be run from the repo root.

Expected to FAIL while the Rust engine is stubbed. That's the signal.

Usage:
    python parity/case_test.py \\
        --vettd-adapter "/path/to/vettd/node_modules/.bin/tsx /path/to/vettd/tools/parity-adapter.ts" \\
        [--cli-adapter "cargo run -p vettd-cli --bin parity-adapter --"] \\
        [--case <name>]

Arguments:
    --vettd-adapter   Command to invoke the vettd TypeScript adapter (required).
    --cli-adapter     Command to invoke the Rust adapter (default: cargo run).
    --case            Run only the named case (optional; runs all if omitted).
"""

from __future__ import annotations

import argparse
import sys

from adapters import AdapterError, run_adapter
from cases import CASES
from compare import compare, format_result

_DEFAULT_CLI_ADAPTER = "cargo run -q -p vettd-cli --bin parity-adapter --"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Parity case-test runner")
    parser.add_argument(
        "--vettd-adapter",
        required=True,
        metavar="CMD",
        help="Command to invoke the vettd TypeScript adapter",
    )
    parser.add_argument(
        "--cli-adapter",
        default=_DEFAULT_CLI_ADAPTER,
        metavar="CMD",
        help=f"Command to invoke the Rust adapter (default: {_DEFAULT_CLI_ADAPTER!r})",
    )
    parser.add_argument(
        "--case",
        metavar="NAME",
        help="Run only this named case",
    )
    args = parser.parse_args(argv)

    cases = CASES
    if args.case:
        cases = [c for c in CASES if c["name"] == args.case]
        if not cases:
            print(f"ERROR: no case named {args.case!r}", file=sys.stderr)
            print(f"Available cases: {[c['name'] for c in CASES]}", file=sys.stderr)
            return 2

    passed = 0
    failed = 0
    errored = 0

    for case in cases:
        name = case["name"]
        text_files = case["textFiles"]
        all_paths = case["allPaths"]

        try:
            cli_findings = run_adapter(args.cli_adapter, text_files, all_paths)
        except AdapterError as e:
            print(f"ERROR  {name} (cli adapter failed):\n  {e}")
            errored += 1
            continue

        try:
            vettd_findings = run_adapter(args.vettd_adapter, text_files, all_paths)
        except AdapterError as e:
            print(f"ERROR  {name} (vettd adapter failed):\n  {e}")
            errored += 1
            continue

        result = compare(cli_findings, vettd_findings)
        print(format_result(result, skill_name=name))

        if result.passed:
            passed += 1
        else:
            failed += 1

    total = passed + failed + errored
    print(f"\n{'─' * 60}")
    print(f"Results: {passed}/{total} passed  |  {failed} failed  |  {errored} errored")

    return 0 if (failed == 0 and errored == 0) else 1


if __name__ == "__main__":
    sys.exit(main())
