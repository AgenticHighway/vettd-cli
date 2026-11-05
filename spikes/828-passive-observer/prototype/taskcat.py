"""Task category rule set and model allowlist (spike #828, CONTRACTS.md "taskcat.py").

`categorize` is the published rule set D2 names: a closed category derived from tool-mix shares
alone. It reads no content, and the shares it consumes never egress (RunFacts.tool_class_shares is
local only). The boundaries are the published rules; changing any of them is a new rule set and must
bump RULES_VERSION, because `extractor_version` carries it and a re-extraction under a different rule
set is a different observation.

`allowlist_model` mirrors the gate's `formats.model_allowlist`: anything the regex does not accept
(custom provider names, build strings, None) becomes the literal "other".
"""
from __future__ import annotations

import re
from typing import Dict, Optional

RULES_VERSION = "taskcat-1"

CATEGORY_UNSPECIFIED = "unspecified"
CATEGORY_MCP_HEAVY = "mcp_heavy"
CATEGORY_CODE_EDIT = "code_edit"
CATEGORY_SHELL_OPS = "shell_ops"
CATEGORY_CODE_EXPLORE = "code_explore"
CATEGORY_MIXED = "mixed"

# Published boundaries (inclusive: a share exactly at the boundary is inside the category).
MCP_HEAVY_MIN = 0.5
CODE_EDIT_MIN = 0.25
SHELL_OPS_MIN = 0.5
CODE_EXPLORE_MIN = 0.5

# Kept identical to telemetry-field-gate.json `enums.model`; tests/test_taskcat.py fails if the two
# drift apart. A closed list rather than a prefix pattern: a user-named model such as
# claude-<org>-<project> would pass a pattern and carry a name onto the wire. New model ids become
# "other" until the gate (and this list) are versioned forward together.
KNOWN_MODELS = (
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-opus-5",
    "claude-sonnet-5",
    "claude-haiku-4-5-20251001",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-1",
    "claude-sonnet-4-5",
    "claude-sonnet-4",
    "gpt-5",
    "gpt-5-mini",
    "gpt-5-codex",
    "gpt-5.1",
    "gpt-5.1-codex",
    "gpt-5.2",
    "gpt-4.1",
    "o3",
    "o4-mini",
    "codex-mini-latest",
    "gemini-2.5-pro",
    "gemini-3-pro",
    "other",
)
MODEL_OTHER = "other"


def categorize(shares: Dict[str, float]) -> str:
    """Pure: tool-class shares (class -> fraction of tool calls) -> task category.

    Order is precedence: the first rule whose boundary is met wins, so an mcp-heavy run that also
    edits is `mcp_heavy`, not `code_edit`.
    """
    total = sum(shares.values())
    if total == 0:
        return CATEGORY_UNSPECIFIED
    if shares.get("mcp", 0.0) >= MCP_HEAVY_MIN:
        return CATEGORY_MCP_HEAVY
    if shares.get("edit", 0.0) >= CODE_EDIT_MIN:
        return CATEGORY_CODE_EDIT
    if shares.get("shell", 0.0) >= SHELL_OPS_MIN:
        return CATEGORY_SHELL_OPS
    if shares.get("read", 0.0) >= CODE_EXPLORE_MIN:
        return CATEGORY_CODE_EXPLORE
    return CATEGORY_MIXED


def allowlist_model(raw: Optional[str]) -> str:
    """Pure: the harness-reported model id if it is in KNOWN_MODELS, else MODEL_OTHER."""
    return raw if raw in KNOWN_MODELS else MODEL_OTHER

