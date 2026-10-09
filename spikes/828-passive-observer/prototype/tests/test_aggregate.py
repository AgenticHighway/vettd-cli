"""aggregate.py tests (spike #828). Every name, key, count and day below is invented; asset ids are
sha256 of fixture labels and the secrets are built at runtime.

Each test states what it proves and what it cannot prove. None of them can prove that the schema or
the gate list the RIGHT fields; they prove that what aggregate emits is exactly what those two
documents allow, and that the emission is deterministic, sorted, integer-only and pseudonymous.
"""
import copy
import hashlib
import hmac
import json
import os
import random
import re
import sys
import unittest
from typing import Any, Dict, List, Optional, Set, Tuple

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import aggregate  # noqa: E402
from aggregate import Stats, build_envelope, collect_dynamic, run_id_for, to_json_bytes  # noqa: E402
from check_field_gate import check, load_gate  # noqa: E402
from model import (  # noqa: E402
    ASSET_AGENT, ASSET_MCP_SERVER, ASSET_PROMPT, ASSET_RULES_FILE, ASSET_SKILL, BINDING_EXACT, BINDING_MTIME,
    BINDING_NA, KEY_CONTENT, KEY_DESCRIPTOR, KEY_NAME, TIER_INFERRED, AssetKey, AssetObservation, AttributedRun,
    InvocationObs, RunFacts, Segment,
)

PROTO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPIKE_DIR = os.path.dirname(PROTO_DIR)
SCHEMA_PATH = os.path.join(SPIKE_DIR, "telemetry-envelope.schema.json")
GATE_PATH = os.path.join(SPIKE_DIR, "telemetry-field-gate.json")
NULL_UUID = "00000000-0000-4000-8000-000000000000"
TODAY = "2026-03-06"
# Built at runtime so no secret-shaped literal exists in the file.
SECRET_A = ("invented-observer-" + "material-a-" * 2).encode()
SECRET_B = ("invented-observer-" + "material-b-" * 2).encode()


def hex64(label: str) -> str:
    return hashlib.sha256(("fixture:" + label).encode("utf-8")).hexdigest()


def key(asset_type: str, name: str, basis: str = KEY_NAME, binding: str = BINDING_NA) -> AssetKey:
    return AssetKey(asset_id=hex64(name), asset_type=asset_type, key_basis=basis, name=name, binding=binding)


def inv(asset_type: str, name: str, ts: int, latency: Optional[int] = None, failure: Optional[str] = None,
        child_tokens: Optional[int] = None, corroborated: bool = False) -> InvocationObs:
    return InvocationObs(asset_type=asset_type, name=name, ts_ms=ts, latency_ms=latency, failure_class=failure,
                         is_async=latency is None, corroborated=corroborated, child_tokens_total=child_tokens)


def run_facts(session_key: str, day: str, first_ts: int, tokens: Dict[str, Optional[int]], **overrides) -> RunFacts:
    base = dict(session_key=session_key, harness="claude_code", harness_version="1.2.3", entrypoint_class="cli",
                effort="medium", permission_mode="default", model="claude-sonnet-5", observed_day=day,
                first_ts_ms=first_ts, last_ts_ms=first_ts + 60_000, run_outcome="completed", turns=2, tool_calls=6,
                tool_failures=1, user_denials=1, subagent_runs=1, compactions=0, unpaired_tool_uses=0,
                repeated_tool_calls=0, tokens=tokens, tokens_basis="harness_usage",
                tool_class_shares={"edit": 0.5, "read": 0.5, "shell": 0.0, "mcp": 0.0, "other": 0.0},
                forbids={"harness_session_ids": {session_key}, "slugs": {"invented-slug-" + session_key}})
    base.update(overrides)
    return RunFacts(**base)


def segment(index: int, keys: List[AssetKey], start: int, basis: str = "harness_log") -> Segment:
    return Segment(index=index, start_ts_ms=start, end_ts_ms=start + 30_000, loaded_set_basis=basis, asset_keys=keys)


def obs(k: AssetKey, invocations: List[InvocationObs], cost: Optional[Tuple[int, str]] = None,
        corroborations: Optional[int] = None) -> AssetObservation:
    return AssetObservation(key=k, tier=TIER_INFERRED, direct_evidence_available=bool(invocations),
                            invocations=invocations, context_cost_est=cost, harness_corroborations=corroborations)


ALPHA = key(ASSET_SKILL, "alpha-invented-skill", KEY_CONTENT, BINDING_MTIME)
BETA = key(ASSET_MCP_SERVER, "beta-invented-server", KEY_DESCRIPTOR, BINDING_NA)
GAMMA = key(ASSET_AGENT, "gamma-invented-agent")
DELTA = key(ASSET_RULES_FILE, "delta-invented-rules", KEY_CONTENT, BINDING_EXACT)
EPSILON = key(ASSET_PROMPT, "epsilon-invented-prompt")
T0 = 1_772_000_000_000  # an invented harness-clock ms value; it never reaches the wire


def fixture_runs() -> List[AttributedRun]:
    """Two runs: A (day 03-05) has two segments (a removal split the loaded set); B (day 03-04) has one.
    Between them every nullable object is exercised both null and populated."""
    run_a = run_facts("session-invented-a", "2026-03-05", T0 + 86_400_000,
                      {"input": 1000, "cache_creation": 200, "cache_read": 5000, "cached_input": None,
                       "output": 800, "thinking": 100, "reasoning": None})
    seg0 = segment(0, [ALPHA, BETA, GAMMA, DELTA], run_a.first_ts_ms)
    seg1 = segment(1, [ALPHA, BETA], run_a.first_ts_ms + 40_000)
    obs_a0 = [
        obs(ALPHA, [inv(ASSET_SKILL, ALPHA.name, T0 + 1, 200, "tool_error"), inv(ASSET_SKILL, ALPHA.name, T0 + 2, 300),
                    inv(ASSET_SKILL, ALPHA.name, T0 + 3, 400)], cost=(120, "listing_bytes_div4")),
        obs(BETA, [inv(ASSET_MCP_SERVER, BETA.name, T0 + 4, 1000, "timeout"), inv(ASSET_MCP_SERVER, BETA.name, T0 + 5, 1500)],
            cost=(3400, "tool_schema_bytes_div4")),
        obs(GAMMA, [inv(ASSET_AGENT, GAMMA.name, T0 + 6, None, None, 5000, True),
                    inv(ASSET_AGENT, GAMMA.name, T0 + 7, None, "interrupted", 7000, True)], corroborations=2),
        obs(DELTA, [], cost=(800, "file_bytes_div4")),
    ]
    obs_a1 = [obs(ALPHA, [inv(ASSET_SKILL, ALPHA.name, T0 + 8, 250, "user_denied")]), obs(BETA, [])]
    run_b = run_facts("session-invented-b", "2026-03-04", T0,
                      {"input": 300, "cache_creation": None, "cache_read": None, "cached_input": 120,
                       "output": 90, "thinking": None, "reasoning": 40}, harness="codex", model="gpt-5-mini",
                      tool_class_shares={"edit": 0.0, "read": 0.0, "shell": 1.0, "mcp": 0.0, "other": 0.0})
    seg_b = segment(0, [ALPHA, EPSILON], run_b.first_ts_ms, basis="filesystem")
    obs_b = [obs(ALPHA, [inv(ASSET_SKILL, ALPHA.name, T0 + 9, 700, "some-future-class")]), obs(EPSILON, [])]
    names = {k.asset_id: f"{k.asset_type}:{k.name}" for k in (ALPHA, BETA, GAMMA, DELTA, EPSILON)}
    return [
        AttributedRun(run=run_a, segments=[seg0, seg1], observations={0: obs_a0, 1: obs_a1}, name_map=names),
        AttributedRun(run=run_b, segments=[seg_b], observations={0: obs_b}, name_map=names),
    ]


RESOURCE = {"device_id": NULL_UUID, "device_id_source": "placeholder", "harness": "claude_code",
            "harness_version": "1.2.3", "collector": "prototype", "collector_version": "0.1.0"}
COVERAGE = {"sessions_seen": 2, "sessions_emitted": 2, "sessions_skipped_unparseable": 0, "lines_seen": 40,
            "lines_unknown_type": 1, "bytes_read": 8192, "truncated_sessions": 0, "window_days": 30,
            "cursor_state": "fresh", "extra_local_only_key": "must not egress"}


def build(runs=None, secret: bytes = SECRET_A) -> dict:
    return build_envelope(runs if runs is not None else fixture_runs(), RESOURCE, COVERAGE, TODAY, secret, "test_secret")


# -- tiny schema validator (no jsonschema lib) ---------------------------------------------------


def _typename(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, int):
        return "integer"
    if isinstance(value, float):
        return "number"
    return {str: "string", list: "array", dict: "object"}.get(type(value), "unknown")


def validate(instance: Any, schema: dict, root: dict, path: str = "$") -> List[str]:
    """Walk the subset of JSON Schema the envelope schema uses: $ref, type, const, enum, pattern,
    minimum/maximum, required, properties, additionalProperties:false, items, oneOf."""
    if "$ref" in schema:
        ref = schema["$ref"]
        assert ref.startswith("#/$defs/"), ref
        return validate(instance, root["$defs"][ref[len("#/$defs/"):]], root, path)
    errors: List[str] = []
    if "oneOf" in schema:
        passing = [alt for alt in schema["oneOf"] if not validate(instance, alt, root, path)]
        if len(passing) != 1:
            errors.append(f"{path}: oneOf matched {len(passing)} alternatives")
        return errors
    if "type" in schema:
        allowed = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        if _typename(instance) not in allowed:
            return [f"{path}: type {_typename(instance)} not in {allowed}"]
    if "const" in schema and instance != schema["const"]:
        errors.append(f"{path}: const mismatch")
    if "enum" in schema and instance not in schema["enum"]:
        errors.append(f"{path}: not in enum")
    if "pattern" in schema and isinstance(instance, str) and not re.search(schema["pattern"], instance):
        errors.append(f"{path}: pattern mismatch")
    if isinstance(instance, int) and not isinstance(instance, bool):
        if "minimum" in schema and instance < schema["minimum"]:
            errors.append(f"{path}: below minimum")
        if "maximum" in schema and instance > schema["maximum"]:
            errors.append(f"{path}: above maximum")
    if isinstance(instance, dict):
        for req in schema.get("required", []):
            if req not in instance:
                errors.append(f"{path}: missing required {req}")
        props = schema.get("properties", {})
        for k, v in instance.items():
            if k in props:
                errors.extend(validate(v, props[k], root, f"{path}.{k}"))
            elif schema.get("additionalProperties", True) is False:
                errors.append(f"{path}: additional property {k}")
    if isinstance(instance, list) and "items" in schema:
        for i, item in enumerate(instance):
            errors.extend(validate(item, schema["items"], root, f"{path}[{i}]"))
    return errors


def leaf_paths(value: Any, path: str = "", out: Optional[Set[str]] = None) -> Set[str]:
    """Gate path syntax: dot-joined keys, array elements as []. Null is a leaf."""
    out = set() if out is None else out
    if isinstance(value, dict):
        for k, v in value.items():
            leaf_paths(v, f"{path}.{k}" if path else k, out)
    elif isinstance(value, list):
        for item in value:
            leaf_paths(item, path + "[]", out)
    else:
        out.add(path)
    return out


def walk_numbers(value: Any):
    if isinstance(value, dict):
        for v in value.values():
            yield from walk_numbers(v)
    elif isinstance(value, list):
        for v in value:
            yield from walk_numbers(v)
    elif isinstance(value, (int, float)) and not isinstance(value, bool):
        yield value


class StatsTests(unittest.TestCase):
    def test_from_values_known_and_empty(self):
        """Proves: from_values computes n/sum/min/max/sumsq and an empty input is all zeros (the
        schema has no null inside a stats object). Cannot prove: numeric behaviour beyond int."""
        self.assertEqual(Stats.from_values([200, 300, 400]), {"n": 3, "sum": 900, "min": 200, "max": 400, "sumsq": 290000})
        self.assertEqual(Stats.from_values([]), {"n": 0, "sum": 0, "min": 0, "max": 0, "sumsq": 0})

    def test_rejects_non_integers(self):
        """Proves: a float or bool is refused (the envelope is integer-only by design; a float would
        break determinism across encoders). Cannot prove: every call site filters None first."""
        with self.assertRaises(TypeError):
            Stats.from_values([1, 2.5])
        with self.assertRaises(TypeError):
            Stats.from_values([True])
        with self.assertRaises(TypeError):
            Stats.merge(Stats.from_values([1]), {"n": 1, "sum": 1.0, "min": 1, "max": 1, "sumsq": 1})

    def test_merge_is_associative_and_commutative_on_random_partitions(self):
        """Proves: for random integer samples split into random partitions, folding the partition
        stats in any grouping and order equals from_values over the whole sample — which is what
        lets the cloud merge per-run rows without per-call data. Also that an empty partition is the
        identity (min/max of the empty side never leak a zero). Cannot prove: overflow behaviour in
        a fixed-width implementation (#965 is Rust); Python ints are unbounded."""
        rng = random.Random(828)
        for _ in range(50):
            values = [rng.randint(0, 5000) for _ in range(rng.randint(1, 40))]
            cuts = sorted(rng.sample(range(1, len(values)), min(len(values) - 1, rng.randint(0, 4))))
            parts = [values[i:j] for i, j in zip([0] + cuts, cuts + [len(values)])]
            parts.insert(rng.randint(0, len(parts)), [])  # an empty partition somewhere
            stats = [Stats.from_values(p) for p in parts]
            expected = Stats.from_values(values)
            left = stats[0]
            for s in stats[1:]:
                left = Stats.merge(left, s)
            right = stats[-1]
            for s in reversed(stats[:-1]):
                right = Stats.merge(s, right)
            shuffled = list(stats)
            rng.shuffle(shuffled)
            tree = shuffled[0]
            for s in shuffled[1:]:
                tree = Stats.merge(s, tree) if rng.random() < 0.5 else Stats.merge(tree, s)
            self.assertEqual(left, expected)
            self.assertEqual(right, expected)
            self.assertEqual(tree, expected)


class EnvelopeShapeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with open(SCHEMA_PATH, "r", encoding="utf-8") as fh:
            cls.schema = json.load(fh)
        cls.gate = load_gate(GATE_PATH)
        cls.env = build()

    def test_validates_against_envelope_schema(self):
        """Proves: the emitted envelope satisfies telemetry-envelope.schema.json — every required key,
        closed objects, enums, patterns, bounds, nullable objects as null — as checked by the local
        validator above. Cannot prove: full JSON Schema semantics; only the constructs the schema
        uses are implemented, and a construct the validator ignores is not checked."""
        errors = validate(self.env, self.schema, self.schema)
        self.assertEqual(errors, [])
        # The validator itself must be able to fail, or the assertion above is a tautology.
        broken = copy.deepcopy(self.env)
        broken["records"][0]["assets"][0]["signals"]["extra"] = 1
        broken["records"][0]["tokens"]["input"] = -1
        self.assertEqual(len(validate(broken, self.schema, self.schema)), 2)

    def test_every_leaf_path_is_in_the_gate_and_every_gate_path_is_emitted(self):
        """Proves (the important one): the set of leaf paths aggregate writes is a subset of the
        gate's `fields`, and every required gate field is emitted at least once by the fixture — a
        nullable object counts as emitted when it appears as null or when any child leaf appears.
        Cannot prove: a payload from a different fixture covers every path; coverage depends on the
        fixture exercising every nullable both ways, which this one does by construction."""
        emitted = leaf_paths(self.env)
        fields = self.gate["fields"]
        unknown = sorted(p for p in emitted if p not in fields)
        self.assertEqual(unknown, [], "leaf paths not in the gate")
        missing = []
        for path, spec in fields.items():
            if not spec.get("required", True):
                continue
            present = path in emitted or (spec["type"] == "object" and any(p.startswith(path + ".") for p in emitted))
            if not present:
                missing.append(path)
        self.assertEqual(missing, [], "required gate paths never emitted")

    def test_passes_the_real_gate_checker_with_dynamic_forbids(self):
        """Proves: check_field_gate accepts the envelope when handed the names, session keys and
        slugs the runs carried, i.e. none of those strings is inside any string leaf. Cannot prove:
        that a name shorter than the checker's minimum needle length would be caught."""
        dynamic = collect_dynamic(fixture_runs())
        self.assertEqual(check(self.env, self.gate, dynamic), [])

    def test_no_floats_anywhere(self):
        """Proves: every numeric leaf is an int (tool_class_shares, the only float upstream, never
        egresses; task_category is emitted instead). Cannot prove: the same for payloads built from
        runs whose RunFacts already carry floats — those raise in build_envelope instead."""
        self.assertTrue(all(isinstance(n, int) for n in walk_numbers(self.env)))
        self.assertNotIn("tool_class_shares", json.dumps(self.env))

    def test_resource_and_coverage_carry_only_gate_keys(self):
        """Proves: an extra key in the coverage dict the CLI hands over is dropped and run_id_basis
        comes from the parameter, so a local-only bookkeeping key can never egress by accident.
        Cannot prove: the CLI passes correct values for the keys that are kept."""
        self.assertNotIn("extra_local_only_key", self.env["coverage"])
        self.assertEqual(self.env["coverage"]["run_id_basis"], "test_secret")
        self.assertEqual(set(self.env["resource"]), set(aggregate.RESOURCE_KEYS))
        self.assertEqual(self.env["extractor_version"], "proto-0.1.0+taskcat-1")


class RecordContentTests(unittest.TestCase):
    def setUp(self):
        self.env = build()
        self.by_key = {(r["observed_day"], r["bom_version"]): r for r in self.env["records"]}

    def record_for(self, run_key: str, segment_index: int) -> dict:
        rid = run_id_for(SECRET_A, "claude_code" if run_key.endswith("a") else "codex", run_key, segment_index)
        return next(r for r in self.env["records"] if r["run_id"] == rid)

    def test_run_id_is_the_contract_hmac_and_changes_with_the_secret(self):
        """Proves: run_id = HMAC-SHA256(secret, "harness:session_key") exactly (one record per run), and a
        different secret yields different run_ids for the same runs — pseudonymity rests on the
        secret, not on the hash. Cannot prove: the secret file itself never egresses."""
        expected = hmac.new(SECRET_A, b"claude_code:session-invented-a", hashlib.sha256).hexdigest()
        self.assertIn(expected, [r["run_id"] for r in self.env["records"]])
        other = build(secret=SECRET_B)
        self.assertTrue({r["run_id"] for r in other["records"]}.isdisjoint({r["run_id"] for r in self.env["records"]}))

    def test_two_segments_yield_one_record_with_a_change_count(self):
        """Proves: a run split by the settle rule emits ONE record (run-level tokens and counts are
        never duplicated), carries the session-start set as bom_version, counts the change in
        counts.loaded_set_changes, and the bom list still holds every segment's loaded set once.
        Cannot prove: the settle rule split correctly (attribute's job)."""
        recs = [r for r in self.env["records"] if r["run_id"] == run_id_for(SECRET_A, "claude_code", "session-invented-a")]
        self.assertEqual(len(recs), 1)
        rec = recs[0]
        self.assertEqual(rec["counts"]["loaded_set_changes"], 1)
        self.assertEqual(len(self.env["records"]), 2)
        self.assertEqual(len(self.env["bom"]), 3)
        self.assertIn(rec["bom_version"], {b["bom_version"] for b in self.env["bom"]})

    def test_records_assets_and_bom_are_sorted_regardless_of_input_order(self):
        """Proves: records are in (observed_day, run_id) order, assets in asset_id order and bom in
        bom_version order with unique, sorted asset_ids — so file order carries no time and two
        collectors reading in different orders agree. Cannot prove: sorting of ties beyond run_id."""
        reversed_env = build(runs=list(reversed(fixture_runs())))
        self.assertEqual(reversed_env, self.env)
        days_ids = [(r["observed_day"], r["run_id"]) for r in self.env["records"]]
        self.assertEqual(days_ids, sorted(days_ids))
        self.assertEqual(days_ids[0][0], "2026-03-04")
        for rec in self.env["records"]:
            ids = [a["asset_id"] for a in rec["assets"]]
            self.assertEqual(ids, sorted(ids))
        versions = [b["bom_version"] for b in self.env["bom"]]
        self.assertEqual(versions, sorted(versions))
        self.assertEqual(len(set(versions)), len(versions))
        for entry in self.env["bom"]:
            self.assertEqual(entry["asset_ids"], sorted(set(entry["asset_ids"])))
            self.assertEqual(entry["bom_version"], hashlib.sha256(",".join(entry["asset_ids"]).encode()).hexdigest())

    def test_nullable_objects_are_null_when_absent_and_stats_when_present(self):
        """Proves: tokens_attributed is null unless an invocation carried an exact child total, and
        context_cost_est is null unless attribute supplied an estimate; when present both have the
        contract shape. Cannot prove: attribute chooses the right estimate method."""
        rec0 = self.record_for("session-invented-a", 0)
        assets = {a["asset_id"]: a["signals"] for a in rec0["assets"]}
        self.assertIsNone(assets[ALPHA.asset_id]["tokens_attributed"])
        self.assertEqual(assets[GAMMA.asset_id]["tokens_attributed"],
                         {"n": 2, "sum": 12000, "min": 5000, "max": 7000, "sumsq": 74_000_000})
        self.assertEqual(assets[ALPHA.asset_id]["context_cost_est"], {"tokens": 120, "method": "listing_bytes_div4"})
        self.assertIsNone(assets[GAMMA.asset_id]["context_cost_est"])
        rec_b = self.record_for("session-invented-b", 0)
        eps = next(a["signals"] for a in rec_b["assets"] if a["asset_id"] == EPSILON.asset_id)
        self.assertIsNone(eps["tokens_attributed"])
        self.assertIsNone(eps["context_cost_est"])

    def test_failure_classes_latency_and_corroborations(self):
        """Proves: failures are counted per closed class with an unknown class folded into
        `unknown`; latency stats include only paired invocations (async spawns contribute n=0);
        harness_corroborations is attribute's count, or the invocation markers, or null. Cannot
        prove: the source classified the failures correctly."""
        rec0 = self.record_for("session-invented-a", 0)
        sig = {a["asset_id"]: a["signals"] for a in rec0["assets"]}
        self.assertEqual(sig[ALPHA.asset_id]["failures"], {"tool_error": 1, "timeout": 0, "user_denied": 1, "interrupted": 0, "unknown": 0})  # merged across both segments (one record per run)
        self.assertEqual(sig[ALPHA.asset_id]["latency_ms"], {"n": 4, "sum": 1150, "min": 200, "max": 400, "sumsq": 352500})  # both segments merged
        self.assertEqual(sig[GAMMA.asset_id]["latency_ms"]["n"], 0)
        self.assertEqual(sig[GAMMA.asset_id]["failures"]["interrupted"], 1)
        self.assertEqual(sig[GAMMA.asset_id]["harness_corroborations"], 2)
        self.assertIsNone(sig[ALPHA.asset_id]["harness_corroborations"])
        rec_b = self.record_for("session-invented-b", 0)
        alpha_b = next(a["signals"] for a in rec_b["assets"] if a["asset_id"] == ALPHA.asset_id)
        self.assertEqual(alpha_b["failures"]["unknown"], 1)
        self.assertEqual(alpha_b["invocations"]["n"], 1)

    def test_tokens_and_task_category_per_record(self):
        """Proves: token buckets pass through with absent nullable buckets as null and the two
        non-null buckets as ints, and task_category is taskcat's rule over the local shares (edit
        0.5 -> code_edit; shell 1.0 -> shell_ops). Cannot prove: taskcat's boundaries."""
        rec0 = self.record_for("session-invented-a", 0)
        self.assertEqual(rec0["tokens"], {"input": 1000, "cache_creation": 200, "cache_read": 5000, "cached_input": None,
                                          "output": 800, "thinking": 100, "reasoning": None, "basis": "harness_usage"})
        self.assertEqual(rec0["task_category"], "code_edit")
        rec_b = self.record_for("session-invented-b", 0)
        self.assertEqual(rec_b["tokens"]["cached_input"], 120)
        self.assertIsNone(rec_b["tokens"]["cache_creation"])
        self.assertEqual(rec_b["task_category"], "shell_ops")
        self.assertEqual(rec_b["loaded_set_basis"], "filesystem")


class SerializationTests(unittest.TestCase):
    def test_two_builds_give_identical_bytes(self):
        """Proves: same runs + secret + today -> byte-identical payload, including when the runs are
        given in another order. Cannot prove: determinism across Python versions' json module."""
        first = to_json_bytes(build())
        second = to_json_bytes(build(runs=list(reversed(fixture_runs()))))
        self.assertEqual(first, second)
        self.assertEqual(hashlib.sha256(first).hexdigest(), hashlib.sha256(second).hexdigest())

    def test_json_is_canonical(self):
        """Proves: keys sorted, no whitespace outside strings, ASCII only, one trailing newline, and
        the bytes parse back to the same dict. Cannot prove: canonical float formatting (there are
        no floats, by the no-floats test)."""
        raw = to_json_bytes(build())
        self.assertTrue(raw.endswith(b"\n") and not raw.endswith(b"\n\n"))
        raw.decode("ascii")
        text = raw[:-1].decode()
        self.assertEqual(json.loads(text), build())
        self.assertEqual(text, json.dumps(json.loads(text), sort_keys=True, separators=(",", ":")))
        self.assertNotIn(b": ", raw)
        self.assertNotIn(b", ", raw)

    def test_no_name_session_key_or_timestamp_in_the_bytes(self):
        """Proves: the local-only strings the fixture carries (names, session keys, slugs) and the
        harness-clock ms value never appear in the serialized payload. Cannot prove: the same for
        strings the fixture did not include."""
        raw = to_json_bytes(build()).decode()
        for needle in ("alpha-invented", "beta-invented", "session-invented", "invented-slug", str(T0)):
            self.assertNotIn(needle, raw)


class CollectDynamicTests(unittest.TestCase):
    def test_merges_forbids_and_names_without_mutating_inputs(self):
        """Proves: every forbids bucket of every run is merged, loaded_set_names holds each name_map
        value in both display and bare form plus every asset-key and invocation name, and the runs'
        own forbids are untouched. Cannot prove: the sources populated forbids completely."""
        runs = fixture_runs()
        before = copy.deepcopy(runs[0].run.forbids)
        dynamic = collect_dynamic(runs)
        self.assertEqual(dynamic["harness_session_ids"], {"session-invented-a", "session-invented-b"})
        self.assertEqual(dynamic["slugs"], {"invented-slug-session-invented-a", "invented-slug-session-invented-b"})
        names = dynamic["loaded_set_names"]
        for k in (ALPHA, BETA, GAMMA, DELTA, EPSILON):
            self.assertIn(k.name, names)
            self.assertIn(f"{k.asset_type}:{k.name}", names)
        self.assertNotIn("", names)
        self.assertEqual(runs[0].run.forbids, before)
        self.assertIsNot(dynamic["harness_session_ids"], runs[0].run.forbids["harness_session_ids"])

    def test_empty_runs_still_yield_the_names_set(self):
        """Proves: with no runs the checker still receives a (empty) loaded_set_names set, so a
        caller can rely on the key. Cannot prove: anything about non-empty behaviour."""
        self.assertEqual(collect_dynamic([]), {"loaded_set_names": set()})


if __name__ == "__main__":
    unittest.main()
