//! Claude Code: `<root>/projects/<project>/<session>.jsonl`, plus each
//! session's `<session>/subagents/*.jsonl`.
//!
//! One API response is written as several lines (one per content block),
//! and resumed or forked sessions copy earlier lines into new files. So a
//! request is one `requestId` across all files: it belongs to the session
//! whose file holds its earliest line, and its usage is its line with the
//! most output tokens (the last on a tie), because lines are written as
//! the response streams.

use super::{
    intern, jsonl_in, read_lines, split, subdirs, text_chars, ts_of, u64_at, usage_with,
    workspace_of, Found, History, Req, UseEvent,
};
use crate::data::BUCKETS;
use crate::pricing::{self, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const AGENT: &str = "Claude";
const N: usize = BUCKETS.len();

/// Bucket indices into `BUCKETS`.
const FILE_READS: usize = 0;
const SHELL: usize = 1;
const MCP: usize = 3;
const SKILLS: usize = 4;
const INSTRUCTIONS: usize = 5;
const HOOKS: usize = 6;
const PROMPTS: usize = 7;
const OTHER: usize = 8;

fn tool_bucket(name: &str) -> usize {
    match name {
        "Read" | "Grep" | "Glob" => FILE_READS,
        "Bash" | "PowerShell" => SHELL,
        "Skill" => SKILLS,
        _ if name.starts_with("mcp__") => MCP,
        _ => OTHER,
    }
}

/// `mcp__<server>__<tool>` → server.
fn mcp_server(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("mcp__")?;
    rest.split("__").next().filter(|s| !s.is_empty())
}

/// What a file holds, in order: blocks entering context and the requests
/// they lead up to.
enum Event {
    Block(usize, u64),
    Request(String),
}

struct Best {
    /// The earliest line: its time, session and file.
    first: (i64, String, usize),
    output: u64,
    usage: Value,
    model: String,
    subagent: bool,
}

struct Pending {
    request: String,
    kind: &'static str,
    name: String,
}

pub(super) fn read(root: &Path, history: History, found: &mut Found) {
    let projects = root.join("projects");
    // Top-level session files that exist on disk, opened or not: a
    // subagent line naming one of them belongs to it.
    let mut top: Vec<(PathBuf, String)> = Vec::new();
    let mut subagents: Vec<(PathBuf, String)> = Vec::new();
    for project in subdirs(&projects) {
        for file in jsonl_in(&project) {
            let id = file.file_stem().unwrap().to_string_lossy().into_owned();
            top.push((file, id));
        }
        for folder in subdirs(&project) {
            let id = folder.file_name().unwrap().to_string_lossy().into_owned();
            for file in jsonl_in(&folder.join("subagents")) {
                subagents.push((file, id.clone()));
            }
        }
    }
    let known: HashSet<String> = top.iter().map(|(_, id)| id.clone()).collect();
    let files: Vec<(PathBuf, String, bool)> = top
        .into_iter()
        .map(|(f, id)| (f, id, false))
        .chain(subagents.into_iter().map(|(f, id)| (f, id, true)))
        .filter(|(f, _, _)| history.opens(f))
        .collect();
    let mut files = files;
    files.sort_by_cached_key(|(f, _, subagent)| {
        let created = std::fs::metadata(f).and_then(|m| m.created()).ok();
        (*subagent, created, f.clone())
    });

    let mut best: HashMap<String, Best> = HashMap::new();
    let mut events: Vec<Vec<Event>> = Vec::new();
    let mut tools: HashMap<String, String> = HashMap::new();
    let mut mcp_calls: HashMap<String, (String, String)> = HashMap::new();
    let mut skills: Vec<Pending> = Vec::new();
    let mut rejected: HashSet<String> = HashSet::new();

    for (index, (path, folder, is_subagent)) in files.iter().enumerate() {
        let (lines, broken) = read_lines(path);
        if broken {
            found.meta(AGENT, folder).partial = true;
        }
        let mut file_events = Vec::new();
        let mut current: Option<String> = None;
        // invoked_skills seen before any request in this file.
        let mut waiting: Vec<String> = Vec::new();
        for line in &lines {
            let session = match line.get("sessionId").and_then(Value::as_str) {
                Some(id) if *is_subagent && known.contains(id) => id.to_string(),
                _ => folder.clone(),
            };
            if let Some(ws) = line
                .get("cwd")
                .and_then(Value::as_str)
                .and_then(workspace_of)
            {
                found.meta(AGENT, &session).workspace.get_or_insert(ws);
            }
            match line.get("type").and_then(Value::as_str) {
                Some("custom-title") => title(found, &session, line, "customTitle"),
                Some("ai-title") => title(found, &session, line, "aiTitle"),
                Some("assistant") => {
                    let Some(key) = assistant(
                        line,
                        &session,
                        index,
                        *is_subagent,
                        &mut best,
                        &mut rejected,
                        history.since,
                        found,
                    ) else {
                        continue;
                    };
                    if current.as_ref() != Some(&key) {
                        file_events.push(Event::Request(key.clone()));
                        current = Some(key.clone());
                        for name in waiting.drain(..) {
                            skills.push(Pending {
                                request: key.clone(),
                                kind: "skill",
                                name,
                            });
                        }
                    }
                    for block in blocks(line) {
                        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                            continue;
                        }
                        let (Some(id), Some(name)) = (
                            block.get("id").and_then(Value::as_str),
                            block.get("name").and_then(Value::as_str),
                        ) else {
                            continue;
                        };
                        tools.insert(id.to_string(), name.to_string());
                        if name == "Skill" {
                            if let Some(skill) =
                                block.pointer("/input/skill").and_then(Value::as_str)
                            {
                                skills.push(Pending {
                                    request: key.clone(),
                                    kind: "skill",
                                    name: skill.to_string(),
                                });
                            }
                        } else if let Some(server) = mcp_server(name) {
                            mcp_calls
                                .entry(id.to_string())
                                .or_insert((key.clone(), server.to_string()));
                        }
                    }
                }
                Some("user") => match line.pointer("/message/content") {
                    Some(Value::String(text)) => {
                        file_events.push(Event::Block(PROMPTS, text.chars().count() as u64))
                    }
                    Some(Value::Array(items)) => {
                        for block in items {
                            match block.get("type").and_then(Value::as_str) {
                                Some("tool_result") => {
                                    let name = block
                                        .get("tool_use_id")
                                        .and_then(Value::as_str)
                                        .and_then(|id| tools.get(id))
                                        .map_or("", String::as_str);
                                    let chars = block.get("content").map_or(0, text_chars);
                                    file_events.push(Event::Block(tool_bucket(name), chars));
                                }
                                Some("text") => file_events.push(Event::Block(
                                    PROMPTS,
                                    block.get("text").map_or(0, text_chars),
                                )),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                },
                Some("attachment") => {
                    let Some(a) = line.get("attachment") else {
                        continue;
                    };
                    let field = |k: &str| a.get(k).map_or(0, text_chars);
                    let block = match a.get("type").and_then(Value::as_str) {
                        Some("invoked_skills") => {
                            for name in a
                                .get("skills")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(|s| s.get("name").and_then(Value::as_str))
                            {
                                match &current {
                                    Some(key) => skills.push(Pending {
                                        request: key.clone(),
                                        kind: "skill",
                                        name: name.to_string(),
                                    }),
                                    None => waiting.push(name.to_string()),
                                }
                            }
                            Some((SKILLS, field("skills")))
                        }
                        Some("skill_listing") => Some((SKILLS, field("content"))),
                        Some("instructions") => Some((INSTRUCTIONS, field("files"))),
                        Some("file") => Some((INSTRUCTIONS, field("content"))),
                        Some("hook_additional_context") => Some((HOOKS, field("content"))),
                        // prompt_snapshot, deferred_tools_*, hook_success and
                        // reminders: whether they reach the model is
                        // unverified, so they aren't counted.
                        _ => None,
                    };
                    if let Some((bucket, chars)) = block {
                        file_events.push(Event::Block(bucket, chars));
                    }
                }
                _ => {}
            }
        }
        events.push(file_events);
    }
    found.limit_hits += rejected.len() as u32;

    // Estimate token sources per request, from the file that owns it.
    let mut sources: HashMap<String, [u64; N]> = HashMap::new();
    for (index, file_events) in events.iter().enumerate() {
        let mut pending = [0u64; N];
        let mut previous: Option<(u64, u64)> = None;
        for event in file_events {
            match event {
                Event::Block(bucket, chars) => pending[*bucket] += chars,
                Event::Request(key) => {
                    let b = &best[key];
                    let context = u64_at(&b.usage, "input_tokens")
                        + u64_at(&b.usage, "cache_read_input_tokens")
                        + u64_at(&b.usage, "cache_creation_input_tokens");
                    let growth = match previous {
                        None => context,
                        Some((ctx, out)) => context.saturating_sub(ctx + out),
                    };
                    if b.first.2 == index {
                        sources
                            .entry(key.clone())
                            .or_insert(split(growth, &pending));
                    }
                    pending = [0; N];
                    previous = Some((context, b.output));
                }
            }
        }
    }

    for (key, b) in &best {
        let u = &b.usage;
        let write = u64_at(u, "cache_creation_input_tokens");
        let split_5m = u.pointer("/cache_creation/ephemeral_5m_input_tokens");
        let split_1h = u.pointer("/cache_creation/ephemeral_1h_input_tokens");
        let usage = Usage {
            fresh: u64_at(u, "input_tokens"),
            cache_read: u64_at(u, "cache_read_input_tokens"),
            write_5m: split_5m.and_then(Value::as_u64).unwrap_or(0),
            write_1h: split_1h.and_then(Value::as_u64).unwrap_or(0),
            output: b.output,
        };
        let standard = |k: &str| {
            u.get(k)
                .and_then(Value::as_str)
                .is_none_or(|v| v == "standard")
        };
        // A cache write without its 5-minute / 1-hour split can't be
        // priced: the two rates differ.
        let split_known = write == 0 || usage.write_5m + usage.write_1h == write;
        let cost = (split_known && standard("speed") && standard("service_tier"))
            .then(|| pricing::price(&b.model, &usage))
            .flatten();
        let clamp = |v: u64| v.min(u64::from(u32::MAX)) as u32;
        found.requests.push(Req {
            agent: AGENT,
            session: b.first.1.clone(),
            ts: b.first.0,
            worker: if b.subagent { "Subagent" } else { "Main" },
            model: intern(&b.model),
            tokens: [
                clamp(usage.fresh),
                clamp(usage.cache_read),
                clamp(write),
                clamp(usage.output),
            ],
            cost,
            sources: sources.get(key).copied().unwrap_or([0; N]),
        });
    }

    // Skill uses once per request and skill; MCP uses once per call.
    let mut once: HashSet<(String, String)> = HashSet::new();
    for p in skills {
        if let Some(b) = best.get(&p.request) {
            if once.insert((p.request.clone(), p.name.clone())) {
                found.uses.push(UseEvent {
                    agent: AGENT,
                    session: b.first.1.clone(),
                    ts: b.first.0,
                    kind: p.kind,
                    name: p.name,
                    copy: "unknown",
                    estimated: false,
                });
            }
        }
    }
    for (request, server) in mcp_calls.into_values() {
        if let Some(b) = best.get(&request) {
            found.uses.push(UseEvent {
                agent: AGENT,
                session: b.first.1.clone(),
                ts: b.first.0,
                kind: "MCP",
                name: server,
                copy: "unknown",
                estimated: false,
            });
        }
    }
}

fn title(found: &mut Found, session: &str, line: &Value, key: &str) {
    if let Some(t) = line
        .get(key)
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        found.meta(AGENT, session).title = Some(t.to_string());
    }
}

fn blocks(line: &Value) -> impl Iterator<Item = &Value> {
    line.pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Records one assistant line. Returns its request key, or None for
/// placeholder and error lines, which are read only for limit hits.
#[allow(clippy::too_many_arguments)]
fn assistant(
    line: &Value,
    session: &str,
    file: usize,
    in_subagent_file: bool,
    best: &mut HashMap<String, Best>,
    rejected: &mut HashSet<String>,
    since: i64,
    found: &mut Found,
) -> Option<String> {
    let model = line.pointer("/message/model").and_then(Value::as_str);
    let key = line
        .get("requestId")
        .or_else(|| line.pointer("/message/id"))
        .and_then(Value::as_str);
    let ts = ts_of(line);
    let error = line.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true);
    if model == Some("<synthetic>") || error {
        let five_hour = [
            line.get("quotaLimits"),
            line.pointer("/apiErrorParams/rate_limit_info"),
        ]
        .into_iter()
        .flatten()
        .flat_map(|v| match v {
            Value::Array(items) => items.iter().collect::<Vec<_>>(),
            other => vec![other],
        })
        .any(|limit| {
            limit.get("rateLimitType").and_then(Value::as_str) == Some("five_hour")
                && limit.get("status").and_then(Value::as_str) == Some("rejected")
        });
        let id = key.or_else(|| line.get("uuid").and_then(Value::as_str));
        if five_hour && ts.is_some_and(|t| t >= since) {
            if let Some(id) = id {
                rejected.insert(id.to_string());
            }
        }
        return None;
    }
    let (Some(key), Some(ts)) = (key, ts) else {
        found.meta(AGENT, session).missing_at(ts);
        return None;
    };
    let Some(usage) = usage_with(
        line.pointer("/message/usage"),
        &["input_tokens", "output_tokens"],
    ) else {
        found.meta(AGENT, session).missing_at(Some(ts));
        return None;
    };
    let output = u64_at(usage, "output_tokens");
    let subagent =
        in_subagent_file || line.get("isSidechain").and_then(Value::as_bool) == Some(true);
    let entry = best.entry(key.to_string()).or_insert_with(|| Best {
        first: (ts, session.to_string(), file),
        output,
        usage: usage.clone(),
        model: model.unwrap_or("unknown").to_string(),
        subagent,
    });
    if ts < entry.first.0 {
        entry.first = (ts, session.to_string(), file);
        entry.subagent = subagent;
    }
    if output >= entry.output {
        entry.output = output;
        entry.usage = usage.clone();
    }
    Some(key.to_string())
}
