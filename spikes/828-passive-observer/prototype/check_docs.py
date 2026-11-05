#!/usr/bin/env python3
"""Run the gate's dynamic forbids over prose (the spike answer, the scope note, the printed
ranking) so a local-only name, id, path, hostname or username cannot ride out inside a document.

Usage: python3 check_docs.py --dynamic <payload>.dynamic.json --public-names public-names.txt FILE...

Rules: every entry of the strict sets (slugs, cwd_and_branches, harness_session_ids, agent_ids,
tool_use_ids, message_ids, current_username, hostname, home_dir) is forbidden as a substring;
entries of loaded_set_names are forbidden as whole words when they look like asset names (eight or
more characters containing '-' or '_') and are not on the public-names allowlist — short names such
as "run" or "loop" are ordinary English and are left to human review, which the README says.
"""
from __future__ import annotations

import argparse
import json
import re
import sys

STRICT = ("slugs", "cwd_and_branches", "harness_session_ids", "agent_ids", "tool_use_ids", "message_ids",
          "current_username", "hostname", "home_dir")


def load_public(path: str | None) -> set:
    if not path:
        return set()
    names = set()
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if line and not line.startswith("#"):
            names.add(line.split(":", 1)[-1].lower())
    return names


def scan(text: str, dynamic: dict, public: set) -> list:
    findings = []
    lowered = text.lower()
    for name in STRICT:
        floor = 6 if name in ("cwd_and_branches", "slugs") else 3  # short branch names are ordinary words
        for entry in dynamic.get(name, []):
            e = str(entry).strip().lower()
            if len(e) >= floor and e in lowered:
                findings.append(f"{name}: an entry of length {len(e)} appears")
    for entry in dynamic.get("loaded_set_names", []):
        e = str(entry).strip()
        bare = e.split(":", 1)[-1]
        if len(bare) < 8 or not any(ch in bare for ch in "-_") or bare.lower() in public:
            continue
        if re.search(r"(?<![A-Za-z0-9_-])" + re.escape(bare) + r"(?![A-Za-z0-9_-])", text, re.IGNORECASE):
            findings.append(f"loaded_set_names: a non-public asset name of length {len(bare)} appears")
    return findings


def main(argv=None) -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--dynamic", required=True)
    p.add_argument("--public-names")
    p.add_argument("files", nargs="+")
    a = p.parse_args(argv)
    dynamic = json.load(open(a.dynamic, encoding="utf-8"))
    public = load_public(a.public_names)
    total = 0
    for path in a.files:
        for f in scan(open(path, encoding="utf-8").read(), dynamic, public):
            print(f"{path}: {f}")
            total += 1
    print(f"check_docs: {total} finding(s) in {len(a.files)} file(s)", file=sys.stderr)
    return 1 if total else 0


if __name__ == "__main__":
    sys.exit(main())
