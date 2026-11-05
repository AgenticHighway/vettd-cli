"""rank.py tests (spike #828). Every asset, count, model and price below is invented; asset ids are
sha256 of fixture labels. Envelopes are built directly as dicts so these tests do not depend on
aggregate.py's fixture.

Each test states what it proves and what it cannot prove. None of them can prove D5 chose the RIGHT
floors; they prove the code implements the published floors, the Wilson ordering rule and the
non-causal copy exactly as the contract states them.
"""
import builtins
import hashlib
import json
import os
import re
import sys
import tempfile
import unittest
from typing import Dict, List, Optional

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import rank as rank_module  # noqa: E402
from rank import COPY, FLOORS, RankResult, evidence_state, rank, render, wilson  # noqa: E402

# Local copy of the forbidden list from CONTRACTS.md "lint_copy.py", so this test does not depend
# on lint_copy.py. Case-insensitive.
FORBIDDEN = [re.compile(rx, re.IGNORECASE) for rx in (
    r"\bcauses?\b", r"\bbecause of\b", r"\bimproves?\b", r"\bmakes? (?:you|your|it)\b", r"\bfaster than\b",
    r"\bbetter than\b", r"\bworse than\b", r"% ?(?:better|worse)\b", r"\bsaves?\b", r"\bproves?\b", r"\bguarantee",
    r"\$\d", r"(?<!observed )\b(?:un)?reliable\b",
)]
RATE_RE = re.compile(r"\brates?\b", re.IGNORECASE)
HEDGE_RE = re.compile(r"observed|in \d+ calls", re.IGNORECASE)
HARNESS = "claude_code"
MODEL = "claude-sonnet-5"


def lint_lines(text: str) -> List[str]:
    findings = []
    for line in text.splitlines():
        findings.extend(f"{rx.pattern} in {line!r}" for rx in FORBIDDEN if rx.search(line))
        if RATE_RE.search(line) and not HEDGE_RE.search(line):
            findings.append(f"unhedged rate in {line!r}")
    return findings


def hex64(label: str) -> str:
    return hashlib.sha256(("fixture:" + label).encode("utf-8")).hexdigest()


def asset(label: str, asset_type: str = "mcp_server", n: int = 0, tool_error: int = 0, timeout: int = 0,
          user_denied: int = 0, interrupted: int = 0, unknown: int = 0, latency: Optional[List[int]] = None,
          child_tokens: Optional[List[int]] = None, cost: Optional[int] = None, tier: str = "inferred") -> dict:
    lat = latency or []
    ct = child_tokens
    return {
        "asset_id": hex64(label), "asset_type": asset_type, "key_basis": "name_hash", "tier": tier,
        "binding": "not_applicable", "direct_evidence_available": n > 0,
        "signals": {
            "invocations": {"n": n},
            "failures": {"tool_error": tool_error, "timeout": timeout, "user_denied": user_denied,
                         "interrupted": interrupted, "unknown": unknown},
            "harness_corroborations": None,
            "latency_ms": {"n": len(lat), "sum": sum(lat), "min": min(lat) if lat else 0, "max": max(lat) if lat else 0,
                           "sumsq": sum(v * v for v in lat)},
            "tokens_attributed": None if not ct else {"n": len(ct), "sum": sum(ct), "min": min(ct), "max": max(ct),
                                                      "sumsq": sum(v * v for v in ct)},
            "context_cost_est": None if cost is None else {"tokens": cost, "method": "file_bytes_div4"},
        },
    }


def record(label: str, assets: List[dict], day: str = "2026-03-05", model: str = MODEL, category: str = "code_edit",
           tokens: Optional[Dict[str, Optional[int]]] = None) -> dict:
    tk = {"input": 1000, "cache_creation": None, "cache_read": None, "cached_input": None, "output": 500,
          "thinking": None, "reasoning": None, "basis": "harness_usage"}
    tk.update(tokens or {})
    return {"run_id": hex64("run-" + label), "observed_day": day, "model": model, "entrypoint_class": "cli",
            "effort": "medium", "permission_mode": "default", "task_category": category, "bom_version": hex64("bom"),
            "loaded_set_basis": "harness_log", "run_outcome": "completed",
            "counts": {"turns": 1, "tool_calls": 1, "tool_failures": 0, "user_denials": 0, "subagent_runs": 0,
                       "compactions": 0, "unpaired_tool_uses": 0, "repeated_tool_calls": 0},
            "tokens": tk, "assets": assets}


def envelope(records: List[dict], harness: str = HARNESS) -> dict:
    return {"envelope_version": "0.1.0", "extractor_version": "proto-0.1.0+taskcat-1", "gate_version": 1,
            "emitted_day": "2026-03-06",
            "resource": {"device_id": "00000000-0000-4000-8000-000000000000", "device_id_source": "placeholder",
                         "harness": harness, "harness_version": "1.0.0", "collector": "prototype",
                         "collector_version": "0.1.0"},
            "records": records, "bom": [], "coverage": {}}


def names_for(*labels_and_types) -> Dict[str, str]:
    return {hex64(label): f"{asset_type}:{label}" for label, asset_type in labels_and_types}


def populated() -> tuple:
    """A stratum with one asset in every list: ranked (two, to order), early, insufficient (two, to
    sort), loaded-only (rules file with and without a cost basis), plus an agent with exact tokens."""
    recs = [
        record("r1", [asset("zeta-invented", n=50, latency=[100] * 50), asset("eta-invented", n=1000, tool_error=6, timeout=4,
                                                                            user_denied=3, latency=[400] * 200),
                      asset("theta-invented", n=25, tool_error=1), asset("iota-invented", n=7),
                      asset("kappa-invented", n=12, tool_error=2), asset("lambda-invented", "rules_file", cost=800),
                      asset("mu-invented", "prompt"), asset("nu-invented", "agent", n=60, child_tokens=[5000] * 4)]),
        record("r2", [asset("iota-invented", n=0)], day="2026-03-04"),
    ]
    names = names_for(("zeta-invented", "mcp_server"), ("eta-invented", "mcp_server"), ("theta-invented", "mcp_server"),
                      ("iota-invented", "mcp_server"), ("kappa-invented", "mcp_server"), ("lambda-invented", "rules_file"),
                      ("mu-invented", "prompt"), ("nu-invented", "agent"))
    return envelope(recs), names


class WilsonTests(unittest.TestCase):
    def test_zero_of_twenty_upper_bound_is_point_one_six_one(self):
        """Proves: wilson(0, 20) is [0, 0.161] to three decimals — the D5 claim that twenty clean
        calls are informative. Hand value: z^2 = 3.8416; centre = 3.8416/40 = 0.09604; margin =
        1.96 * sqrt(3.8416/1600) = 0.09604; denominator 1.19208; hi = 0.19208/1.19208 = 0.1611.
        Cannot prove: the interval's coverage properties, only the arithmetic."""
        lo, hi = wilson(0, 20)
        self.assertEqual(lo, 0.0)
        self.assertAlmostEqual(hi, 0.161, places=3)

    def test_five_of_fifty_matches_hand_value(self):
        """Proves: wilson(5, 50) is [0.043, 0.214] to three decimals. Hand value: p = 0.1; centre =
        0.1 + 3.8416/100 = 0.138416; margin = 1.96 * sqrt(0.09/50 + 3.8416/10000) = 1.96 * 0.046735
        = 0.091601; denominator 1.076832; lo = 0.046815/1.076832 = 0.0435; hi = 0.230017/1.076832
        = 0.2136. Cannot prove: behaviour for a different z."""
        lo, hi = wilson(5, 50)
        self.assertAlmostEqual(lo, 0.043, places=3)
        self.assertAlmostEqual(hi, 0.214, places=3)

    def test_zero_calls_is_the_whole_range_and_bounds_are_clamped(self):
        """Proves: n = 0 yields [0, 1] (no information) and k = n yields hi = 1.0, lo < 1, so a row
        can never show an impossible interval. Cannot prove: numerical stability for huge n."""
        self.assertEqual(wilson(0, 0), (0.0, 1.0))
        lo, hi = wilson(30, 30)
        self.assertEqual(hi, 1.0)
        self.assertLess(lo, 1.0)


class EvidenceStateTests(unittest.TestCase):
    def test_floors_are_the_published_ones(self):
        """Proves: FLOORS holds exactly D5's display floors; a change here is a product decision and
        must fail a test. Cannot prove: the floors are the right ones."""
        self.assertEqual(FLOORS, {"count": 1, "tokens": 3, "latency": 5, "rate_show": 20, "rate_order": 50})

    def test_rate_bands(self):
        """Proves: a rate is insufficient below 20, early evidence from 20 to 49, observed from 50 —
        the boundaries are inclusive at the floor. Cannot prove: what the display does with each
        state (see render tests)."""
        self.assertEqual(evidence_state("rate", 19), "insufficient_evidence")
        self.assertEqual(evidence_state("rate", 20), "early_evidence")
        self.assertEqual(evidence_state("rate", 49), "early_evidence")
        self.assertEqual(evidence_state("rate", 50), "observed")

    def test_count_tokens_latency_floors_and_special_states(self):
        """Proves: count/tokens/latency are observed at their floor and insufficient below it; None
        means no coverage (never observable), applicable=False means not applicable, and an unknown
        signal name raises rather than returning a state. Cannot prove: callers pass the right n."""
        self.assertEqual(evidence_state("count", 0), "insufficient_evidence")
        self.assertEqual(evidence_state("count", 1), "observed")
        self.assertEqual(evidence_state("tokens", 2), "insufficient_evidence")
        self.assertEqual(evidence_state("tokens", 3), "observed")
        self.assertEqual(evidence_state("latency", 4), "insufficient_evidence")
        self.assertEqual(evidence_state("latency", 5), "observed")
        self.assertEqual(evidence_state("latency", None), "no_coverage")
        self.assertEqual(evidence_state("rate", 500, applicable=False), "not_applicable")
        with self.assertRaises(ValueError):
            evidence_state("duration", 10)


class RankingTests(unittest.TestCase):
    def test_upper_bound_ordering_punishes_low_n_not_high_n(self):
        """Proves: k=0/n=50 (hi 0.071) ranks BELOW k=10/n=1000 (hi 0.018): ordering is by the
        interval's upper bound, not by the point rate, so a thin clean record does not beat a thick
        record with a few non-successes. Cannot prove: that users read the order this way."""
        env = envelope([record("r", [asset("clean-thin", n=50), asset("thick-invented", n=1000, tool_error=10)])])
        result = rank(env, {}, "fix the invented thing", HARNESS)
        self.assertEqual([r.asset_id for r in result.ranked], [hex64("thick-invented"), hex64("clean-thin")])
        self.assertLess(result.ranked[0].hi, result.ranked[1].hi)

    def test_tiebreak_is_minus_n_then_asset_id(self):
        """Proves: equal upper bounds order by larger n first, then asset_id ascending — the key is
        (hi, -n, asset_id) exactly. Cannot prove: tie behaviour for hi values differing in the last
        float digit."""
        ids = sorted([hex64("same-a"), hex64("same-b")])
        env = envelope([record("r", [asset("same-b", n=60), asset("same-a", n=60), asset("bigger-invented", n=120)])])
        result = rank(env, {}, "fix", HARNESS)
        ordered = [r.asset_id for r in result.ranked]
        self.assertEqual(ordered[0], hex64("bigger-invented"))  # same k=0, larger n -> smaller hi anyway
        self.assertEqual(ordered[1:], ids)

    def test_lists_are_separated_by_floor_and_insufficient_is_sorted_by_n_desc_with_needs(self):
        """Proves: n >= 50 ranked, 20 <= n < 50 early, n < 20 insufficient with needs = 20 - n and
        sorted by n descending; rules files and prompts go to loaded_only regardless of n, and a
        state of not_applicable. Cannot prove: the floors themselves (see FLOORS test)."""
        env, names = populated()
        result = rank(env, names, "fix", HARNESS)
        self.assertEqual({r.asset_id for r in result.ranked}, {hex64("zeta-invented"), hex64("eta-invented"), hex64("nu-invented")})
        self.assertEqual([r.asset_id for r in result.early], [hex64("theta-invented")])
        self.assertEqual([(r.asset_id, r.n, r.needs) for r in result.insufficient],
                         [(hex64("kappa-invented"), 12, 8), (hex64("iota-invented"), 7, 13)])
        self.assertEqual([r.asset_id for r in result.loaded_only], sorted([hex64("lambda-invented"), hex64("mu-invented")]))
        self.assertTrue(all(r.rate_state == "not_applicable" for r in result.loaded_only))
        self.assertEqual(result.early[0].rate_state, "early_evidence")

    def test_aggregation_across_records_counts_only_rate_bearing_failures(self):
        """Proves: the same asset in three records merges (n sums to 60 -> ranked) and k counts
        tool_error + timeout only; user_denied, interrupted and unknown are kept apart and never
        raise the rate. Cannot prove: the sources classified denials correctly."""
        recs = [record(f"r{i}", [asset("split-invented", n=20, tool_error=1, timeout=1, user_denied=2, interrupted=1, unknown=1)])
                for i in range(3)]
        result = rank(envelope(recs), {}, "fix", HARNESS)
        row = result.ranked[0]
        self.assertEqual((row.n, row.k, row.user_denied, row.interrupted, row.unknown, row.runs), (60, 6, 6, 3, 3, 3))
        self.assertEqual(result.run_count, 3)

    def test_stratum_filters_by_task_category_model_and_harness(self):
        """Proves: only records whose task_category matches the stated task's category (keyword
        table) enter the rows; other categories appear as context counts; a model filter narrows
        further; a harness mismatch yields an empty result. Cannot prove: the keyword table maps
        every real task well — it is a prototype placeholder."""
        recs = [record("edit1", [asset("a-invented", n=60)], category="code_edit"),
                record("explore1", [asset("a-invented", n=60)], category="code_explore"),
                record("edit2", [asset("a-invented", n=60)], category="code_edit", model="gpt-5-mini")]
        env = envelope(recs)
        result = rank(env, {}, "fix the invented bug", HARNESS)
        self.assertEqual(result.task_category, "code_edit")
        self.assertEqual(result.ranked[0].n, 120)
        self.assertEqual(result.context, [("code_explore", 1)])
        self.assertEqual(result.models, {MODEL: 1, "gpt-5-mini": 1})
        narrowed = rank(env, {}, "fix the invented bug", HARNESS, model=MODEL)
        self.assertEqual(narrowed.ranked[0].n, 60)
        pooled = rank(env, {}, "something without keywords", HARNESS)
        self.assertEqual((pooled.task_category, pooled.run_count), ("unspecified", 3))
        self.assertEqual(rank(env, {}, "fix", "codex").run_count, 0)

    def test_rank_reads_no_files(self):
        """Proves: rank() completes with builtins.open replaced by a function that raises, so it
        cannot be reading prices.json or anything else. Cannot prove: the same for render()."""
        env, names = populated()
        real_open = builtins.open

        def refuse(*args, **kwargs):
            raise AssertionError("rank() opened a file")

        builtins.open = refuse
        try:
            result = rank(env, names, "fix", HARNESS)
        finally:
            builtins.open = real_open
        self.assertIsInstance(result, RankResult)


class RenderTests(unittest.TestCase):
    def setUp(self):
        self.env, self.names = populated()
        self.result = rank(self.env, self.names, "fix the invented module", HARNESS)
        self.prices_dir = tempfile.TemporaryDirectory()
        self.prices_path = os.path.join(self.prices_dir.name, "prices-invented.json")
        with open(self.prices_path, "w", encoding="utf-8") as fh:
            json.dump({"as_of": "2031-01-01", "per_million_tokens": {
                MODEL: {"input": 1.0, "cache_creation": 1.0, "cache_read": 1.0, "output": 2.0}, "other": None}}, fh)

    def tearDown(self):
        self.prices_dir.cleanup()

    def out(self, scrub=False, public=frozenset(), prices=None) -> str:
        return render(self.result, scrub=scrub, public_names=set(public), prices_path=prices or self.prices_path)

    def test_every_ranked_row_shows_tier_state_and_the_count_phrase(self):
        """Proves: each ranked row carries tier=<tier>, state=observed and "<k> non-successes in
        <n> calls" with a 95% interval, one row per ranked asset, in ranked order. Cannot prove: the
        row is readable; only that the required elements are present."""
        text = self.out()
        rows = re.findall(r"^\s*\d+\. (\S+)  tier=(\w+) state=(\w+)  (\d+) non-successes in (\d+) calls \(95% interval [\d.]+%–[\d.]+%\)",
                          text, re.MULTILINE)
        self.assertEqual(len(rows), len(self.result.ranked))
        for (name, tier, state, k, n), row in zip(rows, self.result.ranked):
            self.assertEqual(name, self.names[row.asset_id])
            self.assertEqual((tier, state, int(k), int(n)), (row.tier, "observed", row.k, row.n))
        self.assertIn("early_evidence", text)
        self.assertIn("needs 8 more", text)
        self.assertIn("needs 13 more", text)
        # Loaded-only rows show the state of the one signal that applies to them (context cost):
        # observed when an estimate exists, no_coverage when there is no basis for one.
        self.assertIn("rules_file:lambda-invented  tier=inferred state=observed  context cost est. 800 tokens (file_bytes_div4)", text)
        self.assertIn("prompt:mu-invented  tier=inferred state=no_coverage  no context-cost basis", text)
        self.assertIn("child tokens mean 5000 in 4 exactly attributed runs (observed)", text)
        self.assertIn("3 user denials and 0 interruptions excluded", text)

    def test_scrub_replaces_names_unless_public(self):
        """Proves: with scrub every name becomes "<type>:<asset_id[:12]>" and no local name appears;
        a name in public_names survives; without scrub names appear. Cannot prove: public_names was
        curated correctly by the person running it."""
        scrubbed = self.out(scrub=True)
        for display in self.names.values():
            self.assertNotIn(display, scrubbed)
        zeta = hex64("zeta-invented")
        self.assertIn(f"mcp_server:{zeta[:12]}", scrubbed)
        self.assertNotIn(zeta[:13], scrubbed)
        partly = self.out(scrub=True, public={"mcp_server:zeta-invented"})
        self.assertIn("mcp_server:zeta-invented", partly)
        self.assertNotIn("mcp_server:eta-invented", partly)
        self.assertIn("mcp_server:eta-invented", self.out())

    def test_cost_line_names_the_price_table_date_and_is_a_display_time_derivation(self):
        """Proves: the cost line is computed from the stratum's tokens x the table for the run's
        model (2 runs x (1000 in x 1.0 + 500 out x 2.0) per million = USD 0.00 rounds; with a
        million-token record it is USD 2.00), names the table's date, says "(display-time
        derivation, not stored)", and a model without a price entry says so rather than inventing a
        figure; a missing table yields the unavailable line, not an exception. Cannot prove: the
        prices are current — that is what the date is for."""
        env = envelope([record("big", [asset("v-invented", n=60)], tokens={"input": 1_000_000, "output": 500_000}),
                        record("np", [asset("v-invented", n=60)], model="other")])
        result = rank(env, {}, "fix", HARNESS)
        text = render(result, scrub=False, public_names=set(), prices_path=self.prices_path)
        self.assertIn("price table dated 2031-01-01", text)
        self.assertIn("(display-time derivation, not stored)", text)
        self.assertIn(f"{MODEL}: USD 2.00 over 1 runs", text)
        self.assertIn("other: no price entry in the table dated 2031-01-01", text)
        missing = render(result, scrub=False, public_names=set(), prices_path=os.path.join(self.prices_dir.name, "absent.json"))
        self.assertIn("price table unavailable", missing)
        self.assertNotIn("USD", missing)

    def test_empty_stratum_renders_the_empty_state(self):
        """Proves: with no runs at all the empty-state line renders and no rows; with runs only in
        other categories the view pools them and SAYS so (the pooled line), still naming the other
        categories as context. Cannot prove: anything about a populated matched stratum."""
        env = envelope([record("x", [asset("w-invented", n=60)], category="code_explore")])
        text = render(rank(env, {}, "fix", HARNESS), scrub=False, public_names=set(), prices_path=self.prices_path)
        self.assertNotIn(COPY["empty"], text)
        self.assertIn("pools every task category", text)
        self.assertIn("code_explore 1 runs", text)
        empty = render(rank(envelope([]), {}, "fix", HARNESS), scrub=False, public_names=set(), prices_path=self.prices_path)
        self.assertIn(COPY["empty"], empty)
        self.assertNotIn("non-successes", empty)

    def test_rendered_output_passes_the_local_lint(self):
        """Proves: the full rendered text of a populated stratum (with names, cost lines, denials
        and every list present) contains no forbidden causal phrase, no money sign followed by a
        digit, and every line naming a rate is hedged. Cannot prove: copy the template placeholders
        are filled with from user-chosen names — the task text and names are local input."""
        self.assertEqual(lint_lines(self.out()), [])
        self.assertEqual(lint_lines(self.out(scrub=True)), [])


class CopyTests(unittest.TestCase):
    def test_no_copy_template_contains_a_forbidden_phrase(self):
        """Proves: every template in COPY is free of the forbidden causal phrases from CONTRACTS.md
        (causes, because of, improves, makes you/it, faster/better/worse than, % better/worse,
        saves, proves, guarantee, $<digit>, bare reliable) and every template that names a rate
        contains "observed". Cannot prove: prose assembled outside COPY is clean — the rendered-
        output test covers that for the fixture."""
        self.assertGreater(len(COPY), 10)
        for key_, template in COPY.items():
            for rx in FORBIDDEN:
                self.assertIsNone(rx.search(template), f"COPY[{key_!r}] matches {rx.pattern}")
            if RATE_RE.search(template):
                self.assertIn("observed", template.lower(), f"COPY[{key_!r}] names a rate without 'observed'")

    def test_lint_regexes_can_fail(self):
        """Proves: the local forbidden list actually matches the phrases it is meant to catch and
        the hedge rule flags an unhedged rate line, so the COPY test is not a tautology. Cannot
        prove: parity with lint_copy.py's exact regex text."""
        self.assertTrue(lint_lines("this asset improves your results"))
        self.assertTrue(lint_lines("costs $12 per run"))
        self.assertTrue(lint_lines("a reliable server"))
        self.assertFalse(lint_lines("observed reliable in this stratum"))
        self.assertTrue(lint_lines("failure rate 3%"))
        self.assertFalse(lint_lines("observed non-success rate 3%"))
        self.assertFalse(lint_lines("2 non-successes in 40 calls"))

    def test_render_uses_only_copy_templates(self):
        """Proves: every non-empty rendered line matches some COPY template once placeholders are
        replaced by a wildcard, so no string outside COPY reaches the output (and the lint over COPY
        is sufficient). Cannot prove: the substituted values are clean."""
        env, names = populated()
        text = render(rank(env, names, "fix", HARNESS), scrub=False, public_names=set(),
                      prices_path=os.path.join(tempfile.gettempdir(), "no-such-invented-prices.json"))
        patterns = [re.compile("^" + re.sub(r"\\{[^}]*\\}", ".*", re.escape(t)) + "$") for t in COPY.values()]
        for line in text.splitlines():
            self.assertTrue(any(p.match(line) for p in patterns), f"line not from COPY: {line!r}")


if __name__ == "__main__":
    unittest.main()



class PooledFallbackTests(unittest.TestCase):
    """The matched task-category stratum falls back to a visibly pooled view only when it is empty.

    Proves: with no runs in the matched category the result pools every category and says so
    (pooled_categories True, the pooled copy line rendered); with runs in the matched category the
    other categories stay out (pooled_categories False). Cannot prove the keyword table maps a task
    to the right category; that is task_category_for's own test.
    """

    def _env(self, categories):
        import hashlib
        recs = []
        for i, cat in enumerate(categories):
            rid = hashlib.sha256(f"r{i}".encode()).hexdigest()
            recs.append({"run_id": rid, "observed_day": "2026-01-01", "model": "claude-sonnet-5", "entrypoint_class": "cli",
                         "effort": "medium", "permission_mode": "default", "task_category": cat,
                         "bom_version": rid, "loaded_set_basis": "harness_log", "run_outcome": "completed",
                         "counts": {"turns": 1, "tool_calls": 0, "tool_failures": 0, "user_denials": 0, "subagent_runs": 0,
                                    "compactions": 0, "unpaired_tool_uses": 0, "repeated_tool_calls": 0},
                         "tokens": {"input": 1, "cache_creation": None, "cache_read": None, "cached_input": None,
                                    "output": 1, "thinking": None, "reasoning": None, "basis": "harness_usage"},
                         "assets": []})
        return {"resource": {"harness": "claude_code"}, "records": recs}

    def test_empty_matched_stratum_pools_visibly(self):
        res = rank(self._env(["mixed", "mixed"]), {}, "deploy and build the thing", "claude_code")
        self.assertEqual("shell_ops", res.task_category)
        self.assertTrue(res.pooled_categories)
        self.assertEqual(2, res.run_count)
        self.assertIn("pools every task category", render(res, scrub=True, public_names=set()))

    def test_populated_matched_stratum_never_pools(self):
        res = rank(self._env(["shell_ops", "mixed"]), {}, "deploy and build the thing", "claude_code")
        self.assertFalse(res.pooled_categories)
        self.assertEqual(1, res.run_count)
        self.assertNotIn("pools every task category", render(res, scrub=True, public_names=set()))
