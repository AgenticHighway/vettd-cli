"""Ranked, confidence-tagged display of one envelope for a stated task (spike #828, D5).

Everything here is display logic over the wire envelope plus the local name map; nothing is stored
and nothing egresses. The rules are D5's, presented as display floors rather than statistics:

- `evidence_state` per (asset, signal): observed / early_evidence / insufficient_evidence /
  not_applicable / no_coverage. Insufficient evidence is a state, not a low rank.
- An observed non-success rate (tool_error + timeout over invocations; denials, interruptions and
  unknowns never count) is shown with its 95% Wilson interval from n >= 20 calls and used for
  ordering only from n >= 50, ordered by the interval's UPPER bound ascending with tiebreak
  (hi, -n, asset_id): the conservative rule punishes low n, not high n.
- Below 20 the display is "k non-successes in n calls" with "needs N more", in a separate list
  sorted by n descending, never interleaved with the ranked list.
- Rules files and prompts can never be Direct: they are listed apart with a context-cost estimate
  only and no non-success figure.
- Strata: harness x model x task category. The task category is read from the stated task with a
  keyword table; other categories are shown as context, never merged. effort / permission_mode /
  entrypoint_class / day are pooled with a visible caption (recorded, so a later version can split).
- Cost is a display-time derivation from tokens x the dated price table; it is never stored.

`rank()` reads no files. `render()` reads only the price table it is pointed at. Every string the
output is built from lives in COPY so tests/test_rank.py and lint_copy.py can reject causal phrasing.
"""
from __future__ import annotations

import json
import math
import os
import re
from collections import Counter
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Set, Tuple

from aggregate import Stats
from model import DIRECT_CAPABLE_TYPES, TIER_DIRECT, TIER_INFERRED, TIER_LOADED
from sources.base import RATE_BEARING_FAILURES

FLOORS = {"count": 1, "tokens": 3, "latency": 5, "rate_show": 20, "rate_order": 50}

OBSERVED = "observed"
EARLY = "early_evidence"
INSUFFICIENT = "insufficient_evidence"
NOT_APPLICABLE = "not_applicable"
NO_COVERAGE = "no_coverage"
SIGNALS = ("count", "tokens", "latency", "rate")

CATEGORY_UNSPECIFIED = "unspecified"
# First category whose keyword appears (as a whole word) in the stated task wins; precedence order.
TASK_KEYWORDS: Tuple[Tuple[str, Tuple[str, ...]], ...] = (
    ("mcp_heavy", ("mcp", "connector", "connectors", "integration")),
    ("code_edit", ("edit", "fix", "implement", "refactor", "write", "add", "change", "migrate", "patch")),
    ("code_explore", ("explore", "understand", "explain", "review", "audit", "find", "read", "investigate")),
    ("shell_ops", ("shell", "deploy", "build", "run", "install", "test", "tests")),
)
TIER_RANK = {TIER_DIRECT: 0, TIER_LOADED: 1, TIER_INFERRED: 2}
PRICED_BUCKETS = ("input", "cache_creation", "cache_read", "output")
DEFAULT_PRICES_PATH = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "prices.json")

# Every template the output is built from. No causal verb, no money sign, and every template that
# names a rate says "observed" (lint_copy's hedge rule cannot see "{n}" as a number).
COPY: Dict[str, str] = {
    "header": "Observed asset evidence for task: {task}",
    "stratum": "Stratum: harness={harness} model={model} task_category={category} ({runs} runs over {days} observed days)",
    "stratum_note": "The task category was read from the stated task with a keyword table; other categories are listed as context, never merged in.",
    "pooled": "Pooled in this view (recorded, not stratified): effort, permission_mode, entrypoint_class, day.",
    "models_pooled": "Models pooled in this view: {models}",
    "empty": "No runs in this stratum yet. Nothing is ranked; the empty view is the expected state until evidence accrues.",
    "pooled_categories": "No runs observed in task category {category}; this view pools every task category in this harness ({runs} runs). Read it as context for the stated task, not as evidence for it.",
    "context_pooled": "Task categories included in this pooled view: {items}",
    "invalid_rows": "{count} asset rows skipped: more non-successes than calls, an inconsistent record.",
    "section_ranked": "Ranked by the upper bound of the 95% interval on the observed non-success rate, ascending (n >= {floor} calls):",
    "section_early": "Early evidence, shown with its interval but not ordered ({lo} <= n < {hi} calls):",
    "never_invoked": "Loaded in these runs but never invoked ({count} assets: {by_type}): no invocation evidence; listed in the payload, not ranked.",
    "section_insufficient": "Not enough evidence yet (sorted by calls seen; never interleaved with the ranked list):",
    "section_loaded": "Loaded-only assets (rules files, prompts): context-cost estimate only, no non-success figure applies:",
    "row_rate": "{rank:>3}. {name}  tier={tier} state={state}  {k} non-successes in {n} calls (95% interval {lo}–{hi}) over {runs} runs{extras}",
    "row_early": "  -  {name}  tier={tier} state={state}  {k} non-successes in {n} calls (95% interval {lo}–{hi}) over {runs} runs{extras}",
    "row_insufficient": "  -  {name}  tier={tier} state={state}  {k} non-successes in {n} calls; needs {needs} more calls for an interval{extras}",
    "row_loaded": "  -  {name}  tier={tier} state={state}  context cost est. {cost} tokens ({methods}) in {runs} runs",
    "row_loaded_no_cost": "  -  {name}  tier={tier} state={state}  no context-cost basis in {runs} runs",
    "latency": "; latency mean {mean} ms in {n} paired calls ({state})",
    "latency_state": "; latency {state} ({n} paired calls)",
    "tokens": "; child tokens mean {mean} in {n} exactly attributed runs ({state})",
    "tokens_state": "; child tokens {state}",
    "excluded": "; {denied} user denials and {interrupted} interruptions excluded from the count",
    "context": "Context, other task categories in this harness (not merged): {items}",
    "context_item": "{category} {runs} runs",
    "cost_header": "Cost (display-time derivation, not stored), from tokens in this stratum and the price table dated {date}:",
    "cost_line": "  {model}: USD {amount} over {runs} runs",
    "cost_no_price": "  {model}: no price entry in the table dated {date} ({runs} runs; tokens counted, cost not derived)",
    "cost_unavailable": "Cost (display-time derivation, not stored): price table unavailable, nothing derived.",
    "footer": "Every figure above is an observation from harness logs on this machine, not a causal claim.",
}


def wilson(k: int, n: int, z: float = 1.96) -> Tuple[float, float]:
    """Wilson score interval for k non-successes in n calls. n = 0 is the whole range [0, 1]."""
    if n <= 0:
        return (0.0, 1.0)
    p = k / n
    z2 = z * z
    denom = 1.0 + z2 / n
    centre = p + z2 / (2.0 * n)
    margin = z * math.sqrt(p * (1.0 - p) / n + z2 / (4.0 * n * n))
    return (max(0.0, (centre - margin) / denom), min(1.0, (centre + margin) / denom))


def evidence_state(signal: str, n: Optional[int], applicable: bool = True) -> str:
    """State of one signal given how many observations back it. `n=None` means the signal was never
    observable in this stratum (no coverage), which is different from n=0. `applicable=False` is the
    caller saying the signal does not exist for this asset type (a rate for a rules file)."""
    if signal not in SIGNALS:
        raise ValueError(f"unknown signal {signal!r}")
    if not applicable:
        return NOT_APPLICABLE
    if n is None:
        return NO_COVERAGE
    if signal == "rate":
        if n >= FLOORS["rate_order"]:
            return OBSERVED
        return EARLY if n >= FLOORS["rate_show"] else INSUFFICIENT
    return OBSERVED if n >= FLOORS[signal] else INSUFFICIENT


def task_category_for(task: str) -> str:
    words = set(re.findall(r"[a-z]+", task.lower()))
    for category, keywords in TASK_KEYWORDS:
        if words & set(keywords):
            return category
    return CATEGORY_UNSPECIFIED


@dataclass
class AssetRow:
    asset_id: str
    asset_type: str
    tier: str
    direct_evidence_available: bool
    runs: int = 0  # runs with at least one invocation of this asset
    loaded_runs: int = 0  # runs the asset was loaded in, invoked or not
    n: int = 0  # invocations
    k: int = 0  # rate-bearing non-successes (tool_error + timeout)
    user_denied: int = 0
    interrupted: int = 0
    unknown: int = 0
    latency: Dict[str, int] = field(default_factory=lambda: Stats.from_values([]))
    tokens_attributed: Optional[Dict[str, int]] = None
    context_cost_tokens: Optional[int] = None
    context_cost_methods: List[str] = field(default_factory=list)
    context_cost_runs: int = 0
    lo: float = 0.0
    hi: float = 1.0
    rate_state: str = INSUFFICIENT
    needs: int = FLOORS["rate_show"]


@dataclass
class RankResult:
    task: str
    harness: str
    model: Optional[str]
    task_category: str
    names: Dict[str, str]
    run_count: int = 0
    day_count: int = 0
    ranked: List[AssetRow] = field(default_factory=list)
    early: List[AssetRow] = field(default_factory=list)
    insufficient: List[AssetRow] = field(default_factory=list)
    loaded_only: List[AssetRow] = field(default_factory=list)
    context: List[Tuple[str, int]] = field(default_factory=list)  # (task_category, runs) not in the stratum
    models: Dict[str, int] = field(default_factory=dict)  # model -> runs in the stratum
    tokens_by_model: Dict[str, Dict[str, int]] = field(default_factory=dict)
    pooled_categories: bool = False  # True only when the matched task category had no runs
    invalid_rows: int = 0  # rows skipped because their counts were inconsistent


def rank(envelope: dict, name_map: Dict[str, str], task: str, harness: str, model: Optional[str] = None) -> RankResult:
    """Pure: envelope dict + local names -> RankResult. Reads no files."""
    category = task_category_for(task)
    resource = envelope.get("resource", {})
    records = envelope.get("records", []) if resource.get("harness") == harness else []
    in_model = [r for r in records if model is None or model in _models_of(r)]
    stratum = [r for r in in_model if category == CATEGORY_UNSPECIFIED or r["task_category"] == category]
    pooled_categories = category == CATEGORY_UNSPECIFIED and bool(in_model)
    if not stratum and in_model:
        # Nothing observed in the matched task category: pool every category in this harness/model
        # rather than show an empty view — but say so in the header, and never merge silently when
        # the matched stratum does have runs.
        stratum = list(in_model)
        pooled_categories = True
    others = Counter(r["task_category"] for r in in_model if r["task_category"] != category)
    result = RankResult(task=task, harness=harness, model=model, task_category=category, names=dict(name_map),
                        run_count=len(stratum), day_count=len({r["observed_day"] for r in stratum}),
                        context=sorted(others.items()), models=dict(_runs_per_model(stratum)),
                        pooled_categories=pooled_categories)
    result.tokens_by_model = _tokens_by_model(stratum)
    for row in _accumulate(stratum).values():
        _classify(row, result)
    result.ranked.sort(key=lambda r: (r.hi, -r.n, r.asset_id))
    result.early.sort(key=lambda r: r.asset_id)  # an order for determinism, not a ranking
    result.insufficient.sort(key=lambda r: (-r.n, r.asset_id))
    result.loaded_only.sort(key=lambda r: r.asset_id)
    return result


def _models_of(rec: dict) -> Set[str]:
    return {rec["model"]} | {e["model"] for e in (rec.get("tokens_by_model") or [])}


def _runs_per_model(records: List[dict]) -> Counter:
    """Runs in which each model produced tokens (a run with sub-agents on another model counts
    for both), falling back to the run's dominant model when no per-model split was recorded."""
    counts: Counter = Counter()
    for rec in records:
        if rec.get("tokens", {}).get("basis") == "none":
            continue
        entries = rec.get("tokens_by_model") or [{"model": rec["model"]}]
        for model in {e["model"] for e in entries}:
            counts[model] += 1
    return counts


def _tokens_by_model(records: List[dict]) -> Dict[str, Dict[str, int]]:
    out: Dict[str, Dict[str, int]] = {}
    for rec in records:
        if rec.get("tokens", {}).get("basis") == "none":
            continue  # no usage evidence is not zero tokens
        entries = rec.get("tokens_by_model") or [dict(rec["tokens"], model=rec["model"])]
        for entry in entries:
            bucket = out.setdefault(entry["model"], {b: 0 for b in PRICED_BUCKETS})
            for b in PRICED_BUCKETS:
                bucket[b] += entry.get(b) or 0
    return out


def _accumulate(records: List[dict]) -> Dict[str, AssetRow]:
    """Merge every asset row of the stratum by asset_id using the mergeable stats."""
    rows: Dict[str, AssetRow] = {}
    for rec in records:
        for asset in rec["assets"]:
            row = rows.get(asset["asset_id"])
            if row is None:
                row = AssetRow(asset["asset_id"], asset["asset_type"], asset["tier"], False)
                rows[row.asset_id] = row
            _fold(row, asset)
    return rows


def _fold(row: AssetRow, asset: dict) -> None:
    signals = asset["signals"]
    failures = signals["failures"]
    row.loaded_runs += 1
    if signals["invocations"]["n"] > 0:
        row.runs += 1
    row.n += signals["invocations"]["n"]
    row.k += sum(failures[cls] for cls in RATE_BEARING_FAILURES)
    row.user_denied += failures["user_denied"]
    row.interrupted += failures["interrupted"]
    row.unknown += failures["unknown"]
    row.latency = Stats.merge(row.latency, signals["latency_ms"])
    attributed = signals["tokens_attributed"]
    if attributed is not None:
        row.tokens_attributed = attributed if row.tokens_attributed is None else Stats.merge(row.tokens_attributed, attributed)
    cost = signals["context_cost_est"]
    if cost is not None:
        row.context_cost_tokens = (row.context_cost_tokens or 0) + cost["tokens"]
        row.context_cost_runs += 1
        if cost["method"] not in row.context_cost_methods:
            row.context_cost_methods.append(cost["method"])
            row.context_cost_methods.sort()
    if TIER_RANK.get(asset["tier"], 99) < TIER_RANK.get(row.tier, 99):
        row.tier = asset["tier"]
    row.direct_evidence_available = row.direct_evidence_available or bool(asset["direct_evidence_available"])


def _classify(row: AssetRow, result: RankResult) -> None:
    if row.k > row.n:  # more non-successes than calls: an invalid record, never displayed
        result.invalid_rows += 1
        return
    applicable = row.asset_type in DIRECT_CAPABLE_TYPES
    row.rate_state = evidence_state("rate", row.n, applicable)
    row.lo, row.hi = wilson(row.k, row.n)
    row.needs = max(0, FLOORS["rate_show"] - row.n)
    if not applicable:
        result.loaded_only.append(row)
    elif row.rate_state == OBSERVED:
        result.ranked.append(row)
    elif row.rate_state == EARLY:
        result.early.append(row)
    else:
        result.insufficient.append(row)


# -- rendering ----------------------------------------------------------------------------------


def display_name(row: AssetRow, names: Dict[str, str], scrub: bool, public_names: Set[str]) -> str:
    """Local display name, or "<type>:<asset_id prefix>" when scrubbing and the name is not public."""
    full = names.get(row.asset_id)
    if full is None or (scrub and full not in public_names):
        return f"{row.asset_type}:{row.asset_id[:12]}"
    return full


def _pct(value: float) -> str:
    return f"{value * 100:.1f}%"


def _extras(row: AssetRow) -> str:
    parts: List[str] = []
    ln = row.latency["n"]
    state = evidence_state("latency", ln)
    if state == OBSERVED:
        parts.append(COPY["latency"].format(mean=round(row.latency["sum"] / ln), n=ln, state=state))
    else:
        parts.append(COPY["latency_state"].format(state=state, n=ln))
    if row.asset_type == "agent":
        stats = row.tokens_attributed
        state = evidence_state("tokens", None if stats is None else stats["n"])
        if state == OBSERVED:
            parts.append(COPY["tokens"].format(mean=round(stats["sum"] / stats["n"]), n=stats["n"], state=state))
        else:
            parts.append(COPY["tokens_state"].format(state=state))
    if row.user_denied or row.interrupted:
        parts.append(COPY["excluded"].format(denied=row.user_denied, interrupted=row.interrupted))
    return "".join(parts)


def _rate_row(template: str, row: AssetRow, name: str, index: int) -> str:
    return template.format(rank=index, name=name, tier=row.tier, state=row.rate_state, k=row.k, n=row.n,
                           lo=_pct(row.lo), hi=_pct(row.hi), runs=row.runs, extras=_extras(row))


def _loaded_row(row: AssetRow, name: str) -> str:
    state = evidence_state("count", row.context_cost_runs if row.context_cost_tokens is not None else None)
    if row.context_cost_tokens is None:
        return COPY["row_loaded_no_cost"].format(name=name, tier=row.tier, state=state, runs=row.loaded_runs)
    return COPY["row_loaded"].format(name=name, tier=row.tier, state=state, cost=row.context_cost_tokens,
                                     methods=",".join(row.context_cost_methods), runs=row.context_cost_runs)


def _load_prices(path: str) -> Optional[dict]:
    try:
        with open(path, "r", encoding="utf-8") as fh:
            table = json.load(fh)
    except (OSError, ValueError):
        return None
    return table if isinstance(table, dict) else None


def _cost_lines(result: RankResult, prices: Optional[dict]) -> List[str]:
    if prices is None:
        return [COPY["cost_unavailable"]]
    date = str(prices.get("as_of", "unknown"))
    table = prices.get("per_million_tokens") or {}
    lines = [COPY["cost_header"].format(date=date)]
    for model in sorted(result.tokens_by_model):
        runs = result.models.get(model, 0)
        prices_for_model = table.get(model)
        if not isinstance(prices_for_model, dict):
            lines.append(COPY["cost_no_price"].format(model=model, date=date, runs=runs))
            continue
        tokens = result.tokens_by_model[model]
        amount = sum(tokens[b] * float(prices_for_model.get(b) or 0.0) for b in PRICED_BUCKETS) / 1_000_000
        lines.append(COPY["cost_line"].format(model=model, amount=f"{amount:.2f}", runs=runs))
    return lines


def render(result: RankResult, scrub: bool, public_names: Set[str], prices_path: Optional[str] = None) -> str:
    """Text rendering. Reads only the price table; every line comes from a COPY template."""
    name = lambda row: display_name(row, result.names, scrub, public_names)  # noqa: E731
    lines = [COPY["header"].format(task=result.task),
             COPY["stratum"].format(harness=result.harness, model=result.model or "all", category=result.task_category,
                                    runs=result.run_count, days=result.day_count),
             COPY["stratum_note"], COPY["pooled"]]
    if result.model is None and len(result.models) > 1:
        lines.append(COPY["models_pooled"].format(models=", ".join(f"{m} ({n} runs)" for m, n in sorted(result.models.items()))))
    if result.pooled_categories:
        lines.append(COPY["pooled_categories"].format(category=result.task_category, runs=result.run_count))
    if result.run_count == 0:
        lines.append(COPY["empty"])
    if result.invalid_rows:
        lines.append(COPY["invalid_rows"].format(count=result.invalid_rows))
    if result.ranked:
        lines.append(COPY["section_ranked"].format(floor=FLOORS["rate_order"]))
        lines.extend(_rate_row(COPY["row_rate"], row, name(row), i) for i, row in enumerate(result.ranked, start=1))
    if result.early:
        lines.append(COPY["section_early"].format(lo=FLOORS["rate_show"], hi=FLOORS["rate_order"]))
        lines.extend(_rate_row(COPY["row_early"], row, name(row), 0) for row in result.early)
    seen = [row for row in result.insufficient if row.n > 0]
    never = [row for row in result.insufficient if row.n == 0]
    if seen:
        lines.append(COPY["section_insufficient"])
        lines.extend(COPY["row_insufficient"].format(name=name(row), tier=row.tier, state=row.rate_state, k=row.k,
                                                     n=row.n, needs=row.needs, extras=_extras(row))
                     for row in seen)
    if never:
        by_type = ", ".join(f"{n} {t}" for t, n in sorted(Counter(row.asset_type for row in never).items()))
        lines.append(COPY["never_invoked"].format(count=len(never), by_type=by_type))
    if result.loaded_only:
        lines.append(COPY["section_loaded"])
        lines.extend(_loaded_row(row, name(row)) for row in result.loaded_only)
    if result.context:
        items = "; ".join(COPY["context_item"].format(category=c, runs=n) for c, n in result.context)
        lines.append((COPY["context_pooled"] if result.pooled_categories else COPY["context"]).format(items=items))
    lines.extend(_cost_lines(result, _load_prices(prices_path or DEFAULT_PRICES_PATH)))
    lines.append(COPY["footer"])
    return "\n".join(lines) + "\n"
