"""Tests for sources/claude_code.py against the invented fixture under fixtures/claude_home.

Every value in the fixture is invented. The literal ZQXSENTINEL sits in every content position so
the no-content-survives test cannot pass vacuously. Expected hashes and byte counts are recomputed
here from the fixture bytes, independently of the parser.
"""
import calendar
import hashlib
import json
import os
import re
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from sources.base import FAILURE_TOOL_ERROR, FAILURE_USER_DENIED, RATE_BEARING_FAILURES, Cursor, SessionRef  # noqa: E402
from sources.claude_code import ClaudeCodeSource, link_children  # noqa: E402

PROTO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TESTS_DIR = os.path.dirname(os.path.abspath(__file__))
CLAUDE_HOME = os.path.join(PROTO, "fixtures", "claude_home")
SID = "0f1e2d3c-4b5a-6978-8a9b-0c1d2e3f4a5b"
MAIN_PATH = os.path.join(CLAUDE_HOME, "projects", "-fixture-project", SID + ".ndjson")
CHILD_PATH = os.path.join(CLAUDE_HOME, "projects", "-fixture-project", SID, "subagents", "agent-fx1.ndjson")
META_PATH = CHILD_PATH.replace(".ndjson", ".meta.json")
SENTINEL = "ZQX" + "SENTINEL"  # built at runtime so this file itself does not carry the marker verbatim
AGENT_TOOL_USE = "toolu_fx00000005"


def _ms(hour, minute, second, milli=0):
    return calendar.timegm((2026, 8, 15, hour, minute, second)) * 1000 + milli


def _objects(path):
    """Parse the fixture independently of the source: one dict per well-formed line."""
    out = []
    with open(path, "rb") as fh:
        for raw in fh:
            try:
                out.append(json.loads(raw))
            except ValueError:
                pass
    return out


def _mtime_ms(path):
    return int(os.stat(path).st_mtime * 1000)


def _temp_session(lines):
    """A throwaway claude_home inside the tests dir (suffix .tmp is gitignored); returns (tmpdir, path)."""
    tmp = tempfile.TemporaryDirectory(prefix="cc-src-", suffix=".tmp", dir=TESTS_DIR)
    pdir = os.path.join(tmp.name, "projects", "-tmp-project")
    os.makedirs(pdir)
    path = os.path.join(pdir, "11111111-2222-4333-8444-555555555555.ndjson")
    with open(path, "wb") as fh:
        fh.write(lines)
    return tmp, path


class ClaudeCodeSourceTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.now_ms = _mtime_ms(MAIN_PATH)
        cls.source = ClaudeCodeSource(CLAUDE_HOME)
        cls.refs = cls.source.discover(CLAUDE_HOME, window_days=3650, now_ms=cls.now_ms)
        mains = [r for r in cls.refs if r.kind == "main"]
        children = [r for r in cls.refs if r.kind == "child"]
        cls.main_ref, cls.child_ref = mains[0], children[0]
        cls.main, cls.main_cursor = cls.source.read(cls.main_ref)
        cls.child, cls.child_cursor = cls.source.read(cls.child_ref)
        cls.calls = {c.tool_use_id: c for c in cls.main.tool_calls}

    # -- discovery ---------------------------------------------------------------------------

    def test_discover_returns_main_and_linked_child(self):
        """Proves the on-disk layout maps to one main ref plus one child ref keyed by parent stem
        with agentType/toolUseId/spawnDepth from the sibling meta file, and that the meta file's
        free-text description is not carried. Cannot prove behaviour across several projects."""
        self.assertEqual(len(self.refs), 2)
        self.assertEqual((self.main_ref.session_key, self.main_ref.kind), (SID, "main"))
        self.assertEqual((self.child_ref.kind, self.child_ref.parent_key, self.child_ref.session_key), ("child", SID, "fx1"))
        self.assertEqual(self.child_ref.child_meta["agentType"], "fx-reviewer")
        self.assertEqual(self.child_ref.child_meta["toolUseId"], AGENT_TOOL_USE)
        self.assertEqual(self.child_ref.child_meta["spawnDepth"], "1")
        self.assertNotIn("description", self.child_ref.child_meta)

    def test_discover_window_filters_on_mtime(self):
        """Proves files older than window_days (by mtime) are not discovered. Cannot prove the
        window is right under clock skew between harness and collector."""
        stale = self.source.discover(CLAUDE_HOME, window_days=1, now_ms=self.now_ms + 3 * 86_400_000)
        self.assertEqual(stale, [])
        fresh = self.source.discover(CLAUDE_HOME, window_days=1, now_ms=self.now_ms + 3_600_000)
        self.assertEqual(len(fresh), 2)

    # -- tool calls --------------------------------------------------------------------------

    def test_tool_use_pairs_with_result_across_lines(self):
        """Proves a tool_use on an assistant line pairs with the tool_result on a later user
        line by id, latency is the harness-clock difference, and a use without a result stays
        unpaired. Cannot prove pairing when a result precedes its use (never seen in the format)."""
        ok = self.calls["toolu_fx00000001"]
        self.assertTrue(ok.paired)
        self.assertEqual(ok.latency_ms, 1500)
        self.assertEqual((ok.is_error, ok.failure_class, ok.name), (False, None, "Bash"))
        self.assertEqual(self.calls["toolu_fx00000004"].latency_ms, 1200)
        orphan = self.calls["toolu_fx00000007"]
        self.assertFalse(orphan.paired)
        self.assertIsNone(orphan.latency_ms)
        self.assertEqual(sum(1 for c in self.main.tool_calls if c.paired), len(self.main.tool_calls) - 1)

    def test_denial_and_tool_error_are_different_classes(self):
        """Proves two is_error results split by the denial phrase into user_denied vs tool_error,
        and that only tool_error is rate-bearing (why the split exists). Cannot prove the phrase
        list survives a harness wording change."""
        denied, errored = self.calls["toolu_fx00000002"], self.calls["toolu_fx00000003"]
        self.assertTrue(denied.is_error and errored.is_error)
        self.assertFalse(denied.interrupted)
        self.assertEqual(denied.failure_class, FAILURE_USER_DENIED)
        self.assertEqual(errored.failure_class, FAILURE_TOOL_ERROR)
        self.assertIn(FAILURE_TOOL_ERROR, RATE_BEARING_FAILURES)
        self.assertNotIn(FAILURE_USER_DENIED, RATE_BEARING_FAILURES)

    def test_mcp_tool_call_resolves_server(self):
        """Proves an mcp__<server>__<tool> call carries its server segment. Cannot prove
        server identity for tools whose names contain extra double underscores."""
        call = self.calls["toolu_fx00000004"]
        self.assertEqual((call.name, call.server), ("mcp__srvfx__tool", "srvfx"))
        self.assertIsNone(self.calls["toolu_fx00000001"].server)

    def test_agent_spawn_links_child_via_meta_tool_use_id(self):
        """Proves the Agent spawn is async (ack text + toolUseResult.isAsync), carries the
        subagent type and child key, and that the child ref's meta toolUseId points back at that
        tool_use while attributionAgent corroborates the type. Cannot prove linkage when meta.json
        is missing."""
        spawn = self.calls[AGENT_TOOL_USE]
        self.assertTrue(spawn.paired and spawn.is_async)
        self.assertEqual((spawn.agent_type, spawn.child_key), ("fx-reviewer", "fx1"))
        self.assertEqual(self.child_ref.child_meta["toolUseId"], spawn.tool_use_id)
        self.assertEqual(self.child.ref.session_key, spawn.child_key)
        self.assertEqual(self.child.ref.child_meta.get("corroborated"), "true")
        self.assertNotIn("corroborated", self.main.ref.child_meta)
        grep = self.child.tool_calls[0]
        self.assertEqual((grep.name, grep.latency_ms, self.child.user_turns), ("Grep", 400, 1))
        linked = link_children([self.child, self.main])
        self.assertEqual([f.ref.session_key for f in linked], [SID])
        self.assertIs(linked[0].children[0], self.child)

    # -- usage -------------------------------------------------------------------------------

    def test_split_response_usage_deduped_by_message_id(self):
        """Proves a response split over several assistant lines counts once: totals equal the
        first-per-message-id sum recomputed from the fixture and are strictly below the naive
        per-line sum. Cannot prove dedupe across parent and child (extract's job)."""
        per_line = [o["message"] for o in _objects(MAIN_PATH) if o.get("type") == "assistant"]
        first = {}
        for m in per_line:
            first.setdefault(m["id"], m["usage"])
        self.assertGreater(len(per_line), len(first))
        self.assertEqual(set(self.main.usages), set(first))
        self.assertEqual(sum(u.output_tokens for u in self.main.usages.values()), sum(u["output_tokens"] for u in first.values()))
        self.assertLess(sum(u.output_tokens for u in self.main.usages.values()), sum(m["usage"]["output_tokens"] for m in per_line))
        u1 = self.main.usages["msg_fx00000001"]
        self.assertEqual((u1.input_tokens, u1.cache_creation, u1.cache_read, u1.thinking, u1.model), (100, 20, 30, 7, "claude-fixture-1"))
        self.assertIsNone(u1.cached_input)
        self.assertEqual(self.main.models, {"claude-fixture-1": len(first)})

    # -- in-band assets and loaded set --------------------------------------------------------

    def test_invoked_skill_body_hashed_in_band_with_synthetic_call(self):
        """Proves the isMeta <command-name> line yields a skill_body asset whose sha256 equals
        hashlib over the text after the closing tag, plus a self-paired Skill call with no
        measurable latency (async). Cannot prove the body equals the SKILL.md on disk."""
        meta_lines = [o for o in _objects(MAIN_PATH) if o.get("type") == "user" and o.get("isMeta")]
        text = meta_lines[0]["message"]["content"]
        body = text[text.index("</command-name>") + len("</command-name>"):].encode("utf-8")
        assets = [a for a in self.main.in_band_assets if a.kind == "skill_body"]
        self.assertEqual([(a.name, a.content_sha256, a.byte_len) for a in assets],
                         [("skill-alpha", hashlib.sha256(body).hexdigest(), len(body))])
        skills = [c for c in self.main.tool_calls if c.name == "Skill"]
        self.assertEqual(sorted(c.skill for c in skills), ["skill-alpha", "skill-beta"])
        synthetic = next(c for c in skills if c.skill == "skill-alpha")
        self.assertTrue(synthetic.paired and synthetic.is_async)
        self.assertNotIn(synthetic.tool_use_id, self.main.forbids["tool_use_ids"])

    def test_nested_memory_rules_file_hashed_in_band(self):
        """Proves nested_memory becomes a rules_file asset named by basename with sha256/bytes of
        the in-band body, attached to the initial loaded-set event. Cannot prove the body matches
        the file the harness read if the log said it differed from disk."""
        att = next(o["attachment"] for o in _objects(MAIN_PATH) if o.get("type") == "attachment" and o["attachment"]["type"] == "nested_memory")
        body = att["content"]["content"].encode("utf-8")
        rules = [a for a in self.main.in_band_assets if a.kind == "rules_file"]
        self.assertEqual([(a.name, a.content_sha256, a.byte_len) for a in rules],
                         [("RULES.md", hashlib.sha256(body).hexdigest(), len(body))])
        initial = next(e for e in self.main.loaded_events if e.kind == "initial")
        self.assertEqual(initial.rules_files, ["RULES.md"])

    def test_loaded_events_from_listing_and_deltas(self):
        """Proves the first deferred_tools_delta is 'initial' and the next is 'delta' with its
        pending server cleared, skill_listing carries names and per-name listing bytes, and
        schema bytes are summed per MCP server from the fixture's own lines. Cannot prove the
        settle rule (attribute's job)."""
        atts = [o["attachment"] for o in _objects(MAIN_PATH) if o.get("type") == "attachment"]
        deferred = [a for a in atts if a["type"] == "deferred_tools_delta"]
        events = self.main.loaded_events
        self.assertEqual([e.kind for e in events], ["initial", "initial", "initial", "delta", "delta"])
        first, agents, skills, second, instr = events
        self.assertEqual(first.tool_names, deferred[0]["addedNames"])
        self.assertEqual((first.pending_mcp, first.failed_mcp), (["srvpend"], ["srvfail"]))
        srvfx_bytes = sum(len(ln) for ln in deferred[0]["addedLines"] if ln.startswith("mcp__srvfx__"))
        self.assertEqual(first.tool_schema_bytes, {"srvfx": srvfx_bytes})
        self.assertEqual(agents.agent_types, ["Explore", "fx-reviewer"])
        listing = next(a for a in atts if a["type"] == "skill_listing")
        alpha_line = next(ln for ln in listing["content"].split("\n") if "skill-alpha" in ln)
        self.assertEqual(skills.skills, ["skill-alpha", "skill-beta"])
        self.assertEqual(skills.listing_bytes["skill-alpha"], len(alpha_line))
        self.assertEqual((second.tool_names, second.pending_mcp, second.removed, second.readded), (["mcp__srvpend__ping"], [], [], []))
        self.assertEqual(second.tool_schema_bytes, {"srvpend": len(deferred[1]["addedLines"][0])})
        self.assertEqual((instr.tool_names, instr.ts_ms), ([], _ms(10, 0, 13, 100)))

    # -- counters and env --------------------------------------------------------------------

    def test_turns_compactions_and_env(self):
        """Proves user turns count prompt lines only (not isMeta, not tool-result lines), a
        summary line counts as a compaction, and version/entrypoint/permission/effort plus the
        first/last harness timestamps are taken from the lines. Cannot prove turn semantics for
        harness versions that put prompts elsewhere."""
        self.assertEqual(self.main.user_turns, 2)
        self.assertEqual(self.main.compactions, 1)
        self.assertEqual((self.main.harness_version, self.main.entrypoint, self.main.permission_mode, self.main.effort),
                         ("3.4.5", "cli", "acceptEdits", "high"))
        self.assertEqual((self.main.first_ts_ms, self.main.last_ts_ms), (_ms(10, 0, 0), _ms(10, 1, 2)))
        self.assertEqual(self.main.last_stop_reason, "end_turn")
        self.assertFalse(self.main.truncated)

    def test_unknown_and_malformed_lines_counted_not_parsed(self):
        """Proves every physical line is seen, the one non-consumed type and the one malformed
        line are counted separately, and neither disturbs the tool-call count. Cannot prove that
        a consumed type with an unexpected inner shape is detected."""
        with open(MAIN_PATH, "rb") as fh:
            data = fh.read()
        self.assertEqual(self.main.lines_seen, data.count(b"\n"))
        self.assertEqual(self.main.bytes_read, len(data))
        self.assertEqual(self.main.lines_unknown_type, 1)
        self.assertEqual(self.main.parse_errors, 1)
        self.assertEqual(len(self.main.tool_calls), 8)

    def test_forbids_buckets_populated(self):
        """Proves every local-only identifier the log carries lands in its forbids bucket for the
        gate checker, and harness built-in tool/agent names do not. Cannot prove the checker
        applies them."""
        f = self.main.forbids
        self.assertEqual(f["slugs"], {"fixture-slug"})
        self.assertEqual(f["cwd_and_branches"], {"/fixture/cwd", "fixture-branch"})
        self.assertEqual(f["harness_session_ids"], {SID})
        self.assertEqual(f["agent_ids"], {"fx1"})
        self.assertEqual(f["tool_use_ids"], {f"toolu_fx0000000{i}" for i in range(1, 8)})
        self.assertEqual(f["message_ids"], {f"msg_fx0000000{i}" for i in range(1, 9)})
        self.assertTrue({"skill-alpha", "skill-beta", "srvfx", "mcp__srvfx__tool", "srvpend", "srvfail",
                         "mcp__srvpend__ping", "fx-reviewer", "RULES.md", "srvfx-server"} <= f["loaded_set_names"])
        self.assertTrue(f["loaded_set_names"].isdisjoint({"Bash", "Read", "Explore", "Edit", "Agent", "Skill"}))
        self.assertEqual(self.child.forbids["agent_ids"], {"fx1"})
        self.assertEqual(self.child.forbids["tool_use_ids"], {"toolu_fxc0000001"})

    def test_no_content_string_survives_parse(self):
        """Proves the sentinel planted in every content position of the fixture (prompt, thinking,
        tool input, tool result, toolUseResult bodies, attachment bodies, summary, unknown line,
        meta description) is absent from the parsed facts, refs and cursors. Cannot prove absence
        of content that is not marked with the sentinel."""
        planted = 0
        for path in (MAIN_PATH, CHILD_PATH, META_PATH):
            with open(path, "rb") as fh:
                planted += fh.read().count(SENTINEL.encode())
        self.assertGreaterEqual(planted, 40)
        for obj in (self.main, self.child, self.refs, self.main_cursor, self.child_cursor):
            self.assertNotIn(SENTINEL, repr(obj))
        self.assertNotIn("/fixture/out", repr(self.main))  # toolUseResult.outputFile is a dropped body field

    # -- truncation and cursors --------------------------------------------------------------

    def test_truncation_needs_recent_mtime_and_open_stop_reason(self):
        """Proves truncated is set only when the file changed within 120 s of now AND the last
        assistant stop_reason is not end_turn. Cannot prove a session that ended without an
        end_turn but stopped being written is distinguished from a live one after 120 s."""
        line = json.dumps({"type": "assistant", "timestamp": "2026-08-15T10:00:00.000Z",
                           "message": {"id": "msg_fxt0000001", "model": "claude-fixture-1", "stop_reason": "tool_use",
                                       "content": []}}).encode() + b"\n"
        tmp, path = _temp_session(line)
        with tmp:
            ref = SessionRef(path=path, harness="claude_code", session_key="t", kind="main")
            mtime = _mtime_ms(path)
            live, _ = ClaudeCodeSource(tmp.name, now_ms=mtime + 1_000).read(ref)
            settled, _ = ClaudeCodeSource(tmp.name, now_ms=mtime + 200_000).read(ref)
        self.assertTrue(live.truncated)
        self.assertFalse(settled.truncated)
        finished, _ = ClaudeCodeSource(CLAUDE_HOME, now_ms=self.now_ms).read(self.main_ref)
        self.assertFalse(finished.truncated)

    def test_cursor_resume_and_partial_trailing_line(self):
        """Proves the returned cursor sits at the end of the last complete line, a resumed read
        consumes nothing already read, and a partial trailing line is neither counted nor
        advanced past. Cannot prove inode semantics on non-POSIX filesystems."""
        with open(MAIN_PATH, "rb") as fh:
            size = len(fh.read())
        self.assertEqual((self.main_cursor.byte_offset, self.main_cursor.inode), (size, os.stat(MAIN_PATH).st_ino))
        again, cursor2 = self.source.read(self.main_ref, self.main_cursor)
        self.assertEqual((again.lines_seen, again.tool_calls, cursor2.byte_offset), (0, [], size))
        full = b'{"type":"user","timestamp":"2026-08-15T10:00:00.000Z","message":{"role":"user","content":"hi"}}\n'
        tmp, path = _temp_session(full + full + b'{"type":"user","timestamp":"2026-08-15T10:00:01.000Z", "message":')
        with tmp:
            ref = SessionRef(path=path, harness="claude_code", session_key="t", kind="main")
            facts, cursor = ClaudeCodeSource(tmp.name, now_ms=0).read(ref)
            resumed, cursor_b = ClaudeCodeSource(tmp.name, now_ms=0).read(ref, Cursor(path=path, byte_offset=len(full), inode=None))
        self.assertEqual((facts.lines_seen, facts.user_turns, facts.parse_errors, cursor.byte_offset), (2, 2, 0, 2 * len(full)))
        self.assertEqual((resumed.lines_seen, cursor_b.byte_offset), (1, 2 * len(full)))


if __name__ == "__main__":
    unittest.main()
