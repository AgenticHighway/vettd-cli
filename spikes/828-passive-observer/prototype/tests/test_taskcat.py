"""taskcat.py tests (spike #828). Every share and model id below is invented.

Each test states what it proves and what it cannot prove. None of them can prove the published
boundaries are the RIGHT boundaries; they prove the code implements the boundaries the contract
publishes under RULES_VERSION, inclusively, in the stated precedence.
"""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import taskcat  # noqa: E402
from taskcat import KNOWN_MODELS, RULES_VERSION, allowlist_model, categorize  # noqa: E402

PROTO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GATE_PATH = os.path.normpath(
    os.path.join(os.path.dirname(PROTO_DIR), "..", "..", "telemetry-field-gate.json")
)


class CategorizeBoundaries(unittest.TestCase):
    def test_rules_version_pins_the_boundaries(self):
        """Proves: the boundary constants are the ones published as taskcat-1 (0.5 / 0.25 / 0.5 /
        0.5). If any boundary changes this fails, which is the point: a changed rule set must ship
        under a new RULES_VERSION, since extractor_version carries it.
        Cannot prove: that callers actually embed RULES_VERSION in extractor_version."""
        self.assertEqual(RULES_VERSION, "taskcat-1")
        self.assertEqual((taskcat.MCP_HEAVY_MIN, taskcat.CODE_EDIT_MIN, taskcat.SHELL_OPS_MIN, taskcat.CODE_EXPLORE_MIN),
                         (0.5, 0.25, 0.5, 0.5))

    def test_total_zero_is_unspecified(self):
        """Proves: no tool calls (empty shares or all-zero shares) yields `unspecified`, never
        `mixed`, so a run with nothing to classify is distinguishable from a genuinely mixed one.
        Cannot prove: how extract builds the shares for an empty run (see test_extract)."""
        self.assertEqual(categorize({}), "unspecified")
        self.assertEqual(categorize({"edit": 0.0, "read": 0.0, "shell": 0.0, "mcp": 0.0, "other": 0.0}), "unspecified")

    def test_mcp_boundary_is_inclusive_at_half(self):
        """Proves: mcp share exactly 0.5 is `mcp_heavy` (>=), and 0.5 minus one call's worth is not.
        Cannot prove: behaviour for shares that do not come from a count/total ratio."""
        self.assertEqual(categorize({"mcp": 0.5, "other": 0.5}), "mcp_heavy")
        self.assertNotEqual(categorize({"mcp": 0.49, "other": 0.51}), "mcp_heavy")

    def test_edit_boundary_is_inclusive_at_quarter(self):
        """Proves: edit share exactly 0.25 is `code_edit`; just below it, with no other rule met,
        the run is `mixed`.
        Cannot prove: that 0.25 is the right threshold for real tool mixes."""
        self.assertEqual(categorize({"edit": 0.25, "other": 0.75}), "code_edit")
        self.assertEqual(categorize({"edit": 0.24, "other": 0.76}), "mixed")

    def test_shell_and_read_boundaries_inclusive_at_half(self):
        """Proves: shell and read shares exactly 0.5 select `shell_ops` / `code_explore`, and just
        below they fall to `mixed` when nothing else applies.
        Cannot prove: anything about classes the rule set does not name."""
        self.assertEqual(categorize({"shell": 0.5, "other": 0.5}), "shell_ops")
        self.assertEqual(categorize({"shell": 0.49, "other": 0.51}), "mixed")
        self.assertEqual(categorize({"read": 0.5, "other": 0.5}), "code_explore")
        self.assertEqual(categorize({"read": 0.49, "other": 0.51}), "mixed")

    def test_precedence_mcp_then_edit_then_shell_then_read(self):
        """Proves: when several boundaries are met the earlier rule wins (mcp > edit > shell >
        read), so a category is a deterministic function of the shares.
        Cannot prove: that this precedence matches what a person would call the task."""
        self.assertEqual(categorize({"mcp": 0.5, "edit": 0.5}), "mcp_heavy")
        self.assertEqual(categorize({"edit": 0.25, "shell": 0.75}), "code_edit")
        self.assertEqual(categorize({"edit": 0.25, "read": 0.75}), "code_edit")
        self.assertEqual(categorize({"shell": 0.5, "read": 0.5}), "shell_ops")

    def test_shares_from_integer_ratios_hit_boundaries_exactly(self):
        """Proves: shares computed the way extract computes them (count/total) land exactly on the
        published boundaries for 1/2 and 1/4, so the inclusive comparison is not defeated by float
        representation.
        Cannot prove: exactness for ratios that are not dyadic (those are not boundaries)."""
        self.assertEqual(categorize({"mcp": 2 / 4, "other": 2 / 4}), "mcp_heavy")
        self.assertEqual(categorize({"edit": 1 / 4, "other": 3 / 4}), "code_edit")


class AllowlistModel(unittest.TestCase):
    def test_regex_matches_the_gate(self):
        """Proves: KNOWN_MODELS is identical to the gate's enums.model, so
        the extractor cannot emit a model id the gate would reject, and a gate change without a
        matching taskcat change fails here.
        Cannot prove: that the gate regex itself is the intended allowlist."""
        with open(GATE_PATH, "rb") as fh:
            gate = json.load(fh)
        self.assertEqual(list(KNOWN_MODELS), gate["enums"]["model"])

    def test_allowlisted_families_pass_through_unchanged(self):
        """Proves: an id in each allowlisted family is returned verbatim (claude-x is the canonical
        case from the task), so the payload keeps a usable model id for cost rendering.
        Cannot prove: that every real provider id fits these families."""
        for raw in ("claude-sonnet-5", "gpt-4.1", "o3", "codex-mini-latest", "gemini-2.5-pro", "other"):
            self.assertEqual(allowlist_model(raw), raw, raw)

    def test_invented_provider_name_becomes_other(self):
        """Proves: an off-allowlist provider name (D2: custom provider names) becomes the literal
        "other" rather than egressing.
        Cannot prove: that "other" is never mistaken for a real model downstream."""
        self.assertEqual(allowlist_model("fxprovider-custom-9"), "other")
        self.assertEqual(allowlist_model("unknown"), "other")

    def test_no_normalisation_and_non_strings(self):
        """Proves: uppercase, surrounding whitespace, a trailing newline, empty and None all map to
        "other" — the function matches the wire format exactly and never repairs input.
        Cannot prove: that a harness never reports a model id in a case the allowlist rejects."""
        for raw in ("Claude-X", " claude-x", "claude-x\n", "", None, 42):
            self.assertEqual(allowlist_model(raw), "other", repr(raw))


if __name__ == "__main__":
    unittest.main()
