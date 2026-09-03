#!/usr/bin/env python3
"""Passive-observer prototype entry point (spike #828).

read local session state → extract signals → attribute to assets → aggregate per run → gate-check the
written payload → print a ranked, confidence-tagged asset list for a stated task.

This never posts anywhere. It writes one payload file and a sibling `<out>.dynamic.json` holding the
local-only forbid sets the gate checker consumed (names, ids); that sibling is a local artifact and is
not part of the payload.
"""
from __future__ import annotations

import argparse
import hmac
import re
import datetime as _dt
import hashlib
import json
import os
import sys
import time
from typing import Dict, List, Optional

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from sources.base import SessionFacts, SessionRef  # noqa: E402
from sources.claude_code import ClaudeCodeSource  # noqa: E402
from sources.codex import CodexSource  # noqa: E402
import extract as _extract  # noqa: E402
import attribute as _attribute  # noqa: E402
import aggregate as _aggregate  # noqa: E402
import rank as _rank  # noqa: E402
import check_field_gate as _gate  # noqa: E402
from cursor_store import CursorStore  # noqa: E402
from model import AttributedRun  # noqa: E402

NULL_UUID = "00000000-0000-4000-8000-000000000000"
_SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+$")  # plain semver only; build metadata can carry a hostname


def semver_or_unknown(value: str) -> str:
    """Harness versions must be semver or 'unknown': a build string can carry a hostname or commit."""
    value = (value or "").strip()
    return value if _SEMVER_RE.match(value) else "unknown"
COLLECTOR_VERSION = "0.1.0"
ROOT_DIR = os.path.dirname(HERE)
# The gate/schema live at the repo root and prices under crates/vettd-cli/resources/
# since the #965 port promoted them out of the spike directory.
REPO_ROOT = os.path.normpath(os.path.join(ROOT_DIR, "..", ".."))


def _configure_stdio() -> None:
    """Keep Unicode help and ranking output usable under the legacy Windows code page."""
    if os.name != "nt":
        return
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            reconfigure(encoding="utf-8", errors="replace")


def _default_root(harness: str) -> str:
    home = os.path.expanduser("~")
    return os.path.join(home, ".claude" if harness == "claude_code" else ".codex")


def _load_secret(path: str) -> bytes:
    with open(path, "rb") as fh:
        secret = fh.read()
    if len(secret) < 16:
        raise SystemExit("secret file must hold at least 16 bytes")
    return secret


def _group(refs: List[SessionRef]):
    mains = [r for r in refs if r.kind == "main"]
    children: Dict[str, List[SessionRef]] = {}
    for r in refs:
        if r.kind == "child" and r.parent_key:
            children.setdefault(r.parent_key, []).append(r)
    return mains, children


def run_pipeline(args) -> int:
    secret = _load_secret(args.secret_file)
    now_ms = args.now_ms if args.now_ms is not None else int(time.time() * 1000)
    today = args.today or _dt.datetime.fromtimestamp(now_ms / 1000, _dt.timezone.utc).strftime("%Y-%m-%d")
    root = args.root or _default_root(args.harness)
    source = ClaudeCodeSource(root) if args.harness == "claude_code" else CodexSource(root)
    store = CursorStore(args.cursor_store) if args.cursor_store else None

    refs = source.discover(root, args.window_days, now_ms)
    mains, children = _group(refs)
    coverage = {
        "sessions_seen": len(mains),
        "sessions_emitted": 0,
        "sessions_skipped_unparseable": 0,
        "lines_seen": 0,
        "lines_unknown_type": 0,
        "bytes_read": 0,
        "truncated_sessions": 0,
        "window_days": args.window_days,
        "cursor_state": "resumed" if (store and store.entries()) else "fresh",
        "run_id_basis": "test_secret",
    }
    fs_index = _attribute.FsIndex(
        claude_home=root if args.harness == "claude_code" else None,
        codex_home=root if args.harness == "codex" else None,
    )
    attributed: List[AttributedRun] = []
    pending_cursors: Dict[str, object] = {}
    harness_version = "unknown"
    for ref in sorted(mains, key=lambda r: r.path):
        child_refs = sorted(children.get(ref.session_key, []), key=lambda r: r.path)
        group_refs = [ref, *child_refs]

        # A record is the cumulative state of one harness run and run_id is its idempotency key.
        # Cursors are therefore change detectors, not the starting point for a partial replacement
        # record. If every file in the group has a cursor, probe from those offsets and emit only
        # when at least one complete line is new. A changed group is then rebuilt from byte zero.
        if store and all(store.get(group_ref.path) is not None for group_ref in group_refs):
            group_changed = False
            probe_cursors = {}
            probe_lines_seen = 0
            probe_lines_unknown = 0
            probe_bytes_read = 0
            for group_ref in group_refs:
                try:
                    delta, new_cursor = source.read(group_ref, store.get(group_ref.path))
                    probe_cursors[group_ref.path] = new_cursor
                    group_changed = group_changed or delta.lines_seen > 0
                    probe_lines_seen += delta.lines_seen
                    probe_lines_unknown += delta.lines_unknown_type
                    probe_bytes_read += delta.bytes_read
                except Exception:
                    # Retry from byte zero below. Only a failed full read counts as unparseable.
                    group_changed = True
            if not group_changed:
                pending_cursors.update(probe_cursors)
                continue
            coverage["lines_seen"] += probe_lines_seen
            coverage["lines_unknown_type"] += probe_lines_unknown
            coverage["bytes_read"] += probe_bytes_read

        group_cursors = {}
        group_failed = False
        try:
            facts, new_cursor = source.read(ref, None)
            if store:
                group_cursors[ref.path] = new_cursor
            for child_ref in child_refs:
                try:
                    child_facts, child_cursor = source.read(child_ref, None)
                    facts.children.append(child_facts)
                    if store:
                        group_cursors[child_ref.path] = child_cursor
                except Exception:  # fail-open: preserve the prior complete record and retry later
                    coverage["sessions_skipped_unparseable"] += 1
                    group_failed = True
        except Exception as exc:  # fail-open: count and move on
            coverage["sessions_skipped_unparseable"] += 1
            if args.verbose:
                print(f"skip unparseable session: {type(exc).__name__}", file=sys.stderr)
            continue
        if group_failed:
            continue
        pending_cursors.update(group_cursors)
        if facts.lines_seen + sum(c.lines_seen for c in facts.children) == 0:
            continue
        coverage["lines_seen"] += facts.lines_seen + sum(c.lines_seen for c in facts.children)
        coverage["lines_unknown_type"] += facts.lines_unknown_type + sum(c.lines_unknown_type for c in facts.children)
        coverage["bytes_read"] += facts.bytes_read + sum(c.bytes_read for c in facts.children)
        if facts.truncated:
            coverage["truncated_sessions"] += 1
        if facts.harness_version and facts.harness_version != "unknown":
            harness_version = facts.harness_version
        run = _extract.extract(facts, now_ms)
        attributed.append(_attribute.attribute(run, fs_index, secret))
        coverage["sessions_emitted"] += 1
    resource = {
        "device_id": NULL_UUID,
        "device_id_source": "placeholder",
        "harness": args.harness,
        "harness_version": semver_or_unknown(harness_version),
        "collector": "prototype",
        "collector_version": COLLECTOR_VERSION,
    }
    envelope = _aggregate.build_envelope(attributed, resource, coverage, today, secret, "test_secret")
    dynamic = _aggregate.collect_dynamic(attributed)
    name_map: Dict[str, str] = {}
    for ar in attributed:
        name_map.update(ar.name_map)
    dynamic.setdefault("current_username", set()).add(os.environ.get("USER") or os.environ.get("USERNAME") or "")
    try:
        import socket
        dynamic.setdefault("hostname", set()).add(socket.gethostname())
    except Exception:
        pass
    dynamic.setdefault("home_dir", set()).add(os.path.expanduser("~"))

    gate = _gate.load_gate(args.gate)
    violations = _gate.check(envelope, gate, dynamic)
    if violations:
        print("REFUSING TO WRITE: payload fails the telemetry field gate:", file=sys.stderr)
        for v in violations:
            print("  " + v, file=sys.stderr)
        return 2
    payload_bytes = _aggregate.to_json_bytes(envelope)
    with open(args.out, "wb") as fh:
        fh.write(payload_bytes)
    with open(args.out + ".dynamic.json", "w") as fh:
        json.dump({k: sorted(v) for k, v in dynamic.items()}, fh, indent=1, sort_keys=True)
    if store:  # cursors advance only once the payload is on disk; a refused payload is re-read next time
        for path, cur in pending_cursors.items():
            store.set(path, cur)
        store.save()
    print(f"wrote {args.out} ({len(payload_bytes)} bytes, sha256 {hashlib.sha256(payload_bytes).hexdigest()[:16]}...)")
    print(f"gate: OK ({len(gate['fields'])} allowed leaf paths, 0 violations)")

    public = set()
    if args.public_names:
        public = {l.strip() for l in open(args.public_names) if l.strip() and not l.startswith("#")}
    result = _rank.rank(envelope, name_map, args.task, args.harness, args.model)
    print(_rank.render(result, scrub=args.scrub, public_names=public, prices_path=args.prices))

    if args.synthetic_demo:
        syn_runs = synthetic_demo_runs(secret, args.harness)
        syn_env = _aggregate.build_envelope(syn_runs, resource, dict(coverage, sessions_seen=len(syn_runs), sessions_emitted=len(syn_runs), sessions_skipped_unparseable=0, lines_seen=0, lines_unknown_type=0, bytes_read=0, truncated_sessions=0), today, secret, "test_secret")
        syn_dyn = _aggregate.collect_dynamic(syn_runs)
        syn_viol = _gate.check(syn_env, gate, syn_dyn)
        if syn_viol:
            print("synthetic demo payload fails the gate; not written", file=sys.stderr)
            return 2
        with open(args.out + ".synthetic.json", "wb") as fh:
            fh.write(_aggregate.to_json_bytes(syn_env))
        syn_names: Dict[str, str] = {}
        for ar in syn_runs:
            syn_names.update(ar.name_map)
        print("\n" + "=" * 78)
        print("SYNTHETIC DEMO — invented counts, not observations; shown only so the populated")
        print("ranking layout can be seen. Written to " + args.out + ".synthetic.json")
        print("=" * 78)
        print(_rank.render(_rank.rank(syn_env, syn_names, args.task, args.harness, None), scrub=False, public_names=set(), prices_path=args.prices))
    return 0


def synthetic_demo_runs(secret: bytes, harness: str) -> List[AttributedRun]:
    """Invented runs so the populated ranking layout can be shown once. Every name starts with
    SYNTHETIC and every count is made up; the payload is written to a separate file and labelled."""
    from model import (ASSET_MCP_SERVER, ASSET_RULES_FILE, ASSET_SKILL, BINDING_EXACT, BINDING_NA, KEY_CONTENT,
                       KEY_NAME, TIER_INFERRED, AssetKey, AssetObservation, InvocationObs, RunFacts, Segment)

    def key(asset_type: str, name: str) -> AssetKey:
        digest = hmac.new(secret, f"{asset_type}:{name}".encode(), "sha256").hexdigest()
        basis = KEY_CONTENT if asset_type in (ASSET_SKILL, ASSET_RULES_FILE) else KEY_NAME
        return AssetKey(asset_id=digest, asset_type=asset_type, key_basis=basis, name=name,
                        binding=BINDING_EXACT if basis == KEY_CONTENT else BINDING_NA)

    # (name, type, invocations per run, failures per run pattern, latency ms base)
    spec = [
        ("SYNTHETIC-server-alpha", ASSET_MCP_SERVER, 2, (0, 0, 0, 1, 0), 900),
        ("SYNTHETIC-server-beta", ASSET_MCP_SERVER, 2, (1, 0, 1, 0, 1), 1400),
        ("SYNTHETIC-skill-gamma", ASSET_SKILL, 1, (0, 0, 0, 0, 0), 300),
        ("SYNTHETIC-server-delta", ASSET_MCP_SERVER, 1, (0, 1, 0, 0, 0), 2200),
        ("SYNTHETIC-rules-epsilon", ASSET_RULES_FILE, 0, (), 0),
    ]
    keys = {name: key(t, name) for name, t, _, _, _ in spec}
    runs: List[AttributedRun] = []
    n_runs = 60
    for i in range(n_runs):
        day = _dt.date(2026, 8, 1) + _dt.timedelta(days=i % 28)
        first = int(_dt.datetime(day.year, day.month, day.day, 9, 0, tzinfo=_dt.timezone.utc).timestamp() * 1000)
        run = RunFacts(session_key=f"synthetic-{i:03d}", harness=harness, harness_version="0.0.0",
                       entrypoint_class="cli", effort="medium", permission_mode="default", model="claude-sonnet-5",
                       observed_day=day.isoformat(), first_ts_ms=first, last_ts_ms=first + 600000, run_outcome="completed",
                       turns=3 + i % 4, tool_calls=12, tokens={"input": 1000 + 37 * i, "cache_creation": 200, "cache_read": 5000,
                       "cached_input": None, "output": 800 + 11 * i, "thinking": 100, "reasoning": None}, tokens_basis="harness_usage",
                       tool_class_shares=({"shell": 0.6, "read": 0.2, "edit": 0.1, "mcp": 0.1, "other": 0.0} if i % 3
                                          else {"shell": 0.3, "read": 0.3, "edit": 0.2, "mcp": 0.2, "other": 0.0}))
        obs = []
        for name, t, per_run, pattern, base in spec:
            k = keys[name]
            invs = []
            if per_run and i < (25 if name.endswith("delta") else n_runs):
                for j in range(per_run):
                    fail = pattern[(i + j) % len(pattern)] if pattern else 0
                    invs.append(InvocationObs(asset_type=t, name=name, ts_ms=first + 1000 * j,
                                              latency_ms=base + 50 * ((i + j) % 7),
                                              failure_class="tool_error" if fail else None))
            if name.endswith("gamma") and i >= 8:
                invs = []  # gamma is invoked in only 8 runs: insufficient evidence by design
            obs.append(AssetObservation(key=k, tier=TIER_INFERRED, direct_evidence_available=bool(invs), invocations=invs,
                                        context_cost_est=(1200, "file_bytes_div4") if t != ASSET_MCP_SERVER else (3400, "tool_schema_bytes_div4"),
                                        harness_corroborations=None))
        seg = Segment(index=0, start_ts_ms=first, end_ts_ms=first + 600000, loaded_set_basis="harness_log",
                      asset_keys=list(keys.values()))
        seg.bom_version = _attribute.bom_version(k.asset_id for k in keys.values())
        runs.append(AttributedRun(run=run, segments=[seg], observations={0: obs},
                                  name_map={k.asset_id: f"{k.asset_type}:{k.name}" for k in keys.values()}))
    return runs


def main(argv: Optional[List[str]] = None) -> int:
    _configure_stdio()
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--harness", choices=["claude_code", "codex"], required=True)
    p.add_argument("--root", help="harness home dir (default ~/.claude or ~/.codex)")
    p.add_argument("--task", required=True, help="the stated task the ranking is for (free text, local only)")
    p.add_argument("--secret-file", required=True, help="device-local secret used for run_id and name_hash pseudonyms")
    p.add_argument("--out", required=True, help="payload path (never posted)")
    p.add_argument("--today", help="UTC day for emitted_day (default: today)")
    p.add_argument("--now-ms", type=int, default=None, help="collector 'now' in ms (tests)")
    p.add_argument("--window-days", type=int, default=30)
    p.add_argument("--model", default=None, help="model stratum to display (default: all)")
    p.add_argument("--scrub", action="store_true", help="replace asset names with hashes in the printed ranking")
    p.add_argument("--public-names", help="file of display names allowed through --scrub (public assets)")
    p.add_argument("--cursor-store", help="path of the local cursor store (resumable reads)")
    p.add_argument("--synthetic-demo", action="store_true", help="also print a labelled synthetic ranking")
    p.add_argument("--gate", default=os.path.join(REPO_ROOT, "telemetry-field-gate.json"))
    p.add_argument("--prices", default=os.path.join(REPO_ROOT, "crates", "vettd-cli", "resources", "observe-prices.json"))
    p.add_argument("--verbose", action="store_true")
    args = p.parse_args(argv)
    return run_pipeline(args)


if __name__ == "__main__":
    sys.exit(main())
