"""Wire envelope assembly (spike #828, CONTRACTS.md "aggregate.py").

This is the only module that produces egress. It turns AttributedRuns into the envelope that
../telemetry-envelope.schema.json describes — one record per (run, segment) — and nothing else: every
key written here is a gate field, every number is an int, every string is an enum value, a hex64
hash, a UTC day or a version. Names and harness session keys enter only as HMAC preimages.

Determinism (D3): same runs + same secret + same `today` → byte-identical `to_json_bytes` output.
Records are sorted by (observed_day, run_id), assets by asset_id, bom entries by bom_version, and the
JSON is canonical (sorted keys, no whitespace, ASCII), so neither input order nor time survives.

`collect_dynamic` is the local-only counterpart: it gathers every session-derived string the runs
carried (the sources' forbids buckets plus every asset and invocation name) so check_field_gate can
prove none of them is inside any string leaf of the payload.
"""
from __future__ import annotations

import hashlib
import hmac
import json
from typing import Any, Dict, Iterable, List, Optional, Set

import taskcat
from model import RunFacts, AssetObservation, AttributedRun, Segment
from sources.base import FAILURE_CLASSES, FAILURE_UNKNOWN

ENVELOPE_VERSION = "0.1.0"
GATE_VERSION = 1
PROTOTYPE_VERSION = "0.1.0"
# Carries the task-category rule-set version: a re-extraction under a different rule set is a
# different observation (D2), so the rule version is part of the extractor identity.
EXTRACTOR_VERSION = f"proto-{PROTOTYPE_VERSION}+{taskcat.RULES_VERSION}"

RESOURCE_KEYS = ("device_id", "device_id_source", "harness", "harness_version", "collector", "collector_version")
COVERAGE_INT_KEYS = ("sessions_seen", "sessions_emitted", "sessions_skipped_unparseable", "lines_seen",
                     "lines_unknown_type", "bytes_read", "truncated_sessions", "window_days")
COUNT_KEYS = ("turns", "tool_calls", "tool_failures", "user_denials", "subagent_runs", "compactions",
              "unpaired_tool_uses", "repeated_tool_calls")
TOKEN_KEYS = ("input", "cache_creation", "cache_read", "cached_input", "output", "thinking", "reasoning")
NON_NULL_TOKEN_KEYS = ("input", "output")
STATS_KEYS = ("n", "sum", "min", "max", "sumsq")
DYNAMIC_NAMES_SET = "loaded_set_names"


def _int(value: Any, what: str) -> int:
    """The envelope carries integers only: a bool, float or None here is an upstream bug, not a value."""
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"{what}: expected int, got {type(value).__name__}")
    return value


class Stats:
    """Mergeable {n, sum, min, max, sumsq} over integers (#965 rollup rule: never a percentile).

    An empty set is all zeros because the schema has no null inside a stats object; `merge` treats
    an n=0 side as absent so that zero can never become a false minimum.
    """

    @staticmethod
    def from_values(values: Iterable[int]) -> Dict[str, int]:
        vals = [_int(v, "stats value") for v in values]
        if not vals:
            return {"n": 0, "sum": 0, "min": 0, "max": 0, "sumsq": 0}
        return {"n": len(vals), "sum": sum(vals), "min": min(vals), "max": max(vals), "sumsq": sum(v * v for v in vals)}

    @staticmethod
    def merge(a: Dict[str, int], b: Dict[str, int]) -> Dict[str, int]:
        a = {k: _int(a[k], f"stats.{k}") for k in STATS_KEYS}
        b = {k: _int(b[k], f"stats.{k}") for k in STATS_KEYS}
        if a["n"] == 0:
            return b
        if b["n"] == 0:
            return a
        return {"n": a["n"] + b["n"], "sum": a["sum"] + b["sum"], "min": min(a["min"], b["min"]),
                "max": max(a["max"], b["max"]), "sumsq": a["sumsq"] + b["sumsq"]}


def run_id_for(secret: bytes, harness: str, session_key: str, segment_index: Optional[int] = None) -> str:
    """HMAC-SHA256(secret, "harness:session_key"): deterministic locally so re-extraction is
    idempotent, unlinkable remotely because the secret never egresses (D2). One record per run:
    a loaded-set change inside a run is a count on the record, not a second record, so run-level
    tokens and counts are never duplicated. `segment_index` is accepted for compatibility and
    ignored."""
    message = f"{harness}:{session_key}".encode("utf-8")
    return hmac.new(secret, message, hashlib.sha256).hexdigest()


def bom_version_for(asset_ids: Iterable[str]) -> str:
    """sha256 over the sorted, de-duplicated asset ids of a loaded set."""
    return hashlib.sha256(",".join(sorted(set(asset_ids))).encode("utf-8")).hexdigest()


def build_envelope(runs: List[AttributedRun], resource: dict, coverage: dict, today: str, secret: bytes,
                   run_id_basis: str) -> dict:
    """One record per run. Every segment's loaded set goes into bom[]; the record carries the
    session-start set as bom_version and the number of settled changes as a count, and its assets
    are merged across segments so run-level tokens and counts appear exactly once. `resource` and
    `coverage` are copied key by key so a stray extra key in either can never reach the wire; a
    missing key raises (fail loud)."""
    records: List[dict] = []
    bom: Dict[str, List[str]] = {}
    for attributed in runs:
        if not attributed.segments:
            continue
        versions = []
        for segment in attributed.segments:
            asset_ids = sorted({key.asset_id for key in segment.asset_keys})
            version = segment.bom_version or bom_version_for(asset_ids)
            bom.setdefault(version, asset_ids)
            versions.append(version)
        records.append(_record(attributed, versions[0], secret))
    records.sort(key=lambda r: (r["observed_day"], r["run_id"]))
    return {
        "envelope_version": ENVELOPE_VERSION,
        "extractor_version": EXTRACTOR_VERSION,
        "gate_version": GATE_VERSION,
        "emitted_day": today,
        "resource": {k: resource[k] for k in RESOURCE_KEYS},
        "records": records,
        "bom": [{"bom_version": v, "asset_ids": bom[v]} for v in sorted(bom)],
        "coverage": _coverage(coverage, run_id_basis),
    }


def _merged_observations(attributed: AttributedRun) -> List[AssetObservation]:
    """Observations of the same asset across segments merge into one (invocations concatenated,
    direct evidence OR-ed, first context-cost estimate kept, corroborations summed when any)."""
    merged: Dict[str, AssetObservation] = {}
    for segment in attributed.segments:
        for obs in attributed.observations.get(segment.index, []):
            cur = merged.get(obs.key.asset_id)
            if cur is None:
                merged[obs.key.asset_id] = AssetObservation(key=obs.key, tier=obs.tier,
                                                            direct_evidence_available=obs.direct_evidence_available,
                                                            invocations=list(obs.invocations),
                                                            context_cost_est=obs.context_cost_est,
                                                            harness_corroborations=obs.harness_corroborations)
                continue
            cur.invocations.extend(obs.invocations)
            cur.direct_evidence_available = cur.direct_evidence_available or obs.direct_evidence_available
            if cur.context_cost_est is None:
                cur.context_cost_est = obs.context_cost_est
            if obs.harness_corroborations is not None:
                cur.harness_corroborations = (cur.harness_corroborations or 0) + obs.harness_corroborations
            if cur.tier != obs.tier:
                cur.tier = "inferred"  # a disagreement between segments is never promoted
    return sorted(merged.values(), key=lambda o: o.key.asset_id)


def _record(attributed: AttributedRun, bom_version: str, secret: bytes) -> dict:
    run = attributed.run
    observations = _merged_observations(attributed)
    segment = attributed.segments[0]
    return {
        "run_id": run_id_for(secret, run.harness, run.session_key),
        "observed_day": run.observed_day,
        "model": run.model,
        "entrypoint_class": run.entrypoint_class,
        "effort": run.effort,
        "permission_mode": run.permission_mode,
        "task_category": taskcat.categorize(run.tool_class_shares),
        "bom_version": bom_version,
        "loaded_set_basis": segment.loaded_set_basis,
        "run_outcome": run.run_outcome,
        "counts": {**{k: _int(getattr(run, k), f"counts.{k}") for k in COUNT_KEYS},
                   "loaded_set_changes": len(attributed.segments) - 1},
        "tokens": _tokens(run.tokens, run.tokens_basis),
        "tokens_by_model": _tokens_by_model(run),
        "assets": [_asset(obs) for obs in observations],
    }


def _tokens_by_model(run: RunFacts) -> List[dict]:
    """One entry per model id, sorted; when the run recorded totals but no per-model split, the
    totals are attributed to the run's model so the per-model view is never silently empty."""
    by_model = dict(run.tokens_by_model)
    if not by_model and run.tokens_basis != "none":
        by_model = {run.model: run.tokens}
    out = []
    for model in sorted(by_model):
        entry = _tokens(by_model[model], "unused")
        entry.pop("basis", None)
        out.append({"model": taskcat.allowlist_model(model), **entry})
    return out


def _tokens(tokens: Dict[str, Optional[int]], basis: str) -> dict:
    """Nullable buckets stay null when absent (a provider without the bucket is 'absent', not zero);
    the two non-null buckets default to 0 when nothing was counted."""
    out: Dict[str, Any] = {}
    for key in TOKEN_KEYS:
        value = tokens.get(key)
        if value is None:
            out[key] = 0 if key in NON_NULL_TOKEN_KEYS else None
        else:
            out[key] = _int(value, f"tokens.{key}")
    out["basis"] = basis
    return out


def _asset(obs: AssetObservation) -> dict:
    invocations = obs.invocations
    failures = {cls: 0 for cls in FAILURE_CLASSES}
    for inv in invocations:
        if inv.failure_class is not None:
            failures[inv.failure_class if inv.failure_class in failures else FAILURE_UNKNOWN] += 1
    child_totals = [inv.child_tokens_total for inv in invocations if inv.child_tokens_total is not None]
    cost = obs.context_cost_est
    return {
        "asset_id": obs.key.asset_id,
        "asset_type": obs.key.asset_type,
        "key_basis": obs.key.key_basis,
        "tier": obs.tier,
        "binding": obs.key.binding,
        "direct_evidence_available": bool(obs.direct_evidence_available),
        "signals": {
            "invocations": {"n": len(invocations)},
            "failures": failures,
            "harness_corroborations": _corroborations(obs),
            "latency_ms": Stats.from_values(inv.latency_ms for inv in invocations if inv.latency_ms is not None),
            "tokens_attributed": Stats.from_values(child_totals) if child_totals else None,
            "context_cost_est": None if cost is None else {"tokens": _int(cost[0], "context_cost_est.tokens"),
                                                            "method": cost[1]},
        },
    }


def _corroborations(obs: AssetObservation) -> Optional[int]:
    """attribute's explicit count wins; otherwise the invocations' own markers are counted, and the
    result is null (not 0) when no marker was seen, because the harness may simply have none."""
    if obs.harness_corroborations is not None:
        return _int(obs.harness_corroborations, "harness_corroborations")
    marked = sum(1 for inv in obs.invocations if inv.corroborated)
    return marked or None


def _coverage(coverage: dict, run_id_basis: str) -> dict:
    out: Dict[str, Any] = {k: _int(coverage[k], f"coverage.{k}") for k in COVERAGE_INT_KEYS}
    out["cursor_state"] = coverage["cursor_state"]
    out["run_id_basis"] = run_id_basis
    return out


def to_json_bytes(envelope: dict) -> bytes:
    """Canonical encoding: sorted keys, no whitespace, ASCII only, one trailing newline. `allow_nan`
    is off so a float could never be smuggled in as NaN/Infinity either."""
    text = json.dumps(envelope, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False)
    return (text + "\n").encode("ascii")


def _bare_name(display: str) -> str:
    """name_map values are "<asset_type>:<name>"; the bare name is the stronger forbid needle."""
    _, sep, rest = display.partition(":")
    return rest if sep else display


def collect_dynamic(runs: List[AttributedRun]) -> Dict[str, Set[str]]:
    """Merged local-only forbid sets for the gate checker. Every source bucket is carried over as-is
    and `loaded_set_names` receives every asset and invocation name the runs mention, both as the
    name_map display form and as the bare name. Inputs are never mutated."""
    out: Dict[str, Set[str]] = {}
    names: Set[str] = set()
    for attributed in runs:
        for bucket, values in attributed.run.forbids.items():
            out.setdefault(bucket, set()).update(str(v) for v in values if v)
        for display in attributed.name_map.values():
            names.add(display)
            names.add(_bare_name(display))
        for segment in attributed.segments:
            names.update(key.name for key in segment.asset_keys)
        for observations in attributed.observations.values():
            for obs in observations:
                names.add(obs.key.name)
                names.update(inv.name for inv in obs.invocations)
    names.discard("")
    out.setdefault(DYNAMIC_NAMES_SET, set()).update(names)
    return out
