"""
loader.py — Directory-to-file-map loader for the parity fixture-test.

Loads a skill directory into the adapter input envelope format:
  textFiles: dict[relpath, utf8_content]  (UTF-8 decodable files only)
  allPaths:  list[relpath]                (every file, posix-normalized)

Loader rules (documented here — out of scope for engine parity):
  - Paths are relative to the skill root, normalized to POSIX separators.
  - Files that cannot be decoded as UTF-8 are included in allPaths only
    (binary files), not in textFiles. This matches the vettd adapter's
    isBinaryPath + TextDecoder pattern.
  - No size cap is applied here; the adapters themselves impose limits.
  - Hidden directories (starting with ".") are skipped.
  - Symlinks are followed shallowly (os.walk followlinks=False).

Note: loader parity (this loader vs. vettd's zip-extract path) is explicitly
out of scope for issue #133. This loader exists only to feed both engines an
identical input map.
"""

from __future__ import annotations

import os
from pathlib import Path, PurePosixPath
from typing import NamedTuple


class SkillFileMap(NamedTuple):
    text_files: dict[str, str]
    all_paths: list[str]


def load_skill_dir(root: str | Path) -> SkillFileMap:
    """Walk *root* and return a file-map envelope for both adapters.

    Args:
        root: Path to the skill root directory (the one containing SKILL.md).

    Returns:
        SkillFileMap with text_files and all_paths populated.
    """
    root = Path(root).resolve()
    text_files: dict[str, str] = {}
    all_paths: list[str] = []

    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        # Skip hidden directories in-place (modifying dirnames prunes walk).
        dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))

        for name in sorted(filenames):
            abs_path = Path(dirpath) / name
            rel = PurePosixPath(abs_path.relative_to(root)).as_posix()
            all_paths.append(rel)

            try:
                content = abs_path.read_text(encoding="utf-8")
                text_files[rel] = content
            except (UnicodeDecodeError, OSError):
                # Binary or unreadable file — present in allPaths, absent from textFiles.
                pass

    return SkillFileMap(text_files=text_files, all_paths=all_paths)


def discover_skill_roots(base: str | Path) -> list[Path]:
    """Find all skill roots under *base* (dirs containing SKILL.md or skill.md).

    Returns paths sorted for deterministic ordering.
    """
    base = Path(base).resolve()
    roots: list[Path] = []

    for dirpath, dirnames, filenames in os.walk(base, followlinks=False):
        dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))
        if "SKILL.md" in filenames or "skill.md" in filenames:
            roots.append(Path(dirpath))

    return sorted(roots)
