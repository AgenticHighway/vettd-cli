//! Codex: `<root>/sessions/YYYY/MM/DD/rollout-*.jsonl` and
//! `<root>/archived_sessions/*.jsonl`, with thread titles from
//! `<root>/session_index.jsonl`.
//!
//! A request is one `token_usage_record` (once per `response_id`). A
//! subagent thread counts in the session at the root of its
//! `parent_thread_id` chain. `cached_input_tokens` is inside
//! `input_tokens` and reasoning is inside `output_tokens`, which the
//! totals confirm; a record where they don't add up marks its session
//! partial.

use super::{
    intern, jsonl_in, read_lines, split, subdirs, text_chars, ts_of, u64_at, usage_with,
    workspace_of, Found, History, Reading, Req, UseEvent,
};
use crate::data::BUCKETS;
use crate::fixture::Window;
use crate::pricing::{self, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const AGENT: &str = "Codex";
const N: usize = BUCKETS.len();
const TOOL_OUTPUT: usize = 2;
const INSTRUCTIONS: usize = 5;
const PROMPTS: usize = 7;
const OTHER: usize = 8;

/// `exec` output stays one bucket: splitting file reads from shell
/// output would need the command text.
fn tool_bucket(name: &str) -> usize {
    match name {
        "exec" | "js" => TOOL_OUTPUT,
        _ => OTHER,
    }
}

struct Thread {
    parent: Option<String>,
    cwd: Option<String>,
}

/// A request before its thread is resolved to a session.
struct Pending {
    thread: String,
    req: Req,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    out.extend(jsonl_in(dir));
    for sub in subdirs(dir) {
        walk(&sub, out);
    }
}

pub(super) fn read(root: &Path, history: History, found: &mut Found) {
    let mut files = Vec::new();
    walk(&root.join("sessions"), &mut files);
    files.extend(jsonl_in(&root.join("archived_sessions")));
    files.retain(|f| history.opens(f));

    let mut threads: HashMap<String, Thread> = HashMap::new();
    let mut partial: HashSet<String> = HashSet::new();
    let mut pending: Vec<Pending> = Vec::new();
    let mut skill_reads: Vec<(String, i64, String, &'static str)> = Vec::new();
    let mut responses: HashSet<String> = HashSet::new();
    let mut calls: HashSet<String> = HashSet::new();
    let mut hit_turns: HashSet<String> = HashSet::new();
    let mut missing: Vec<(String, i64)> = Vec::new();

    for path in &files {
        let (lines, broken) = read_lines(path);
        let Some(meta) = lines
            .iter()
            .find(|l| l.get("type").and_then(Value::as_str) == Some("session_meta"))
            .and_then(|l| l.get("payload"))
        else {
            continue;
        };
        let Some(id) = meta.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        // The same rollout in sessions/ and archived_sessions/ counts once.
        if threads.contains_key(&id) {
            continue;
        }
        threads.insert(
            id.clone(),
            Thread {
                parent: meta
                    .get("parent_thread_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                cwd: meta.get("cwd").and_then(Value::as_str).map(str::to_string),
            },
        );
        if broken {
            partial.insert(id.clone());
        }
        let mut chars = [0u64; N];
        chars[INSTRUCTIONS] += meta.get("base_instructions").map_or(0, text_chars);
        let mut names: HashMap<String, String> = HashMap::new();
        let mut model = "unknown";
        let mut previous: Option<(u64, u64)> = None;
        for line in &lines {
            let p = line.get("payload").unwrap_or(&Value::Null);
            let ptype = p.get("type").and_then(Value::as_str);
            match line.get("type").and_then(Value::as_str) {
                Some("turn_context") => {
                    if let Some(m) = p.get("model").and_then(Value::as_str) {
                        model = intern(m);
                    }
                }
                Some("response_item") => match ptype {
                    Some("message") => {
                        let bucket = match p.get("role").and_then(Value::as_str) {
                            Some("user") => PROMPTS,
                            Some("developer") => INSTRUCTIONS,
                            _ => continue,
                        };
                        chars[bucket] += p.get("content").map_or(0, text_chars);
                    }
                    Some("function_call" | "custom_tool_call") => {
                        let (Some(call), Some(name)) = (
                            p.get("call_id").and_then(Value::as_str),
                            p.get("name").and_then(Value::as_str),
                        ) else {
                            continue;
                        };
                        names.insert(call.to_string(), name.to_string());
                        if name != "exec" || !calls.insert(call.to_string()) {
                            continue;
                        }
                        let input = p.get("input").or_else(|| p.get("arguments"));
                        if let (Some((skill, class)), Some(ts)) = (
                            input.and_then(Value::as_str).and_then(skill_read),
                            ts_of(line),
                        ) {
                            skill_reads.push((id.clone(), ts, skill, class));
                        }
                    }
                    Some("function_call_output" | "custom_tool_call_output") => {
                        let name = p
                            .get("call_id")
                            .and_then(Value::as_str)
                            .and_then(|c| names.get(c))
                            .map(String::as_str)
                            .or_else(|| p.get("name").and_then(Value::as_str))
                            .unwrap_or("");
                        chars[tool_bucket(name)] += p.get("output").map_or(0, text_chars);
                    }
                    _ => {}
                },
                Some("token_usage_record") => {
                    let Some(response) = p.get("response_id").and_then(Value::as_str) else {
                        partial.insert(id.clone());
                        continue;
                    };
                    let counts = ["input_tokens", "output_tokens", "total_tokens"];
                    let (Some(u), Some(ts)) = (usage_with(p.get("usage"), &counts), ts_of(line))
                    else {
                        partial.insert(id.clone());
                        if let Some(ts) = ts_of(line) {
                            missing.push((id.clone(), ts));
                        }
                        continue;
                    };
                    let input = u64_at(u, "input_tokens");
                    let cached = u64_at(u, "cached_input_tokens");
                    let output = u64_at(u, "output_tokens");
                    let written = u64_at(u, "cache_write_input_tokens");
                    // Where cache writes belong is unknown, and a record
                    // whose parts don't add up can't be split honestly.
                    if input + output != u64_at(u, "total_tokens") || written > 0 || cached > input
                    {
                        partial.insert(id.clone());
                    }
                    let growth = match previous {
                        None => input,
                        Some((ctx, out)) => input.saturating_sub(ctx + out),
                    };
                    previous = Some((input, output));
                    let sources = split(growth, &chars);
                    chars = [0; N];
                    if !responses.insert(response.to_string()) {
                        continue;
                    }
                    let usage = Usage {
                        fresh: input.saturating_sub(cached),
                        cache_read: cached,
                        write_5m: written,
                        write_1h: 0,
                        output,
                    };
                    let clamp = |v: u64| v.min(u64::from(u32::MAX)) as u32;
                    pending.push(Pending {
                        thread: id.clone(),
                        req: Req {
                            agent: AGENT,
                            session: String::new(),
                            ts,
                            worker: "Main",
                            model,
                            tokens: [
                                clamp(usage.fresh),
                                clamp(cached),
                                clamp(written),
                                clamp(output),
                            ],
                            cost: pricing::price(model, &usage),
                            sources,
                        },
                    });
                }
                Some("event_msg") => match ptype {
                    Some("token_count") => readings(line, p, &mut found.readings),
                    Some("task_complete") => {
                        let limit = p.pointer("/error/codex_error_info").and_then(Value::as_str)
                            == Some("usage_limit_exceeded");
                        let turn = p.get("turn_id").and_then(Value::as_str);
                        if let (true, Some(turn), Some(ts)) = (limit, turn, ts_of(line)) {
                            if ts >= history.since {
                                hit_turns.insert(turn.to_string());
                            }
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    found.limit_hits += hit_turns.len() as u32;

    // Each thread's session is the root of its parent chain on disk.
    let root_of = |thread: &str| -> String {
        let mut current = thread.to_string();
        let mut steps = 0;
        while let Some(parent) = threads.get(&current).and_then(|t| t.parent.clone()) {
            if !threads.contains_key(&parent) || steps > threads.len() {
                break;
            }
            current = parent;
            steps += 1;
        }
        current
    };
    let titles = titles(root);
    for (thread, info) in &threads {
        let root = root_of(thread);
        let meta = found.meta(AGENT, &root);
        if partial.contains(thread) {
            meta.partial = true;
        }
        if &root == thread {
            meta.workspace = info.cwd.as_deref().and_then(workspace_of);
            meta.title = titles.get(thread).map(|t| t.1.clone());
        }
    }
    for (thread, ts) in missing {
        found.meta(AGENT, &root_of(&thread)).missing_at(Some(ts));
    }
    for Pending { thread, mut req } in pending {
        req.session = root_of(&thread);
        if req.session != thread {
            req.worker = "Subagent";
        }
        found.requests.push(req);
    }
    for (thread, ts, name, copy) in skill_reads {
        found.uses.push(UseEvent {
            agent: AGENT,
            session: root_of(&thread),
            ts,
            kind: "skill",
            name,
            copy,
            estimated: true,
        });
    }
}

/// The latest `thread_name` per thread id, by `updated_at`.
fn titles(root: &Path) -> HashMap<String, (String, String)> {
    let mut titles: HashMap<String, (String, String)> = HashMap::new();
    let (lines, _) = read_lines(&root.join("session_index.jsonl"));
    for line in lines {
        let (Some(id), Some(name)) = (
            line.get("id").and_then(Value::as_str),
            line.get("thread_name").and_then(Value::as_str),
        ) else {
            continue;
        };
        let updated = match line.get("updated_at") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => format!("{other:0>20}"),
            None => String::new(),
        };
        let newer = titles.get(id).is_none_or(|(at, _)| updated >= *at);
        if newer {
            titles.insert(id.to_string(), (updated, name.to_string()));
        }
    }
    titles
}

/// An `exec` input that reads a `SKILL.md`: the skill's folder name and
/// the path class of the copy read. Estimated: nothing records that the
/// skill was then followed, and a skill used without reading it is missed.
pub(super) fn skill_read(input: &str) -> Option<(String, &'static str)> {
    let at = input.find("SKILL.md")?;
    let start = input[..at]
        .rfind(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | '='))
        .map_or(0, |i| i + 1);
    let path = input[start..at].replace('\\', "/");
    let name = path.trim_end_matches('/').rsplit('/').next()?.to_string();
    if name.is_empty() {
        return None;
    }
    let class = if path.contains("plugin") {
        "plugin"
    } else if path.contains(".agents/skills/") {
        "workspace"
    } else if path.contains(".codex/skills/") {
        "user"
    } else {
        "other"
    };
    Some((name, class))
}

/// Both `rate_limits` sides of one `token_count` event. A null side is
/// kept with length 0 so it can be counted and noted.
fn readings(line: &Value, p: &Value, out: &mut Vec<Reading>) {
    let (Some(limits), Some(ts)) = (p.get("rate_limits").filter(|v| !v.is_null()), ts_of(line))
    else {
        return;
    };
    for side in ["primary", "secondary"] {
        let s = limits.get(side).unwrap_or(&Value::Null);
        let reading = match (
            s.get("window_minutes").and_then(Value::as_u64),
            s.get("used_percent").and_then(Value::as_f64),
            s.get("resets_at").and_then(Value::as_i64),
        ) {
            (Some(minutes), Some(percent), Some(resets_at)) => Reading {
                ts,
                minutes: minutes as u32,
                percent,
                resets_at,
            },
            _ => Reading {
                ts,
                minutes: 0,
                percent: 0.0,
                resets_at: 0,
            },
        };
        out.push(reading);
    }
}

/// The two Codex rows. A window is found by its length, whichever side
/// carries it. Its current instance ends at the latest `resets_at` (which
/// drifts by seconds between events) and starts `window_minutes` earlier;
/// every reading in that span counts, as a running maximum, because
/// concurrent sessions report slightly older shares.
pub(super) fn windows(readings: &[Reading], midnight: i64, notes: &mut Vec<String>) -> Vec<Window> {
    let nulls = readings.iter().filter(|r| r.minutes == 0).count();
    let other = readings
        .iter()
        .filter(|r| r.minutes != 0 && r.minutes != 300 && r.minutes != 10_080)
        .count();
    if nulls > 0 {
        notes.push(format!("Codex: skipped {nulls} empty rate_limits sides"));
    }
    if other > 0 {
        notes.push(format!(
            "Codex: skipped {other} rate_limits readings of another window length"
        ));
    }
    super::windows_for(AGENT, "logs", readings, midnight)
}
