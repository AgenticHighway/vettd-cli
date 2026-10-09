"""Codex source tests (spike #828). Every value in the fixtures is invented.

Each test states what it proves and what it cannot prove. None of them can prove that a real Codex
rollout has the shape the fixture has: the format is taken from the protocol crate and was not
verified against a live file (see the module docstring of sources/codex.py). They prove that the
reader applies the contract's rules to that shape.
"""
import os
import sys
import tempfile
import time
import unittest
from datetime import datetime, timezone

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from sources.base import (  # noqa: E402
    FAILURE_TOOL_ERROR,
    FAILURE_USER_DENIED,
    Cursor,
    SessionRef,
)
from sources.codex import APPROVAL_TO_PERMISSION, CodexSource, mcp_server_of  # noqa: E402

PROTO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CODEX_HOME = os.path.join(PROTO_DIR, "fixtures", "codex_home")
LIVE_SESSION_ID = "0f0f0f0f-1111-4222-8333-444444444444"
ARCHIVED_SESSION_ID = "0f0f0f0f-2222-4222-8333-666666666666"
SENTINEL = "ZQX" + "SENTINEL"  # built at runtime so the test file itself never carries the literal
WINDOW_DAYS = 3660


def utc_ms(y: int, mo: int, d: int, h: int, mi: int, s: int, ms: int = 0) -> int:
    return int(datetime(y, mo, d, h, mi, s, ms * 1000, tzinfo=timezone.utc).timestamp() * 1000)


def discover_all():
    source = CodexSource(CODEX_HOME)
    return source, source.discover(CODEX_HOME, WINDOW_DAYS, int(time.time() * 1000))


def read_live():
    source, refs = discover_all()
    ref = next(r for r in refs if r.session_key == LIVE_SESSION_ID)
    return source.read(ref, None)


class DiscoverTests(unittest.TestCase):
    def test_both_session_dirs_are_discovered_and_keyed_by_session_meta_id(self):
        """Proves sessions/ and archived_sessions/ are both walked recursively, every ref is a main
        session, and session_key is session_meta.payload.id rather than the file stem. Cannot prove
        the real Codex layout nests sessions by date or names files rollout-*."""
        _, refs = discover_all()
        keys = {r.session_key for r in refs}
        self.assertEqual(keys, {LIVE_SESSION_ID, ARCHIVED_SESSION_ID})
        self.assertTrue(all(r.kind == "main" and r.harness == "codex" for r in refs))
        self.assertTrue(any("archived_sessions" in r.path for r in refs))

    def test_stem_fallback_and_window_filter(self):
        """Proves a file with no session_meta is keyed by its file stem and that a file whose mtime
        is older than the window is not discovered. Cannot prove mtime is a good proxy for session
        recency on a real machine (an archived file could be touched by a backup tool)."""
        with tempfile.TemporaryDirectory() as tmp:
            sessions = os.path.join(tmp, "sessions", "2025", "06", "01")
            os.makedirs(sessions)
            fresh = os.path.join(sessions, "rollout-nometa.ndjson")
            stale = os.path.join(sessions, "rollout-stale.ndjson")
            for path in (fresh, stale):
                with open(path, "w", encoding="utf-8") as fh:
                    fh.write('{"timestamp":"2025-06-01T00:00:00.000Z","type":"turn_context","payload":{}}\n')
            now_ms = int(time.time() * 1000)
            os.utime(stale, (now_ms / 1000 - 40 * 86400, now_ms / 1000 - 40 * 86400))
            refs = CodexSource(tmp).discover(tmp, 30, now_ms)
        self.assertEqual([r.session_key for r in refs], ["rollout-nometa"])


class PairingTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.facts, cls.cursor = read_live()
        cls.by_id = {c.tool_use_id: c for c in cls.facts.tool_calls}

    def test_outputs_pair_with_calls_by_call_id(self):
        """Proves function_call/custom_tool_call items are paired with their *_output items by
        call_id, that latency is the harness-timestamp difference, and that a call with no output
        stays unpaired. Cannot prove that a real rollout never reuses a call_id."""
        self.assertEqual(len(self.facts.tool_calls), 7)
        self.assertEqual(self.by_id["call_fixture0001"].latency_ms, 1000)
        self.assertEqual(self.by_id["call_fixture0002"].latency_ms, 1500)
        self.assertEqual(self.by_id["call_fixture0003"].latency_ms, 250)
        self.assertEqual(self.by_id["call_fixture0004"].latency_ms, 1000)
        self.assertEqual(self.by_id["call_fixture0004"].name, "apply_patch")
        unpaired = [c.tool_use_id for c in self.facts.tool_calls if not c.paired]
        self.assertEqual(unpaired, ["call_fixture0007"])

    def test_failure_classes_from_success_flag_and_denial_phrase(self):
        """Proves an output object with success:false is tool_error, an output whose text says the
        user rejected the command is user_denied (so it never counts toward the rate), and plain or
        success:true outputs carry no failure class. Cannot prove Codex actually writes
        success:false or that exact phrase for those outcomes."""
        self.assertEqual(self.by_id["call_fixture0003"].failure_class, FAILURE_TOOL_ERROR)
        self.assertEqual(self.by_id["call_fixture0006"].failure_class, FAILURE_USER_DENIED)
        self.assertTrue(self.by_id["call_fixture0006"].is_error)
        for call_id in ("call_fixture0001", "call_fixture0002", "call_fixture0004"):
            self.assertIsNone(self.by_id[call_id].failure_class)
            self.assertFalse(self.by_id[call_id].is_error)

    def test_mcp_begin_end_events_create_and_fail_a_call(self):
        """Proves an MCP call that only appears as mcp_tool_call_begin/end (no function_call item)
        still becomes a ToolCall with the reported server, is paired by the end event, and an Err
        result is tool_error. Cannot prove those events are persisted in real rollouts at all."""
        call = self.by_id["call_fixture0005"]
        self.assertEqual(call.server, "beta-search")
        self.assertEqual(call.name, "beta-search__find")
        self.assertEqual(call.latency_ms, 1000)
        self.assertEqual(call.failure_class, FAILURE_TOOL_ERROR)


class ServerExtractionTests(unittest.TestCase):
    def test_server_namespace_rules(self):
        """Proves the server is the namespace before the last `__`, with or without the `mcp__`
        prefix, that a trailing `_<12 hex>` hash suffix is stripped from it, that a non-hex or
        wrong-length suffix is kept, and that built-in names without `__` yield None. Cannot prove
        where the real length-limit hash lands in a long name."""
        self.assertEqual(mcp_server_of("mcp__alpha-tools__lookup"), "alpha-tools")
        self.assertEqual(mcp_server_of("alpha-tools__lookup"), "alpha-tools")
        self.assertEqual(mcp_server_of("outer__inner__tool"), "outer__inner")
        self.assertEqual(mcp_server_of("gamma-server_0123456789ab__query_things"), "gamma-server")
        self.assertEqual(mcp_server_of("gamma-server_0123456789xy__query_things"), "gamma-server_0123456789xy")
        self.assertEqual(mcp_server_of("gamma-server_0123456789__q"), "gamma-server_0123456789")
        self.assertIsNone(mcp_server_of("shell"))
        self.assertIsNone(mcp_server_of("__tool"))
        self.assertIsNone(mcp_server_of(None))

    def test_servers_land_on_parsed_tool_calls(self):
        """Proves the two function_call-named MCP calls in the fixture carry their server (one via
        the mcp__ prefix, one via a hash-suffixed namespace) while built-in calls carry none.
        Cannot prove a real server name never itself ends in `_<12 hex>`."""
        facts, _ = read_live()
        by_id = {c.tool_use_id: c for c in facts.tool_calls}
        self.assertEqual(by_id["call_fixture0002"].server, "alpha-tools")
        self.assertEqual(by_id["call_fixture0003"].server, "gamma-server")
        self.assertIsNone(by_id["call_fixture0001"].server)
        self.assertIsNone(by_id["call_fixture0004"].server)


class TokenTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.facts, _ = read_live()

    def test_cumulative_counters_become_per_turn_deltas(self):
        """Proves consecutive token_count totals are differenced into Usage rows keyed tc-<n>, each
        tagged with the model of the enclosing turn_context, with cached input subtracted from
        input. Cannot prove the crate's input_tokens really includes cached tokens or that
        token_count fires exactly once per turn."""
        u1, u2 = self.facts.usages["tc-1"], self.facts.usages["tc-2"]
        self.assertEqual((u1.model, u1.input_tokens, u1.cached_input, u1.output_tokens, u1.reasoning),
                         ("gpt-invented-1", 800, 200, 100, 40))
        self.assertEqual((u2.model, u2.input_tokens, u2.cached_input, u2.output_tokens, u2.reasoning),
                         ("gpt-invented-2", 800, 700, 250, 50))
        self.assertIsNone(u1.cache_creation)
        self.assertIsNone(u1.cache_read)
        self.assertIsNone(u1.thinking)
        self.assertEqual(u2.ts_ms, utc_ms(2026, 1, 1, 10, 1, 7))

    def test_backwards_counter_marks_truncated_and_emits_no_row(self):
        """Proves a token_count whose totals fall below the previous ones sets facts.truncated and
        produces no Usage row (so tokens are undercounted, never double counted). Cannot prove
        what a backwards counter means on a real machine (resume, fork, or a rewrite)."""
        self.assertTrue(self.facts.truncated)
        self.assertNotIn("tc-3", self.facts.usages)
        self.assertEqual(len(self.facts.usages), 2)


class RunShapeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.facts, cls.cursor = read_live()

    def test_session_meta_and_turn_context_populate_run_shape(self):
        """Proves harness_version comes from cli_version, entrypoint from originator, effort and
        permission_mode from the first turn_context (on-request maps to default), and that a model
        change across turn_contexts shows up as two models. Cannot prove first-turn-wins is the
        right rule when effort changes mid-session."""
        self.assertEqual(self.facts.harness_version, "0.99.1")
        self.assertEqual(self.facts.entrypoint, "codex_cli_rs")
        self.assertEqual(self.facts.effort, "medium")
        self.assertEqual(self.facts.permission_mode, "default")
        self.assertEqual(self.facts.models, {"gpt-invented-1": 1, "gpt-invented-2": 1})
        self.assertEqual(self.facts.first_ts_ms, utc_ms(2026, 1, 1, 10, 0, 0))
        self.assertEqual(self.facts.last_ts_ms, utc_ms(2026, 1, 1, 10, 1, 13))
        self.assertIsNone(self.facts.last_stop_reason)
        self.assertEqual(self.facts.loaded_events, [])

    def test_approval_policy_mapping_is_the_published_one(self):
        """Proves the four Codex approval policies map to the gate's permission_mode enum as the
        contract states (and the archived fixture's `never` reads back as bypass). Cannot prove
        Codex has no fifth policy value; an unknown one maps to unknown by construction."""
        self.assertEqual(APPROVAL_TO_PERMISSION,
                         {"untrusted": "default", "on-failure": "auto", "on-request": "default", "never": "bypass"})
        source, refs = discover_all()
        facts, _ = source.read(next(r for r in refs if r.session_key == ARCHIVED_SESSION_ID), None)
        self.assertEqual(facts.permission_mode, "bypass")
        self.assertEqual(facts.effort, "high")
        self.assertEqual(facts.entrypoint, "codex_vscode")

    def test_user_turns_exclude_harness_injected_context(self):
        """Proves user-role messages count as turns except the harness-injected environment block,
        and that an assistant-role message is not a turn.
        Cannot prove the injected-prefix list is complete for every Codex release."""
        self.assertEqual(self.facts.user_turns, 2)

    def test_compactions_counted_from_event_and_rollout_line(self):
        """Proves both an event_msg context_compacted and a compacted rollout line increment
        compactions. Cannot prove a real rollout does not write both for one compaction, in which
        case this rule double counts (flagged in the module docstring)."""
        self.assertEqual(self.facts.compactions, 2)

    def test_unknown_types_are_counted_not_parsed(self):
        """Proves an unknown top-level type and an unknown event_msg subtype each count once in
        lines_unknown_type, a malformed line counts in parse_errors, consumed-but-empty types
        (reasoning) count in neither, and every line counts in lines_seen. Cannot prove the
        consumed-type list matches every type a real rollout carries."""
        self.assertEqual(self.facts.lines_unknown_type, 2)
        self.assertEqual(self.facts.parse_errors, 1)
        self.assertEqual(self.facts.lines_seen, 29)
        self.assertGreater(self.facts.bytes_read, 0)


class ForbidsAndContentTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.facts, cls.cursor = read_live()

    def test_forbids_buckets_hold_ids_paths_and_names(self):
        """Proves the session id and parent thread id, cwd, branch, repo url and commit, call ids,
        response item ids, the nickname and every MCP tool/server name are harvested into the
        dynamic-forbids buckets the gate checker consumes. Cannot prove the buckets catch fields a
        newer Codex adds."""
        f = self.facts.forbids
        self.assertEqual(f["harness_session_ids"], {LIVE_SESSION_ID, "0f0f0f0f-1111-4222-8333-555555555555"})
        self.assertEqual(f["cwd_and_branches"], {"/srv/invented/workspace-q7", "feature/invented-q7",
                                                 "ssh://invented.example/q7.git",
                                                 "0123456789abcdef0123456789abcdef01234567"})
        self.assertEqual(f["agent_ids"], {"invented-nick"})
        self.assertEqual(f["tool_use_ids"], {f"call_fixture000{i}" for i in range(1, 8)})
        self.assertTrue({"msg_fixture0001", "fc_fixture0001", "rs_fixture0001", "ctc_fixture0001"} <= f["message_ids"])
        self.assertEqual(f["loaded_set_names"], {"mcp__alpha-tools__lookup", "alpha-tools",
                                                 "gamma-server_0123456789ab__query_things", "gamma-server",
                                                 "beta-search", "beta-search__find"})

    def test_no_session_content_survives_parsing(self):
        """Proves the sentinel placed in user and assistant text, reasoning, tool arguments, patch
        input, tool outputs, MCP arguments and error text, the compaction summary and unknown lines
        appears nowhere in the facts or the cursor. Cannot prove content cannot leak through a
        field a future edit adds; it proves the current projection drops every content position
        the fixture covers."""
        self.assertNotIn(SENTINEL, repr(self.facts))
        self.assertNotIn(SENTINEL, repr(self.cursor))
        fingerprints = {c.input_fingerprint for c in self.facts.tool_calls}
        self.assertEqual(len(fingerprints), 7)
        self.assertTrue(all(len(fp) == 64 for fp in fingerprints))


class CursorTests(unittest.TestCase):
    def test_cursor_lands_on_eof_and_resume_reads_nothing_new(self):
        """Proves a full read leaves the cursor at the file's byte size on a line boundary and that
        resuming from it reads zero lines, while a cursor for a different inode restarts from zero.
        Cannot prove behaviour under concurrent appends (covered by the non-blocking suite)."""
        source, refs = discover_all()
        ref = next(r for r in refs if r.session_key == LIVE_SESSION_ID)
        facts, cursor = source.read(ref, None)
        self.assertEqual(cursor.byte_offset, os.path.getsize(ref.path))
        again, cursor2 = source.read(ref, cursor)
        self.assertEqual(again.lines_seen, 0)
        self.assertEqual(cursor2.byte_offset, cursor.byte_offset)
        stale = Cursor(path=ref.path, byte_offset=cursor.byte_offset, inode=(cursor.inode or 0) + 1)
        restarted, _ = source.read(ref, stale)
        self.assertEqual(restarted.lines_seen, facts.lines_seen)

    def test_read_accepts_a_hand_built_ref(self):
        """Proves read() depends only on ref.path, so a caller may construct SessionRef directly
        (as observe.py does for cursors). Cannot prove anything about discover()."""
        path = os.path.join(CODEX_HOME, "archived_sessions", "rollout-archived-fixture.ndjson")
        facts, _ = CodexSource(CODEX_HOME).read(SessionRef(path=path, harness="codex", session_key="x", kind="main"))
        self.assertEqual(facts.user_turns, 1)
        self.assertEqual(facts.usages["tc-1"].input_tokens, 300)


if __name__ == "__main__":
    unittest.main()
