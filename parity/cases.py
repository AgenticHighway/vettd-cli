"""
cases.py — Hard-coded synthetic cases for the parity case-test.

Each case exercises a specific engine behavior and is designed to eventually
pass once the Rust engine reaches parity with the vettd scanner for that
behavior. Cases are expected to FAIL while the Rust engine is stubbed.

Case shape:
  {
    "name":      str,           # human label for diff output
    "textFiles": dict[str,str], # relpath → content
    "allPaths":  list[str],     # all relpath (including binary-only files)
  }
"""

from __future__ import annotations

_SKILL_MD_MINIMAL = """\
---
name: test-skill
version: 1.0.0
description: A minimal test skill for parity checking.
author: test
license: MIT
compatibility: any
---

# Test Skill

Does nothing harmful.
"""

_SKILL_MD_MISSING_FIELDS = """\
---
name: incomplete-skill
---

# Incomplete Skill
"""

CASES: list[dict] = [
    {
        "name": "clean-skill-no-extras",
        "description": (
            "Minimal well-formed skill: SKILL.md present, no scripts/evals/references. "
            "Expects only structure info findings."
        ),
        "textFiles": {
            "SKILL.md": _SKILL_MD_MINIMAL,
        },
        "allPaths": ["SKILL.md"],
    },
    {
        "name": "missing-skill-md",
        "description": (
            "No SKILL.md at all. Both scanners must emit a critical structure finding."
        ),
        "textFiles": {},
        "allPaths": [],
    },
    {
        "name": "skill-with-scripts",
        "description": (
            "Skill has a scripts/ directory. Both scanners must emit a scripts-category finding."
        ),
        "textFiles": {
            "SKILL.md": _SKILL_MD_MINIMAL,
            "scripts/run.sh": "#!/bin/bash\necho hello\n",
        },
        "allPaths": ["SKILL.md", "scripts/run.sh"],
    },
    {
        "name": "skill-with-evals",
        "description": (
            "Skill has an evals/ directory. Both scanners must emit an evals-category finding."
        ),
        "textFiles": {
            "SKILL.md": _SKILL_MD_MINIMAL,
            "evals/suite.json": '{"cases": []}',
        },
        "allPaths": ["SKILL.md", "evals/suite.json"],
    },
    {
        "name": "skill-with-references",
        "description": (
            "Skill has a references/ directory. Both scanners must emit a references structure finding."
        ),
        "textFiles": {
            "SKILL.md": _SKILL_MD_MINIMAL,
            "references/guide.md": "# Guide\n\nSome reference material.\n",
        },
        "allPaths": ["SKILL.md", "references/guide.md"],
    },
    {
        "name": "skill-all-extras",
        "description": (
            "Full skill with scripts, evals, and references. "
            "Exercises all structural flags simultaneously."
        ),
        "textFiles": {
            "SKILL.md": _SKILL_MD_MINIMAL,
            "scripts/run.sh": "#!/bin/bash\necho run\n",
            "evals/suite.json": '{"cases": []}',
            "references/guide.md": "# Guide\n",
        },
        "allPaths": [
            "SKILL.md",
            "scripts/run.sh",
            "evals/suite.json",
            "references/guide.md",
        ],
    },
    {
        "name": "empty-input",
        "description": (
            "Zero files, zero paths. Both scanners must not crash and must "
            "emit the missing-SKILL.md finding."
        ),
        "textFiles": {},
        "allPaths": [],
    },
]
