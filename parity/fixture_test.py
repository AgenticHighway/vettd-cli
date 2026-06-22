"""
fixture_test.py — Parity fixture-test runner.

Discovers skill roots under a fixture directory, runs both adapters against
each skill using an identical file-map, and prints a deterministic pass/fail
diff per skill. The fixture directory is intentionally external to this repo
(path-mappable) so large fixture sets don't need to be committed here.

Expected to FAIL while the Rust engine is stubbed. That's the signal.

Usage:
    python parity/fixture_test.py \\
        --fixtures /path/to/skills/directory \\
        --vettd-adapter "/path/to/tsx /path/to/vettd/tools/parity-adapter.ts" \\
        [--cli-adapter "cargo run -p vettd-cli --bin parity-adapter --"] \\
        [--skill <name>]

Arguments:
    --fixtures        Path to a directory containing skill roots (required).
    --vettd-adapter   Command to invoke the vettd TypeScript adapter (required).
    --cli-adapter     Command to invoke the Rust adapter (default: cargo run).
    --skill           Run only the skill root whose name matches this string.
"""

from __future__ import annotations

import argparse
import sys

from adapters import AdapterError, run_adapter
from compare import compare, format_result
from loader import discover_skill_roots, load_skill_dir

_DEFAULT_CLI_ADAPTER = "cargo run -q -p vettd-cli --bin parity-adapter --"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Parity fixture-test runner")
    parser.add_argument(
        "--fixtures",
        required=True,
        metavar="DIR",
        help="Path to directory containing skill roots",
    )
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
        "--skill",
        metavar="NAME",
        help="Run only the skill root whose directory name matches this string",
    )
    args = parser.parse_args(argv)

    roots = discover_skill_roots(args.fixtures)
    if not roots:
        print(f"ERROR: no skill roots found under {args.fixtures!r}", file=sys.stderr)
        print("A skill root is any directory containing SKILL.md or skill.md.", file=sys.stderr)
        return 2

    if args.skill:
        roots = [r for r in roots if r.name == args.skill]
        if not roots:
            print(f"ERROR: no skill root named {args.skill!r} under {args.fixtures!r}", file=sys.stderr)
            return 2

    passed = 0
    failed = 0
    errored = 0

    for root in roots:
        name = root.name
        file_map = load_skill_dir(root)

        try:
            cli_findings = run_adapter(
                args.cli_adapter, file_map.text_files, file_map.all_paths
            )
        except AdapterError as e:
            print(f"ERROR  {name} (cli adapter failed):\n  {e}")
            errored += 1
            continue

        try:
            vettd_findings = run_adapter(
                args.vettd_adapter, file_map.text_files, file_map.all_paths
            )
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
    print(f"Fixture dir: {args.fixtures}")

    return 0 if (failed == 0 and errored == 0) else 1


if __name__ == "__main__":
    sys.exit(main())
