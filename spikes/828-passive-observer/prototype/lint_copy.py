"""Causal-language lint for passive-observer copy (spike #828).

Everything the observer reports is observational. This lint rejects the phrases that turn an
observation into a claim (causes, improves, saves, proves, better/faster than, guarantees, dollar
amounts, bare "reliable") and requires any line that names a rate to carry the hedge that says where
the number came from ("observed ..." or "... in N calls"). It runs over rank.py's copy templates in
tests and over the spike's markdown docs and worked example from the CLI.

`lint(text) -> List[str]`: one finding per (line, rule), formatted ``"<lineno>: <rule>: <detail>"``.
CLI: ``python3 lint_copy.py FILE...`` prints ``<path>:<finding>`` and exits 1 when anything is found.
"""
from __future__ import annotations

import re
import sys
from typing import List, Tuple

# (rule id, regex). All matched case-insensitively.
FORBIDDEN_PHRASES: Tuple[Tuple[str, str], ...] = (
    ("causes", r"\bcauses?\b"),
    ("because_of", r"\bbecause of\b"),
    ("improves", r"\bimproves?\b"),
    ("makes_you", r"\bmakes? (?:you|your|it)\b"),
    ("faster_than", r"\bfaster than\b"),
    ("better_than", r"\bbetter than\b"),
    ("worse_than", r"\bworse than\b"),
    ("percent_better_worse", r"% ?(?:better|worse)\b"),
    ("saves", r"\bsaves?\b"),
    ("proves", r"\bproves?\b"),
    ("guarantee", r"\bguarantee"),
    ("dollar_amount", r"\$\d"),
    # "reliable"/"unreliable" is allowed only as "observed reliable" / "observed unreliable".
    ("bare_reliable", r"(?<!observed )\b(?:un)?reliable\b"),
)
_FORBIDDEN = [(rule, re.compile(rx, re.IGNORECASE)) for rule, rx in FORBIDDEN_PHRASES]

# A line that names a rate must say where the number came from.
_RATE_RE = re.compile(r"(?i)\brates?\b(?![ _-]limit)")  # "rate limit" is a policy, not a statistic
_HEDGE_RE = re.compile(r"observed|in \d+ calls", re.IGNORECASE)
RATE_RULE = "rate_without_hedge"


def lint(text: str) -> List[str]:
    findings: List[str] = []
    for lineno, line in enumerate(text.splitlines(), start=1):
        for rule, rx in _FORBIDDEN:
            match = rx.search(line)
            if match:
                findings.append(f"{lineno}: {rule}: forbidden phrase {match.group(0)!r}")
        if _RATE_RE.search(line) and not _HEDGE_RE.search(line):
            findings.append(f"{lineno}: {RATE_RULE}: a line naming a rate must say 'observed' or 'in N calls'")
    return findings


def main(argv: List[str] | None = None) -> int:
    paths = sys.argv[1:] if argv is None else argv
    if not paths:
        print("usage: lint_copy.py FILE...", file=sys.stderr)
        return 2
    total = 0
    for path in paths:
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except OSError as exc:
            print(f"lint_copy: {exc}", file=sys.stderr)
            return 2
        for finding in lint(text):
            print(f"{path}:{finding}")
            total += 1
    print(f"lint_copy: {total} finding(s) in {len(paths)} file(s)", file=sys.stderr)
    return 1 if total else 0


if __name__ == "__main__":
    sys.exit(main())
