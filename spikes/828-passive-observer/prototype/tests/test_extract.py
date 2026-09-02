"""extract.py tests (spike #828). Inputs are SessionFacts / ToolCall / Usage objects built here;
no source module is involved, so these tests prove extract's rules independently of any parser.
Every id, name, fingerprint and number is invented.

Each test states what it proves and what it cannot prove.
"""
import calendar
import os
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from model import ASSET_AGENT, ASSET_MCP_SERVER, ASSET_SKILL  # noqa: E402
from sources.base import (  # noqa: E402
    FAILURE_INTERRUPTED,
    FAILURE_TIMEOUT,
    FAILURE_TOOL_ERROR,
    FAILURE_UNKNOWN,
    FAILURE_USER_DENIED,
    LoadedSetEvent,
    SessionFacts,
    SessionRef,
    ToolCall,
    Usage,
)
import extract  # noqa: E402
from extract import extract as run_extract  # noqa: E402

T0 = calendar.timegm((2026, 3, 10, 12, 0, 0)) * 1000  # 2026-03-10T12:00:00Z, invented
NOW = T0 + 3_600_000
FP_A = "fp-alpha"
FP_B = "fp-beta"


def ref(key="fx-main", kind="main", parent=None, meta=None):
    return SessionRef(path="fixture/" + key + ".ndjson", harness="claude_code", session_key=key, kind=kind,
                      parent_key=parent, child_meta=dict(meta or {}))


def session(**over):
    over.setdefault("ref", ref())
    over.setdefault("first_ts_ms", T0)
    over.setdefault("last_ts_ms", T0 + 60_000)
    over.setdefault("last_stop_reason", "end_turn")
    return SessionFacts(**over)


_seq = [0]


def call(name, ts=T0, fp=FP_A, paired=True, latency=1_000, **kw):
    _seq[0] += 1
    kw.setdefault("tool_use_id", "tu-%03d" % _seq[0])
    return ToolCall(name=name, ts_ms=ts, input_fingerprint=fp, result_ts_ms=(ts + latency) if paired else None, **kw)


def usage(mid, inp=0, out=0, model="claude-sonnet-5", **kw):
    return Usage(message_id=mid, model=model, ts_ms=T0, input_tokens=inp, output_tokens=out, **kw)


def child(key, tool_use_id, agent_type="fx-reviewer", corroborated=True, **over):
    meta = {"agentType": agent_type, "toolUseId": tool_use_id}
    if corroborated:
        meta["corroborated"] = "true"
    over.setdefault("ref", ref(key, kind="child", parent="fx-main", meta=meta))
    over.setdefault("first_ts_ms", T0 + 5_000)
    over.setdefault("last_ts_ms", T0 + 50_000)
    return session(**over)


class Tokens(unittest.TestCase):
    def test_dedupe_across_parent_and_child_sharing_message_ids(self):
        """Proves: a usage keyed by the same provider message id in the parent and in a child is
        summed once for the run (150/15, not 250/25), and the child-only message is still added.
        Cannot prove: that real transcripts ever share message ids (the rule is defensive)."""
        shared = usage("m-shared", inp=100, out=10)
        c = child("agent-c1", "tu-spawn", usages={"m-shared": shared, "m-child": usage("m-child", inp=50, out=5)})
        facts = session(usages={"m-shared": shared}, children=[c])
        run = run_extract(facts, NOW)
        self.assertEqual((run.tokens["input"], run.tokens["output"]), (150, 15))
        self.assertEqual(run.tokens_basis, "harness_usage")

    def test_no_usage_means_basis_none_and_null_buckets(self):
        """Proves: with no usage records the basis is "none", input/output are 0 and every
        provider-specific bucket is None (absent, not zero).
        Cannot prove: that aggregate keeps None as JSON null."""
        run = run_extract(session(), NOW)
        self.assertEqual(run.tokens_basis, "none")
        self.assertEqual(run.tokens["input"], 0)
        self.assertEqual(run.tokens["output"], 0)
        for key in ("cache_creation", "cache_read", "cached_input", "thinking", "reasoning"):
            self.assertIsNone(run.tokens[key], key)

    def test_nullable_bucket_sums_only_reported_values(self):
        """Proves: a bucket one response reports and another omits sums the reported value (5), and
        a bucket nobody reports stays None, so Claude-style and Codex-style usages coexist.
        Cannot prove: the provider semantics of each bucket."""
        facts = session(usages={"m1": usage("m1", cache_read=None), "m2": usage("m2", cache_read=5)})
        run = run_extract(facts, NOW)
        self.assertEqual(run.tokens["cache_read"], 5)
        self.assertIsNone(run.tokens["cached_input"])


class Subagents(unittest.TestCase):
    def test_child_tokens_total_attached_to_parent_agent_invocation(self):
        """Proves: the parent's Agent InvocationObs carries the linked child's exact total
        (input+output+cache_creation+cache_read = 300+40+0+1000 = 1340), is corroborated when the
        child transcript said so, and subagent_runs counts the child. A second Agent call with no
        linked child gets child_tokens_total None and corroborated False.
        Cannot prove: that the source links meta.json toolUseId correctly (that is its own test)."""
        spawn = call("Agent", agent_type="fx-reviewer", child_key="agent-c1", is_async=True)
        orphan = call("Agent", ts=T0 + 10, agent_type="fx-writer")
        c = child("agent-c1", spawn.tool_use_id,
                  usages={"m-c1": usage("m-c1", inp=300, out=40, cache_read=1000), "m-c2": usage("m-c2", thinking=7)})
        run = run_extract(session(tool_calls=[spawn, orphan], children=[c]), NOW)
        agents = [i for i in run.invocations if i.asset_type == ASSET_AGENT]
        self.assertEqual(len(agents), 2)
        self.assertEqual(agents[0].child_tokens_total, 1340)
        self.assertTrue(agents[0].corroborated)
        self.assertIsNone(agents[0].latency_ms)  # async spawn: the parent's result is only an ack
        self.assertIsNone(agents[1].child_tokens_total)
        self.assertFalse(agents[1].corroborated)
        self.assertEqual(run.subagent_runs, 1)

    def test_child_linked_by_meta_tool_use_id_without_child_key(self):
        """Proves: a child whose meta toolUseId equals the parent's Agent tool_use id is linked
        even when the parent result carried no agentId (D4 linkage), and a child with no usage
        yields child_tokens_total None rather than 0.
        Cannot prove: behaviour when both linkage keys disagree."""
        spawn = call("Agent", agent_type="fx-reviewer")
        c = child("agent-c9", spawn.tool_use_id, corroborated=False)
        run = run_extract(session(tool_calls=[spawn], children=[c]), NOW)
        agent = run.invocations[0]
        self.assertIsNone(agent.child_tokens_total)
        self.assertFalse(agent.corroborated)
        self.assertIsNone(agent.failure_class)  # child ended with end_turn

    def test_child_outcome_becomes_agent_failure_class(self):
        """Proves: the child's outcome, not the parent's spawn ack, classifies the agent
        invocation: an interrupted child -> interrupted, a truncated child -> unknown, and a
        parent-level denial wins over any child state. None of these is rate-bearing.
        Cannot prove: that a child's completion means the delegated task succeeded."""
        s1, s2, s3 = (call("Agent", ts=T0 + i, agent_type="fx-reviewer") for i in range(3))
        s3.is_error = True
        s3.failure_class = FAILURE_USER_DENIED
        c1 = child("c-int", s1.tool_use_id, tool_calls=[call("Read", paired=False)])
        c2 = child("c-trunc", s2.tool_use_id, truncated=True)
        c3 = child("c-denied", s3.tool_use_id, truncated=True)
        run = run_extract(session(tool_calls=[s1, s2, s3], children=[c1, c2, c3]), NOW)
        classes = [i.failure_class for i in run.invocations if i.asset_type == ASSET_AGENT]
        self.assertEqual(classes, [FAILURE_INTERRUPTED, FAILURE_UNKNOWN, FAILURE_USER_DENIED])
        self.assertEqual(run.user_denials, 1)
        self.assertEqual(run.tool_failures, 0)

    def test_child_tool_calls_merge_into_parent_counts_and_invocations(self):
        """Proves: a child's tool calls count toward the run's tool_calls, failures and unpaired
        counts and its MCP call appears as an invocation, while the child's user_turns do not
        become run turns.
        Cannot prove: attribution of those invocations to assets (attribute.py)."""
        spawn = call("Agent", agent_type="fx-reviewer")
        c = child("c-tools", spawn.tool_use_id, user_turns=5, tool_calls=[
            call("mcp__fxsrv__lookup", server="fxsrv", is_error=True, failure_class=FAILURE_TOOL_ERROR),
            call("Read", paired=False),
        ])
        run = run_extract(session(user_turns=7, tool_calls=[spawn], children=[c]), NOW)
        self.assertEqual(run.tool_calls, 3)
        self.assertEqual(run.tool_failures, 1)
        self.assertEqual(run.unpaired_tool_uses, 1)
        self.assertEqual(run.turns, 7)
        self.assertIn(ASSET_MCP_SERVER, [i.asset_type for i in run.invocations])


class ToolCallCounts(unittest.TestCase):
    def test_repeated_tool_calls_counts_members_of_groups_of_three_or_more(self):
        """Proves: only calls whose (name, fingerprint) pair occurs >= 3 times count, each
        occurrence counted (3 -> 3, 4 -> 4); a pair occurring twice contributes 0, and the same
        fingerprint under a different tool name is a different group.
        Cannot prove: that repeats are non-convergence rather than legitimate polling."""
        base = [call("Bash", fp=FP_A) for _ in range(3)] + [call("Bash", fp=FP_B) for _ in range(2)] + [call("Read", fp=FP_A)]
        self.assertEqual(run_extract(session(tool_calls=base), NOW).repeated_tool_calls, 3)
        self.assertEqual(run_extract(session(tool_calls=base + [call("Bash", fp=FP_A)]), NOW).repeated_tool_calls, 4)
        self.assertEqual(run_extract(session(tool_calls=base[3:]), NOW).repeated_tool_calls, 0)

    def test_unpaired_tool_uses_counted_and_still_counted_as_calls(self):
        """Proves: calls with no result are counted in unpaired_tool_uses AND in tool_calls, so
        coverage gaps are visible without shrinking the call count.
        Cannot prove: why a result is missing (crash vs still running)."""
        calls = [call("Read"), call("Read", paired=False), call("Grep", paired=False), call("Bash"), call("Edit")]
        run = run_extract(session(tool_calls=calls), NOW)
        self.assertEqual(run.unpaired_tool_uses, 2)
        self.assertEqual(run.tool_calls, 5)

    def test_failure_classes_split_into_tool_failures_and_denials(self):
        """Proves: tool_error and timeout count as tool_failures; user_denied counts only as a
        denial; interrupted and unknown count as neither, so denials and interrupts can never
        inflate a non-success rate.
        Cannot prove: that the source classified each result correctly."""
        calls = [
            call("Bash", is_error=True, failure_class=FAILURE_TOOL_ERROR),
            call("Bash", is_error=True, failure_class=FAILURE_TOOL_ERROR),
            call("Bash", is_error=True, failure_class=FAILURE_TIMEOUT),
            call("Edit", is_error=True, failure_class=FAILURE_USER_DENIED),
            call("Edit", is_error=True, failure_class=FAILURE_INTERRUPTED),
            call("Edit", failure_class=FAILURE_UNKNOWN),
        ]
        run = run_extract(session(tool_calls=calls), NOW)
        self.assertEqual(run.tool_failures, 3)
        self.assertEqual(run.user_denials, 1)

    def test_turns_pass_through_without_recount(self):
        """Proves: turns is exactly the source's user_turns; tool calls, results, children and
        loaded events in the tree do not change it. The source is where isMeta and tool_result-only
        lines are excluded (tested there); extract must not second-guess that count.
        Cannot prove: that the source's exclusion rules are right."""
        c = child("c-turns", "tu-none", user_turns=4, tool_calls=[call("Read")])
        facts = session(user_turns=7, tool_calls=[call("Read"), call("Bash")], children=[c],
                        loaded_events=[LoadedSetEvent(ts_ms=T0, kind="initial", skills=["fx-skill"])])
        self.assertEqual(run_extract(facts, NOW).turns, 7)
        self.assertEqual(run_extract(session(user_turns=0, tool_calls=[call("Read")]), NOW).turns, 0)


class ToolClassShares(unittest.TestCase):
    TABLE = {
        "edit": ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"],
        "read": ["Read", "Glob", "Grep", "LS", "WebFetch", "WebSearch"],
        "shell": ["Bash", "shell", "exec"],
        "other": ["Skill", "Agent", "TodoWrite"],
    }

    def test_classification_table(self):
        """Proves: every name the contract lists lands in its class, unknown names are `other`,
        `mcp__*` names are `mcp`, and a Codex-style `<server>__<tool>` name with `server` set is
        `mcp` even without the prefix.
        Cannot prove: that the list of built-in names is complete for future harness versions."""
        for cls, names in self.TABLE.items():
            for name in names:
                self.assertEqual(extract.tool_class(name, None), cls, name)
        self.assertEqual(extract.tool_class("mcp__fxsrv__lookup", "fxsrv"), "mcp")
        self.assertEqual(extract.tool_class("mcp__fxsrv__lookup", None), "mcp")
        self.assertEqual(extract.tool_class("fxsrv__lookup", "fxsrv"), "mcp")
        self.assertEqual(extract.tool_class("read", None), "other")  # case-sensitive: not the Read tool

    def test_shares_sum_to_one_and_match_counts(self):
        """Proves: shares are count/total per class over the run and sum to 1 (5 edit, 6 read, 3
        shell, 2 mcp, 4 other of 20), and every class key is present even when zero.
        Cannot prove: that these shares are a good proxy for the task (taskcat's concern)."""
        calls = [call(n) for names in self.TABLE.values() for n in names]
        calls += [call("mcp__fxsrv__lookup", server="fxsrv"), call("fxsrv__lookup", server="fxsrv"), call("fx-unknown-tool")]
        run = run_extract(session(tool_calls=calls), NOW)
        self.assertEqual(len(calls), 20)
        self.assertAlmostEqual(sum(run.tool_class_shares.values()), 1.0)
        self.assertEqual(run.tool_class_shares, {"edit": 0.25, "read": 0.3, "shell": 0.15, "mcp": 0.1, "other": 0.2})

    def test_no_calls_gives_all_zero_shares(self):
        """Proves: a run without tool calls has every share 0.0 (taskcat maps that to
        `unspecified`) rather than raising on division by zero.
        Cannot prove: taskcat's mapping (tested in test_taskcat)."""
        run = run_extract(session(), NOW)
        self.assertEqual(set(run.tool_class_shares), {"edit", "read", "shell", "mcp", "other"})
        self.assertEqual(sum(run.tool_class_shares.values()), 0.0)


class RunShape(unittest.TestCase):
    def test_entrypoint_class_mapping(self):
        """Proves: the substring rules and their precedence (remote > ide > sdk > cli > unknown),
        case-insensitively: "sdk-cli" is sdk, "remote-cli" is remote, jetbrains/vscode/ide are ide.
        Cannot prove: that real harness entrypoint strings contain these substrings."""
        cases = {
            "cli": "cli", "codex_cli_rs": "cli", "sdk-cli": "sdk", "sdk-ts": "sdk", "vscode": "ide",
            "jetbrains-plugin": "ide", "some-ide": "ide", "remote-cli": "remote", "Remote-Control": "remote",
            "unknown": "unknown", "": "unknown", None: "unknown",
        }
        for raw, want in cases.items():
            self.assertEqual(extract.entrypoint_class(raw), want, repr(raw))
        self.assertEqual(run_extract(session(entrypoint="sdk-cli"), NOW).entrypoint_class, "sdk")

    def test_permission_mode_mapping(self):
        """Proves: harness camelCase modes map to the gate enum, plan/default/auto pass through,
        anything else (including a Codex approval policy) is unknown; matching is exact.
        Cannot prove: that the codex source pre-maps its approval policies."""
        cases = {
            "acceptEdits": "accept_edits", "bypassPermissions": "bypass", "dontAsk": "dont_ask",
            "plan": "plan", "default": "default", "auto": "auto",
            "on-request": "unknown", "acceptedits": "unknown", "": "unknown", None: "unknown",
        }
        for raw, want in cases.items():
            self.assertEqual(extract.permission_mode(raw), want, repr(raw))
        self.assertEqual(run_extract(session(permission_mode="bypassPermissions"), NOW).permission_mode, "bypass")

    def test_effort_normalised_to_closed_enum(self):
        """Proves: effort values the gate lists pass through and any other value is unknown, so
        the payload cannot carry an off-enum effort.
        Cannot prove: the contract's intent (it is silent on effort; this is a stated choice)."""
        for raw in ("minimal", "low", "medium", "high", "xhigh"):
            self.assertEqual(extract.effort_class(raw), raw)
        for raw in ("max", "High", "", None):
            self.assertEqual(extract.effort_class(raw), "unknown", repr(raw))

    def test_run_outcome_decision_table(self):
        """Proves: each branch and its precedence. truncated beats everything; compacted needs
        compactions>0 AND a non-end_turn stop; interrupted (an unpaired call, or the last call
        marked interrupted) beats completed; completed needs end_turn; otherwise unknown. An
        interrupt that is not the last call does not make a finished run interrupted.
        Cannot prove: that the task itself finished (never claimed)."""
        unpaired = [call("Read", paired=False)]
        cut = call("Bash", interrupted=True)
        cases = [
            (dict(truncated=True, compactions=1, last_stop_reason="tool_use", tool_calls=unpaired), "truncated"),
            (dict(compactions=1, last_stop_reason="tool_use", tool_calls=unpaired), "compacted"),
            (dict(compactions=1, last_stop_reason=None), "compacted"),
            (dict(compactions=1, last_stop_reason="end_turn", tool_calls=unpaired), "interrupted"),
            (dict(compactions=1, last_stop_reason="end_turn"), "completed"),
            (dict(last_stop_reason="end_turn", tool_calls=[call("Read"), cut]), "interrupted"),
            (dict(last_stop_reason="end_turn", tool_calls=[cut, call("Read")]), "completed"),
            (dict(last_stop_reason="end_turn", tool_calls=[call("Read")]), "completed"),
            (dict(last_stop_reason="tool_use"), "unknown"),
            (dict(last_stop_reason=None), "unknown"),
        ]
        for over, want in cases:
            self.assertEqual(run_extract(session(**over), NOW).run_outcome, want, over)

    def test_model_is_most_frequent_then_allowlisted(self):
        """Proves: the dominant model is chosen by response count in the MAIN transcript (a child
        on another model does not change it; tokens_by_model carries the split), then allowlisted:
        a winning invented provider becomes "other", a winning claude-x passes, ties break
        deterministically, and no models at all is "other". Cannot prove: that response count is
        the best notion of a run's dominant model."""
        self.assertEqual(run_extract(session(models={"claude-sonnet-5": 3, "fxprovider-custom-9": 5}), NOW).model, "other")
        self.assertEqual(run_extract(session(models={"claude-sonnet-5": 5, "gpt-5": 2}), NOW).model, "claude-sonnet-5")
        c = child("c-model", "tu-none", models={"claude-sonnet-5": 2})
        self.assertEqual(run_extract(session(models={"gpt-5": 2}, children=[c]), NOW).model, "gpt-5")
        self.assertEqual(run_extract(session(models={}, children=[c]), NOW).model, "claude-sonnet-5")
        self.assertEqual(run_extract(session(models={"gpt-5": 2, "claude-sonnet-5": 2}), NOW).model, "claude-sonnet-5")
        self.assertEqual(run_extract(session(models={}), NOW).model, "other")


class ObservedDay(unittest.TestCase):
    def test_observed_day_is_utc_day_of_first_timestamp(self):
        """Proves: observed_day is the UTC calendar day of first_ts_ms at the exact boundary: half
        a second after midnight UTC is that day, one millisecond before midnight UTC is the day
        before — and this holds under a host TZ where the local day differs (each case runs under
        a POSIX offset that pushes the local date across midnight; the test asserts that skew is
        real before asserting the result, so it cannot pass vacuously on a UTC host).
        Cannot prove: behaviour if a source ever emits non-UTC-normalised timestamps."""
        midnight = calendar.timegm((2026, 3, 10, 0, 0, 0)) * 1000
        # (POSIX TZ string, ts_ms, expected UTC day); POSIX offsets are inverted: "AAA10" is UTC-10.
        cases = [("AAA10", midnight + 500, "2026-03-10"), ("BBB-14", midnight - 1, "2026-03-09")]
        old_tz = os.environ.get("TZ")
        try:
            for tz, ts, want in cases:
                os.environ["TZ"] = tz
                time.tzset()
                local_day = time.strftime("%Y-%m-%d", time.localtime(ts // 1000))
                self.assertNotEqual(local_day, want, "test setup: local day must differ from the UTC day")
                run = run_extract(session(first_ts_ms=ts, last_ts_ms=ts + 10), NOW)
                self.assertEqual(run.observed_day, want, tz)
        finally:
            if old_tz is None:
                os.environ.pop("TZ", None)
            else:
                os.environ["TZ"] = old_tz
            time.tzset()

    def test_span_covers_children_and_falls_back_to_now(self):
        """Proves: first/last span the tree (a child ending after the parent extends last_ts_ms),
        and a session with no harness timestamp at all uses now_ms for both rather than raising.
        Cannot prove: that a timestamp-less session is worth emitting at all."""
        c = child("c-late", "tu-none", last_ts_ms=T0 + 90_000)
        run = run_extract(session(children=[c]), NOW)
        self.assertEqual((run.first_ts_ms, run.last_ts_ms), (T0, T0 + 90_000))
        bare = run_extract(session(first_ts_ms=None, last_ts_ms=None), NOW)
        self.assertEqual((bare.first_ts_ms, bare.last_ts_ms), (NOW, NOW))


class Invocations(unittest.TestCase):
    def test_skill_mcp_and_agent_invocations_with_latency_rules(self):
        """Proves: a Skill call yields a skill invocation, an mcp__ call yields an mcp_server
        invocation named after the server, an Agent call yields an agent invocation; latency is
        result minus call for paired sync calls and None for async or unpaired calls; failure
        classes pass through.
        Cannot prove: how attribute.py keys these names (name_hash vs content_hash)."""
        calls = [
            call("Skill", skill="fx-skill", latency=250),
            call("Skill", skill="fx-skill-body", is_async=True, latency=0),  # synthetic in-band body
            call("mcp__fxsrv__lookup", server="fxsrv", paired=False, is_error=True, failure_class=FAILURE_TOOL_ERROR),
            call("Agent", agent_type="fx-reviewer", latency=4_000),
            call("Bash"),
        ]
        run = run_extract(session(tool_calls=calls), NOW)
        got = [(i.asset_type, i.name, i.latency_ms, i.failure_class) for i in run.invocations]
        self.assertEqual(got, [
            (ASSET_SKILL, "fx-skill", 250, None),
            (ASSET_SKILL, "fx-skill-body", None, None),
            (ASSET_MCP_SERVER, "fxsrv", None, FAILURE_TOOL_ERROR),
            (ASSET_AGENT, "fx-reviewer", 4_000, None),
        ])


class Bookkeeping(unittest.TestCase):
    def test_forbids_and_coverage_merge_over_tree(self):
        """Proves: forbid buckets union across parent and children (the checker must see a child's
        ids too), coverage counters sum over the tree, compactions sum, and loaded events come from
        the parent only.
        Cannot prove: that the checker consumes these buckets (test_gate)."""
        c = child("c-fb", "tu-none", lines_seen=4, bytes_read=40, compactions=1, lines_unknown_type=1, parse_errors=1,
                  loaded_events=[LoadedSetEvent(ts_ms=T0, kind="initial", skills=["fx-child-skill"])],
                  forbids={"agent_ids": {"agent-fx-child"}, "loaded_set_names": {"fx-child-skill"}})
        facts = session(lines_seen=10, bytes_read=100, compactions=2, children=[c],
                        loaded_events=[LoadedSetEvent(ts_ms=T0, kind="initial", skills=["fx-skill"])],
                        forbids={"loaded_set_names": {"fx-skill"}, "harness_session_ids": {"fx-main"}})
        run = run_extract(facts, NOW)
        self.assertEqual(run.forbids["loaded_set_names"], {"fx-skill", "fx-child-skill"})
        self.assertEqual(run.forbids["agent_ids"], {"agent-fx-child"})
        self.assertEqual((run.lines_seen, run.bytes_read, run.lines_unknown_type, run.parse_errors), (14, 140, 1, 1))
        self.assertEqual(run.compactions, 3)
        self.assertEqual([e.skills for e in run.loaded_events], [["fx-skill"]])
        self.assertEqual((run.session_key, run.harness), ("fx-main", "claude_code"))


if __name__ == "__main__":
    unittest.main()
