"""RunFacts -> AttributedRun (spike #828, CONTRACTS.md "attribute.py").

Turns a run's loaded-set events, in-band assets and invocations into segments (one per settled
loaded set), one AssetKey per loaded asset per segment, and one AssetObservation per key. Names
stay local (AssetKey.name, name_map); only hashes reach aggregate.

Choices the contract leaves open, stated here:
  - Key precedence when several bases apply: in-band body (harness_log_exact) > local tree/file
    (mtime rule) > descriptor > name_hash. The in-band hash is of what the harness injected, the
    tree hash is of the directory now; they are different preimages, so an invoked skill and a
    listed-only copy of the same skill get different asset_ids.
  - Agents with a local `<claude_home>/agents/<type>.md` get a content_hash of that file under the
    mtime rule (D4: "agents/rules/prompts = file hash"); "anything else -> name_hash" applies only
    when no local file exists.
  - descriptor_hash rows carry binding `not_applicable`: a descriptor is configuration, and the
    mtime rule speaks about content the harness loaded.
  - `harness_corroborations` is a count for agents with at least one invocation in the segment and
    None otherwise (a listed-only agent has nothing to corroborate).
  - The asset dir's max mtime includes directory entries, so a file deleted after the listing
    still moves the binding to `unproven`.
  - Filesystem basis (no in-band listing, e.g. Codex) seeds segment 0 with every asset the index
    knows for that harness; invoked assets are always members of the segment they were invoked in.
"""
from __future__ import annotations

import hashlib
import hmac
import json
import os
import re
import tomllib
from dataclasses import dataclass
from typing import Dict, Iterable, List, Optional, Set, Tuple

from model import (
    ASSET_AGENT,
    ASSET_MCP_SERVER,
    ASSET_RULES_FILE,
    ASSET_SKILL,
    BINDING_EXACT,
    BINDING_MTIME,
    BINDING_NA,
    BINDING_UNPROVEN,
    KEY_CONTENT,
    KEY_DESCRIPTOR,
    KEY_NAME,
    TIER_INFERRED,
    AssetKey,
    AssetObservation,
    AttributedRun,
    InvocationObs,
    RunFacts,
    Segment,
)
from sources.base import HARNESS_CLAUDE_CODE, HARNESS_CODEX, InBandAsset, LoadedSetEvent

BASIS_HARNESS_LOG = "harness_log"
BASIS_FILESYSTEM = "filesystem"
BASIS_NONE = "none"

# Harness built-ins are not assets: their spawns count in run counts only (CONTRACTS.md).
BUILTIN_AGENT_TYPES = frozenset(
    {"Explore", "Plan", "general-purpose", "claude", "Bash", "statusline-setup", "claude-code-guide", "output-style-setup"}
)
SECRET_FLAGS = frozenset({"--api-key", "--token", "--password", "--secret", "-k", "--bearer"})
_BEARER_RE = re.compile(r"(?:^|[^A-Za-z0-9])(?:sk|ghp|gho|ghu|ghs|xox[abp]|AKIA|ah|ntn)[_-][A-Za-z0-9_-]{8,}")
_JWT_RE = re.compile(r"eyJ[A-Za-z0-9_-]{10,}\.")
_OPAQUE_RE = re.compile(r"^(?=.*[A-Za-z])(?=.*[0-9])[A-Za-z0-9_+/=-]{32,}$")


# -- hashing primitives --------------------------------------------------------------------------


def canonical_json(obj: object) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def name_hash(secret: bytes, asset_type: str, name: str) -> str:
    """HMAC-SHA256(secret, "<type>:<name>"): a pseudonym with no cross-device meaning."""
    return hmac.new(secret, f"{asset_type}:{name}".encode("utf-8"), "sha256").hexdigest()


def bom_version(asset_ids: Iterable[str]) -> str:
    """sha256 over the sorted, de-duplicated asset ids — identical to aggregate.bom_version_for,
    so a segment's bom_version always equals the hash of the bom[] entry that is emitted."""
    return sha256_hex(",".join(sorted(set(asset_ids))).encode("utf-8"))


def mcp_server_of(tool_name: str) -> Optional[str]:
    """`mcp__<server>__<tool>` -> server; None for anything else."""
    parts = tool_name.split("__")
    if tool_name.startswith("mcp__") and len(parts) >= 3 and parts[1]:
        return parts[1]
    return None


# -- MCP descriptors -----------------------------------------------------------------------------


def is_secret_shaped(token: str) -> bool:
    return bool(_BEARER_RE.search(token) or _JWT_RE.search(token) or _OPAQUE_RE.match(token))


def _looks_like_path(token: str) -> bool:
    return "/" in token or "\\" in token


def strip_args(args: object) -> List[str]:
    """Args minus path-shaped tokens, secret-shaped tokens, and the value after a secret flag.
    The flag itself stays (it is part of how the server is invoked, not what it is given)."""
    out: List[str] = []
    drop_next = False
    for raw in (args if isinstance(args, list) else []):
        token = str(raw)
        if drop_next:
            drop_next = False
            continue
        flag, sep, _ = token.partition("=")
        if flag in SECRET_FLAGS:
            out.append(flag)
            drop_next = not sep
            continue
        if _looks_like_path(token) or is_secret_shaped(token):
            continue
        out.append(token)
    return out


def canonical_descriptor(raw: dict) -> dict:
    """The stripped descriptor whose canonical JSON is hashed (D4). `command` is the basename or,
    for url servers, the host class; env keeps names only."""
    url = raw.get("url")
    if isinstance(url, str) and url.strip():
        transport = "http"
        command = "https" if url.strip().lower().startswith("https://") else "http"
    else:
        transport = "stdio"
        command = re.split(r"[\\/]", str(raw.get("command") or ""))[-1]
    env = raw.get("env")
    return {
        "transport": transport,
        "command": command,
        "args": strip_args(raw.get("args")),
        "env_names": sorted(str(k) for k in env) if isinstance(env, dict) else [],
    }


def descriptor_hash(raw: dict) -> str:
    return sha256_hex(canonical_json(canonical_descriptor(raw)).encode("utf-8"))


# -- filesystem index ----------------------------------------------------------------------------


@dataclass(frozen=True)
class LocalAsset:
    content_hash: str
    max_mtime_ms: int


class FsIndex:
    """Lazily hashed, read-only view of the local asset files of one or two harness homes. Each
    asset is hashed at most once per instance. Unreadable entries are skipped (fail-open): a
    permission error degrades that asset to name_hash, never the run."""

    def __init__(self, claude_home: Optional[str] = None, codex_home: Optional[str] = None) -> None:
        self.claude_home = claude_home
        self.codex_home = codex_home
        self._loaded = False
        self._skills: Dict[str, LocalAsset] = {}
        self._agents: Dict[str, LocalAsset] = {}
        self._mcp: Dict[str, Dict[str, str]] = {HARNESS_CLAUDE_CODE: {}, HARNESS_CODEX: {}}

    def skill(self, name: str) -> Optional[LocalAsset]:
        self._load()
        return self._skills.get(name)

    def agent(self, name: str) -> Optional[LocalAsset]:
        self._load()
        return self._agents.get(name)

    def mcp_descriptor(self, harness: str, name: str) -> Optional[str]:
        self._load()
        return self._mcp.get(harness, {}).get(name)

    def listed(self, harness: str) -> Dict[str, Set[str]]:
        """Every asset name the filesystem knows for `harness` (the filesystem-basis loaded set)."""
        self._load()
        out: Dict[str, Set[str]] = {ASSET_MCP_SERVER: set(self._mcp.get(harness, {}))}
        if harness == HARNESS_CLAUDE_CODE:
            out[ASSET_SKILL] = set(self._skills)
            out[ASSET_AGENT] = set(self._agents)
        return out

    def _load(self) -> None:
        if self._loaded:
            return
        self._loaded = True
        if self.claude_home:
            self._skills = _index_skills(os.path.join(self.claude_home, "skills"))
            self._agents = _index_agents(os.path.join(self.claude_home, "agents"))
            for fname in (".claude.json", "settings.json"):
                for name, raw in _json_servers(os.path.join(self.claude_home, fname)).items():
                    self._mcp[HARNESS_CLAUDE_CODE].setdefault(name, descriptor_hash(raw))
        if self.codex_home:
            for name, raw in _toml_servers(os.path.join(self.codex_home, "config.toml")).items():
                self._mcp[HARNESS_CODEX].setdefault(name, descriptor_hash(raw))


def _mtime_ms(path: str) -> int:
    return os.stat(path).st_mtime_ns // 1_000_000


def _tree_asset(root: str) -> LocalAsset:
    """sha256 over the sorted (relative posix path, sha256(file)) pairs of every regular file under
    `root`, plus the max mtime over files and directory entries."""
    pairs: List[List[str]] = []
    mtimes: List[int] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        mtimes.append(_mtime_ms(dirpath))
        for fn in sorted(filenames):
            path = os.path.join(dirpath, fn)
            if not os.path.isfile(path):
                continue
            with open(path, "rb") as fh:
                digest = sha256_hex(fh.read())
            pairs.append([os.path.relpath(path, root).replace(os.sep, "/"), digest])
            mtimes.append(_mtime_ms(path))
    pairs.sort()
    return LocalAsset(sha256_hex(canonical_json(pairs).encode("utf-8")), max(mtimes))


def _index_skills(root: str) -> Dict[str, LocalAsset]:
    out: Dict[str, LocalAsset] = {}
    if not os.path.isdir(root):
        return out
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        if "SKILL.md" not in filenames:
            continue
        try:
            out.setdefault(os.path.basename(dirpath), _tree_asset(dirpath))
        except OSError:
            continue
    return out


def _index_agents(root: str) -> Dict[str, LocalAsset]:
    out: Dict[str, LocalAsset] = {}
    if not os.path.isdir(root):
        return out
    for entry in sorted(os.listdir(root)):
        path = os.path.join(root, entry)
        if not entry.endswith(".md") or not os.path.isfile(path):
            continue
        try:
            with open(path, "rb") as fh:
                out[entry[:-3]] = LocalAsset(sha256_hex(fh.read()), _mtime_ms(path))
        except OSError:
            continue
    return out


def _servers(table: object) -> Dict[str, dict]:
    if not isinstance(table, dict):
        return {}
    return {str(k): v for k, v in table.items() if isinstance(v, dict)}


def _json_servers(path: str) -> Dict[str, dict]:
    try:
        with open(path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        return {}
    return _servers(data.get("mcpServers") if isinstance(data, dict) else None)


def _toml_servers(path: str) -> Dict[str, dict]:
    try:
        with open(path, "rb") as fh:
            data = tomllib.load(fh)
    except (OSError, ValueError):  # TOMLDecodeError is a ValueError
        return {}
    return _servers(data.get("mcp_servers"))


# -- segments (settle rule) ----------------------------------------------------------------------


class _SegState:
    """One segment under construction: names loaded in it by asset type, MCP membership at tool
    granularity (a server is loaded while it has at least one tool), the harness timestamp each
    name was listed at, and the byte counts behind the context-cost estimates."""

    def __init__(self, index: int, start_ts: int) -> None:
        self.index = index
        self.start_ts = start_ts
        self.names: Dict[str, Set[str]] = {}
        self.mcp_tools: Dict[str, Set[str]] = {}
        self.listed_ts: Dict[Tuple[str, str], int] = {}
        self.listing_bytes: Dict[str, int] = {}
        self.schema_bytes: Dict[str, int] = {}
        self.mcp_corroborations: Dict[str, int] = {}  # server name -> harness attribution markers (local)

    def fork(self, index: int, start_ts: int) -> "_SegState":
        nxt = _SegState(index, start_ts)
        nxt.mcp_corroborations = self.mcp_corroborations
        nxt.names = {t: set(v) for t, v in self.names.items()}
        nxt.mcp_tools = {s: set(v) for s, v in self.mcp_tools.items()}
        nxt.listed_ts = dict(self.listed_ts)
        nxt.listing_bytes = dict(self.listing_bytes)
        nxt.schema_bytes = dict(self.schema_bytes)
        return nxt

    def add(self, asset_type: str, name: str, ts: Optional[int] = None) -> None:
        self.names.setdefault(asset_type, set()).add(name)
        if ts is not None:
            self.listed_ts.setdefault((asset_type, name), ts)

    def absorb(self, ev: LoadedSetEvent) -> None:
        for n in ev.skills:
            self.add(ASSET_SKILL, n, ev.ts_ms)
        for n, b in ev.listing_bytes.items():
            self.listing_bytes[n] = self.listing_bytes.get(n, 0) + b
        for n in ev.rules_files:
            self.add(ASSET_RULES_FILE, n, ev.ts_ms)
        for n in ev.agent_types:
            if n not in BUILTIN_AGENT_TYPES:
                self.add(ASSET_AGENT, n, ev.ts_ms)
        for n in ev.removed:
            self._mcp_tool(n, ev.ts_ms, present=False)
        for n in ev.tool_names + ev.readded:
            self._mcp_tool(n, ev.ts_ms, present=True)
        for s, b in ev.tool_schema_bytes.items():
            self.schema_bytes[s] = self.schema_bytes.get(s, 0) + b

    def _mcp_tool(self, tool_name: str, ts: int, present: bool) -> None:
        server = mcp_server_of(tool_name)
        if server is None:
            return
        tools = self.mcp_tools.setdefault(server, set())
        if present:
            tools.add(tool_name)
            self.listed_ts.setdefault((ASSET_MCP_SERVER, server), ts)
        else:
            tools.discard(tool_name)

    def members(self) -> List[Tuple[str, str]]:
        out = {(t, n) for t, names in self.names.items() for n in names}
        out |= {(ASSET_MCP_SERVER, s) for s, tools in self.mcp_tools.items() if tools}
        return sorted(out)


def folds(ev: LoadedSetEvent, prior_pending: Set[str]) -> bool:
    """The settle rule as published: a delta folds into the current segment when it removes and
    re-adds nothing and every added name is `mcp__<S>__*` for an S the harness had earlier
    reported as pending (an async MCP connect completing, not a config change)."""
    if ev.removed or ev.readded or ev.skills or ev.agent_types or ev.rules_files:
        return False
    return all(mcp_server_of(n) in prior_pending for n in ev.tool_names)


def settle(events: List[LoadedSetEvent], first_ts: int) -> List[_SegState]:
    """`initial` events never split; a `delta` splits unless `folds` says otherwise."""
    segs = [_SegState(0, first_ts)]
    pending: Set[str] = set()
    for ev in events:
        if ev.kind != "initial" and not folds(ev, pending):
            segs.append(segs[-1].fork(len(segs), ev.ts_ms))
        segs[-1].absorb(ev)
        pending.update(ev.pending_mcp)
    return segs


def _segment_for(segs: List[_SegState], ts_ms: int) -> _SegState:
    for seg in reversed(segs):
        if seg.start_ts <= ts_ms:
            return seg
    return segs[0]


# -- attribution ---------------------------------------------------------------------------------


def attribute(run: RunFacts, fs_index: FsIndex, secret: bytes) -> AttributedRun:
    basis = _basis(run, fs_index)
    segs = settle(run.loaded_events, run.first_ts_ms)
    for _seg in segs:
        _seg.mcp_corroborations = run.mcp_corroborations
    if basis == BASIS_FILESYSTEM:
        for asset_type, names in fs_index.listed(run.harness).items():
            for name in names:
                segs[0].add(asset_type, name)
    in_band: Dict[Tuple[str, str], InBandAsset] = {}
    for asset in run.in_band_assets:
        asset_type = ASSET_RULES_FILE if asset.kind == "rules_file" else ASSET_SKILL
        in_band.setdefault((asset_type, asset.name), asset)
        _segment_for(segs, asset.ts_ms).add(asset_type, asset.name)
    invs: Dict[Tuple[int, str, str], List[InvocationObs]] = {}
    for inv in run.invocations:
        if inv.asset_type == ASSET_AGENT and inv.name in BUILTIN_AGENT_TYPES:
            continue
        seg = _segment_for(segs, inv.ts_ms)
        seg.add(inv.asset_type, inv.name)
        invs.setdefault((seg.index, inv.asset_type, inv.name), []).append(inv)

    segments: List[Segment] = []
    observations: Dict[int, List[AssetObservation]] = {}
    name_map: Dict[str, str] = {}
    for i, st in enumerate(segs):
        end_ts = segs[i + 1].start_ts if i + 1 < len(segs) else max(st.start_ts, run.last_ts_ms)
        obs = [_observe(run.harness, t, n, st, fs_index, secret, in_band.get((t, n)), invs.get((st.index, t, n), []))
               for t, n in st.members()]
        obs.sort(key=lambda o: o.key.asset_id)
        for o in obs:
            name_map[o.key.asset_id] = f"{o.key.asset_type}:{o.key.name}"
        keys = [o.key for o in obs]
        segments.append(Segment(index=st.index, start_ts_ms=st.start_ts, end_ts_ms=end_ts, loaded_set_basis=basis,
                                asset_keys=keys, bom_version=bom_version(k.asset_id for k in keys)))
        observations[st.index] = obs
    return AttributedRun(run=run, segments=segments, observations=observations, name_map=name_map)


def _basis(run: RunFacts, fs_index: FsIndex) -> str:
    if run.loaded_events:
        return BASIS_HARNESS_LOG
    if any(fs_index.listed(run.harness).values()):
        return BASIS_FILESYSTEM
    return BASIS_NONE


def _observe(harness: str, asset_type: str, name: str, st: _SegState, fs_index: FsIndex, secret: bytes,
             band: Optional[InBandAsset], inv_list: List[InvocationObs]) -> AssetObservation:
    """Every row is `inferred` in this prototype (historical read, filesystem-now hashes);
    direct_evidence_available says whether a production collector could have attributed Direct."""
    key = _key_for(harness, asset_type, name, st, fs_index, secret, band)
    corroborations = sum(1 for inv in inv_list if inv.corroborated) if asset_type == ASSET_AGENT and inv_list else None
    if asset_type == ASSET_MCP_SERVER and inv_list and st.mcp_corroborations.get(name) is not None:
        corroborations = st.mcp_corroborations[name]  # assistant lines the harness itself attributed to this server
    return AssetObservation(key=key, tier=TIER_INFERRED, direct_evidence_available=bool(inv_list),
                            invocations=inv_list, context_cost_est=_context_cost(asset_type, name, st, band),
                            harness_corroborations=corroborations)


def _key_for(harness: str, asset_type: str, name: str, st: _SegState, fs_index: FsIndex, secret: bytes,
             band: Optional[InBandAsset]) -> AssetKey:
    if band is not None and asset_type in (ASSET_SKILL, ASSET_RULES_FILE):
        return AssetKey(band.content_sha256, asset_type, KEY_CONTENT, name, BINDING_EXACT)
    local: Optional[LocalAsset] = None
    if harness == HARNESS_CLAUDE_CODE and asset_type == ASSET_SKILL:
        local = fs_index.skill(name)
    elif harness == HARNESS_CLAUDE_CODE and asset_type == ASSET_AGENT:
        local = fs_index.agent(name)
    if local is not None:
        return AssetKey(local.content_hash, asset_type, KEY_CONTENT, name,
                        _mtime_binding(local, st.listed_ts.get((asset_type, name))))
    if asset_type == ASSET_MCP_SERVER:
        digest = fs_index.mcp_descriptor(harness, name)
        if digest:
            return AssetKey(digest, asset_type, KEY_DESCRIPTOR, name, BINDING_NA)
    return AssetKey(name_hash(secret, asset_type, name), asset_type, KEY_NAME, name, BINDING_NA)


def _mtime_binding(local: LocalAsset, listed_ts: Optional[int]) -> str:
    """mtime_proven only when the whole asset dir is older than the harness's listing timestamp;
    without a listing timestamp (filesystem basis) nothing binds the hash to what was loaded."""
    if listed_ts is not None and local.max_mtime_ms < listed_ts:
        return BINDING_MTIME
    return BINDING_UNPROVEN


def _context_cost(asset_type: str, name: str, st: _SegState, band: Optional[InBandAsset]) -> Optional[Tuple[int, str]]:
    if asset_type == ASSET_SKILL and name in st.listing_bytes:
        return (st.listing_bytes[name] // 4, "listing_bytes_div4")
    if asset_type == ASSET_RULES_FILE and band is not None:
        return (band.byte_len // 4, "file_bytes_div4")
    if asset_type == ASSET_MCP_SERVER and name in st.schema_bytes:
        return (st.schema_bytes[name] // 4, "tool_schema_bytes_div4")
    return None
