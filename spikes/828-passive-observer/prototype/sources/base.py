"""Shared, harness-neutral data model for the passive-observer prototype.

Everything a harness source produces is one of the types below. The invariant that matters most:
no field on any of these types may hold free text from a session. Names are kept (locally) because
attribution needs them and because the gate checker consumes them as dynamic forbids; message
content, tool inputs, tool results, prompts, summaries and file contents are hashed or counted at
parse time and never stored.

`ts_ms` values are harness-clock milliseconds since the epoch. Durations are always differences of
harness timestamps, never collector-clock readings.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, Iterator, List, Optional, Protocol, Tuple

HARNESS_CLAUDE_CODE = "claude_code"
HARNESS_CODEX = "codex"

FAILURE_TOOL_ERROR = "tool_error"
FAILURE_TIMEOUT = "timeout"
FAILURE_USER_DENIED = "user_denied"
FAILURE_INTERRUPTED = "interrupted"
FAILURE_UNKNOWN = "unknown"
FAILURE_CLASSES = (FAILURE_TOOL_ERROR, FAILURE_TIMEOUT, FAILURE_USER_DENIED, FAILURE_INTERRUPTED, FAILURE_UNKNOWN)
# Only these classes count toward the observed non-success rate.
RATE_BEARING_FAILURES = (FAILURE_TOOL_ERROR, FAILURE_TIMEOUT)


@dataclass(frozen=True)
class SessionRef:
    """A discoverable session file. `session_key` is the harness's own session identifier and is
    local-only: it is HMAC'd into run_id and never egresses."""

    path: str
    harness: str
    session_key: str
    kind: str  # "main" | "child"
    parent_key: Optional[str] = None
    child_meta: Dict[str, str] = field(default_factory=dict)  # e.g. {"agentType": "...", "toolUseId": "..."}


@dataclass(frozen=True)
class Cursor:
    """Byte-offset cursor for resumable, non-blocking reads. A partial trailing line is never
    consumed; `byte_offset` always points at a line boundary."""

    path: str
    byte_offset: int
    inode: Optional[int] = None


@dataclass
class ToolCall:
    """One tool_use paired (when possible) with its tool_result. `input_fingerprint` is a hash of
    the canonical input, used only for the local repeated-call indicator; the input itself is
    discarded at parse time."""

    tool_use_id: str
    name: str
    ts_ms: int
    message_id: Optional[str] = None
    result_ts_ms: Optional[int] = None
    is_error: Optional[bool] = None
    interrupted: bool = False
    is_async: bool = False
    failure_class: Optional[str] = None  # one of FAILURE_CLASSES, None when successful
    input_fingerprint: str = ""
    server: Optional[str] = None  # MCP server name for mcp tools (local only)
    skill: Optional[str] = None  # skill name for Skill invocations (local only)
    agent_type: Optional[str] = None  # sub-agent type for Agent spawns (local only)
    child_key: Optional[str] = None  # linked child session key, if any

    @property
    def paired(self) -> bool:
        return self.result_ts_ms is not None

    @property
    def latency_ms(self) -> Optional[int]:
        if self.result_ts_ms is None:
            return None
        return max(0, self.result_ts_ms - self.ts_ms)


@dataclass
class Usage:
    """Token usage of one API response, keyed by the provider message id so a response that is
    split over several log lines counts once."""

    message_id: str
    model: str
    ts_ms: int
    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation: Optional[int] = None
    cache_read: Optional[int] = None
    cached_input: Optional[int] = None
    thinking: Optional[int] = None
    reasoning: Optional[int] = None


@dataclass
class LoadedSetEvent:
    """What the harness said was loaded, at a harness timestamp. `kind` is "initial" for the
    session-start listing and "delta" for later changes."""

    ts_ms: int
    kind: str
    skills: List[str] = field(default_factory=list)
    tool_names: List[str] = field(default_factory=list)
    agent_types: List[str] = field(default_factory=list)
    rules_files: List[str] = field(default_factory=list)  # basenames only (local)
    pending_mcp: List[str] = field(default_factory=list)
    failed_mcp: List[str] = field(default_factory=list)
    removed: List[str] = field(default_factory=list)
    readded: List[str] = field(default_factory=list)
    listing_bytes: Dict[str, int] = field(default_factory=dict)  # name -> bytes of its listing line
    tool_schema_bytes: Dict[str, int] = field(default_factory=dict)  # server -> bytes of tool lines


@dataclass
class InBandAsset:
    """An asset whose content appeared in the log itself, so it can be hashed exactly without the
    filesystem (rules files via nested memory; invoked skill bodies)."""

    kind: str  # "rules_file" | "skill_body"
    name: str  # basename or skill name (local only)
    content_sha256: str
    byte_len: int
    ts_ms: int


@dataclass
class SessionFacts:
    """Everything extracted from one session file (plus its children). Local-only names and ids
    live here; the aggregate step is the only thing that turns this into egress."""

    ref: SessionRef
    harness_version: str = "unknown"
    entrypoint: str = "unknown"
    permission_mode: str = "unknown"
    effort: str = "unknown"
    models: Dict[str, int] = field(default_factory=dict)  # model -> response count
    first_ts_ms: Optional[int] = None
    last_ts_ms: Optional[int] = None
    user_turns: int = 0
    tool_calls: List[ToolCall] = field(default_factory=list)
    usages: Dict[str, Usage] = field(default_factory=dict)
    loaded_events: List[LoadedSetEvent] = field(default_factory=list)
    in_band_assets: List[InBandAsset] = field(default_factory=list)
    compactions: int = 0
    last_stop_reason: Optional[str] = None
    children: List["SessionFacts"] = field(default_factory=list)
    lines_seen: int = 0
    lines_unknown_type: int = 0
    bytes_read: int = 0
    parse_errors: int = 0
    truncated: bool = False
    # Harness-native attribution markers per MCP server name (assistant lines that name the server
    # that produced the response); local names, counts only.
    mcp_attribution_counts: Dict[str, int] = field(default_factory=dict)
    # Local-only forbids harvested while parsing (never egress; fed to the gate checker).
    forbids: Dict[str, set] = field(default_factory=dict)

    def note_forbid(self, bucket: str, value: Optional[str]) -> None:
        if value:
            self.forbids.setdefault(bucket, set()).add(str(value))


class Source(Protocol):
    """A harness source. Implementations must be non-blocking readers: open read-only, stream
    line by line, never hold a file open across calls, and never write into the harness's
    directories."""

    harness: str

    def discover(self, root: str, window_days: int, now_ms: int) -> List[SessionRef]: ...

    def read(self, ref: SessionRef, cursor: Optional[Cursor] = None) -> Tuple[SessionFacts, Cursor]: ...


def iter_lines(path: str, start_offset: int = 0, max_bytes: Optional[int] = None) -> Iterator[Tuple[int, bytes]]:
    """Stream complete lines from `path` starting at `start_offset`. Yields (end_offset, line)
    where end_offset is the byte offset just past the line's newline, so it can be persisted as
    a cursor. A trailing partial line (no newline) is not yielded. Memory is bounded by the
    longest single line, never by the file size."""
    read = 0
    with open(path, "rb") as fh:
        if start_offset:
            fh.seek(start_offset)
        offset = start_offset
        for line in fh:
            if not line.endswith(b"\n"):
                break
            offset += len(line)
            read += len(line)
            yield offset, line
            if max_bytes is not None and read >= max_bytes:
                break
