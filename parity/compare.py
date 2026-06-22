"""
compare.py — Cross-scanner finding comparison engine.

Implements the parity spec from issue #133:
  https://github.com/AgenticHighway/vettd-cli/issues/133#issuecomment-4770174535

Rules:
  - Filter: drop findings where source != "vettd" on both sides.
  - Match key: (category, ruleId, label).
  - Exact-match fields: category, ruleId, label, severity, intent, chainId, source.
  - Fuzzy field: detail — strip "Detected in <path>:<line> — " prefix, compare remainder.
  - Excluded: filepath, owaspLlmCategory, and all server-derived fields.
  - Order: ignored; both sides sorted by match key before comparison.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any

# ── Types ─────────────────────────────────────────────────────────────────────

Finding = dict[str, Any]

# Regex matching the "Detected in <path>:<line> — " prefix in detail strings.
_DETAIL_PREFIX_RE = re.compile(r"^Detected in [^:]+:\d+ — ")

EXACT_FIELDS = ("category", "ruleId", "label", "severity", "intent", "chainId", "source")
EXCLUDED_FIELDS = frozenset(
    ("filepath", "owaspLlmCategory", "id", "skillAuditId", "fingerprint", "sources", "index")
)


@dataclass
class FieldMismatch:
    key: tuple[str, str, str]  # (category, ruleId, label)
    field: str
    cli_value: Any
    vettd_value: Any


@dataclass
class CompareResult:
    """Structured diff between CLI and vettd scanner outputs for one skill."""

    unmatched_cli: list[Finding] = field(default_factory=list)
    """Findings present in CLI output but absent in vettd output."""

    unmatched_vettd: list[Finding] = field(default_factory=list)
    """Findings present in vettd output but absent in CLI output."""

    field_mismatches: list[FieldMismatch] = field(default_factory=list)
    """Matched findings where compared fields differ."""

    matched_count: int = 0
    """Number of findings that matched on key and all compared fields."""

    @property
    def passed(self) -> bool:
        return (
            not self.unmatched_cli
            and not self.unmatched_vettd
            and not self.field_mismatches
        )


# ── Public API ────────────────────────────────────────────────────────────────


def compare(cli_findings: list[Finding], vettd_findings: list[Finding]) -> CompareResult:
    """Compare CLI findings against vettd findings per the parity spec.

    Both lists are mutated (sorted) in-place; pass copies if preservation matters.
    """
    cli = _filter(cli_findings)
    vettd = _filter(vettd_findings)

    cli_index = _index(cli)
    vettd_index = _index(vettd)

    result = CompareResult()

    all_keys = set(cli_index) | set(vettd_index)
    for key in sorted(all_keys):
        cli_f = cli_index.get(key)
        vettd_f = vettd_index.get(key)

        if cli_f is None:
            result.unmatched_vettd.append(vettd_f)  # type: ignore[arg-type]
            continue
        if vettd_f is None:
            result.unmatched_cli.append(cli_f)
            continue

        mismatches = _compare_fields(key, cli_f, vettd_f)
        if mismatches:
            result.field_mismatches.extend(mismatches)
        else:
            result.matched_count += 1

    return result


# ── Formatting ────────────────────────────────────────────────────────────────


def format_result(result: CompareResult, skill_name: str = "") -> str:
    """Return a human-readable diff string."""
    lines: list[str] = []
    header = f"{'PASS' if result.passed else 'FAIL'}"
    if skill_name:
        header += f"  {skill_name}"
    lines.append(header)

    if result.passed:
        lines.append(f"  {result.matched_count} finding(s) matched exactly.")
        return "\n".join(lines)

    if result.unmatched_cli:
        lines.append(f"  CLI-only ({len(result.unmatched_cli)} finding(s)):")
        for f in result.unmatched_cli:
            lines.append(f"    - [{f.get('severity','?')}] {f.get('category','?')} / {f.get('label','?')!r}")

    if result.unmatched_vettd:
        lines.append(f"  vettd-only ({len(result.unmatched_vettd)} finding(s)):")
        for f in result.unmatched_vettd:
            lines.append(f"    - [{f.get('severity','?')}] {f.get('category','?')} / {f.get('label','?')!r}")

    if result.field_mismatches:
        lines.append(f"  Field mismatches ({len(result.field_mismatches)}):")
        for m in result.field_mismatches:
            cat, rule_id, label = m.key
            lines.append(f"    [{cat} / {rule_id!r} / {label!r}] .{m.field}:")
            lines.append(f"      cli  : {m.cli_value!r}")
            lines.append(f"      vettd: {m.vettd_value!r}")

    if result.matched_count:
        lines.append(f"  {result.matched_count} finding(s) matched exactly.")

    return "\n".join(lines)


# ── Internal helpers ──────────────────────────────────────────────────────────


def _filter(findings: list[Finding]) -> list[Finding]:
    """Drop third-party findings (source != "vettd")."""
    return [f for f in findings if f.get("source", "vettd") == "vettd"]


def _match_key(f: Finding) -> tuple[str, str, str]:
    return (
        f.get("category", ""),
        f.get("ruleId", ""),
        f.get("label", ""),
    )


def _index(findings: list[Finding]) -> dict[tuple[str, str, str], Finding]:
    """Index findings by match key. Last writer wins on key collision (shouldn't happen)."""
    return {_match_key(f): f for f in findings}


def _strip_detail_prefix(detail: str) -> str:
    """Remove the 'Detected in <path>:<line> — ' prefix from a detail string."""
    return _DETAIL_PREFIX_RE.sub("", detail, count=1)


def _compare_fields(
    key: tuple[str, str, str],
    cli_f: Finding,
    vettd_f: Finding,
) -> list[FieldMismatch]:
    mismatches: list[FieldMismatch] = []

    for field_name in EXACT_FIELDS:
        cli_val = cli_f.get(field_name)
        vettd_val = vettd_f.get(field_name)
        if cli_val != vettd_val:
            mismatches.append(FieldMismatch(key=key, field=field_name, cli_value=cli_val, vettd_value=vettd_val))

    # Fuzzy detail comparison: strip location prefix, compare remainder.
    cli_detail = _strip_detail_prefix(cli_f.get("detail", "") or "")
    vettd_detail = _strip_detail_prefix(vettd_f.get("detail", "") or "")
    if cli_detail != vettd_detail:
        mismatches.append(FieldMismatch(key=key, field="detail", cli_value=cli_detail, vettd_value=vettd_detail))

    return mismatches
