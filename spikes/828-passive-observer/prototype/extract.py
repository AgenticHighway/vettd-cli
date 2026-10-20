"""SessionFacts (with its child tree) -> RunFacts (spike #828, CONTRACTS.md "extract.py").

Harness-neutral derivation of per-run facts. Everything here is arithmetic over the typed facts a
source produced; no session content exists at this layer to leak.

Scope of "the run" (the contract leaves this implicit, so it is stated here):
  - Counts, token totals, invocations, in-band assets and forbids merge over the WHOLE tree
    (main transcript plus every linked child transcript). Usages are deduplicated by provider
    message id across the tree so a response logged in two places counts once.
  - `run_outcome`, `turns`, `loaded_events` and `truncated` describe the MAIN transcript only: a
    child's "user" lines are the parent's task prompt, not a person's turns, and the loaded set the
    segments are cut from is the parent's.
  - A parent `Agent` call is linked to a direct child by `child_key` (toolUseResult.agentId) or by
    the child's `child_meta["toolUseId"]` (D4). Its InvocationObs carries the child's exact token
    total and the child's outcome; a linked child's own tokens are ALSO in the run totals.
  - "Total tokens" of a child = input + output + cache_creation + cache_read. `cached_input`,
    `thinking` and `reasoning` are subsets of input/output for the providers that report them and
    are excluded so nothing is counted twice.
"""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
from typing import Dict, Iterator, List, Optional

import taskcat
from model import ASSET_AGENT, ASSET_MCP_SERVER, ASSET_SKILL, InvocationObs, RunFacts
from sources.base import (
    FAILURE_INTERRUPTED,
    FAILURE_UNKNOWN,
    FAILURE_USER_DENIED,
    RATE_BEARING_FAILURES,
    SessionFacts,
    ToolCall,
    Usage,
)

OUTCOME_TRUNCATED = "truncated"
OUTCOME_COMPACTED = "compacted"
OUTCOME_INTERRUPTED = "interrupted"
OUTCOME_COMPLETED = "completed"
OUTCOME_UNKNOWN = "unknown"

TOOL_CLASSES = ("edit", "read", "shell", "mcp", "other")
EDIT_TOOLS = frozenset({"Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"})
READ_TOOLS = frozenset({"Read", "Glob", "Grep", "LS", "WebFetch", "WebSearch"})
SHELL_TOOLS = frozenset({"Bash", "shell", "exec"})
REPEAT_THRESHOLD = 3

PERMISSION_MODES = {
    "acceptEdits": "accept_edits",
    "bypassPermissions": "bypass",
    "dontAsk": "dont_ask",
    "plan": "plan",
    "default": "default",
    "auto": "auto",
}
EFFORTS = frozenset({"minimal", "low", "medium", "high", "xhigh"})

# (envelope key, Usage attribute); the first two are never null on the wire.
TOKEN_BUCKETS = (
    ("input", "input_tokens"),
    ("output", "output_tokens"),
    ("cache_creation", "cache_creation"),
    ("cache_read", "cache_read"),
    ("cached_input", "cached_input"),
    ("thinking", "thinking"),
    ("reasoning", "reasoning"),
)
NON_NULL_BUCKETS = ("input", "output")
TOTAL_BUCKETS = ("input", "output", "cache_creation", "cache_read")


def extract(facts: SessionFacts, now_ms: int) -> RunFacts:
    """Derive RunFacts for the run rooted at `facts`. `now_ms` is only the fallback for a session
    that carries no harness timestamp at all."""
    tree = list(walk(facts))
    calls = [c for f in tree for c in f.tool_calls]
    usages = dedupe_usages(tree)
    first, last = _span(tree, now_ms)
    return RunFacts(
        session_key=facts.ref.session_key,
        harness=facts.ref.harness,
        harness_version=facts.harness_version,
        entrypoint_class=entrypoint_class(facts.entrypoint),
        effort=effort_class(facts.effort),
        permission_mode=permission_mode(facts.permission_mode),
        model=taskcat.allowlist_model(dominant_model(tree)),
        observed_day=utc_day(first),
        first_ts_ms=first,
        last_ts_ms=last,
        run_outcome=run_outcome(facts),
        turns=facts.user_turns,
        tool_calls=len(calls),
        tool_failures=sum(1 for c in calls if c.failure_class in RATE_BEARING_FAILURES),
        user_denials=sum(1 for c in calls if c.failure_class == FAILURE_USER_DENIED),
        subagent_runs=len(tree) - 1,
        compactions=sum(f.compactions for f in tree),
        unpaired_tool_uses=sum(1 for c in calls if not c.paired),
        repeated_tool_calls=repeated_tool_calls(calls),
        tokens=sum_tokens(usages),
        tokens_by_model=sum_tokens_by_model(usages),
        mcp_corroborations=merge_mcp_corroborations(tree),
        tokens_basis="harness_usage" if usages else "none",
        tool_class_shares=tool_class_shares(calls),
        invocations=invocations(facts),
        loaded_events=list(facts.loaded_events),
        in_band_assets=[a for f in tree for a in f.in_band_assets],
        lines_seen=sum(f.lines_seen for f in tree),
        lines_unknown_type=sum(f.lines_unknown_type for f in tree),
        bytes_read=sum(f.bytes_read for f in tree),
        parse_errors=sum(f.parse_errors for f in tree),
        truncated=facts.truncated,
        forbids=merge_forbids(tree),
    )


# -- tree helpers --------------------------------------------------------------------------------


def walk(facts: SessionFacts) -> Iterator[SessionFacts]:
    """Depth-first, parent before children, in the order the source linked them."""
    yield facts
    for child in facts.children:
        yield from walk(child)


def dedupe_usages(tree: List[SessionFacts]) -> Dict[str, Usage]:
    """One Usage per provider message id across the tree. A streamed response is written as several
    lines whose usage grows as output streams, so the entry with the largest output count wins;
    on a tie the first occurrence (parent first) is kept."""
    seen: Dict[str, Usage] = {}
    for f in tree:
        for mid, usage in f.usages.items():
            current = seen.get(mid)
            if current is None or usage.output_tokens > current.output_tokens:
                seen[mid] = usage
    return seen


def merge_mcp_corroborations(tree: List[SessionFacts]) -> Dict[str, int]:
    out: Dict[str, int] = {}
    for f in tree:
        for server, n in f.mcp_attribution_counts.items():
            out[server] = out.get(server, 0) + n
    return out


def merge_forbids(tree: List[SessionFacts]) -> Dict[str, set]:
    out: Dict[str, set] = {}
    for f in tree:
        for bucket, values in f.forbids.items():
            out.setdefault(bucket, set()).update(values)
    return out


def _span(tree: List[SessionFacts], now_ms: int) -> "tuple[int, int]":
    firsts = [f.first_ts_ms for f in tree if f.first_ts_ms is not None]
    lasts = [f.last_ts_ms for f in tree if f.last_ts_ms is not None]
    first = min(firsts) if firsts else now_ms
    last = max(lasts) if lasts else first
    return first, max(first, last)


def dominant_model(tree: List[SessionFacts]) -> str:
    """Most frequent model by response count in the MAIN transcript (tree[0]); the whole tree is
    used only when the main transcript carried no model. Sub-agents may run on a different model,
    which is why the envelope also carries tokens_by_model. Ties break on the smaller name so the
    result is deterministic. "unknown" when no response carried a model."""
    counts: Counter = Counter()
    if tree and tree[0].models:
        counts.update(tree[0].models)
    else:
        for f in tree:
            counts.update(f.models)
    if not counts:
        return "unknown"
    return sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[0][0]


# -- tokens --------------------------------------------------------------------------------------


def sum_tokens(usages: Dict[str, Usage]) -> Dict[str, Optional[int]]:
    """Envelope-shaped token totals. Nullable buckets stay None unless at least one usage reported
    a value for them, so a provider without that bucket is "absent", not zero."""
    out: Dict[str, Optional[int]] = {key: (0 if key in NON_NULL_BUCKETS else None) for key, _ in TOKEN_BUCKETS}
    for usage in usages.values():
        for key, attr in TOKEN_BUCKETS:
            value = getattr(usage, attr)
            if value is None:
                continue
            out[key] = (out[key] or 0) + value
    return out


def sum_tokens_by_model(usages: Dict[str, Usage]) -> Dict[str, Dict[str, Optional[int]]]:
    """Envelope-shaped token totals per allowlisted model id (sub-agents may run on another model)."""
    by_model: Dict[str, Dict[str, Usage]] = {}
    for mid, usage in usages.items():
        by_model.setdefault(taskcat.allowlist_model(usage.model), {})[mid] = usage
    return {model: sum_tokens(group) for model, group in sorted(by_model.items())}


def total_tokens(tokens: Dict[str, Optional[int]]) -> int:
    """input + output + cache_creation + cache_read (see module docstring for the exclusions)."""
    return sum(tokens.get(key) or 0 for key in TOTAL_BUCKETS)


def child_tokens_total(child: SessionFacts) -> Optional[int]:
    """Exact total of a child run from its own transcript tree; None when the child carries no
    usage record at all (no evidence is not zero)."""
    usages = dedupe_usages(list(walk(child)))
    if not usages:
        return None
    return total_tokens(sum_tokens(usages))


# -- tool calls ----------------------------------------------------------------------------------


def repeated_tool_calls(calls: List[ToolCall]) -> int:
    """Number of calls belonging to a (name, input_fingerprint) group of size >= REPEAT_THRESHOLD."""
    groups = Counter((c.name, c.input_fingerprint) for c in calls)
    return sum(n for n in groups.values() if n >= REPEAT_THRESHOLD)


def tool_class(name: str, server: Optional[str]) -> str:
    """Published tool-mix classification (D2). MCP is decided first: a Codex MCP tool is named
    `<server>__<tool>` without the `mcp__` prefix but has `server` set."""
    if server or name.startswith("mcp__"):
        return "mcp"
    if name in EDIT_TOOLS:
        return "edit"
    if name in READ_TOOLS:
        return "read"
    if name in SHELL_TOOLS:
        return "shell"
    return "other"


def tool_class_shares(calls: List[ToolCall]) -> Dict[str, float]:
    """count/total per class; every class present (0.0 when absent) so the shape is stable.
    All zeros when there are no calls, which taskcat maps to `unspecified`."""
    counts = Counter(tool_class(c.name, c.server) for c in calls)
    total = len(calls)
    return {cls: (counts[cls] / total if total else 0.0) for cls in TOOL_CLASSES}


def _latency(call: ToolCall) -> Optional[int]:
    return None if (call.is_async or not call.paired) else call.latency_ms


# -- run shape -----------------------------------------------------------------------------------


def entrypoint_class(raw: Optional[str]) -> str:
    s = (raw or "").lower()
    if "remote" in s:
        return "remote"
    if "vscode" in s or "jetbrains" in s or "ide" in s:
        return "ide"
    if "sdk" in s:
        return "sdk"
    if "cli" in s:
        return "cli"
    return "unknown"


PERMISSION_ENUM = ("default", "plan", "accept_edits", "bypass", "auto", "dont_ask", "unknown")


def permission_mode(raw: Optional[str]) -> str:
    """Claude Code raw values are mapped; a value already in the closed enum (a source that
    pre-maps, like the Codex approval policy) passes through unchanged."""
    if raw in PERMISSION_ENUM:
        return raw
    return PERMISSION_MODES.get(raw or "", "unknown")


def effort_class(raw: Optional[str]) -> str:
    """Closed enum from the gate; anything else (including harness values the gate does not list)
    is "unknown". The contract is silent on effort; this keeps the payload gate-clean."""
    return raw if raw in EFFORTS else "unknown"


def utc_day(ts_ms: int) -> str:
    return datetime.fromtimestamp(ts_ms // 1000, tz=timezone.utc).strftime("%Y-%m-%d")


def run_outcome(facts: SessionFacts) -> str:
    """Decision table, first match wins: truncated > compacted > interrupted > completed > unknown.
    Only the transcript `facts` itself is consulted (children have their own outcome)."""
    if facts.truncated:
        return OUTCOME_TRUNCATED
    if facts.compactions > 0 and facts.last_stop_reason != "end_turn":
        return OUTCOME_COMPACTED
    if _interrupted_at_end(facts.tool_calls):
        return OUTCOME_INTERRUPTED
    if facts.last_stop_reason == "end_turn":
        return OUTCOME_COMPLETED
    return OUTCOME_UNKNOWN


def _interrupted_at_end(calls: List[ToolCall]) -> bool:
    """Any call without a result, or the last call in transcript order marked interrupted."""
    if any(not c.paired for c in calls):
        return True
    return bool(calls) and calls[-1].interrupted


# -- invocations ---------------------------------------------------------------------------------


def invocations(facts: SessionFacts) -> List[InvocationObs]:
    """Explicit asset invocations over the tree: parent's calls first, then each child's."""
    out: List[InvocationObs] = []
    for call in facts.tool_calls:
        if call.skill:
            out.append(InvocationObs(asset_type=ASSET_SKILL, name=call.skill, ts_ms=call.ts_ms,
                                     latency_ms=_latency(call), failure_class=call.failure_class,
                                     is_async=call.is_async))
        elif call.server:
            out.append(InvocationObs(asset_type=ASSET_MCP_SERVER, name=call.server, ts_ms=call.ts_ms,
                                     latency_ms=_latency(call), failure_class=call.failure_class,
                                     is_async=call.is_async))
        elif call.agent_type:
            out.append(_agent_invocation(call, _linked_child(facts, call)))
    for child in facts.children:
        out.extend(invocations(child))
    return out


def _linked_child(facts: SessionFacts, call: ToolCall) -> Optional[SessionFacts]:
    for child in facts.children:
        if call.child_key and child.ref.session_key == call.child_key:
            return child
        if child.ref.child_meta.get("toolUseId") == call.tool_use_id:
            return child
    return None


def _agent_invocation(call: ToolCall, child: Optional[SessionFacts]) -> InvocationObs:
    """The parent's result is only a spawn ack (D4): outcome and tokens come from the child when
    one is linked. The parent's own failure class (a denied or failed spawn) still takes precedence
    because then there is no child run to speak of."""
    failure = call.failure_class
    corroborated = False
    tokens: Optional[int] = None
    if child is not None:
        failure = failure or _child_failure_class(child)
        corroborated = str(child.ref.child_meta.get("corroborated", "")).lower() == "true"
        tokens = child_tokens_total(child)
    return InvocationObs(asset_type=ASSET_AGENT, name=call.agent_type or "", ts_ms=call.ts_ms,
                         latency_ms=_latency(call), failure_class=failure, is_async=call.is_async,
                         corroborated=corroborated, child_tokens_total=tokens)


def _child_failure_class(child: SessionFacts) -> Optional[str]:
    """Child outcome -> failure class. Never a rate-bearing class: a child's own tool errors are
    counted on the tools it called, not on the agent as a whole."""
    outcome = run_outcome(child)
    if outcome == OUTCOME_INTERRUPTED:
        return FAILURE_INTERRUPTED
    if outcome in (OUTCOME_TRUNCATED, OUTCOME_UNKNOWN):
        return FAILURE_UNKNOWN
    return None
