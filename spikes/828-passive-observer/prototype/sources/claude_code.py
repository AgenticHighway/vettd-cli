"""Claude Code session source for the spike #828 passive-observer prototype.

Layout read (verified against harness 2.1.258):
  <root>/projects/<project>/<session>.jsonl                      main transcript
  <root>/projects/<project>/<session>/subagents/agent-<id>.jsonl  child transcript
  <root>/projects/<project>/<session>/subagents/agent-<id>.meta.json {agentType, toolUseId, spawnDepth}
Fixtures use `.ndjson` (the repo ignores `*.jsonl`); both suffixes are discovered.

Privacy invariant: every kept line is projected through `_project` before any other use. Inside
that projection, content-bearing values (message text, thinking, tool inputs, tool results,
attachment bodies, `toolUseResult` bodies) are reduced to hashes, booleans and byte lengths; the raw
line is then dropped. Names and harness ids survive only in `SessionFacts.forbids`, which never
egress. Unconsumed line types (and unconsumed attachment subtypes) are counted, not interpreted.
"""
from __future__ import annotations

import calendar
import dataclasses
import hashlib
import json
import os
import re
import time
from datetime import datetime, timezone
from typing import Any, Dict, Iterable, List, Optional, Tuple

from sources.base import (
    FAILURE_TOOL_ERROR,
    FAILURE_USER_DENIED,
    HARNESS_CLAUDE_CODE,
    Cursor,
    InBandAsset,
    LoadedSetEvent,
    SessionFacts,
    SessionRef,
    ToolCall,
    Usage,
    iter_lines,
)

CONSUMED_TYPES = frozenset({"user", "assistant", "attachment", "summary"})
CONSUMED_ATTACHMENTS = frozenset(
    {"skill_listing", "deferred_tools_delta", "agent_listing_delta", "mcp_instructions_delta", "nested_memory"}
)
SESSION_SUFFIXES = (".jsonl", ".ndjson")
TRUNCATION_GRACE_MS = 120_000
ASYNC_ACK_PREFIX = "Async agent launched"
DENIAL_RE = re.compile(r"doesn't want to proceed|rejected by the user|denied by the user|permission (?:was )?denied"
                       r"|Request interrupted by user")
COMMAND_NAME_RE = re.compile(r"<command-name>([^<\s]+)</command-name>")
# Harness built-ins are not assets (CONTRACTS.md attribute.py) and are kept out of the dynamic
# forbids: as substrings they would collide with legitimate enum values (agent, plan, code_edit).
BUILTIN_AGENT_TYPES = frozenset(
    {"Explore", "Plan", "general-purpose", "claude", "Bash", "statusline-setup", "claude-code-guide", "output-style-setup"}
)
TOP_KEYS = (
    "type", "uuid", "parentUuid", "timestamp", "sessionId", "isSidechain", "agentId", "version",
    "entrypoint", "permissionMode", "effort", "sourceToolAssistantUUID", "isMeta",
)
USAGE_KEYS = ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens")


class _ReadState:
    """Per-read scratch: open tool calls awaiting a result, and the bookkeeping the contract keys
    on 'first' events. Discarded when read() returns."""

    def __init__(self) -> None:
        self.open: Dict[str, ToolCall] = {}
        self.seen_message_ids: set = set()
        self.seen_deferred = False
        self.initial_event: Optional[LoadedSetEvent] = None
        self.rules_files: List[str] = []
        self.synthetic = 0
        self.corroborated = False
        self.pending_skill: Optional[dict] = None  # a <skill-format>true command waiting for its body line


class ClaudeCodeSource:
    harness = HARNESS_CLAUDE_CODE

    def __init__(self, root: str, now_ms: Optional[int] = None) -> None:
        self.root = root
        self._now_ms = now_ms

    # -- discovery -------------------------------------------------------------------------------

    def discover(self, root: str, window_days: int, now_ms: int) -> List[SessionRef]:
        self._now_ms = now_ms
        cutoff = now_ms - window_days * 86_400_000
        refs: List[SessionRef] = []
        projects = os.path.join(root, "projects")
        for project in sorted(_listdir(projects)):
            pdir = os.path.join(projects, project)
            for entry in sorted(_listdir(pdir)):
                stem = _session_stem(entry)
                if stem is None:
                    continue
                path = os.path.join(pdir, entry)
                if _within_window(path, cutoff):
                    refs.append(SessionRef(path=path, harness=self.harness, session_key=stem, kind="main"))
                subagents = os.path.join(pdir, stem, "subagents")
                refs.extend(self._discover_children(subagents, stem, cutoff))
                for wf in sorted(_listdir(os.path.join(subagents, "workflows"))):
                    refs.extend(self._discover_children(os.path.join(subagents, "workflows", wf), stem, cutoff))
        return refs

    def _discover_children(self, subdir: str, parent_key: str, cutoff: int) -> List[SessionRef]:
        refs: List[SessionRef] = []
        for entry in sorted(_listdir(subdir)):
            stem = _session_stem(entry)
            if stem is None or not stem.startswith("agent-"):
                continue
            path = os.path.join(subdir, entry)
            if not _within_window(path, cutoff):
                continue
            agent_id = stem[len("agent-"):]
            meta = _read_child_meta(os.path.join(subdir, stem + ".meta.json"))
            meta["agentId"] = agent_id
            refs.append(SessionRef(path=path, harness=self.harness, session_key=agent_id, kind="child",
                                   parent_key=parent_key, child_meta=meta))
        return refs

    # -- reading ---------------------------------------------------------------------------------

    def read(self, ref: SessionRef, cursor: Optional[Cursor] = None) -> Tuple[SessionFacts, Cursor]:
        facts = SessionFacts(ref=ref)
        st = os.stat(ref.path)
        start = _resume_offset(cursor, ref.path, st)
        state = _ReadState()
        expected_agent = ref.child_meta.get("agentType") if ref.kind == "child" else None
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
            projected = _project(raw, len(line), expected_agent)
            del raw
            _apply(facts, state, projected)
        facts.truncated = self._is_truncated(facts, st.st_mtime)
        if ref.kind == "child" and state.corroborated:
            facts.ref = dataclasses.replace(ref, child_meta={**ref.child_meta, "corroborated": "true"})
        return facts, Cursor(path=ref.path, byte_offset=offset, inode=st.st_ino)

    def _is_truncated(self, facts: SessionFacts, mtime_s: float) -> bool:
        now_ms = self._now_ms if self._now_ms is not None else int(time.time() * 1000)
        recent = abs(now_ms - int(mtime_s * 1000)) <= TRUNCATION_GRACE_MS
        return recent and facts.last_stop_reason != "end_turn"


def link_children(all_facts: Iterable[SessionFacts]) -> List[SessionFacts]:
    """Attach child facts to their parent's `children` by `parent_key`; returns the main sessions.
    Convenience for the caller, which reads each discovered ref (main and child) separately so
    every file keeps its own cursor."""
    items = list(all_facts)
    mains = {f.ref.session_key: f for f in items if f.ref.kind == "main"}
    for f in items:
        if f.ref.kind == "child" and f.ref.parent_key in mains:
            mains[f.ref.parent_key].children.append(f)
    return list(mains.values())


# -- projection (the only place raw content is touched) -----------------------------------------


def _project(raw: dict, line_len: int, expected_agent: Optional[str]) -> dict:
    p: Dict[str, Any] = {k: raw.get(k) for k in TOP_KEYS if k in raw}
    p["line_len"] = line_len
    p["names"] = _harvest_names(raw)
    msg = raw.get("message")
    p["message"] = _project_message(msg, bool(raw.get("isMeta"))) if isinstance(msg, dict) else None
    tur = raw.get("toolUseResult")
    p["toolUseResult"] = (
        {k: tur.get(k) for k in ("interrupted", "isAsync", "agentId", "status") if k in tur}
        if isinstance(tur, dict) else {}
    )
    att = raw.get("attachment")
    p["attachment"] = _project_attachment(att) if isinstance(att, dict) else None
    attribution = raw.get("attributionAgent")
    p["attribution_matches"] = bool(expected_agent) and attribution == expected_agent
    server = raw.get("attributionMcpServer")
    p["mcp_attribution"] = server if isinstance(server, str) and server else None
    return p


def _harvest_names(raw: dict) -> List[Tuple[str, str]]:
    out: List[Tuple[str, str]] = []
    for key, bucket in (("slug", "slugs"), ("cwd", "cwd_and_branches"), ("gitBranch", "cwd_and_branches"),
                        ("sessionId", "harness_session_ids"), ("agentId", "agent_ids")):
        value = raw.get(key)
        if isinstance(value, str) and value:
            out.append((bucket, value))
    for name in _names_in(raw.get("mcpMeta")):
        out.append(("loaded_set_names", name))
    return out


def _names_in(node: Any, depth: int = 0) -> List[str]:
    """String values under any `name` key of an mcpMeta blob (server identity), nothing else."""
    if depth > 4 or not isinstance(node, dict):
        return []
    found: List[str] = []
    for key, value in node.items():
        if key == "name" and isinstance(value, str) and value:
            found.append(value)
        elif isinstance(value, dict):
            found.extend(_names_in(value, depth + 1))
    return found


def _project_message(msg: dict, is_meta: bool) -> dict:
    content = msg.get("content")
    blocks = [_project_block(b) for b in content if isinstance(b, dict)] if isinstance(content, list) else []
    usage = msg.get("usage")
    return {
        "id": msg.get("id") if isinstance(msg.get("id"), str) else None,
        "model": msg.get("model") if isinstance(msg.get("model"), str) else None,
        "stop_reason": msg.get("stop_reason") if isinstance(msg.get("stop_reason"), str) else None,
        "usage": _project_usage(usage) if isinstance(usage, dict) else None,
        "content_is_str": isinstance(content, str),
        "blocks": blocks,
        "command": _command_from_text(content) if is_meta else None,
        "meta_text": _meta_text_digest(content) if is_meta else None,
        "injected": _is_injected(content),
    }


def _project_usage(usage: dict) -> dict:
    out = {k: _int_or_none(usage.get(k)) for k in USAGE_KEYS}
    details = usage.get("output_tokens_details")
    out["thinking_tokens"] = _int_or_none(details.get("thinking_tokens")) if isinstance(details, dict) else None
    return out


def _project_block(block: dict) -> dict:
    kind = block.get("type")
    pb: Dict[str, Any] = {"type": kind}
    if kind == "tool_use":
        inp = block.get("input")
        fields = inp if isinstance(inp, dict) else {}
        pb["id"] = block.get("id") if isinstance(block.get("id"), str) else None
        pb["name"] = block.get("name") if isinstance(block.get("name"), str) else None
        pb["input_fingerprint"] = _sha256_json(inp)
        pb["skill"] = _str_or_none(fields.get("skill")) or _str_or_none(fields.get("name"))
        pb["agent_type"] = _str_or_none(fields.get("subagent_type"))
    elif kind == "tool_result":
        text = _result_text(block.get("content"))
        pb["tool_use_id"] = block.get("tool_use_id") if isinstance(block.get("tool_use_id"), str) else None
        pb["is_error"] = block.get("is_error")
        pb["denial"] = bool(DENIAL_RE.search(text))
        pb["async_ack"] = text.startswith(ASYNC_ACK_PREFIX)
    return pb


def _result_text(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(b.get("text", "") for b in content if isinstance(b, dict) and isinstance(b.get("text"), str))
    return ""


INJECTED_PREFIXES = ("<task-notification>", "[SYSTEM NOTIFICATION", "<wake ", "<webhook-payload", "<system-reminder>")


def _is_injected(content: Any) -> bool:
    """A user line the harness injected (a task notification, a wake, a reminder-only line) is
    not a person's turn. Decided on the leading bytes of the text, which are then discarded."""
    text = _result_text(content).lstrip()
    if not text:
        return False
    if text.startswith("<system-reminder>") and text.rstrip().endswith("</system-reminder>"):
        return True
    return any(text.startswith(prefix) for prefix in INJECTED_PREFIXES if prefix != "<system-reminder>")


def _command_from_text(content: Any) -> Optional[dict]:
    text = _result_text(content)
    m = COMMAND_NAME_RE.search(text)
    if not m:
        return None
    body = text[m.end():].encode("utf-8")
    return {"name": m.group(1), "sha256": hashlib.sha256(body).hexdigest(), "byte_len": len(body),
            "skill_format": "<skill-format>true</skill-format>" in text}


def _meta_text_digest(content: Any) -> Optional[dict]:
    """Hash and length of a meta line's text (the harness injects a skill body as its own meta
    line right after the command line); the text itself is discarded here."""
    text = _result_text(content)
    if not text:
        return None
    body = text.encode("utf-8")
    return {"sha256": hashlib.sha256(body).hexdigest(), "byte_len": len(body)}


def _project_attachment(att: dict) -> dict:
    kind = att.get("type")
    out: Dict[str, Any] = {"type": kind}
    if kind == "skill_listing":
        names = _str_list(att.get("names"))
        lines = att.get("content").split("\n") if isinstance(att.get("content"), str) else []
        out["names"] = names
        out["listing_bytes"] = {n: len(next((ln for ln in lines if ln.startswith(f"- {n}:")), "")) for n in names}
    elif kind == "deferred_tools_delta":
        for key, field_name in (("addedNames", "added"), ("pendingMcpServers", "pending"), ("failedMcpServers", "failed"),
                                ("removedNames", "removed"), ("readdedNames", "readded")):
            out[field_name] = _str_list(att.get(key))
        out["schema_bytes"] = _schema_bytes(out["added"], _str_list(att.get("addedLines")))
    elif kind == "agent_listing_delta":
        out["types"] = _str_list(att.get("addedTypes"))
        out["is_initial"] = bool(att.get("isInitial"))
    elif kind == "mcp_instructions_delta":
        out["names"] = _str_list(att.get("addedNames"))
    elif kind == "nested_memory":
        content = att.get("content")
        if isinstance(content, dict):  # harness 2.1.x nests {path, type, content, contentDiffersFromDisk}
            content = content.get("content")
        body = content.encode("utf-8") if isinstance(content, str) else b""
        path = att.get("path") if isinstance(att.get("path"), str) else ""
        out["basename"] = os.path.basename(path) or "memory"
        out["sha256"] = hashlib.sha256(body).hexdigest()
        out["byte_len"] = len(body)
    return out


def _schema_bytes(names: List[str], lines: List[str]) -> Dict[str, int]:
    """Bytes of the tool-listing lines per MCP server. Lines align with names by index (verified);
    fall back to prefix matching when they do not."""
    out: Dict[str, int] = {}
    for i, name in enumerate(names):
        server = _mcp_server(name)
        if server is None:
            continue
        line = lines[i] if i < len(lines) and lines[i].startswith(name) else next((ln for ln in lines if ln.startswith(name)), "")
        out[server] = out.get(server, 0) + len(line)
    return out


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
    _note_env(facts, p)
    kind = p["type"]
    if kind == "summary":
        facts.compactions += 1
    elif kind == "attachment":
        _apply_attachment(facts, state, p["attachment"], ts)
    elif kind == "assistant":
        _apply_assistant(facts, state, p, ts)
    else:
        _apply_user(facts, state, p, ts)


def _note_env(facts: SessionFacts, p: dict) -> None:
    for key, attr in (("version", "harness_version"), ("entrypoint", "entrypoint"), ("effort", "effort")):
        value = p.get(key)
        if isinstance(value, str) and value and getattr(facts, attr) == "unknown":
            setattr(facts, attr, value)
    mode = p.get("permissionMode")
    if isinstance(mode, str) and mode:
        # Most frequent mode wins (a session can enter and leave plan mode); ties keep the earlier one.
        counts = facts.forbids.setdefault("_permission_modes", set())  # local scratch, never a forbid
        counts.add(mode)
        facts._mode_counts = getattr(facts, "_mode_counts", {})
        facts._mode_counts[mode] = facts._mode_counts.get(mode, 0) + 1
        best = max(sorted(facts._mode_counts.items()), key=lambda kv: kv[1])[0]
        facts.permission_mode = best


def _apply_assistant(facts: SessionFacts, state: _ReadState, p: dict, ts: int) -> None:
    msg = p["message"] or {"id": None, "model": None, "stop_reason": None, "usage": None, "blocks": []}
    if p.get("mcp_attribution"):
        facts.mcp_attribution_counts[p["mcp_attribution"]] = facts.mcp_attribution_counts.get(p["mcp_attribution"], 0) + 1
        facts.note_forbid("loaded_set_names", p["mcp_attribution"])
    mid = msg["id"]
    if msg["stop_reason"]:
        facts.last_stop_reason = msg["stop_reason"]
    if mid:
        facts.note_forbid("message_ids", mid)
        if mid not in state.seen_message_ids:  # one API response is split over several lines
            state.seen_message_ids.add(mid)
            model = msg["model"] or "unknown"
            facts.models[model] = facts.models.get(model, 0) + 1
            if msg["usage"] is not None:
                usage = _usage(mid, model, ts, msg["usage"])
                current = facts.usages.get(mid)
                if current is None or usage.output_tokens > current.output_tokens:
                    facts.usages[mid] = usage  # later lines of a streamed response carry the fuller usage
    for b in msg["blocks"]:
        if b["type"] == "tool_use" and b["id"]:
            _open_call(facts, state, b, mid, ts)
    if p.get("attribution_matches"):
        state.corroborated = True


def _usage(mid: str, model: str, ts: int, u: dict) -> Usage:
    return Usage(message_id=mid, model=model, ts_ms=ts, input_tokens=u["input_tokens"] or 0,
                 output_tokens=u["output_tokens"] or 0, cache_creation=u["cache_creation_input_tokens"],
                 cache_read=u["cache_read_input_tokens"], thinking=u["thinking_tokens"])


def _open_call(facts: SessionFacts, state: _ReadState, b: dict, mid: Optional[str], ts: int) -> None:
    call = ToolCall(tool_use_id=b["id"], name=b["name"] or "unknown", ts_ms=ts, message_id=mid,
                    input_fingerprint=b["input_fingerprint"])
    if call.name.startswith("mcp__"):
        call.server = _mcp_server(call.name)
        facts.note_forbid("loaded_set_names", call.name)
        facts.note_forbid("loaded_set_names", call.server)
    elif call.name == "Skill":
        call.skill = b["skill"]
        facts.note_forbid("loaded_set_names", call.skill)
    elif call.name == "Agent":
        call.agent_type = b["agent_type"]
        if call.agent_type and call.agent_type not in BUILTIN_AGENT_TYPES:
            facts.note_forbid("loaded_set_names", call.agent_type)
    facts.note_forbid("tool_use_ids", call.tool_use_id)
    facts.tool_calls.append(call)
    state.open[call.tool_use_id] = call


def _apply_user(facts: SessionFacts, state: _ReadState, p: dict, ts: int) -> None:
    msg = p["message"] or {"content_is_str": False, "blocks": [], "command": None}
    tur = p["toolUseResult"]
    blocks = msg["blocks"]
    results = [b for b in blocks if b["type"] == "tool_result"]
    for b in results:
        _pair_result(state, b, tur, ts)
    if tur.get("agentId"):
        facts.note_forbid("agent_ids", str(tur["agentId"]))
    is_meta = bool(p.get("isMeta"))
    has_text = msg["content_is_str"] or any(b["type"] == "text" for b in blocks)
    result_only = bool(blocks) and len(results) == len(blocks)
    if has_text and not is_meta and not result_only and not msg.get("injected"):
        facts.user_turns += 1
    if is_meta and msg["command"]:
        if msg["command"].get("skill_format"):
            state.pending_skill = {"name": msg["command"]["name"], "ts": ts}  # body arrives on the next meta line
        else:
            _record_skill_invocation(facts, state, msg["command"], ts)
    elif is_meta and state.pending_skill and msg.get("meta_text"):
        cmd = {"name": state.pending_skill["name"], **msg["meta_text"]}
        state.pending_skill = None
        _record_skill_invocation(facts, state, cmd, ts)


def _pair_result(state: _ReadState, b: dict, tur: dict, ts: int) -> None:
    call = state.open.pop(b["tool_use_id"], None) if b["tool_use_id"] else None
    if call is None:  # a result with no open call: nothing to attach it to (no field for this)
        return
    call.result_ts_ms = ts
    call.is_error = bool(b["is_error"])
    call.interrupted = bool(tur.get("interrupted"))
    call.is_async = bool(tur.get("isAsync")) or b["async_ack"]
    if call.name == "Agent" and tur.get("agentId"):
        call.child_key = str(tur["agentId"])
    if call.is_error and (call.interrupted or b["denial"]):
        call.failure_class = FAILURE_USER_DENIED
    elif call.is_error:
        call.failure_class = FAILURE_TOOL_ERROR


def _record_skill_invocation(facts: SessionFacts, state: _ReadState, cmd: dict, ts: int) -> None:
    name = cmd["name"]
    facts.in_band_assets.append(InBandAsset(kind="skill_body", name=name, content_sha256=cmd["sha256"],
                                            byte_len=cmd["byte_len"], ts_ms=ts))
    state.synthetic += 1
    # Paired with itself. `is_async=True` is how the shared model expresses "latency None" for a
    # paired call (extract nulls latency for async calls); the harness injects the body without a
    # tool round-trip, so there is no latency to measure.
    facts.tool_calls.append(ToolCall(tool_use_id=f"synthetic-skill-{state.synthetic}", name="Skill", ts_ms=ts,
                                     result_ts_ms=ts, is_error=False, is_async=True, skill=name,
                                     input_fingerprint=_sha256_json({"skill": name})))
    facts.note_forbid("loaded_set_names", name)


def _apply_attachment(facts: SessionFacts, state: _ReadState, a: Optional[dict], ts: int) -> None:
    kind = a["type"] if a else None
    if kind not in CONSUMED_ATTACHMENTS:
        facts.lines_unknown_type += 1
        return
    if kind == "skill_listing":
        _forbid_all(facts, a["names"])
        _add_event(state, facts, LoadedSetEvent(ts_ms=ts, kind="initial", skills=a["names"], listing_bytes=a["listing_bytes"]))
    elif kind == "deferred_tools_delta":
        kind_ev = "delta" if state.seen_deferred else "initial"
        state.seen_deferred = True
        for name in a["added"] + a["removed"] + a["readded"]:
            if _mcp_server(name):
                _forbid_all(facts, [name, _mcp_server(name)])
        _forbid_all(facts, a["pending"] + a["failed"])
        _add_event(state, facts, LoadedSetEvent(ts_ms=ts, kind=kind_ev, tool_names=a["added"], pending_mcp=a["pending"],
                                                failed_mcp=a["failed"], removed=a["removed"], readded=a["readded"],
                                                tool_schema_bytes=a["schema_bytes"]))
    elif kind == "agent_listing_delta":
        _forbid_all(facts, [t for t in a["types"] if t not in BUILTIN_AGENT_TYPES])
        _add_event(state, facts, LoadedSetEvent(ts_ms=ts, kind="initial" if a["is_initial"] else "delta", agent_types=a["types"]))
    elif kind == "mcp_instructions_delta":  # tool names unchanged; the pending server resolved
        _forbid_all(facts, a["names"])
        _add_event(state, facts, LoadedSetEvent(ts_ms=ts, kind="delta"))
    elif kind == "nested_memory":
        facts.in_band_assets.append(InBandAsset(kind="rules_file", name=a["basename"], content_sha256=a["sha256"],
                                                byte_len=a["byte_len"], ts_ms=ts))
        facts.note_forbid("loaded_set_names", a["basename"])
        state.rules_files.append(a["basename"])
        if state.initial_event is not None:
            state.initial_event.rules_files.append(a["basename"])


def _add_event(state: _ReadState, facts: SessionFacts, ev: LoadedSetEvent) -> None:
    facts.loaded_events.append(ev)
    if ev.kind == "initial" and state.initial_event is None:
        state.initial_event = ev
        ev.rules_files.extend(state.rules_files)  # nested_memory seen before the listing


# -- small helpers ---------------------------------------------------------------------------------


def _forbid_all(facts: SessionFacts, names: Iterable[Optional[str]]) -> None:
    for name in names:
        facts.note_forbid("loaded_set_names", name)


def _mcp_server(name: Optional[str]) -> Optional[str]:
    if not isinstance(name, str) or not name.startswith("mcp__"):
        return None
    parts = name.split("__")
    return parts[1] if len(parts) >= 3 and parts[1] else None


def _listdir(path: str) -> List[str]:
    try:
        return os.listdir(path)
    except OSError:
        return []


def _session_stem(entry: str) -> Optional[str]:
    for suffix in SESSION_SUFFIXES:
        if entry.endswith(suffix) and len(entry) > len(suffix):
            return entry[: -len(suffix)]
    return None


def _within_window(path: str, cutoff_ms: int) -> bool:
    try:
        return os.path.isfile(path) and int(os.stat(path).st_mtime * 1000) >= cutoff_ms
    except OSError:
        return False


def _read_child_meta(path: str) -> Dict[str, str]:
    try:
        with open(path, "rb") as fh:
            meta = json.load(fh)
    except (OSError, ValueError):
        return {}
    if not isinstance(meta, dict):
        return {}
    out: Dict[str, str] = {}
    for key in ("agentType", "toolUseId", "spawnDepth"):
        value = meta.get(key)
        if isinstance(value, (str, int)) and not isinstance(value, bool):
            out[key] = str(value)
    return out


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
