"""Gate checker tests (spike #828). Every value below is invented; hashes are sha256 of fixture labels.

Each test states what it proves and what it cannot prove. None of them can prove that the gate JSON
itself lists the right fields: they prove that the checker enforces whatever the gate says.
"""
import copy
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from check_field_gate import DEFAULT_GATE_PATH, check, load_gate  # noqa: E402

PROTO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NULL_UUID = "00000000-0000-4000-8000-000000000000"
DAY = "2026-01-01"


def hex64(label: str) -> str:
    return hashlib.sha256(("fixture:" + label).encode("utf-8")).hexdigest()


def minimal_valid_payload() -> dict:
    """One record, one asset, one bom entry: the smallest payload that satisfies every gate rule."""
    asset_id = hex64("asset-a")
    bom_version = hex64("bom-a")
    signals = {
        "invocations": {"n": 3},
        "failures": {"tool_error": 1, "timeout": 0, "user_denied": 0, "interrupted": 0, "unknown": 0},
        "harness_corroborations": None,
        "latency_ms": {"n": 3, "sum": 900, "min": 200, "max": 400, "sumsq": 290000},
        "tokens_attributed": None,
        "context_cost_est": {"tokens": 120, "method": "listing_bytes_div4"},
    }
    asset = {
        "asset_id": asset_id, "asset_type": "skill", "key_basis": "name_hash", "tier": "inferred",
        "binding": "not_applicable", "direct_evidence_available": True, "signals": signals,
    }
    record = {
        "run_id": hex64("run-a"), "observed_day": DAY, "model": "claude-sonnet-5",
        "entrypoint_class": "cli", "effort": "medium", "permission_mode": "default",
        "task_category": "mixed", "bom_version": bom_version, "loaded_set_basis": "harness_log",
        "run_outcome": "completed",
        "counts": {"turns": 2, "tool_calls": 5, "tool_failures": 1, "user_denials": 0, "subagent_runs": 0,
                   "compactions": 0, "unpaired_tool_uses": 0, "loaded_set_changes": 0, "repeated_tool_calls": 0},
        "tokens": {"input": 100, "cache_creation": None, "cache_read": 40, "cached_input": None,
                   "output": 30, "thinking": None, "reasoning": None, "basis": "harness_usage"},
        "tokens_by_model": [{"model": "claude-sonnet-5", "input": 100, "cache_creation": None, "cache_read": 40,
                             "cached_input": None, "output": 30, "thinking": None, "reasoning": None}],
        "assets": [asset],
    }
    return {
        "envelope_version": "0.1.0", "extractor_version": "proto-0.1.0", "gate_version": 1, "emitted_day": DAY,
        "resource": {"device_id": NULL_UUID, "device_id_source": "placeholder", "harness": "claude_code",
                     "harness_version": "1.0.0", "collector": "prototype", "collector_version": "0.1.0"},
        "records": [record],
        "bom": [{"bom_version": bom_version, "asset_ids": [asset_id]}],
        "coverage": {"sessions_seen": 1, "sessions_emitted": 1, "sessions_skipped_unparseable": 0, "lines_seen": 20,
                     "lines_unknown_type": 0, "bytes_read": 4096, "truncated_sessions": 0, "window_days": 30,
                     "cursor_state": "fresh", "run_id_basis": "test_secret"},
    }


class GateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.gate = load_gate(DEFAULT_GATE_PATH)

    @staticmethod
    def mutated():
        """Deep copy of the minimal payload for a test to alter in place."""
        return copy.deepcopy(minimal_valid_payload())

    def assert_violation(self, violations, path, rule):
        prefix = f"{path}: {rule}: "
        self.assertTrue(any(v.startswith(prefix) for v in violations), f"expected {prefix!r} in {violations}")

    # ---- positive -------------------------------------------------------------------------------

    def test_minimal_payload_passes_with_empty_dynamic_sets(self):
        """Proves the helper payload satisfies every static gate rule with no dynamic sets (None and {}).
        Cannot prove the gate lists everything a real emitter writes."""
        self.assertEqual(check(minimal_valid_payload(), self.gate, None), [])
        self.assertEqual(check(minimal_valid_payload(), self.gate, {}), [])

    def test_minimal_payload_passes_with_populated_non_matching_sets(self):
        """Proves populated dynamic sets only fail on an actual substring hit. Cannot prove a real
        loaded-set name never collides with a permitted enum value (that is the emitter's exposure)."""
        dynamic = {
            "loaded_set_names": {"quantum-widget-skill", "orbital-lint"},
            "cwd_and_branches": {"/opt/invented/workspace", "feature/invented-branch"},
            "harness_session_ids": {"11111111-2222-4333-8444-555555555555"},
            "current_username": {"nobody-invented"},
        }
        self.assertEqual(check(minimal_valid_payload(), self.gate, dynamic), [])

    def test_dynamic_entries_shorter_than_three_chars_are_skipped(self):
        """Proves empty and 1-2 char set entries are ignored ("cl" would otherwise hit "claude_code") while a
        3-char entry is enforced on a free string field ("cli" inside extractor_version). Enum fields are
        exempt by design (see EnumFieldsAreClosed). Cannot prove 3 is the right floor."""
        self.assertEqual(check(minimal_valid_payload(), self.gate, {"slugs": {"", "cl", "ai"}}), [])
        payload = minimal_valid_payload()
        payload["extractor_version"] = "proto-cli-1"
        self.assert_violation(check(payload, self.gate, {"slugs": {"cli"}}),
                              "extractor_version", "dynamic:slugs")

    def test_nullable_object_accepts_null_and_populated_but_not_scalar(self):
        """Proves a nullable object leaf may be null or a full object, and that a scalar there is a type
        error. Cannot prove the object's children are semantically right, only present and typed."""
        payload = self.mutated()
        payload["records"][0]["assets"][0]["signals"]["tokens_attributed"] = {"n": 1, "sum": 50, "min": 50, "max": 50, "sumsq": 2500}
        self.assertEqual(check(payload, self.gate, None), [])
        payload["records"][0]["assets"][0]["signals"]["tokens_attributed"] = 5
        self.assert_violation(check(payload, self.gate, None), "records[0].assets[0].signals.tokens_attributed", "type_mismatch")

    # ---- structure ------------------------------------------------------------------------------

    def test_unknown_leaf_key_fails(self):
        """Proves a key the gate does not list is a violation reported on its parent path; the key
        itself is never echoed because it could be content (the disclosure.rs
        coverage rule). Cannot prove keys inside a string value are seen; that is what patterns are for."""
        payload = self.mutated()
        payload["records"][0]["duration_ms"] = 1234
        self.assert_violation(check(payload, self.gate, None), "records[0]", "unknown_key")

    def test_unknown_intermediate_object_fails_even_when_empty(self):
        """Proves an unlisted object with no leaves is still rejected, so a walker that only sees leaves
        cannot be bypassed by an empty container. Cannot prove non-empty unknown objects report children."""
        payload = self.mutated()
        payload["resource"]["extra"] = {}
        self.assert_violation(check(payload, self.gate, None), "resource", "unknown_key")

    def test_missing_required_key_fails(self):
        """Proves a required field absent from a written object is reported on the parent path.
        Cannot prove that every field the schema marks required is also required in the gate."""
        payload = self.mutated()
        del payload["coverage"]["window_days"]
        self.assert_violation(check(payload, self.gate, None), "coverage", "missing_required")

    def test_null_in_non_nullable_field_fails(self):
        """Proves null is only accepted where the gate says nullable. Cannot prove nullable fields are
        the right ones."""
        payload = self.mutated()
        payload["records"][0]["tokens"]["input"] = None
        self.assert_violation(check(payload, self.gate, None), "records[0].tokens.input", "null_not_allowed")

    def test_boolean_is_not_an_integer(self):
        """Proves bool does not pass as an integer count even though Python treats True as 1.
        Cannot prove other integer-like types (floats) are rejected; see the type check for that."""
        payload = self.mutated()
        payload["records"][0]["counts"]["turns"] = True
        self.assert_violation(check(payload, self.gate, None), "records[0].counts.turns", "type_mismatch")

    # ---- value-level negatives, one per case ------------------------------------------------------

    def test_absolute_path_in_string_fails(self):
        """Proves an absolute POSIX path inside a string leaf is caught by the value-level pattern.
        Cannot prove relative paths or paths without a second segment are caught."""
        payload = self.mutated()
        payload["extractor_version"] = "/opt/invented/workspace/tool"
        self.assert_violation(check(payload, self.gate, None), "extractor_version", "pattern:abs_posix_path")

    def test_url_in_string_fails(self):
        """Proves a URL scheme inside a string leaf is caught. Cannot prove scheme-less hostnames are
        caught by this rule (dotted_host covers those)."""
        payload = self.mutated()
        payload["extractor_version"] = "https://invented.example/path"
        self.assert_violation(check(payload, self.gate, None), "extractor_version", "pattern:url_scheme")

    def test_loaded_set_name_via_dynamic_set_fails_case_insensitively(self):
        """Proves a loaded-set name handed over at run time is caught as a case-insensitive substring
        and that the message does not echo the name. Cannot prove names the emitter failed to collect."""
        payload = self.mutated()
        payload["extractor_version"] = "Quantum-Widget-Skill"
        violations = check(payload, self.gate, {"loaded_set_names": {"quantum-widget-skill"}})
        self.assert_violation(violations, "extractor_version", "dynamic:loaded_set_names")
        self.assertFalse(any("widget" in v.lower() for v in violations), violations)

    def test_second_resolution_timestamp_in_observed_day_fails(self):
        """Proves a day path only accepts YYYY-MM-DD, so finer time resolution cannot egress there.
        Cannot prove the day itself is a UTC day rather than a local one."""
        payload = self.mutated()
        payload["records"][0]["observed_day"] = "2026-01-01T12:34:56Z"
        self.assert_violation(check(payload, self.gate, None), "records[0].observed_day", "format_mismatch")

    def test_uuid_in_run_id_fails_exact_hex64(self):
        """Proves a hash path rejects a uuid (a harness session id) by exact hex64 format.
        Cannot prove a hex64 value is a real HMAC rather than a copied hash."""
        payload = self.mutated()
        payload["records"][0]["run_id"] = NULL_UUID
        self.assert_violation(check(payload, self.gate, None), "records[0].run_id", "format_mismatch")

    def test_uuid_outside_device_id_in_free_string_fails(self):
        """Proves the uuid_any pattern catches a uuid in a string whose own format would allow it
        (extractor_version admits hex and dashes). Cannot prove device_id is the only allowed uuid path
        beyond what the gate lists."""
        payload = self.mutated()
        payload["extractor_version"] = "11111111-2222-4333-8444-555555555555"
        self.assert_violation(check(payload, self.gate, None), "extractor_version", "pattern:uuid_any")

    def test_bad_enum_fails(self):
        """Proves enums are closed. Cannot prove enum membership is semantically right."""
        payload = self.mutated()
        payload["records"][0]["effort"] = "extreme"
        self.assert_violation(check(payload, self.gate, None), "records[0].effort", "not_in_enum")

    def test_non_hex_asset_id_fails(self):
        """Proves asset_id must be exactly 64 lowercase hex chars. Cannot prove it is content-derived."""
        payload = self.mutated()
        payload["records"][0]["assets"][0]["asset_id"] = "g" * 64
        self.assert_violation(check(payload, self.gate, None), "records[0].assets[0].asset_id", "format_mismatch")

    def test_off_allowlist_model_fails(self):
        """Proves a custom provider model id is rejected rather than passed through (taskcat must map it
        to 'other'). Cannot prove the allowlist covers every legitimate provider prefix."""
        payload = self.mutated()
        payload["records"][0]["model"] = "acme" + "-finetune-v3"
        self.assert_violation(check(payload, self.gate, None), "records[0].model", "not_in_enum")

    def test_bearer_like_value_fails(self):
        """Proves a token-shaped value is caught by the defence-in-depth pattern. Cannot prove every
        provider's token prefix is in the list."""
        payload = self.mutated()
        payload["extractor_version"] = "gh" + "p_" + "x1y2z3w4v5u6t7s8"
        self.assert_violation(check(payload, self.gate, None), "extractor_version", "pattern:bearer_like")

    def test_epoch_number_in_count_field_fails(self):
        """Proves a unix-seconds value in a count is flagged by the epoch rule (as well as bounds).
        Cannot prove values outside the two epoch ranges are timestamps of some other scale."""
        payload = self.mutated()
        payload["records"][0]["counts"]["turns"] = 1_700_000_000
        violations = check(payload, self.gate, None)
        self.assert_violation(violations, "records[0].counts.turns", "epoch_in_number")
        self.assert_violation(violations, "records[0].counts.turns", "out_of_bounds")

    def test_epoch_rule_fires_where_bounds_do_not(self):
        """Proves the epoch rule is independent of bounds: bytes_read admits 1.7e9 but the value is
        still rejected. Cannot prove a genuine 1.7 GB read never happens."""
        payload = self.mutated()
        payload["coverage"]["bytes_read"] = 1_700_000_000
        violations = check(payload, self.gate, None)
        self.assert_violation(violations, "coverage.bytes_read", "epoch_in_number")
        self.assertFalse(any("out_of_bounds" in v for v in violations), violations)

    def test_sum_of_squares_units_are_exempt_from_epoch_rule(self):
        """Proves ms2/tokens2 leaves may hold epoch-sized magnitudes. Cannot prove a timestamp could
        never be smuggled through a sumsq field; the exemption is a deliberate trade."""
        payload = self.mutated()
        payload["records"][0]["assets"][0]["signals"]["latency_ms"]["sumsq"] = 1_700_000_000
        self.assertEqual(check(payload, self.gate, None), [])

    def test_whitespace_in_string_fails(self):
        """Proves any whitespace in a string leaf is rejected (no permitted value contains one).
        Cannot prove that free text without whitespace is caught by this rule alone."""
        payload = self.mutated()
        payload["extractor_version"] = "proto 0.1.0"
        self.assert_violation(check(payload, self.gate, None), "extractor_version", "pattern:whitespace")

    def test_mcp_tool_name_in_string_fails(self):
        """Proves an MCP tool/server name in harness form is caught. Cannot prove server names outside
        the mcp__ form (Codex's <server>__<tool>) are caught by this pattern."""
        payload = self.mutated()
        payload["extractor_version"] = "mcp__" + "invented-server" + "__list"
        self.assert_violation(check(payload, self.gate, None), "extractor_version", "pattern:mcp_tool_name")

    # ---- CLI ------------------------------------------------------------------------------------

    def test_cli_exit_codes(self):
        """Proves the CLI exits 0 on a passing payload, 1 on violations (printing each), and honours
        --dynamic. Cannot prove behaviour on malformed JSON beyond the non-zero exit."""
        script = os.path.join(PROTO_DIR, "check_field_gate.py")
        with tempfile.TemporaryDirectory() as tmp:
            good = os.path.join(tmp, "good.json")
            bad = os.path.join(tmp, "bad.json")
            names = os.path.join(tmp, "names.json")
            payload = self.mutated()
            with open(good, "w", encoding="utf-8") as fh:
                json.dump(payload, fh)
            payload["records"][0]["extra"] = "x"
            with open(bad, "w", encoding="utf-8") as fh:
                json.dump(payload, fh)
            with open(names, "w", encoding="utf-8") as fh:
                json.dump({"loaded_set_names": ["proto-0.1.0"]}, fh)
            ok = subprocess.run([sys.executable, script, good], capture_output=True, text=True)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertEqual(ok.stdout, "")
            failed = subprocess.run([sys.executable, script, bad], capture_output=True, text=True)
            self.assertEqual(failed.returncode, 1)
            self.assertIn("records[0]: unknown_key", failed.stdout)
            dyn = subprocess.run([sys.executable, script, good, "--dynamic", names], capture_output=True, text=True)
            self.assertEqual(dyn.returncode, 1)
            self.assertIn("extractor_version: dynamic:loaded_set_names", dyn.stdout)


if __name__ == "__main__":
    unittest.main()


class EnumFieldsAreClosed(unittest.TestCase):
    """A dynamic-forbid name that is a substring of an enum literal must not fail an enum field.

    Proves: enum-typed fields are exempt from the substring rule because their value space is
    closed (the value is one of the gate's own literals, so it cannot carry a local-only name).
    Cannot prove: that a non-enum string field with the same substring is caught — that is the
    existing dynamic-forbid test's job, and it must still fail there.
    """

    def test_short_name_inside_enum_literal_passes(self):
        payload = minimal_valid_payload()
        payload["records"][0]["run_outcome"] = "truncated"
        gate = load_gate()
        violations = check(payload, gate, {"loaded_set_names": {"run", "cat"}})
        self.assertEqual([], [v for v in violations if "run_outcome" in v], violations)

    def test_same_name_still_caught_on_a_free_string_field(self):
        payload = minimal_valid_payload()
        payload["extractor_version"] = "proto-run-1"
        gate = load_gate()
        violations = check(payload, gate, {"loaded_set_names": {"run"}})
        self.assertTrue(any("extractor_version" in v and "dynamic:loaded_set_names" in v for v in violations), violations)

