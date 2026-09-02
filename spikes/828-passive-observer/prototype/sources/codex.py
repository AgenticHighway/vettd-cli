"""Codex rollout source for the spike #828 passive-observer prototype.

Format assumed from the openai/codex protocol crate (`RolloutLine` / `RolloutItem`). It is NOT
verified against a real rollout file: this machine has no Codex state, so the reader is
fixture-tested only and every structural assumption below is a claim about the crate, not about
observed logs. #965 must re-verify against a live `~/.codex` before porting.

  <root>/sessions/YYYY/MM/DD/rollout-*.jsonl      live sessions
  <root>/archived_sessions/**/rollout-*.jsonl     archived sessions
  one object per line: {"timestamp": ISO8601, "type": T, "payload": {...}}
  T = session_meta | turn_context | response_item | event_msg | compacted

Fixtures use `.ndjson` (the repo ignores `*.jsonl`); both suffixes are discovered.

Interpretation choices a real file could overturn (each is one constant or one small function):
- `token_count.info.total_token_usage` is cumulative for the session; per-turn `Usage` rows are the
  deltas, keyed `tc-<n>`. `input_tokens` is taken to include cached tokens (the crate's
  `non_cached_input()` subtracts them), so a row stores input minus cached. A counter that goes
  backwards marks the session `truncated`, re-baselines, and emits nothing for that event.
- MCP tools reach the model as `<server>__<tool>`, optionally prefixed `mcp__`; the server is the
  namespace before the last `__`, minus a trailing `_<12 hex>` length-limit hash suffix. An
  `mcp_tool_call_begin` for the same call id overrides the parsed server with the reported one.
- Both a `compacted` rollout line and an `event_msg` of type `context_compacted` count as a
  compaction; if a real rollout writes both for one compaction this double counts.
- A user-role message whose text starts with `<environment_context>` or `<user_instructions>` is
  harness-injected, not a user turn. The text is examined for that prefix and then dropped.
- Failure classes: `mcp_tool_call_end` with `Err` (or `Ok.is_error`) and an output object with
  `success: false` are `tool_error`; an output whose text says "rejected by user" is `user_denied`.
- No stop reason exists in this format, so `last_stop_reason` stays None; there is no in-band
  loaded set, so `loaded_events` stays empty and attribution falls back to the filesystem basis.

Privacy invariant: every kept line is projected through `_project` before any other use. Message
text, reasoning, tool arguments, tool outputs, MCP results and compaction summaries are reduced to
hashes and booleans inside that projection and the raw line is dropped. Names and harness ids
survive only in `SessionFacts.forbids`, which never egress.
"""
from __future__ import annotations

import calendar
import hashlib
import json
import os
import re
from datetime import datetime, timezone
from typing import Any, Dict, Iterator, List, Optional, Tuple

from sources.base import (
    FAILURE_TOOL_ERROR,
    FAILURE_USER_DENIED,
    HARNESS_CODEX,
    Cursor,
    SessionFacts,
    SessionRef,
    ToolCall,
    Usage,
    iter_lines,
)

CONSUMED_TYPES = frozenset({"session_meta", "turn_context", "response_item", "event_msg", "compacted"})
CONSUMED_ITEMS = frozenset(
    {"message", "function_call", "function_call_output", "custom_tool_call", "custom_tool_call_output", "reasoning"}
)
CONSUMED_EVENTS = frozenset({"token_count", "mcp_tool_call_begin", "mcp_tool_call_end", "context_compacted"})
SESSION_DIRS = ("sessions", "archived_sessions")
SESSION_SUFFIXES = (".jsonl", ".ndjson")
PEEK_BYTES = 65_536  # discover() reads at most this much of a file to find session_meta
APPROVAL_TO_PERMISSION = {"untrusted": "default", "on-failure": "auto", "on-request": "default", "never": "bypass"}
TOKEN_KEYS = ("input_tokens", "cached_input_tokens", "output_tokens", "reasoning_output_tokens")
INJECTED_PREFIXES = ("<environment_context>", "<user_instructions>")
HASH_SUFFIX_RE = re.compile(r"_[0-9a-f]{12}$")
DENIAL_RE = re.compile(r"rejected by user|denied by user")


class _ReadState:
    """Per-read scratch, discarded when read() returns: every tool call by call_id (a call can
    receive both an mcp_tool_call_end and a function_call_output), the model of the current turn
    and the last cumulative token counter."""

    def __init__(self) -> None:
        self.calls: Dict[str, ToolCall] = {}
        self.model = "unknown"
        self.baseline: Optional[Dict[str, int]] = None
        self.token_events = 0


class CodexSource:
    harness = HARNESS_CODEX

    def __init__(self, root: str) -> None:
        self.root = root

    # -- discovery -------------------------------------------------------------------------------

    def discover(self, root: str, window_days: int, now_ms: int) -> List[SessionRef]:
        cutoff = now_ms - window_days * 86_400_000
        refs: List[SessionRef] = []
        for sub in SESSION_DIRS:
            for path in _walk_sessions(os.path.join(root, sub)):
                if not _within_window(path, cutoff):
                    continue
                key = _peek_session_id(path) or _stem(os.path.basename(path))
                refs.append(SessionRef(path=path, harness=self.harness, session_key=key, kind="main"))
        return refs

    # -- reading ---------------------------------------------------------------------------------

    def read(self, ref: SessionRef, cursor: Optional[Cursor] = None) -> Tuple[SessionFacts, Cursor]:
        facts = SessionFacts(ref=ref)
        st = os.stat(ref.path)
        start = _resume_offset(cursor, ref.path, st)
        state = _ReadState()
        offset = start
        for offset, line in iter_lines(ref.path, start):
            facts.lines_seen += 1
            facts.bytes_read += len(line)
            raw = _decode(line)
            if raw is None:
                facts.parse_errors += 1
                continue
            if raw.get("type") not in CONSUMED_TYPES:
                facts.lines_unknown_type += 1
                continue
            projected = _project(raw)
            del raw
            _apply(facts, state, projected)
        return facts, Cursor(path=ref.path, byte_offset=offset, inode=st.st_ino)


def mcp_server_of(name: Optional[str]) -> Optional[str]:
    """Server namespace of a Codex MCP tool name, None for a built-in tool (no `__`). The optional
    `mcp__` prefix is dropped, the namespace is everything before the last `__`, and a trailing
    `_<12 hex>` length-limit hash suffix is stripped from it."""
    if not isinstance(name, str):
        return None
    bare = name[len("mcp__"):] if name.startswith("mcp__") else name
    namespace, sep, tool = bare.rpartition("__")
    if not sep or not namespace or not tool:
        return None
    return HASH_SUFFIX_RE.sub("", namespace) or namespace


# -- projection (the only place raw content is touched) -----------------------------------------


def _project(raw: dict) -> dict:
    kind = raw.get("type")
    payload = raw.get("payload")
    payload = payload if isinstance(payload, dict) else {}
    names: List[Tuple[str, str]] = []
    p: Dict[str, Any] = {"type": kind, "timestamp": raw.get("timestamp"), "names": names}
    if kind == "session_meta":
        p["meta"] = _project_meta(payload, names)
    elif kind == "turn_context":
        p["turn"] = _project_turn(payload, names)
    elif kind == "response_item":
        p["item"] = _project_item(payload, names)
    elif kind == "event_msg":
        p["event"] = _project_event(payload, names)
    return p  # "compacted": the summary message is not read


def _project_meta(payload: dict, names: List[Tuple[str, str]]) -> dict:
    _note(names, "harness_session_ids", payload.get("id"))
    _note(names, "harness_session_ids", payload.get("parent_thread_id"))
    _note(names, "cwd_and_branches", payload.get("cwd"))
    _note(names, "agent_ids", payload.get("agent_nickname"))
    git = payload.get("git")
    if isinstance(git, dict):
        for key in ("branch", "repository_url", "commit_hash"):
            _note(names, "cwd_and_branches", git.get(key))
    return {"cli_version": _str_or_none(payload.get("cli_version")), "originator": _str_or_none(payload.get("originator"))}


def _project_turn(payload: dict, names: List[Tuple[str, str]]) -> dict:
    _note(names, "cwd_and_branches", payload.get("cwd"))
    sandbox = payload.get("sandbox_policy")
    if isinstance(sandbox, dict):
        for root in _str_list(sandbox.get("writable_roots")):
            _note(names, "cwd_and_branches", root)
    policy = payload.get("approval_policy")
    return {
        "model": _str_or_none(payload.get("model")),
        "effort": _str_or_none(payload.get("effort")),
        "permission_mode": APPROVAL_TO_PERMISSION.get(policy, "unknown") if isinstance(policy, str) else "unknown",
    }


def _project_item(payload: dict, names: List[Tuple[str, str]]) -> dict:
    kind = payload.get("type")
    out: Dict[str, Any] = {"type": kind}
    _note(names, "message_ids", payload.get("id"))
    if kind == "message":
        out["role"] = _str_or_none(payload.get("role"))
        out["injected"] = _item_text(payload.get("content")).lstrip().startswith(INJECTED_PREFIXES)
    elif kind in ("function_call", "custom_tool_call"):
        out["call_id"] = _str_or_none(payload.get("call_id"))
        out["name"] = _str_or_none(payload.get("name"))
        out["input_fingerprint"] = _fingerprint(payload.get("arguments" if kind == "function_call" else "input"))
        _note(names, "tool_use_ids", out["call_id"])
    elif kind in ("function_call_output", "custom_tool_call_output"):
        out["call_id"] = _str_or_none(payload.get("call_id"))
        out.update(_project_output(payload.get("output")))
    return out


def _project_output(output: Any) -> dict:
    """Reduce a tool output to two booleans. `output` is a string or a {content, success} object
    (a string that decodes to such an object counts as one). The text is examined for the denial
    phrase and then dropped."""
    success: Optional[bool] = None
    text = output if isinstance(output, str) else ""
    obj = _maybe_json_object(output)
    if obj is not None:
        if isinstance(obj.get("content"), str):
            text = obj["content"]
        if isinstance(obj.get("success"), bool):
            success = obj["success"]
    return {"failed": success is False, "denial": bool(DENIAL_RE.search(text))}


def _project_event(payload: dict, names: List[Tuple[str, str]]) -> dict:
    kind = payload.get("type")
    out: Dict[str, Any] = {"type": kind}
    if kind == "token_count":
        info = payload.get("info")
        total = info.get("total_token_usage") if isinstance(info, dict) else None
        out["total"] = {k: _int_or_none(total.get(k)) or 0 for k in TOKEN_KEYS} if isinstance(total, dict) else None
    elif kind in ("mcp_tool_call_begin", "mcp_tool_call_end"):
        inv = payload.get("invocation")
        inv = inv if isinstance(inv, dict) else {}
        out["call_id"] = _str_or_none(payload.get("call_id"))
        out["server"] = _str_or_none(inv.get("server"))
        out["tool"] = _str_or_none(inv.get("tool"))
        out["input_fingerprint"] = _fingerprint(inv.get("arguments"))
        out["failed"] = _mcp_failed(payload.get("result"))
        _note(names, "tool_use_ids", out["call_id"])
        _note(names, "loaded_set_names", out["server"])
    return out


def _mcp_failed(result: Any) -> Optional[bool]:
    """None when there is no result (begin events); True for `Err` or an `Ok` whose MCP
    CallToolResult says is_error; the error text is never read."""
    if not isinstance(result, dict):
        return None
    if "Err" in result:
        return True
    ok = result.get("Ok")
    return isinstance(ok, dict) and ok.get("is_error") is True


# -- application of projected lines -------------------------------------------------------------


def _apply(facts: SessionFacts, state: _ReadState, p: dict) -> None:
    for bucket, value in p["names"]:
        facts.note_forbid(bucket, value)
    ts = _parse_ts(p.get("timestamp"))
    if ts is not None:
        facts.first_ts_ms = ts if facts.first_ts_ms is None else min(facts.first_ts_ms, ts)
        facts.last_ts_ms = ts if facts.last_ts_ms is None else max(facts.last_ts_ms, ts)
    else:
        ts = facts.last_ts_ms if facts.last_ts_ms is not None else 0
    kind = p["type"]
    if kind == "session_meta":
        _apply_meta(facts, p["meta"])
    elif kind == "turn_context":
        _apply_turn(facts, state, p["turn"])
    elif kind == "response_item":
        _apply_item(facts, state, p["item"], ts)
    elif kind == "event_msg":
        _apply_event(facts, state, p["event"], ts)
    elif kind == "compacted":
        facts.compactions += 1


def _apply_meta(facts: SessionFacts, meta: dict) -> None:
    if meta["cli_version"] and facts.harness_version == "unknown":
        facts.harness_version = meta["cli_version"]
    if meta["originator"] and facts.entrypoint == "unknown":
        facts.entrypoint = meta["originator"]


def _apply_turn(facts: SessionFacts, state: _ReadState, turn: dict) -> None:
    if turn["model"]:
        state.model = turn["model"]
        facts.models[turn["model"]] = facts.models.get(turn["model"], 0) + 1
    if turn["effort"] and facts.effort == "unknown":
        facts.effort = turn["effort"]
    if facts.permission_mode == "unknown":
        facts.permission_mode = turn["permission_mode"]


def _apply_item(facts: SessionFacts, state: _ReadState, item: dict, ts: int) -> None:
    kind = item["type"]
    if kind == "message":
        if item["role"] == "user" and not item["injected"]:
            facts.user_turns += 1
    elif kind in ("function_call", "custom_tool_call"):
        if item["call_id"]:
            _open_call(facts, state, item["call_id"], item["name"] or "unknown", item["input_fingerprint"], ts)
    elif kind in ("function_call_output", "custom_tool_call_output"):
        _pair(state, item["call_id"], ts, failed=item["failed"], denial=item["denial"])
    elif kind not in CONSUMED_ITEMS:  # `reasoning` is consumed and carries nothing kept
        facts.lines_unknown_type += 1


def _apply_event(facts: SessionFacts, state: _ReadState, ev: dict, ts: int) -> None:
    kind = ev["type"]
    if kind == "token_count":
        if ev["total"] is not None:
            _apply_token_count(facts, state, ev["total"], ts)
    elif kind == "mcp_tool_call_begin":
        _begin_mcp(facts, state, ev, ts)
    elif kind == "mcp_tool_call_end":
        _pair(state, ev["call_id"], ts, failed=ev["failed"], denial=False)
    elif kind == "context_compacted":
        facts.compactions += 1
    else:
        facts.lines_unknown_type += 1


def _apply_token_count(facts: SessionFacts, state: _ReadState, total: Dict[str, int], ts: int) -> None:
    prev = state.baseline or {k: 0 for k in TOKEN_KEYS}
    delta = {k: total[k] - prev[k] for k in TOKEN_KEYS}
    state.baseline = total
    if any(v < 0 for v in delta.values()):
        facts.truncated = True  # rewritten or resumed with a fresh counter: re-baselined, nothing emitted
        return
    if not any(delta.values()):
        return  # a repeat of the previous counter (rate-limit refresh): nothing new to attribute
    state.token_events += 1
    mid = f"tc-{state.token_events}"
    facts.usages[mid] = Usage(
        message_id=mid, model=state.model, ts_ms=ts,
        input_tokens=max(0, delta["input_tokens"] - delta["cached_input_tokens"]),
        output_tokens=delta["output_tokens"], cached_input=delta["cached_input_tokens"],
        reasoning=delta["reasoning_output_tokens"],
    )


def _open_call(facts: SessionFacts, state: _ReadState, call_id: str, name: str, fingerprint: str, ts: int) -> ToolCall:
    call = ToolCall(tool_use_id=call_id, name=name, ts_ms=ts, input_fingerprint=fingerprint, server=mcp_server_of(name))
    if call.server:
        facts.note_forbid("loaded_set_names", name)
        facts.note_forbid("loaded_set_names", call.server)
    facts.tool_calls.append(call)
    state.calls[call_id] = call
    return call


def _begin_mcp(facts: SessionFacts, state: _ReadState, ev: dict, ts: int) -> None:
    if not ev["call_id"] or not ev["server"]:
        return
    call = state.calls.get(ev["call_id"])
    if call is None:  # no function_call item carried this call: the event is its only record
        name = f"{ev['server']}__{ev['tool'] or 'unknown'}"
        call = _open_call(facts, state, ev["call_id"], name, ev["input_fingerprint"], ts)
    call.server = ev["server"]  # the harness-reported identity wins over name parsing
    facts.note_forbid("loaded_set_names", call.server)


def _pair(state: _ReadState, call_id: Optional[str], ts: int, failed: Optional[bool], denial: bool) -> None:
    call = state.calls.get(call_id) if call_id else None
    if call is None:  # a result with no known call: nothing to attach it to
        return
    if call.result_ts_ms is None:
        call.result_ts_ms = ts
    if denial:
        call.is_error = True
        call.failure_class = FAILURE_USER_DENIED
    elif failed:
        call.is_error = True
        if call.failure_class is None:
            call.failure_class = FAILURE_TOOL_ERROR
    elif call.is_error is None:
        call.is_error = False


# -- small helpers (candidates to hoist into sources/base.py; kept local to stay in-contract) -------


def _note(names: List[Tuple[str, str]], bucket: str, value: Any) -> None:
    if isinstance(value, str) and value:
        names.append((bucket, value))


def _item_text(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(b.get("text", "") for b in content if isinstance(b, dict) and isinstance(b.get("text"), str))
    return ""


def _maybe_json_object(value: Any) -> Optional[dict]:
    if isinstance(value, dict):
        return value
    if isinstance(value, str) and value[:1] == "{":
        try:
            obj = json.loads(value)
        except ValueError:
            return None
        return obj if isinstance(obj, dict) else None
    return None


def _fingerprint(value: Any) -> str:
    """sha256 of the canonical JSON of a tool input. A JSON-encoded string input is parsed first so
    key order cannot defeat the repeated-call indicator; the input itself is not kept."""
    if isinstance(value, str) and value[:1] in "{[":
        try:
            value = json.loads(value)
        except ValueError:
            pass
    return _sha256_json(value)


def _walk_sessions(top: str) -> Iterator[str]:
    for dirpath, dirnames, filenames in os.walk(top):
        dirnames.sort()
        for entry in sorted(filenames):
            if entry.endswith(SESSION_SUFFIXES) and _stem(entry):
                yield os.path.join(dirpath, entry)


def _stem(entry: str) -> str:
    for suffix in SESSION_SUFFIXES:
        if entry.endswith(suffix):
            return entry[: -len(suffix)]
    return entry


def _peek_session_id(path: str) -> Optional[str]:
    """`session_meta.payload.id` from the head of the file, reading at most PEEK_BYTES."""
    try:
        for _, line in iter_lines(path, 0, max_bytes=PEEK_BYTES):
            raw = _decode(line)
            if raw is not None and raw.get("type") == "session_meta":
                payload = raw.get("payload")
                return _str_or_none(payload.get("id")) if isinstance(payload, dict) else None
    except OSError:
        return None
    return None


def _within_window(path: str, cutoff_ms: int) -> bool:
    try:
        return os.path.isfile(path) and int(os.stat(path).st_mtime * 1000) >= cutoff_ms
    except OSError:
        return False


def _resume_offset(cursor: Optional[Cursor], path: str, st: os.stat_result) -> int:
    if cursor is None or cursor.path != path or cursor.byte_offset > st.st_size:
        return 0
    if cursor.inode is not None and cursor.inode != st.st_ino:
        return 0
    return max(0, cursor.byte_offset)


def _decode(line: bytes) -> Optional[dict]:
    try:
        obj = json.loads(line)
    except ValueError:
        return None
    return obj if isinstance(obj, dict) else None


def _parse_ts(value: Any) -> Optional[int]:
    if not isinstance(value, str) or not value:
        return None
    try:
        dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    dt = dt.astimezone(timezone.utc)
    return calendar.timegm(dt.timetuple()) * 1000 + dt.microsecond // 1000


def _sha256_json(value: Any) -> str:
    canonical = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, default=str)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def _int_or_none(value: Any) -> Optional[int]:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _str_or_none(value: Any) -> Optional[str]:
    return value if isinstance(value, str) and value else None


def _str_list(value: Any) -> List[str]:
    return [v for v in value if isinstance(v, str)] if isinstance(value, list) else []
