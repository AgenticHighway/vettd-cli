"""
adapters.py — Subprocess wrapper for parity adapter binaries.

Each adapter must implement the protocol:
  stdin  — JSON: { "textFiles": {...}, "allPaths": [...] }
  stdout — JSON: { "findings": [...] }
  stderr — human-oriented diagnostics (surfaced on failure)
  exit   — 0 on success, non-zero on error
"""

from __future__ import annotations

import json
import shlex
import subprocess
from typing import Any

Finding = dict[str, Any]
FileMap = dict[str, str]


class AdapterError(Exception):
    """Raised when an adapter subprocess fails or produces unparseable output."""


def run_adapter(command: str, text_files: FileMap, all_paths: list[str]) -> list[Finding]:
    """Run an adapter subprocess and return its findings.

    Args:
        command: Shell-style command string, e.g.
                 "cargo run -p vettd-cli --bin parity-adapter --"
                 or "/path/to/tsx /path/to/parity-adapter.ts"
        text_files: Map of relative path → UTF-8 file content.
        all_paths: Complete list of relative paths (including binary-only files).

    Returns:
        Parsed list of finding dicts from the adapter's stdout.

    Raises:
        AdapterError: If the process exits non-zero or stdout is not valid JSON.
    """
    envelope = json.dumps({"textFiles": text_files, "allPaths": all_paths})
    argv = shlex.split(command)

    try:
        proc = subprocess.run(
            argv,
            input=envelope,
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError as e:
        raise AdapterError(f"Adapter command not found: {argv[0]!r} — {e}") from e

    if proc.returncode != 0:
        stderr_snippet = proc.stderr.strip()[-500:] if proc.stderr else "(no stderr)"
        raise AdapterError(
            f"Adapter exited {proc.returncode}.\n"
            f"Command: {command}\n"
            f"Stderr:  {stderr_snippet}"
        )

    stdout = proc.stdout.strip()
    if not stdout:
        raise AdapterError(
            f"Adapter produced no stdout.\nCommand: {command}\n"
            f"Stderr: {proc.stderr.strip()[-500:] if proc.stderr else '(none)'}"
        )

    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError as e:
        raise AdapterError(
            f"Adapter stdout is not valid JSON: {e}\n"
            f"Output (first 500 chars): {stdout[:500]}"
        ) from e

    if "findings" not in payload or not isinstance(payload["findings"], list):
        raise AdapterError(
            f"Adapter output missing 'findings' list.\nGot keys: {list(payload.keys())}"
        )

    return payload["findings"]
