//! Tests for [`super`], ported from
//! `spikes/828-passive-observer/prototype/tests/test_extract.py`.
//!
//! Inputs are [`SessionFacts`] / [`ToolCall`] / [`Usage`] values built here; no source module is
//! involved, so these prove extract's rules independently of any parser. Every id, name,
//! fingerprint and number is invented.
//!
//! The one exception is `fixture_home_run_facts_match_the_python_oracle`, which runs the real
//! reader over the committed fixture home and pins the numbers the Python prototype produced from
//! the same bytes.
//!
//! Each test states what it proves and what it cannot prove.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::observe::types::{LoadedSetEvent, SessionKind, SessionRef, TokenTotals, Usage};

/// One invocation flattened for comparison: type, name, stamp, latency, async, corroborated, tokens.
type InvocationRow<'a> = (&'a str, &'a str, i64, Option<i64>, bool, bool, Option<i64>);

/// 2026-03-10T12:00:00Z, invented (`test_extract.py:31`).
const T0: i64 = 1_773_144_000_000;
const NOW: i64 = T0 + 3_600_000;
const FP_A: &str = "fp-alpha";
const FP_B: &str = "fp-beta";

const FAILURE_TOOL_ERROR: &str = FAILURE_CLASSES[0];
const FAILURE_TIMEOUT: &str = FAILURE_CLASSES[1];

/// `test_extract.py:37-39`. `meta` is `(key, value)` pairs so the call sites read like the Python's
/// dict literal.
fn session_ref(
    key: &str,
    kind: SessionKind,
    parent: Option<&str>,
    meta: &[(&str, &str)],
) -> SessionRef {
    SessionRef {
        path: format!("fixture/{key}.ndjson").into(),
        harness: "claude_code".to_string(),
        session_key: key.to_string(),
        kind,
        parent_key: parent.map(str::to_string),
        child_meta: meta
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    }
}

/// `test_extract.py:42-47`: a main transcript that started at `T0`, ran a minute and ended its turn.
/// Callers overwrite the fields they are testing, which is what the Python's `**over` does.
fn session() -> SessionFacts {
    SessionFacts {
        ref_: session_ref("fx-main", SessionKind::Main, None, &[]),
        first_ts_ms: Some(T0),
        last_ts_ms: Some(T0 + 60_000),
        last_stop_reason: Some("end_turn".to_string()),
        ..Default::default()
    }
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// `test_extract.py:53-56`: a paired call at `T0` that took a second, with a fresh tool-use id.
fn call_full(name: &str, ts: i64, fp: &str, paired: bool, latency: i64) -> ToolCall {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    ToolCall {
        tool_use_id: format!("tu-{seq:03}"),
        name: name.to_string(),
        ts_ms: ts,
        input_fingerprint: fp.to_string(),
        result_ts_ms: if paired { Some(ts + latency) } else { None },
        ..Default::default()
    }
}

fn call(name: &str) -> ToolCall {
    call_full(name, T0, FP_A, true, 1_000)
}

fn failed(name: &str, class: &str) -> ToolCall {
    ToolCall {
        is_error: Some(true),
        failure_class: Some(class.to_string()),
        ..call(name)
    }
}

/// `test_extract.py:59-60`.
fn usage(mid: &str, inp: i64, out: i64) -> Usage {
    Usage {
        message_id: mid.to_string(),
        model: "claude-sonnet-5".to_string(),
        ts_ms: T0,
        input_tokens: inp,
        output_tokens: out,
        ..Default::default()
    }
}

fn usages(list: Vec<Usage>) -> BTreeMap<String, Usage> {
    list.into_iter()
        .map(|u| (u.message_id.clone(), u))
        .collect()
}

/// `test_extract.py:63-70`: a child transcript linked to its parent by `toolUseId`.
fn child(key: &str, tool_use_id: &str, corroborated: bool) -> SessionFacts {
    let mut meta = vec![("agentType", "fx-reviewer"), ("toolUseId", tool_use_id)];
    if corroborated {
        meta.push(("corroborated", "true"));
    }
    SessionFacts {
        ref_: session_ref(key, SessionKind::Child, Some("fx-main"), &meta),
        first_ts_ms: Some(T0 + 5_000),
        last_ts_ms: Some(T0 + 50_000),
        ..session()
    }
}

fn models(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs
        .iter()
        .map(|(name, n)| ((*name).to_string(), *n))
        .collect()
}

fn names(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

fn agents(run: &RunFacts) -> Vec<&InvocationObs> {
    run.invocations
        .iter()
        .filter(|i| i.asset_type == ASSET_AGENT)
        .collect()
}

// -- Tokens --------------------------------------------------------------------------------------

/// A usage keyed by the same provider message id in the parent and in a child is summed once for the
/// run (150/15, not 250/25), and the child-only message is still added. Double-counting a streamed
/// response logged twice would inflate every token-derived cost estimate downstream. Cannot prove
/// that real transcripts ever share message ids — the rule is defensive.
#[test]
fn dedupe_across_parent_and_child_sharing_message_ids() {
    let shared = usage("m-shared", 100, 10);
    let mut c = child("agent-c1", "tu-spawn", true);
    c.usages = usages(vec![shared.clone(), usage("m-child", 50, 5)]);
    let mut facts = session();
    facts.usages = usages(vec![shared]);
    facts.children = vec![c];
    let run = extract(&facts, NOW);
    assert_eq!((run.tokens.input, run.tokens.output), (Some(150), Some(15)));
    assert_eq!(run.tokens_basis, "harness_usage");
}

/// With no usage records the basis is `none`, input/output are `0` and every provider-specific
/// bucket is absent rather than zero — the cloud must be able to tell "this provider has no cache"
/// from "this run read nothing from cache". Cannot prove that the envelope keeps absence as JSON
/// null (`envelope.rs`).
#[test]
fn no_usage_means_basis_none_and_null_buckets() {
    let run = extract(&session(), NOW);
    assert_eq!(run.tokens_basis, "none");
    assert_eq!((run.tokens.input, run.tokens.output), (Some(0), Some(0)));
    assert_eq!(run.tokens.cache_creation, None);
    assert_eq!(run.tokens.cache_read, None);
    assert_eq!(run.tokens.cached_input, None);
    assert_eq!(run.tokens.thinking, None);
    assert_eq!(run.tokens.reasoning, None);
}

/// A bucket one response reports and another omits sums only the reported value, and a bucket
/// nobody reports stays absent — which is how Claude-style and Codex-style usages coexist in one
/// total without inventing zeros. Cannot prove the provider semantics of each bucket.
#[test]
fn nullable_bucket_sums_only_reported_values() {
    let mut facts = session();
    facts.usages = usages(vec![
        usage("m1", 0, 0),
        Usage {
            cache_read: Some(5),
            ..usage("m2", 0, 0)
        },
    ]);
    let run = extract(&facts, NOW);
    assert_eq!(run.tokens.cache_read, Some(5));
    assert_eq!(run.tokens.cached_input, None);
}

// -- Subagents -----------------------------------------------------------------------------------

/// The parent's `Agent` invocation carries the linked child's exact total
/// (300+40+0+1000 = 1340), is corroborated when the child transcript said so, and `subagent_runs`
/// counts the child; a second `Agent` call with no linked child gets no total and no corroboration.
/// This is what makes a delegated run's cost attributable to the agent that spawned it. Cannot prove
/// that the source links `meta.json` `toolUseId` correctly — that is the source's own test.
#[test]
fn child_tokens_total_attached_to_parent_agent_invocation() {
    let spawn = ToolCall {
        agent_type: Some("fx-reviewer".to_string()),
        child_key: Some("agent-c1".to_string()),
        is_async: true,
        ..call("Agent")
    };
    let orphan = ToolCall {
        agent_type: Some("fx-writer".to_string()),
        ..call_full("Agent", T0 + 10, FP_A, true, 1_000)
    };
    let mut c = child("agent-c1", &spawn.tool_use_id, true);
    c.usages = usages(vec![
        Usage {
            cache_read: Some(1_000),
            ..usage("m-c1", 300, 40)
        },
        Usage {
            thinking: Some(7),
            ..usage("m-c2", 0, 0)
        },
    ]);
    let mut facts = session();
    facts.tool_calls = vec![spawn, orphan];
    facts.children = vec![c];
    let run = extract(&facts, NOW);
    let agents = agents(&run);
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].child_tokens_total, Some(1_340));
    assert!(agents[0].corroborated);
    // An async spawn's result is only an ack, so it has no duration the harness clock can vouch for.
    assert_eq!(agents[0].latency_ms, None);
    assert_eq!(agents[1].child_tokens_total, None);
    assert!(!agents[1].corroborated);
    assert_eq!(run.subagent_runs, 1);
}

/// A child whose meta `toolUseId` equals the parent's `Agent` tool-use id is linked even when the
/// parent result carried no `agentId` (D4 linkage), and a child with no usage yields no total rather
/// than a zero — no evidence is not evidence of zero cost. Cannot prove the behaviour when the two
/// linkage keys disagree.
#[test]
fn child_linked_by_meta_tool_use_id_without_child_key() {
    let spawn = ToolCall {
        agent_type: Some("fx-reviewer".to_string()),
        ..call("Agent")
    };
    let c = child("agent-c9", &spawn.tool_use_id, false);
    let mut facts = session();
    facts.tool_calls = vec![spawn];
    facts.children = vec![c];
    let run = extract(&facts, NOW);
    let agent = &run.invocations[0];
    assert_eq!(agent.child_tokens_total, None);
    assert!(!agent.corroborated);
    // The child ended with end_turn, so there is nothing to charge the agent with.
    assert_eq!(agent.failure_class, None);
}

/// The child's outcome, not the parent's spawn ack, classifies the agent invocation: an interrupted
/// child is `interrupted`, a truncated child is `unknown`, and a parent-level denial wins over any
/// child state. None of the three is rate-bearing, so a sub-agent's fate can never inflate an
/// asset's non-success rate. Cannot prove that a child's completion means the delegated task
/// succeeded.
#[test]
fn child_outcome_becomes_agent_failure_class() {
    let spawns: Vec<ToolCall> = (0..3)
        .map(|i| ToolCall {
            agent_type: Some("fx-reviewer".to_string()),
            ..call_full("Agent", T0 + i, FP_A, true, 1_000)
        })
        .collect();
    let denied = ToolCall {
        is_error: Some(true),
        failure_class: Some(FAILURE_USER_DENIED.to_string()),
        ..spawns[2].clone()
    };
    let mut c1 = child("c-int", &spawns[0].tool_use_id, true);
    c1.tool_calls = vec![call_full("Read", T0, FP_A, false, 0)];
    let mut c2 = child("c-trunc", &spawns[1].tool_use_id, true);
    c2.truncated = true;
    let mut c3 = child("c-denied", &denied.tool_use_id, true);
    c3.truncated = true;
    let mut facts = session();
    facts.tool_calls = vec![spawns[0].clone(), spawns[1].clone(), denied];
    facts.children = vec![c1, c2, c3];
    let run = extract(&facts, NOW);
    let classes: Vec<Option<&str>> = agents(&run)
        .iter()
        .map(|i| i.failure_class.as_deref())
        .collect();
    assert_eq!(
        classes,
        vec![
            Some(FAILURE_INTERRUPTED),
            Some(FAILURE_UNKNOWN),
            Some(FAILURE_USER_DENIED)
        ]
    );
    assert_eq!(run.user_denials, 1);
    assert_eq!(run.tool_failures, 0);
}

/// A child's tool calls count toward the run's `tool_calls`, failures and unpaired counts and its
/// MCP call appears as an invocation, while the child's `user_turns` do not become run turns — a
/// sub-agent's task prompt is not a person taking a turn. Cannot prove the attribution of those
/// invocations to assets (`attribute`).
#[test]
fn child_tool_calls_merge_into_parent_counts_and_invocations() {
    let spawn = ToolCall {
        agent_type: Some("fx-reviewer".to_string()),
        ..call("Agent")
    };
    let mut c = child("c-tools", &spawn.tool_use_id, true);
    c.user_turns = 5;
    c.tool_calls = vec![
        ToolCall {
            server: Some("fxsrv".to_string()),
            ..failed("mcp__fxsrv__lookup", FAILURE_TOOL_ERROR)
        },
        call_full("Read", T0, FP_A, false, 0),
    ];
    let mut facts = session();
    facts.user_turns = 7;
    facts.tool_calls = vec![spawn];
    facts.children = vec![c];
    let run = extract(&facts, NOW);
    assert_eq!(run.tool_calls, 3);
    assert_eq!(run.tool_failures, 1);
    assert_eq!(run.unpaired_tool_uses, 1);
    assert_eq!(run.turns, 7);
    assert!(run
        .invocations
        .iter()
        .any(|i| i.asset_type == ASSET_MCP_SERVER));
}

// -- Tool-call counts ----------------------------------------------------------------------------

/// Only calls whose `(name, fingerprint)` pair occurs at least three times count, and each
/// occurrence is counted (3 -> 3, 4 -> 4), so the number is comparable to `tool_calls`; a pair
/// occurring twice contributes nothing, and the same fingerprint under a different tool name is a
/// different group. Cannot prove that repeats are non-convergence rather than legitimate polling.
#[test]
fn repeated_tool_calls_counts_members_of_groups_of_three_or_more() {
    let base = || {
        let mut calls: Vec<ToolCall> = (0..3)
            .map(|_| call_full("Bash", T0, FP_A, true, 1_000))
            .collect();
        calls.extend((0..2).map(|_| call_full("Bash", T0, FP_B, true, 1_000)));
        calls.push(call_full("Read", T0, FP_A, true, 1_000));
        calls
    };
    let run_with = |calls: Vec<ToolCall>| {
        let mut facts = session();
        facts.tool_calls = calls;
        extract(&facts, NOW).repeated_tool_calls
    };
    assert_eq!(run_with(base()), 3);
    let mut four = base();
    four.push(call_full("Bash", T0, FP_A, true, 1_000));
    assert_eq!(run_with(four), 4);
    assert_eq!(run_with(base()[3..].to_vec()), 0);
}

/// Calls with no result are counted in `unpaired_tool_uses` AND in `tool_calls`, so a coverage gap
/// is visible without silently shrinking the denominator every rate is computed against. Cannot
/// prove why a result is missing (a crash and a still-running call look the same).
#[test]
fn unpaired_tool_uses_counted_and_still_counted_as_calls() {
    let mut facts = session();
    facts.tool_calls = vec![
        call("Read"),
        call_full("Read", T0, FP_A, false, 0),
        call_full("Grep", T0, FP_A, false, 0),
        call("Bash"),
        call("Edit"),
    ];
    let run = extract(&facts, NOW);
    assert_eq!(run.unpaired_tool_uses, 2);
    assert_eq!(run.tool_calls, 5);
}

/// `tool_error` and `timeout` count as tool failures; `user_denied` counts only as a denial;
/// `interrupted` and `unknown` count as neither. That split is the whole reason a denial cannot
/// inflate an asset's published non-success rate: refusing a tool is a fact about the operator.
/// Cannot prove that the source classified each result correctly.
#[test]
fn failure_classes_split_into_tool_failures_and_denials() {
    let mut facts = session();
    facts.tool_calls = vec![
        failed("Bash", FAILURE_TOOL_ERROR),
        failed("Bash", FAILURE_TOOL_ERROR),
        failed("Bash", FAILURE_TIMEOUT),
        failed("Edit", FAILURE_USER_DENIED),
        failed("Edit", FAILURE_INTERRUPTED),
        ToolCall {
            failure_class: Some(FAILURE_UNKNOWN.to_string()),
            ..call("Edit")
        },
    ];
    let run = extract(&facts, NOW);
    assert_eq!(run.tool_failures, 3);
    assert_eq!(run.user_denials, 1);
}

/// `turns` is exactly the source's `user_turns`: tool calls, results, children and loaded events in
/// the tree do not change it. The source is where `isMeta` and tool-result-only lines are excluded,
/// and extract must not second-guess that count or the two layers would disagree about what a turn
/// is. Cannot prove that the source's exclusion rules are right.
#[test]
fn turns_pass_through_without_recount() {
    let mut c = child("c-turns", "tu-none", true);
    c.user_turns = 4;
    c.tool_calls = vec![call("Read")];
    let mut facts = session();
    facts.user_turns = 7;
    facts.tool_calls = vec![call("Read"), call("Bash")];
    facts.children = vec![c];
    facts.loaded_events = vec![LoadedSetEvent {
        ts_ms: T0,
        skills: vec!["fx-skill".to_string()],
        ..Default::default()
    }];
    assert_eq!(extract(&facts, NOW).turns, 7);
    let mut bare = session();
    bare.tool_calls = vec![call("Read")];
    assert_eq!(extract(&bare, NOW).turns, 0);
}

// -- Tool-class shares ---------------------------------------------------------------------------

/// The classification table, in the order `extract` tests it: every name the contract lists lands in
/// its class, unknown names are `other`, `mcp__*` names are `mcp`, and a Codex-style
/// `<server>__<tool>` name with `server` set is `mcp` even without the prefix. MCP is decided first
/// on purpose, so an MCP tool whose suffix reads like an edit is still MCP. Cannot prove that the
/// list of built-in names is complete for future harness versions.
#[test]
fn classification_table() {
    for name in EDIT_TOOLS {
        assert_eq!(tool_class(name, None), "edit", "{name}");
    }
    for name in READ_TOOLS {
        assert_eq!(tool_class(name, None), "read", "{name}");
    }
    for name in SHELL_TOOLS {
        assert_eq!(tool_class(name, None), "shell", "{name}");
    }
    for name in ["Skill", "Agent", "TodoWrite"] {
        assert_eq!(tool_class(name, None), "other", "{name}");
    }
    assert_eq!(tool_class("mcp__fxsrv__lookup", Some("fxsrv")), "mcp");
    assert_eq!(tool_class("mcp__fxsrv__lookup", None), "mcp");
    assert_eq!(tool_class("fxsrv__lookup", Some("fxsrv")), "mcp");
    assert_eq!(tool_class("mcp__fxsrv__Write", None), "mcp");
    // Case-sensitive: `read` is not the `Read` tool.
    assert_eq!(tool_class("read", None), "other");
}

/// Shares are count/total per class over the whole run and sum to 1 (5 edit, 6 read, 3 shell, 2 mcp,
/// 4 other of 20), and every class key is present even when zero, so `taskcat::categorize` always
/// sees the same shape. Cannot prove that these shares are a good proxy for the task — that is
/// taskcat's concern.
#[test]
fn shares_sum_to_one_and_match_counts() {
    let mut calls: Vec<ToolCall> = Vec::new();
    for name in EDIT_TOOLS
        .iter()
        .chain(READ_TOOLS.iter())
        .chain(SHELL_TOOLS.iter())
    {
        calls.push(call(name));
    }
    for name in ["Skill", "Agent", "TodoWrite"] {
        calls.push(call(name));
    }
    calls.push(ToolCall {
        server: Some("fxsrv".to_string()),
        ..call("mcp__fxsrv__lookup")
    });
    calls.push(ToolCall {
        server: Some("fxsrv".to_string()),
        ..call("fxsrv__lookup")
    });
    calls.push(call("fx-unknown-tool"));
    assert_eq!(calls.len(), 20);
    let mut facts = session();
    facts.tool_calls = calls;
    let run = extract(&facts, NOW);
    let total: f64 = run.tool_class_shares.values().sum();
    assert!((total - 1.0).abs() < 1e-9, "{total}");
    let want: BTreeMap<String, f64> = [
        ("edit", 0.25),
        ("read", 0.3),
        ("shell", 0.15),
        ("mcp", 0.1),
        ("other", 0.2),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), *v))
    .collect();
    assert_eq!(run.tool_class_shares, want);
}

/// A run without tool calls has every share `0.0` — which `taskcat` maps to `unspecified` — rather
/// than dividing by zero or omitting the keys. Cannot prove taskcat's mapping (its own tests do).
#[test]
fn no_calls_gives_all_zero_shares() {
    let run = extract(&session(), NOW);
    let keys: Vec<&str> = run.tool_class_shares.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["edit", "mcp", "other", "read", "shell"]);
    assert_eq!(run.tool_class_shares.values().sum::<f64>(), 0.0);
}

// -- Run shape -----------------------------------------------------------------------------------

/// The entrypoint substring rules and their precedence (remote > ide > sdk > cli > unknown),
/// case-insensitively: `sdk-cli` is `sdk` and `remote-cli` is `remote`. Precedence is what stops a
/// remote session being reported as a local CLI one. Cannot prove that real harness entrypoint
/// strings contain these substrings. (The Python also accepts `None`; `""` is the same branch.)
#[test]
fn entrypoint_class_mapping() {
    let cases = [
        ("cli", "cli"),
        ("codex_cli_rs", "cli"),
        ("sdk-cli", "sdk"),
        ("sdk-ts", "sdk"),
        ("vscode", "ide"),
        ("jetbrains-plugin", "ide"),
        ("some-ide", "ide"),
        ("remote-cli", "remote"),
        ("Remote-Control", "remote"),
        ("unknown", "unknown"),
        ("", "unknown"),
    ];
    for (raw, want) in cases {
        assert_eq!(entrypoint_class(raw), want, "{raw:?}");
    }
    let mut facts = session();
    facts.entrypoint = "sdk-cli".to_string();
    assert_eq!(extract(&facts, NOW).entrypoint_class, "sdk");
}

/// Harness camelCase modes map to the gate enum, `plan`/`default`/`auto` pass through, and anything
/// else — including a Codex approval policy — is `unknown`. Matching is exact, so a near-miss
/// spelling degrades to `unknown` instead of guessing a permission posture the operator never chose.
/// Cannot prove that the Codex source pre-maps its approval policies (not ported in v1).
#[test]
fn permission_mode_mapping() {
    let cases = [
        ("acceptEdits", "accept_edits"),
        ("bypassPermissions", "bypass"),
        ("dontAsk", "dont_ask"),
        ("plan", "plan"),
        ("default", "default"),
        ("auto", "auto"),
        ("on-request", "unknown"),
        ("acceptedits", "unknown"),
        ("", "unknown"),
    ];
    for (raw, want) in cases {
        assert_eq!(permission_mode(raw), want, "{raw:?}");
    }
    let mut facts = session();
    facts.permission_mode = "bypassPermissions".to_string();
    assert_eq!(extract(&facts, NOW).permission_mode, "bypass");
}

/// Effort values the gate lists pass through and any other value is `unknown`, so the payload cannot
/// carry an off-enum effort even if a harness invents one. Cannot prove the contract's intent — it
/// is silent on effort and this is a stated choice.
#[test]
fn effort_normalised_to_closed_enum() {
    for raw in ["minimal", "low", "medium", "high", "xhigh"] {
        assert_eq!(effort_class(raw), raw);
    }
    for raw in ["max", "High", ""] {
        assert_eq!(effort_class(raw), "unknown", "{raw:?}");
    }
}

/// Every branch of the outcome decision table and its precedence: truncated beats everything;
/// compacted needs `compactions > 0` AND a non-`end_turn` stop; interrupted (an unpaired call, or a
/// last call marked interrupted) beats completed; completed needs `end_turn`; otherwise unknown. An
/// interrupt that is not the last call does not make a finished run interrupted. Cannot prove that
/// the task itself finished — the outcome never claims that.
#[test]
fn run_outcome_decision_table() {
    let unpaired = || vec![call_full("Read", T0, FP_A, false, 0)];
    let cut = || ToolCall {
        interrupted: true,
        ..call("Bash")
    };
    struct Case {
        truncated: bool,
        compactions: u64,
        stop: Option<&'static str>,
        calls: Vec<ToolCall>,
        want: &'static str,
    }
    let cases = vec![
        Case {
            truncated: true,
            compactions: 1,
            stop: Some("tool_use"),
            calls: unpaired(),
            want: OUTCOME_TRUNCATED,
        },
        Case {
            truncated: false,
            compactions: 1,
            stop: Some("tool_use"),
            calls: unpaired(),
            want: OUTCOME_COMPACTED,
        },
        Case {
            truncated: false,
            compactions: 1,
            stop: None,
            calls: vec![],
            want: OUTCOME_COMPACTED,
        },
        Case {
            truncated: false,
            compactions: 1,
            stop: Some("end_turn"),
            calls: unpaired(),
            want: OUTCOME_INTERRUPTED,
        },
        Case {
            truncated: false,
            compactions: 1,
            stop: Some("end_turn"),
            calls: vec![],
            want: OUTCOME_COMPLETED,
        },
        Case {
            truncated: false,
            compactions: 0,
            stop: Some("end_turn"),
            calls: vec![call("Read"), cut()],
            want: OUTCOME_INTERRUPTED,
        },
        Case {
            truncated: false,
            compactions: 0,
            stop: Some("end_turn"),
            calls: vec![cut(), call("Read")],
            want: OUTCOME_COMPLETED,
        },
        Case {
            truncated: false,
            compactions: 0,
            stop: Some("end_turn"),
            calls: vec![call("Read")],
            want: OUTCOME_COMPLETED,
        },
        Case {
            truncated: false,
            compactions: 0,
            stop: Some("tool_use"),
            calls: vec![],
            want: OUTCOME_UNKNOWN,
        },
        Case {
            truncated: false,
            compactions: 0,
            stop: None,
            calls: vec![],
            want: OUTCOME_UNKNOWN,
        },
    ];
    assert_eq!(cases.len(), 10);
    for (index, case) in cases.into_iter().enumerate() {
        let mut facts = session();
        facts.truncated = case.truncated;
        facts.compactions = case.compactions;
        facts.last_stop_reason = case.stop.map(str::to_string);
        facts.tool_calls = case.calls;
        assert_eq!(extract(&facts, NOW).run_outcome, case.want, "case {index}");
    }
}

/// The dominant model is chosen by response count in the MAIN transcript (a child on another model
/// does not change it — `tokens_by_model` carries the split), then allowlisted: a winning invented
/// provider becomes `other`, a winning `claude-*` passes through, ties break on the smaller name,
/// and no models at all is `other`. The allowlist is what stops a user-named model reaching the
/// wire. Cannot prove that response count is the best notion of a run's dominant model.
#[test]
fn model_is_most_frequent_then_allowlisted() {
    let with_models = |main: &[(&str, u64)], kids: Vec<SessionFacts>| {
        let mut facts = session();
        facts.models = models(main);
        facts.children = kids;
        extract(&facts, NOW).model
    };
    let kid = || {
        let mut c = child("c-model", "tu-none", true);
        c.models = models(&[("claude-sonnet-5", 2)]);
        vec![c]
    };
    assert_eq!(
        with_models(
            &[("claude-sonnet-5", 3), ("fxprovider-custom-9", 5)],
            vec![]
        ),
        "other"
    );
    assert_eq!(
        with_models(&[("claude-sonnet-5", 5), ("gpt-5", 2)], vec![]),
        "claude-sonnet-5"
    );
    assert_eq!(with_models(&[("gpt-5", 2)], kid()), "gpt-5");
    assert_eq!(with_models(&[], kid()), "claude-sonnet-5");
    assert_eq!(
        with_models(&[("gpt-5", 2), ("claude-sonnet-5", 2)], vec![]),
        "claude-sonnet-5"
    );
    assert_eq!(with_models(&[], vec![]), "other");
}

// -- Observed day --------------------------------------------------------------------------------

/// `observed_day` is the UTC calendar day of `first_ts_ms` at the exact boundary: half a second
/// after midnight UTC is that day and one millisecond before midnight UTC is the day before. The day
/// is the cloud's retention key, so a local-time reading would move a record between retention
/// buckets depending on where the machine stands. Cannot prove the non-UTC-host case the way the
/// Python does by setting `TZ`: [`utc_day`] takes no timezone at all, which is the stronger
/// guarantee. Cannot prove anything about a source that emits non-UTC-normalised timestamps.
#[test]
fn observed_day_is_utc_day_of_first_timestamp() {
    // 2026-03-10T00:00:00Z.
    let midnight = 1_773_100_800_000_i64;
    for (ts, want) in [(midnight + 500, "2026-03-10"), (midnight - 1, "2026-03-09")] {
        let mut facts = session();
        facts.first_ts_ms = Some(ts);
        facts.last_ts_ms = Some(ts + 10);
        assert_eq!(extract(&facts, NOW).observed_day, want, "{ts}");
    }
}

/// First/last span the whole tree (a child ending after the parent extends `last_ts_ms`), and a
/// session with no harness timestamp at all uses `now_ms` for both rather than failing — a run must
/// still land on some day, or it would be silently dropped. Cannot prove that a timestamp-less
/// session is worth emitting at all.
#[test]
fn span_covers_children_and_falls_back_to_now() {
    let mut c = child("c-late", "tu-none", true);
    c.last_ts_ms = Some(T0 + 90_000);
    let mut facts = session();
    facts.children = vec![c];
    let run = extract(&facts, NOW);
    assert_eq!((run.first_ts_ms, run.last_ts_ms), (T0, T0 + 90_000));
    let mut bare = session();
    bare.first_ts_ms = None;
    bare.last_ts_ms = None;
    let run = extract(&bare, NOW);
    assert_eq!((run.first_ts_ms, run.last_ts_ms), (NOW, NOW));
}

// -- Invocations ---------------------------------------------------------------------------------

/// A `Skill` call yields a skill invocation, an `mcp__` call an mcp_server invocation named after
/// the server, an `Agent` call an agent invocation; latency is result minus call for paired
/// synchronous calls and absent for async or unpaired ones; failure classes pass through, and a
/// plain tool call is not an invocation of anything. Cannot prove how `attribute` keys these names
/// (`name_hash` vs `content_hash`).
#[test]
fn skill_mcp_and_agent_invocations_with_latency_rules() {
    let mut facts = session();
    facts.tool_calls = vec![
        ToolCall {
            skill: Some("fx-skill".to_string()),
            ..call_full("Skill", T0, FP_A, true, 250)
        },
        ToolCall {
            skill: Some("fx-skill-body".to_string()),
            is_async: true,
            ..call_full("Skill", T0, FP_A, true, 0)
        },
        ToolCall {
            server: Some("fxsrv".to_string()),
            is_error: Some(true),
            failure_class: Some(FAILURE_TOOL_ERROR.to_string()),
            ..call_full("mcp__fxsrv__lookup", T0, FP_A, false, 0)
        },
        ToolCall {
            agent_type: Some("fx-reviewer".to_string()),
            ..call_full("Agent", T0, FP_A, true, 4_000)
        },
        call("Bash"),
    ];
    let run = extract(&facts, NOW);
    let got: Vec<(&str, &str, Option<i64>, Option<&str>)> = run
        .invocations
        .iter()
        .map(|i| {
            (
                i.asset_type.as_str(),
                i.name.as_str(),
                i.latency_ms,
                i.failure_class.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (ASSET_SKILL, "fx-skill", Some(250), None),
            (ASSET_SKILL, "fx-skill-body", None, None),
            (ASSET_MCP_SERVER, "fxsrv", None, Some(FAILURE_TOOL_ERROR)),
            (ASSET_AGENT, "fx-reviewer", Some(4_000), None),
        ]
    );
}

// -- Bookkeeping ---------------------------------------------------------------------------------

/// Forbid buckets union across parent and children — the gate checker must see a child's ids too, or
/// a leaked child name would go unnoticed — coverage counters and compactions sum over the tree, and
/// loaded events come from the parent only, because the parent's listing is the set the segments are
/// cut from. Cannot prove that the checker consumes these buckets (`gate.rs`).
#[test]
fn forbids_and_coverage_merge_over_tree() {
    let mut c = child("c-fb", "tu-none", true);
    c.lines_seen = 4;
    c.bytes_read = 40;
    c.compactions = 1;
    c.lines_unknown_type = 1;
    c.parse_errors = 1;
    c.loaded_events = vec![LoadedSetEvent {
        ts_ms: T0,
        skills: vec!["fx-child-skill".to_string()],
        ..Default::default()
    }];
    c.forbids = BTreeMap::from([
        ("agent_ids".to_string(), names(&["agent-fx-child"])),
        ("loaded_set_names".to_string(), names(&["fx-child-skill"])),
    ]);
    let mut facts = session();
    facts.lines_seen = 10;
    facts.bytes_read = 100;
    facts.compactions = 2;
    facts.children = vec![c];
    facts.loaded_events = vec![LoadedSetEvent {
        ts_ms: T0,
        skills: vec!["fx-skill".to_string()],
        ..Default::default()
    }];
    facts.forbids = BTreeMap::from([
        ("loaded_set_names".to_string(), names(&["fx-skill"])),
        ("harness_session_ids".to_string(), names(&["fx-main"])),
    ]);
    let run = extract(&facts, NOW);
    assert_eq!(
        run.forbids["loaded_set_names"],
        names(&["fx-skill", "fx-child-skill"])
    );
    assert_eq!(run.forbids["agent_ids"], names(&["agent-fx-child"]));
    assert_eq!(
        (
            run.lines_seen,
            run.bytes_read,
            run.lines_unknown_type,
            run.parse_errors
        ),
        (14, 140, 1, 1)
    );
    assert_eq!(run.compactions, 3);
    let skills: Vec<&[String]> = run
        .loaded_events
        .iter()
        .map(|e| e.skills.as_slice())
        .collect();
    assert_eq!(skills, vec![["fx-skill".to_string()].as_slice()]);
    assert_eq!(
        (run.session_key.as_str(), run.harness.as_str()),
        ("fx-main", "claude_code")
    );
}

// -- Differential: the committed fixture home ----------------------------------------------------

/// The real reader over the committed fixture home produces exactly the [`RunFacts`] the Python
/// prototype produces from the same bytes. Every number below was read off
/// `extract.extract(link_children([...])[0], 1_800_000_000_000)` run against
/// `crates/vettd-cli/tests/fixtures/observe/claude_home` with the prototype's own
/// `ClaudeCodeSource`, so this is a differential test with the oracle's answers pinned rather than a
/// restatement of what the Rust happens to do.
///
/// The single deliberate difference is `forbids`: the prototype additionally writes a
/// `_permission_modes` bucket (`claude_code.py:376-382`), which the port keeps as
/// `SessionFacts::mode_counts` instead, so that a closed-enum wire value is not also a dynamic
/// forbid. The assertion below states that absence rather than hiding it.
///
/// Cannot prove the envelope bytes agree — that is Phase 4's byte-identical golden parity test.
#[test]
fn fixture_home_run_facts_match_the_python_oracle() {
    let now = 1_800_000_000_000_i64;
    let runs = fixture_runs(now);
    assert_eq!(runs.len(), 1, "one main transcript with one linked child");
    let run = extract(&runs[0], now);

    assert_eq!(run.session_key, "0f1e2d3c-4b5a-6978-8a9b-0c1d2e3f4a5b");
    assert_eq!(run.harness, "claude_code");
    assert_eq!(run.harness_version, "3.4.5");
    assert_eq!(run.entrypoint_class, "cli");
    assert_eq!(run.effort, "high");
    assert_eq!(run.permission_mode, "accept_edits");
    assert_eq!(run.model, "other");
    assert_eq!(run.observed_day, "2026-08-15");
    assert_eq!(
        (run.first_ts_ms, run.last_ts_ms),
        (1786788000000, 1786788062000)
    );
    assert_eq!(run.run_outcome, OUTCOME_INTERRUPTED);
    assert_eq!((run.turns, run.tool_calls, run.subagent_runs), (2, 9, 1));
    assert_eq!((run.tool_failures, run.user_denials), (1, 1));
    assert_eq!(
        (
            run.compactions,
            run.unpaired_tool_uses,
            run.repeated_tool_calls
        ),
        (1, 1, 0)
    );
    assert_eq!(
        run.tokens,
        TokenTotals {
            input: Some(202),
            output: Some(228),
            cache_creation: Some(20),
            cache_read: Some(1640),
            cached_input: None,
            thinking: Some(14),
            reasoning: None,
        }
    );
    assert_eq!(run.tokens_basis, "harness_usage");
    assert_eq!(
        run.tokens_by_model.keys().collect::<Vec<_>>(),
        vec!["other"]
    );
    assert_eq!(run.tokens_by_model["other"], run.tokens);
    assert!(run.mcp_corroborations.is_empty());
    assert_eq!(
        run.tool_class_shares
            .values()
            .copied()
            .collect::<Vec<f64>>(),
        vec![1.0 / 9.0, 1.0 / 9.0, 3.0 / 9.0, 2.0 / 9.0, 2.0 / 9.0]
    );
    let invoked: Vec<InvocationRow> = run
        .invocations
        .iter()
        .map(|i| {
            (
                i.asset_type.as_str(),
                i.name.as_str(),
                i.ts_ms,
                i.latency_ms,
                i.is_async,
                i.corroborated,
                i.child_tokens_total,
            )
        })
        .collect();
    assert_eq!(
        invoked,
        vec![
            (
                ASSET_MCP_SERVER,
                "srvfx",
                1786788035000,
                Some(1200),
                false,
                false,
                None
            ),
            (
                ASSET_AGENT,
                "fx-reviewer",
                1786788040000,
                None,
                true,
                true,
                Some(28)
            ),
            (
                ASSET_SKILL,
                "skill-alpha",
                1786788045000,
                None,
                true,
                false,
                None
            ),
            (
                ASSET_SKILL,
                "skill-beta",
                1786788050000,
                Some(500),
                false,
                false,
                None
            ),
        ]
    );
    assert!(run.invocations.iter().all(|i| i.failure_class.is_none()));
    let skills: Vec<usize> = run.loaded_events.iter().map(|e| e.skills.len()).collect();
    assert_eq!(skills, vec![0, 0, 2, 0, 0], "main transcript's events only");
    let in_band: Vec<(&str, i64)> = run
        .in_band_assets
        .iter()
        .map(|a| (a.name.as_str(), a.byte_len))
        .collect();
    assert_eq!(in_band, vec![("RULES.md", 45), ("skill-alpha", 52)]);
    assert_eq!(
        (
            run.lines_seen,
            run.lines_unknown_type,
            run.bytes_read,
            run.parse_errors
        ),
        (31, 1, 21554, 1)
    );
    assert!(!run.truncated);
    assert_eq!(
        run.forbids.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            "agent_ids",
            "cwd_and_branches",
            "harness_session_ids",
            "loaded_set_names",
            "message_ids",
            "slugs",
            "tool_use_ids",
        ],
        "the prototype's extra `_permission_modes` bucket is deliberately not reproduced"
    );
    assert_eq!(run.forbids["agent_ids"], names(&["fx1"]));
    assert_eq!(run.forbids["loaded_set_names"].len(), 11);
    assert_eq!(run.forbids["message_ids"].len(), 10);
}

/// Read the committed fixture home through the real Claude Code source, exactly as the pipeline
/// will: discover, read every file from byte 0, then link children to their parents.
fn fixture_runs(now_ms: i64) -> Vec<SessionFacts> {
    use crate::observe::claude_code::{link_children, ClaudeCodeSource};
    use crate::observe::source::Source;
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/observe/claude_home");
    let source = ClaudeCodeSource::with_now_ms(root.clone(), now_ms);
    let refs = source
        .discover(&root, 3650, now_ms)
        .expect("discover the fixture home");
    let facts = refs
        .iter()
        .map(|r| source.read(r, None).expect("read a fixture session").0)
        .collect();
    link_children(facts)
}

/// Invariant: an asset name is a name only when it is non-empty, because the reference tests
/// `if call.skill:` — a truthiness test, not a presence test. A presence test does not merely add a
/// spurious row: when a call carries an empty `skill` alongside a real `server`, it attributes the
/// invocation to the wrong asset *type*, and the empty name then seeds a live `AssetKey` whose
/// `name_hash` reaches `assets[]`. Nothing downstream guards it, on either side, because upstream
/// never emits one.
/// Cannot prove: that the shipped reader cannot produce an empty name — today it cannot, which is
/// why this is latent rather than live.
#[test]
fn an_empty_asset_name_is_not_an_invocation() {
    let mut facts = session();
    let mut empty_skill = call("Skill");
    empty_skill.skill = Some(String::new());
    let mut empty_server = call("mcp__x__t");
    empty_server.server = Some(String::new());
    let mut empty_agent = call("Agent");
    empty_agent.agent_type = Some(String::new());
    facts.tool_calls = vec![empty_skill, empty_server, empty_agent];
    assert!(
        invocations(&facts).is_empty(),
        "an empty name names no asset"
    );

    // The sharp case: an empty higher-precedence name must not shadow a real lower one.
    let mut shadowed = call("mcp__gh__list");
    shadowed.skill = Some(String::new());
    shadowed.server = Some("gh".to_string());
    facts.tool_calls = vec![shadowed];
    let obs = invocations(&facts);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].asset_type, ASSET_MCP_SERVER);
    assert_eq!(obs[0].name, "gh");
}

/// Invariant: an empty parent failure class falls through to the child's, because the reference
/// composes them with a truthiness `or`. Keeping `Some("")` would put a value outside
/// `FAILURE_CLASSES` on an `assets[].signals` key, which the field gate rejects — the run would
/// refuse to emit rather than report a wrong number, but it would still be a self-inflicted refusal.
#[test]
fn an_empty_parent_failure_class_falls_through_to_the_child() {
    let mut facts = session();
    let mut spawn = call("Agent");
    spawn.agent_type = Some("fx-reviewer".to_string());
    spawn.child_key = Some("kid".to_string());
    spawn.failure_class = Some(String::new());
    facts.tool_calls = vec![spawn];
    let mut kid = child("kid", "", false);
    kid.truncated = true;
    facts.children = vec![kid];

    let obs = invocations(&facts);
    assert_eq!(obs.len(), 1);
    assert_eq!(
        obs[0].failure_class.as_deref(),
        Some("unknown"),
        "the child's class must win over an empty parent class"
    );
}
