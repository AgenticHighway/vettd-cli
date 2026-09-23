//! Text rendering of a [`RankResult`].
//!
//! Port of the rendering half of `spikes/828-passive-observer/prototype/rank.py`. Every line the
//! report is built from lives in [`COPY`], so the copy can be reviewed as copy and the lint in
//! [`crate::observe::lint_copy`] can reject causal phrasing over the whole table at once. A
//! renderer that built sentences inline would put the product's voice beyond review;
//! `render_uses_only_copy_templates` is what keeps that from happening again.
//!
//! Two things this module must never do. It must not name a local asset when scrubbing — the
//! display name falls back to `type:asset_id[:12]`. And it must not imply causation: these figures
//! are counts of what co-occurred, and the footer says so on every report.
//!
//! Cost is derived here and nowhere else. It is never stored and never transmitted; the price table
//! is a dated display resource, compiled in so a report needs no network and no config.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde_json::Value;

use crate::observe::rank::{
    evidence_state, AssetRow, RankResult, Signal, FLOOR_RATE_ORDER, FLOOR_RATE_SHOW, OBSERVED,
    PRICED_BUCKETS,
};

/// The dated display price table, compiled in so rendering needs no filesystem.
const PRICES_JSON: &str = include_str!("../../resources/observe-prices.json");

static DEFAULT_PRICES: LazyLock<Option<Value>> =
    LazyLock::new(|| serde_json::from_str(PRICES_JSON).ok());

/// Every template the output is built from.
///
/// Copied verbatim from the prototype, including the U+2013 EN DASH inside intervals. No causal
/// verb, no currency symbol, and every template that names a rate carries the word "observed" —
/// the lint's hedge rule cannot see `{n}` as a number, so the hedge has to be in the prose.
pub(crate) const COPY: [(&str, &str); 31] = [
    ("header", "Observed asset evidence for task: {task}"),
    (
        "stratum",
        "Stratum: harness={harness} model={model} task_category={category} ({runs} runs over {days} observed days)",
    ),
    (
        "stratum_note",
        "The task category was read from the stated task with a keyword table; other categories are listed as context, never merged in.",
    ),
    (
        "pooled",
        "Pooled in this view (recorded, not stratified): effort, permission_mode, entrypoint_class, day.",
    ),
    ("models_pooled", "Models pooled in this view: {models}"),
    (
        "empty",
        "No runs in this stratum yet. Nothing is ranked; the empty view is the expected state until evidence accrues.",
    ),
    (
        "pooled_categories",
        "No runs observed in task category {category}; this view pools every task category in this harness ({runs} runs). Read it as context for the stated task, not as evidence for it.",
    ),
    (
        "context_pooled",
        "Task categories included in this pooled view: {items}",
    ),
    (
        "invalid_rows",
        "{count} asset rows skipped: more non-successes than calls, an inconsistent record.",
    ),
    (
        "section_ranked",
        "Ranked by the upper bound of the 95% interval on the observed non-success rate, ascending (n >= {floor} calls):",
    ),
    (
        "section_early",
        "Early evidence, shown with its interval but not ordered ({lo} <= n < {hi} calls):",
    ),
    (
        "never_invoked",
        "Loaded in these runs but never invoked ({count} assets: {by_type}): no invocation evidence; listed in the payload, not ranked.",
    ),
    (
        "section_insufficient",
        "Not enough evidence yet (sorted by calls seen; never interleaved with the ranked list):",
    ),
    (
        "section_loaded",
        "Loaded-only assets (rules files, prompts): context-cost estimate only, no non-success figure applies:",
    ),
    (
        "row_rate",
        "{rank:>3}. {name}  tier={tier} state={state}  {k} non-successes in {n} calls (95% interval {lo}\u{2013}{hi}) over {runs} runs{extras}",
    ),
    (
        "row_early",
        "  -  {name}  tier={tier} state={state}  {k} non-successes in {n} calls (95% interval {lo}\u{2013}{hi}) over {runs} runs{extras}",
    ),
    (
        "row_insufficient",
        "  -  {name}  tier={tier} state={state}  {k} non-successes in {n} calls; needs {needs} more calls for an interval{extras}",
    ),
    (
        "row_loaded",
        "  -  {name}  tier={tier} state={state}  context cost est. {cost} tokens ({methods}) in {runs} runs",
    ),
    (
        "row_loaded_no_cost",
        "  -  {name}  tier={tier} state={state}  no context-cost basis in {runs} runs",
    ),
    (
        "latency",
        "; latency mean {mean} ms in {n} paired calls ({state})",
    ),
    ("latency_state", "; latency {state} ({n} paired calls)"),
    (
        "tokens",
        "; child tokens mean {mean} in {n} exactly attributed runs ({state})",
    ),
    ("tokens_state", "; child tokens {state}"),
    (
        "excluded",
        "; {denied} user denials and {interrupted} interruptions excluded from the count",
    ),
    (
        "context",
        "Context, other task categories in this harness (not merged): {items}",
    ),
    ("context_item", "{category} {runs} runs"),
    (
        "cost_header",
        "Cost (display-time derivation, not stored), from tokens in this stratum and the price table dated {date}:",
    ),
    ("cost_line", "  {model}: USD {amount} over {runs} runs"),
    (
        "cost_no_price",
        "  {model}: no price entry in the table dated {date} ({runs} runs; tokens counted, cost not derived)",
    ),
    (
        "cost_unavailable",
        "Cost (display-time derivation, not stored): price table unavailable, nothing derived.",
    ),
    (
        "footer",
        "Every figure above is an observation from harness logs on this machine, not a causal claim.",
    ),
];

/// The template registered under `key`. Panics on an unknown key, which is a programming error:
/// every call site is a literal in this file and the panic is unreachable at runtime.
pub(crate) fn copy(key: &str) -> &'static str {
    COPY.iter()
        .find(|(name, _)| *name == key)
        .map(|(_, template)| *template)
        .unwrap_or_else(|| panic!("unknown copy template {key:?}"))
}

/// Substitute `{name}` placeholders in a [`COPY`] template.
///
/// Supports the one alignment spec the templates use, `{name:>width}`, so `row_rate`'s rank column
/// lines up. An unfilled placeholder is left in place rather than silently dropped: a report with a
/// visible `{extras}` is a bug someone will notice, whereas a missing clause is one they will not.
fn fill(template: &str, args: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}').map(|i| open + i) else {
            break;
        };
        let spec = &rest[open + 1..close];
        let (name, width) = match spec.split_once(":>") {
            Some((name, w)) => (name, w.parse::<usize>().ok()),
            None => (spec, None),
        };
        match args.iter().find(|(key, _)| *key == name) {
            Some((_, value)) => match width {
                Some(width) => out.push_str(&format!("{value:>width$}")),
                None => out.push_str(value),
            },
            None => out.push_str(&rest[open..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Shorthand for a `(name, value)` pair with a `Display` value.
fn arg<T: std::fmt::Display>(name: &str, value: T) -> (&str, String) {
    (name, value.to_string())
}

/// The local display name, or `type:asset_id[:12]` when scrubbing and the name is not public.
pub(crate) fn display_name(
    row: &AssetRow,
    names: &BTreeMap<String, String>,
    scrub: bool,
    public_names: &BTreeSet<String>,
) -> String {
    match names.get(&row.asset_id) {
        // Show the name when not scrubbing, or when the operator listed it as public. Anything
        // else falls through to the hashed form — including a name we simply do not know.
        Some(full) if !scrub || public_names.contains(full) => full.clone(),
        _ => format!(
            "{}:{}",
            row.asset_type,
            &row.asset_id[..12.min(row.asset_id.len())]
        ),
    }
}

fn pct(value: f64) -> String {
    format!("{:.1}%", value * 100.0)
}

/// A mean formatted the way Python's `round()` does it.
///
/// `round_ties_even`, not `round`: Python rounds a half to even and Rust's `round` rounds half away
/// from zero, so `2.5` is `2` there and would be `3` here. The golden report is compared byte for
/// byte, so the difference is not cosmetic.
fn mean(sum: i64, n: u64) -> i64 {
    (sum as f64 / n as f64).round_ties_even() as i64
}

/// The latency, child-token and excluded-count clauses appended to a rate row.
fn extras(row: &AssetRow) -> String {
    let mut parts = String::new();
    let ln = row.latency.n;
    let state = evidence_state(Signal::Latency, Some(ln), true);
    if state == OBSERVED {
        parts.push_str(&fill(
            copy("latency"),
            &[
                arg("mean", mean(row.latency.sum, ln)),
                arg("n", ln),
                arg("state", state),
            ],
        ));
    } else {
        parts.push_str(&fill(
            copy("latency_state"),
            &[arg("state", state), arg("n", ln)],
        ));
    }
    if row.asset_type == "agent" {
        let stats = row.tokens_attributed;
        let state = evidence_state(Signal::Tokens, stats.map(|s| s.n), true);
        match stats.filter(|_| state == OBSERVED) {
            Some(stats) => parts.push_str(&fill(
                copy("tokens"),
                &[
                    arg("mean", mean(stats.sum, stats.n)),
                    arg("n", stats.n),
                    arg("state", state),
                ],
            )),
            None => parts.push_str(&fill(copy("tokens_state"), &[arg("state", state)])),
        }
    }
    if row.user_denied > 0 || row.interrupted > 0 {
        parts.push_str(&fill(
            copy("excluded"),
            &[
                arg("denied", row.user_denied),
                arg("interrupted", row.interrupted),
            ],
        ));
    }
    parts
}

fn rate_row(template: &str, row: &AssetRow, name: &str, index: usize) -> String {
    fill(
        template,
        &[
            arg("rank", index),
            arg("name", name),
            arg("tier", &row.tier),
            arg("state", row.rate_state),
            arg("k", row.k),
            arg("n", row.n),
            arg("lo", pct(row.lo)),
            arg("hi", pct(row.hi)),
            arg("runs", row.runs),
            arg("extras", extras(row)),
        ],
    )
}

fn loaded_row(row: &AssetRow, name: &str) -> String {
    let backing = row.context_cost_tokens.map(|_| row.context_cost_runs);
    let state = evidence_state(Signal::Count, backing, true);
    match row.context_cost_tokens {
        None => fill(
            copy("row_loaded_no_cost"),
            &[
                arg("name", name),
                arg("tier", &row.tier),
                arg("state", state),
                arg("runs", row.loaded_runs),
            ],
        ),
        Some(tokens) => fill(
            copy("row_loaded"),
            &[
                arg("name", name),
                arg("tier", &row.tier),
                arg("state", state),
                arg("cost", tokens),
                arg("methods", row.context_cost_methods.join(",")),
                arg("runs", row.context_cost_runs),
            ],
        ),
    }
}

/// The cost lines: tokens times the dated table, derived here and stored nowhere.
fn cost_lines(result: &RankResult, prices: Option<&Value>) -> Vec<String> {
    let Some(prices) = prices else {
        return vec![copy("cost_unavailable").to_string()];
    };
    let date = prices["as_of"].as_str().unwrap_or("unknown").to_string();
    let table = &prices["per_million_tokens"];
    let mut lines = vec![fill(copy("cost_header"), &[arg("date", &date)])];
    for (model, tokens) in &result.tokens_by_model {
        let runs = result.models.get(model).copied().unwrap_or(0);
        let Some(per_model) = table[model].as_object() else {
            lines.push(fill(
                copy("cost_no_price"),
                &[arg("model", model), arg("date", &date), arg("runs", runs)],
            ));
            continue;
        };
        let amount: f64 = PRICED_BUCKETS
            .iter()
            .map(|bucket| {
                let count = tokens.get(*bucket).copied().unwrap_or(0) as f64;
                let price = per_model
                    .get(*bucket)
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                count * price
            })
            .sum::<f64>()
            / 1_000_000.0;
        lines.push(fill(
            copy("cost_line"),
            &[
                arg("model", model),
                arg("amount", format!("{amount:.2}")),
                arg("runs", runs),
            ],
        ));
    }
    lines
}

/// Render `result` using the compiled-in price table.
pub(crate) fn render(result: &RankResult, scrub: bool, public_names: &BTreeSet<String>) -> String {
    render_with_prices(result, scrub, public_names, DEFAULT_PRICES.as_ref())
}

/// Render `result` against an explicit price table; `None` means the table was unavailable.
pub(crate) fn render_with_prices(
    result: &RankResult,
    scrub: bool,
    public_names: &BTreeSet<String>,
    prices: Option<&Value>,
) -> String {
    let name = |row: &AssetRow| display_name(row, &result.names, scrub, public_names);
    let mut lines = vec![
        fill(copy("header"), &[arg("task", &result.task)]),
        fill(
            copy("stratum"),
            &[
                arg("harness", &result.harness),
                arg("model", result.model.as_deref().unwrap_or("all")),
                arg("category", result.task_category),
                arg("runs", result.run_count),
                arg("days", result.day_count),
            ],
        ),
        copy("stratum_note").to_string(),
        copy("pooled").to_string(),
    ];
    if result.model.is_none() && result.models.len() > 1 {
        let models = result
            .models
            .iter()
            .map(|(m, n)| format!("{m} ({n} runs)"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(fill(copy("models_pooled"), &[arg("models", models)]));
    }
    if result.pooled_categories {
        lines.push(fill(
            copy("pooled_categories"),
            &[
                arg("category", result.task_category),
                arg("runs", result.run_count),
            ],
        ));
    }
    if result.run_count == 0 {
        lines.push(copy("empty").to_string());
    }
    if result.invalid_rows > 0 {
        lines.push(fill(
            copy("invalid_rows"),
            &[arg("count", result.invalid_rows)],
        ));
    }
    push_sections(&mut lines, result, &name);
    if !result.context.is_empty() {
        let items = result
            .context
            .iter()
            .map(|(category, runs)| {
                fill(
                    copy("context_item"),
                    &[arg("category", category), arg("runs", runs)],
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let template = if result.pooled_categories {
            copy("context_pooled")
        } else {
            copy("context")
        };
        lines.push(fill(template, &[arg("items", items)]));
    }
    lines.extend(cost_lines(result, prices));
    lines.push(copy("footer").to_string());
    lines.join("\n") + "\n"
}

/// The ranked, early, insufficient, never-invoked and loaded-only sections, in report order.
fn push_sections(
    lines: &mut Vec<String>,
    result: &RankResult,
    name: &impl Fn(&AssetRow) -> String,
) {
    if !result.ranked.is_empty() {
        lines.push(fill(
            copy("section_ranked"),
            &[arg("floor", FLOOR_RATE_ORDER)],
        ));
        for (index, row) in result.ranked.iter().enumerate() {
            lines.push(rate_row(copy("row_rate"), row, &name(row), index + 1));
        }
    }
    if !result.early.is_empty() {
        lines.push(fill(
            copy("section_early"),
            &[arg("lo", FLOOR_RATE_SHOW), arg("hi", FLOOR_RATE_ORDER)],
        ));
        for row in &result.early {
            lines.push(rate_row(copy("row_early"), row, &name(row), 0));
        }
    }
    let seen: Vec<&AssetRow> = result.insufficient.iter().filter(|r| r.n > 0).collect();
    let never: Vec<&AssetRow> = result.insufficient.iter().filter(|r| r.n == 0).collect();
    if !seen.is_empty() {
        lines.push(copy("section_insufficient").to_string());
        for row in seen {
            lines.push(fill(
                copy("row_insufficient"),
                &[
                    arg("name", name(row)),
                    arg("tier", &row.tier),
                    arg("state", row.rate_state),
                    arg("k", row.k),
                    arg("n", row.n),
                    arg("needs", row.needs),
                    arg("extras", extras(row)),
                ],
            ));
        }
    }
    if !never.is_empty() {
        let mut by_type: BTreeMap<&str, u64> = BTreeMap::new();
        for row in &never {
            *by_type.entry(row.asset_type.as_str()).or_insert(0) += 1;
        }
        let rendered = by_type
            .iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(fill(
            copy("never_invoked"),
            &[arg("count", never.len()), arg("by_type", rendered)],
        ));
    }
    if !result.loaded_only.is_empty() {
        lines.push(copy("section_loaded").to_string());
        for row in &result.loaded_only {
            lines.push(loaded_row(row, &name(row)));
        }
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
