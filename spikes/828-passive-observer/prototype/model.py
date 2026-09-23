"""Intermediate types shared by extract → attribute → aggregate → rank.

Local-only fields (names, session keys, shares) never egress: aggregate.py is the only module that
produces the wire envelope, and it emits only what telemetry-field-gate.json lists.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

from sources.base import InBandAsset, LoadedSetEvent

ASSET_SKILL = "skill"
ASSET_MCP_SERVER = "mcp_server"
ASSET_AGENT = "agent"
ASSET_RULES_FILE = "rules_file"
ASSET_PROMPT = "prompt"
DIRECT_CAPABLE_TYPES = (ASSET_SKILL, ASSET_MCP_SERVER, ASSET_AGENT)

TIER_DIRECT = "direct"
TIER_LOADED = "loaded"
TIER_INFERRED = "inferred"

KEY_CONTENT = "content_hash"
KEY_DESCRIPTOR = "descriptor_hash"
KEY_NAME = "name_hash"

BINDING_EXACT = "harness_log_exact"
BINDING_MTIME = "mtime_proven"
BINDING_UNPROVEN = "unproven"
BINDING_NA = "not_applicable"


@dataclass
class InvocationObs:
    """One explicit invocation of an asset inside a run (a Skill call, an MCP tool call resolved
    to its server, or a sub-agent spawn resolved to its agent type)."""

    asset_type: str
    name: str  # local only
    ts_ms: int
    latency_ms: Optional[int] = None  # None for async spawns and unpaired calls
    failure_class: Optional[str] = None
    is_async: bool = False
    corroborated: bool = False  # a harness-native attribution marker agreed
    child_tokens_total: Optional[int] = None  # exact token total of a linked child run (agents)


@dataclass
class RunFacts:
    """Per-run derived facts, harness-neutral. Produced by extract.extract()."""

    session_key: str  # local only; HMAC'd into run_id by aggregate
    harness: str
    harness_version: str
    entrypoint_class: str
    effort: str
    permission_mode: str
    model: str  # allowlisted or "other"
    observed_day: str  # UTC day of first_ts_ms
    first_ts_ms: int
    last_ts_ms: int
    run_outcome: str
    turns: int = 0
    tool_calls: int = 0
    tool_failures: int = 0
    user_denials: int = 0
    subagent_runs: int = 0
    compactions: int = 0
    unpaired_tool_uses: int = 0
    repeated_tool_calls: int = 0
    tokens: Dict[str, Optional[int]] = field(default_factory=dict)  # keys as in the envelope
    tokens_basis: str = "none"
    tokens_by_model: Dict[str, Dict[str, Optional[int]]] = field(default_factory=dict)  # model -> envelope buckets
    mcp_corroborations: Dict[str, int] = field(default_factory=dict)  # server name -> harness attribution markers (local)
    tool_class_shares: Dict[str, float] = field(default_factory=dict)  # local only; taskcat input
    invocations: List[InvocationObs] = field(default_factory=list)
    loaded_events: List[LoadedSetEvent] = field(default_factory=list)
    in_band_assets: List[InBandAsset] = field(default_factory=list)
    lines_seen: int = 0
    lines_unknown_type: int = 0
    bytes_read: int = 0
    parse_errors: int = 0
    truncated: bool = False
    forbids: Dict[str, set] = field(default_factory=dict)  # local only; fed to the gate checker


@dataclass(frozen=True)
class AssetKey:
    asset_id: str  # hex64
    asset_type: str
    key_basis: str
    name: str  # local only (display + dynamic forbids)
    binding: str = BINDING_NA


@dataclass
class Segment:
    """A stretch of a run with one loaded set. A new segment starts only when the settle rule in
    attribute.py says the loaded set genuinely changed."""

    index: int
    start_ts_ms: int
    end_ts_ms: int
    loaded_set_basis: str
    asset_keys: List[AssetKey] = field(default_factory=list)
    bom_version: str = ""  # sha256 of sorted asset_ids


@dataclass
class AssetObservation:
    key: AssetKey
    tier: str
    direct_evidence_available: bool
    invocations: List[InvocationObs] = field(default_factory=list)
    context_cost_est: Optional[Tuple[int, str]] = None  # (tokens, method)
    harness_corroborations: Optional[int] = None


@dataclass
class AttributedRun:
    run: RunFacts
    segments: List[Segment]
    observations: Dict[int, List[AssetObservation]]  # segment index -> observations
    name_map: Dict[str, str] = field(default_factory=dict)  # asset_id -> display name (local only)
