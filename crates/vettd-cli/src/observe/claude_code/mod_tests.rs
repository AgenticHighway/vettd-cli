//! Reader tests, ported one for one from
//! `spikes/828-passive-observer/prototype/tests/test_claude_code_source.py`.
//!
//! They run against the checked-in copy of the prototype's own fixture home,
//! `crates/vettd-cli/tests/fixtures/observe/claude_home/`. Every value in it is invented, and the
//! literal `ZQXSENTINEL` sits in every content position so
//! [`no_content_string_survives_parse`] cannot pass vacuously. Expected hashes and byte counts are
//! recomputed here from the fixture bytes with `sha2` and `serde_json`, independently of the
//! reader — a test that asked the parser what the answer was could not fail when the parser is
//! wrong.
//!
//! **File length, named:** this file is over CONTRIBUTING.md's 400-line budget, as the two test
//! sidecars Phase 1 landed already are (`gate_tests.rs`, `source_tests.rs`). All sixteen tests read
//! the same fixture home through the same [`Fixture`] harness and recompute their expectations with
//! the same handful of helpers; splitting them across files would put that harness behind a module
//! boundary and buy nothing but two shorter files.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use super::*;
use crate::observe::types::{InBandKind, LoadedSetKind, ToolCall, RATE_BEARING_FAILURES};

/// The fixture main session's key, which is also its file stem.
const SID: &str = "0f1e2d3c-4b5a-6978-8a9b-0c1d2e3f4a5b";

/// The `tool_use` id of the fixture's `Agent` spawn, echoed by the child sidecar's `toolUseId`.
const AGENT_TOOL_USE: &str = "toolu_fx00000005";

/// `tool_error` and `user_denied` from `FAILURE_CLASSES`, spelled out for readability.
const FAILURE_TOOL_ERROR: &str = "tool_error";
const FAILURE_USER_DENIED: &str = "user_denied";

/// Built at runtime so this file does not itself carry the marker verbatim — otherwise a grep for
/// leaked content would hit the test that proves there is none.
fn sentinel() -> String {
    format!("{}{}", "ZQX", "SENTINEL")
}

fn fixture_home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/observe/claude_home")
}

fn main_path() -> PathBuf {
    fixture_home()
        .join("projects/-fixture-project")
        .join(format!("{SID}.ndjson"))
}

fn child_path() -> PathBuf {
    fixture_home()
        .join("projects/-fixture-project")
        .join(SID)
        .join("subagents/agent-fx1.ndjson")
}

fn meta_path() -> PathBuf {
    child_path().with_extension("meta.json")
}

/// `int(os.stat(path).st_mtime * 1000)`, the same conversion discovery uses.
fn mtime_ms(path: &Path) -> i64 {
    discover::mtime_ms_of(&fs::metadata(path).expect("stat fixture")).expect("post-epoch mtime")
}

/// A harness timestamp on the fixture's day, as milliseconds (`_ms`, the Python's helper).
fn ms(hour: u32, minute: u32, second: u32, milli: u32) -> i64 {
    NaiveDate::from_ymd_opt(2026, 8, 15)
        .and_then(|day| day.and_hms_milli_opt(hour, minute, second, milli))
        .expect("a real instant")
        .and_utc()
        .timestamp_millis()
}

/// Parse a fixture independently of the reader: one value per well-formed line.
fn objects(path: &Path) -> Vec<Value> {
    let bytes = fs::read(path).expect("read fixture");
    bytes
        .split_inclusive(|&b| b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .collect()
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let mut out = String::new();
    for byte in hasher.finalize() {
        write!(out, "{byte:02x}").expect("writing to a String is infallible");
    }
    out
}

/// Everything the Python's `setUpClass` builds: the discovered refs and both files read once.
struct Fixture {
    refs: Vec<SessionRef>,
    main_ref: SessionRef,
    child_ref: SessionRef,
    main: SessionFacts,
    main_cursor: Cursor,
    child: SessionFacts,
    child_cursor: Cursor,
    now_ms: i64,
}

impl Fixture {
    fn read() -> Fixture {
        let home = fixture_home();
        let now_ms = mtime_ms(&main_path());
        let source = ClaudeCodeSource::with_now_ms(home.clone(), now_ms);
        let refs = source.discover(&home, 3650, now_ms).expect("discover");
        let main_ref = refs
            .iter()
            .find(|r| r.kind == SessionKind::Main)
            .expect("a main ref")
            .clone();
        let child_ref = refs
            .iter()
            .find(|r| r.kind == SessionKind::Child)
            .expect("a child ref")
            .clone();
        let (main, main_cursor) = source.read(&main_ref, None).expect("read main");
        let (child, child_cursor) = source.read(&child_ref, None).expect("read child");
        Fixture {
            refs,
            main_ref,
            child_ref,
            main,
            main_cursor,
            child,
            child_cursor,
            now_ms,
        }
    }

    /// The main session's tool calls by `tool_use_id`.
    fn calls(&self) -> BTreeMap<&str, &ToolCall> {
        self.main
            .tool_calls
            .iter()
            .map(|call| (call.tool_use_id.as_str(), call))
            .collect()
    }
}

/// A throwaway harness home holding one session file with `lines` as its bytes.
fn temp_session(lines: &[u8]) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp dir");
    let pdir = tmp.path().join("projects/-tmp-project");
    fs::create_dir_all(&pdir).expect("create project dir");
    let path = pdir.join("11111111-2222-4333-8444-555555555555.ndjson");
    fs::write(&path, lines).expect("write session");
    (tmp, path)
}

/// A hand-built main ref for a temp session, standing in for discovery.
fn temp_ref(path: &Path) -> SessionRef {
    SessionRef {
        path: path.to_path_buf(),
        harness: HARNESS_CLAUDE_CODE.to_string(),
        session_key: "t".to_string(),
        kind: SessionKind::Main,
        parent_key: None,
        child_meta: BTreeMap::new(),
    }
}

// -- discovery ---------------------------------------------------------------------------------

/// The on-disk layout is the only evidence of a parent/child relationship, so it must map to one
/// main ref plus one child ref keyed by the parent's stem and carrying the sidecar's three
/// allowlisted keys — and the sidecar's free-text `description` must not be among them, because
/// `child_meta` is copied around locally and anything in it is one refactor from the wire.
#[test]
fn discover_returns_main_and_linked_child() {
    let fx = Fixture::read();
    assert_eq!(fx.refs.len(), 2);
    assert_eq!(fx.main_ref.session_key, SID);
    assert_eq!(fx.main_ref.kind, SessionKind::Main);
    assert_eq!(fx.child_ref.kind, SessionKind::Child);
    assert_eq!(fx.child_ref.parent_key.as_deref(), Some(SID));
    assert_eq!(fx.child_ref.session_key, "fx1");
    assert_eq!(
        fx.child_ref.child_meta.get("agentType").map(String::as_str),
        Some("fx-reviewer")
    );
    assert_eq!(
        fx.child_ref.child_meta.get("toolUseId").map(String::as_str),
        Some(AGENT_TOOL_USE)
    );
    assert_eq!(
        fx.child_ref
            .child_meta
            .get("spawnDepth")
            .map(String::as_str),
        Some("1")
    );
    assert!(!fx.child_ref.child_meta.contains_key("description"));
}

/// The window is what bounds how far back a run may look, so it must be decided on the file's
/// mtime and nothing else: a home whose newest transcript predates the window yields no refs at
/// all, and widening `now` back inside the window brings every file back.
#[test]
fn discover_window_filters_on_mtime() {
    let fx = Fixture::read();
    let home = fixture_home();
    let source = ClaudeCodeSource::new(home.clone());
    let stale = source
        .discover(&home, 1, fx.now_ms + 3 * 86_400_000)
        .expect("discover");
    assert_eq!(stale, Vec::new());
    let fresh = source
        .discover(&home, 1, fx.now_ms + 3_600_000)
        .expect("discover");
    assert_eq!(fresh.len(), 2);
}

// -- tool calls --------------------------------------------------------------------------------

/// A tool call and its outcome are written on two different lines, so latency and success can only
/// be known by pairing them by id. A use whose result never arrives must stay unpaired with no
/// latency rather than be reported as a fast success — an unmeasured call is not a good one.
#[test]
fn tool_use_pairs_with_result_across_lines() {
    let fx = Fixture::read();
    let calls = fx.calls();
    let ok = calls["toolu_fx00000001"];
    assert!(ok.paired());
    assert_eq!(ok.latency_ms(), Some(1500));
    assert_eq!(ok.is_error, Some(false));
    assert_eq!(ok.failure_class, None);
    assert_eq!(ok.name, "Bash");
    assert_eq!(calls["toolu_fx00000004"].latency_ms(), Some(1200));
    let orphan = calls["toolu_fx00000007"];
    assert!(!orphan.paired());
    assert_eq!(orphan.latency_ms(), None);
    let paired = fx.main.tool_calls.iter().filter(|c| c.paired()).count();
    assert_eq!(paired, fx.main.tool_calls.len() - 1);
}

/// Failure classes exist so a person's refusal never lands on an asset's scorecard. Two results
/// that both carry `is_error` must therefore split on the denial phrasing, and only `tool_error`
/// may be rate-bearing.
#[test]
fn denial_and_tool_error_are_different_classes() {
    let fx = Fixture::read();
    let calls = fx.calls();
    let denied = calls["toolu_fx00000002"];
    let errored = calls["toolu_fx00000003"];
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(errored.is_error, Some(true));
    assert!(!denied.interrupted);
    assert_eq!(denied.failure_class.as_deref(), Some(FAILURE_USER_DENIED));
    assert_eq!(errored.failure_class.as_deref(), Some(FAILURE_TOOL_ERROR));
    assert!(RATE_BEARING_FAILURES.contains(&FAILURE_TOOL_ERROR));
    assert!(!RATE_BEARING_FAILURES.contains(&FAILURE_USER_DENIED));
}

/// Every call to an MCP tool is evidence about the *server* that serves it, so the server segment
/// must be recovered from the tool name; a non-MCP tool has no server and must not be given one.
#[test]
fn mcp_tool_call_resolves_server() {
    let fx = Fixture::read();
    let calls = fx.calls();
    let call = calls["toolu_fx00000004"];
    assert_eq!(call.name, "mcp__srvfx__tool");
    assert_eq!(call.server.as_deref(), Some("srvfx"));
    assert_eq!(calls["toolu_fx00000001"].server, None);
}

/// A sub-agent run is a separate transcript, so the only way its work can be attributed to the
/// spawn that caused it is the chain `tool_use id -> sidecar toolUseId -> agentId -> child
/// session_key`. The child is corroborated only when its own lines name the agent type its sidecar
/// declared, and `link_children` must put the tree back together from separately-read files.
#[test]
fn agent_spawn_links_child_via_meta_tool_use_id() {
    let fx = Fixture::read();
    let calls = fx.calls();
    let spawn = calls[AGENT_TOOL_USE];
    assert!(spawn.paired() && spawn.is_async);
    assert_eq!(spawn.agent_type.as_deref(), Some("fx-reviewer"));
    assert_eq!(spawn.child_key.as_deref(), Some("fx1"));
    assert_eq!(
        fx.child_ref.child_meta.get("toolUseId").map(String::as_str),
        Some(spawn.tool_use_id.as_str())
    );
    assert_eq!(fx.child.ref_.session_key, spawn.child_key.clone().unwrap());
    assert_eq!(
        fx.child
            .ref_
            .child_meta
            .get("corroborated")
            .map(String::as_str),
        Some("true")
    );
    assert!(!fx.main.ref_.child_meta.contains_key("corroborated"));
    let grep = &fx.child.tool_calls[0];
    assert_eq!(grep.name, "Grep");
    assert_eq!(grep.latency_ms(), Some(400));
    assert_eq!(fx.child.user_turns, 1);
    let linked = link_children(vec![fx.child.clone(), fx.main.clone()]);
    let keys: Vec<&str> = linked.iter().map(|f| f.ref_.session_key.as_str()).collect();
    assert_eq!(keys, vec![SID]);
    assert_eq!(linked[0].children.len(), 1);
    assert_eq!(linked[0].children[0].ref_.session_key, "fx1");
    assert_eq!(
        format!("{:?}", linked[0].children[0]),
        format!("{:?}", fx.child)
    );
}

// -- usage -------------------------------------------------------------------------------------

/// One API response is written across several assistant lines, each repeating a *fuller* usage
/// block. Counting every line would inflate a run's token cost, so a message id may contribute
/// exactly once; the totals must equal the first-per-id sum recomputed from the fixture and be
/// strictly below the naive per-line sum, which is what proves the dedupe did something.
#[test]
fn split_response_usage_deduped_by_message_id() {
    let fx = Fixture::read();
    let per_line: Vec<Value> = objects(&main_path())
        .into_iter()
        .filter(|o| o.get("type").and_then(Value::as_str) == Some("assistant"))
        .map(|mut o| o["message"].take())
        .collect();
    let mut first: BTreeMap<String, Value> = BTreeMap::new();
    for message in &per_line {
        let id = message["id"].as_str().expect("a message id").to_string();
        first.entry(id).or_insert_with(|| message["usage"].clone());
    }
    assert!(per_line.len() > first.len());
    let seen: BTreeSet<&str> = fx.main.usages.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = first.keys().map(String::as_str).collect();
    assert_eq!(seen, expected);
    let total: i64 = fx.main.usages.values().map(|u| u.output_tokens).sum();
    let first_sum: i64 = first
        .values()
        .map(|u| u["output_tokens"].as_i64().unwrap_or(0))
        .sum();
    let naive: i64 = per_line
        .iter()
        .map(|m| m["usage"]["output_tokens"].as_i64().unwrap_or(0))
        .sum();
    assert_eq!(total, first_sum);
    assert!(total < naive);
    let u1 = &fx.main.usages["msg_fx00000001"];
    assert_eq!(
        (
            u1.input_tokens,
            u1.cache_creation,
            u1.cache_read,
            u1.thinking,
            u1.model.as_str()
        ),
        (100, Some(20), Some(30), Some(7), "claude-fixture-1")
    );
    assert_eq!(u1.cached_input, None);
    assert_eq!(
        fx.main.models,
        BTreeMap::from([("claude-fixture-1".to_string(), first.len() as u64)])
    );
}

// -- in-band assets and loaded set -------------------------------------------------------------

/// A skill the harness injects in band never appears as a tool round-trip, so without the
/// synthetic self-paired call the invocation would be invisible and the skill would look unused.
/// The body's hash must be exactly the bytes after the closing tag — that hash is what binds the
/// invocation to a skill on disk — and the synthetic id must not be forbidden, since the harness
/// never wrote it down.
#[test]
fn invoked_skill_body_hashed_in_band_with_synthetic_call() {
    let fx = Fixture::read();
    let meta_lines: Vec<Value> = objects(&main_path())
        .into_iter()
        .filter(|o| {
            o.get("type").and_then(Value::as_str) == Some("user")
                && o.get("isMeta").and_then(Value::as_bool) == Some(true)
        })
        .collect();
    let text = meta_lines[0]["message"]["content"]
        .as_str()
        .expect("meta content is a string");
    let tag = "</command-name>";
    let body = &text[text.find(tag).expect("a command tag") + tag.len()..];
    let assets: Vec<(&str, &str, i64)> = fx
        .main
        .in_band_assets
        .iter()
        .filter(|a| a.kind == InBandKind::SkillBody)
        .map(|a| (a.name.as_str(), a.content_sha256.as_str(), a.byte_len))
        .collect();
    let expected_hash = hex_sha256(body.as_bytes());
    assert_eq!(
        assets,
        vec![("skill-alpha", expected_hash.as_str(), body.len() as i64)]
    );
    let skills: Vec<&ToolCall> = fx
        .main
        .tool_calls
        .iter()
        .filter(|c| c.name == "Skill")
        .collect();
    let mut names: Vec<&str> = skills.iter().filter_map(|c| c.skill.as_deref()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["skill-alpha", "skill-beta"]);
    let synthetic = skills
        .iter()
        .find(|c| c.skill.as_deref() == Some("skill-alpha"))
        .expect("the synthetic call");
    assert!(synthetic.paired() && synthetic.is_async);
    assert!(!fx.main.forbids["tool_use_ids"].contains(&synthetic.tool_use_id));
}

/// A rules file's content arrives in the log itself, so it can be hashed exactly without reading
/// the user's disk — that is what makes the rules-file asset identity independent of a later edit.
/// Only its basename may be kept, and it must attach to the session's initial loaded set even
/// though it was announced before that listing arrived.
#[test]
fn nested_memory_rules_file_hashed_in_band() {
    let fx = Fixture::read();
    let attachment = objects(&main_path())
        .into_iter()
        .find(|o| {
            o.get("type").and_then(Value::as_str) == Some("attachment")
                && o["attachment"]["type"].as_str() == Some("nested_memory")
        })
        .expect("a nested_memory attachment");
    let body = attachment["attachment"]["content"]["content"]
        .as_str()
        .expect("a nested body");
    let rules: Vec<(&str, &str, i64)> = fx
        .main
        .in_band_assets
        .iter()
        .filter(|a| a.kind == InBandKind::RulesFile)
        .map(|a| (a.name.as_str(), a.content_sha256.as_str(), a.byte_len))
        .collect();
    let expected_hash = hex_sha256(body.as_bytes());
    assert_eq!(
        rules,
        vec![("RULES.md", expected_hash.as_str(), body.len() as i64)]
    );
    let initial = fx
        .main
        .loaded_events
        .iter()
        .find(|e| e.kind == LoadedSetKind::Initial)
        .expect("an initial event");
    assert_eq!(initial.rules_files, vec!["RULES.md".to_string()]);
}

/// The loaded set is the denominator of every "loaded but never used" claim, so its shape is
/// contractual: the session's first deferred-tools listing is the initial set and later ones are
/// changes to it, and the per-name listing and per-server schema byte counts — the only measure of
/// what an asset costs in context — must be recomputed from the fixture's own lines.
#[test]
fn loaded_events_from_listing_and_deltas() {
    let fx = Fixture::read();
    let attachments: Vec<Value> = objects(&main_path())
        .into_iter()
        .filter(|o| o.get("type").and_then(Value::as_str) == Some("attachment"))
        .map(|mut o| o["attachment"].take())
        .collect();
    let deferred: Vec<&Value> = attachments
        .iter()
        .filter(|a| a["type"].as_str() == Some("deferred_tools_delta"))
        .collect();
    let events = &fx.main.loaded_events;
    let kinds: Vec<LoadedSetKind> = events.iter().map(|e| e.kind).collect();
    use LoadedSetKind::{Delta, Initial};
    assert_eq!(kinds, vec![Initial, Initial, Initial, Delta, Delta]);
    let (first, agents, skills, second, instr) =
        (&events[0], &events[1], &events[2], &events[3], &events[4]);
    assert_eq!(first.tool_names, str_vec(&deferred[0]["addedNames"]));
    assert_eq!(first.pending_mcp, vec!["srvpend".to_string()]);
    assert_eq!(first.failed_mcp, vec!["srvfail".to_string()]);
    let srvfx_bytes: i64 = str_vec(&deferred[0]["addedLines"])
        .iter()
        .filter(|line| line.starts_with("mcp__srvfx__"))
        .map(|line| line.chars().count() as i64)
        .sum();
    assert_eq!(
        first.tool_schema_bytes,
        BTreeMap::from([("srvfx".to_string(), srvfx_bytes)])
    );
    assert_eq!(agents.agent_types, vec!["Explore", "fx-reviewer"]);
    let listing = attachments
        .iter()
        .find(|a| a["type"].as_str() == Some("skill_listing"))
        .expect("a skill listing");
    let content = listing["content"].as_str().expect("listing content");
    let alpha_line = content
        .split('\n')
        .find(|line| line.contains("skill-alpha"))
        .expect("a line for skill-alpha");
    assert_eq!(skills.skills, vec!["skill-alpha", "skill-beta"]);
    assert_eq!(
        skills.listing_bytes["skill-alpha"],
        alpha_line.chars().count() as i64
    );
    assert_eq!(second.tool_names, vec!["mcp__srvpend__ping".to_string()]);
    assert!(second.pending_mcp.is_empty());
    assert!(second.removed.is_empty());
    assert!(second.readded.is_empty());
    let ping_line = str_vec(&deferred[1]["addedLines"])[0].chars().count() as i64;
    assert_eq!(
        second.tool_schema_bytes,
        BTreeMap::from([("srvpend".to_string(), ping_line)])
    );
    assert!(instr.tool_names.is_empty());
    assert_eq!(instr.ts_ms, ms(10, 0, 13, 100));
}

/// The string values of a JSON array, for recomputing expectations from the fixture.
fn str_vec(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// -- counters and env --------------------------------------------------------------------------

/// `turns` is reported as a measure of human effort, so it must count prompts only: a meta line, a
/// line that is nothing but tool results, and a harness-injected line are all machinery. The four
/// environment strings and the session's first/last harness stamps come from the lines themselves,
/// never from the collector's clock.
#[test]
fn turns_compactions_and_env() {
    let fx = Fixture::read();
    assert_eq!(fx.main.user_turns, 2);
    assert_eq!(fx.main.compactions, 1);
    assert_eq!(
        (
            fx.main.harness_version.as_str(),
            fx.main.entrypoint.as_str(),
            fx.main.permission_mode.as_str(),
            fx.main.effort.as_str()
        ),
        ("3.4.5", "cli", "acceptEdits", "high")
    );
    assert_eq!(fx.main.first_ts_ms, Some(ms(10, 0, 0, 0)));
    assert_eq!(fx.main.last_ts_ms, Some(ms(10, 1, 2, 0)));
    assert_eq!(fx.main.last_stop_reason.as_deref(), Some("end_turn"));
    assert!(!fx.main.truncated);
}

/// A line the reader cannot use must be counted, never guessed at: a malformed line is a parse
/// error and a line of an unconsumed type is its own counter, and neither may silently disappear
/// or disturb the tool-call count. Those counters are how a downstream consumer knows whether a
/// run's numbers rest on the whole transcript.
#[test]
fn unknown_and_malformed_lines_counted_not_parsed() {
    let fx = Fixture::read();
    let data = fs::read(main_path()).expect("read fixture");
    let newlines = data.iter().filter(|&&b| b == b'\n').count() as u64;
    assert_eq!(fx.main.lines_seen, newlines);
    assert_eq!(fx.main.bytes_read, data.len() as u64);
    assert_eq!(fx.main.lines_unknown_type, 1);
    assert_eq!(fx.main.parse_errors, 1);
    assert_eq!(fx.main.tool_calls.len(), 8);
}

/// The gate's dynamic forbids are the proof that no local name reached the wire unhashed, so every
/// identifier the log carries must land in its bucket — and harness built-in tool and agent names
/// must not, because as substrings they would collide with legitimate closed-enum values and make
/// the check fire on every record.
///
/// This also pins the first of the three prototype defects the port fixes: the permission-mode
/// tally is [`SessionFacts::mode_counts`], a real field, and no `_`-prefixed scratch bucket is
/// written into `forbids`, where `claude_code.py:377` puts it and from where it leaks into the
/// envelope's dynamic sidecar.
#[test]
fn forbids_buckets_populated() {
    let fx = Fixture::read();
    let f = &fx.main.forbids;
    assert_eq!(f["slugs"], set(["fixture-slug"]));
    assert_eq!(
        f["cwd_and_branches"],
        set(["/fixture/cwd", "fixture-branch"])
    );
    assert_eq!(f["harness_session_ids"], set([SID]));
    assert_eq!(f["agent_ids"], set(["fx1"]));
    let tool_use_ids: BTreeSet<String> = (1..=7).map(|i| format!("toolu_fx0000000{i}")).collect();
    assert_eq!(f["tool_use_ids"], tool_use_ids);
    let message_ids: BTreeSet<String> = (1..=8).map(|i| format!("msg_fx0000000{i}")).collect();
    assert_eq!(f["message_ids"], message_ids);
    let expected = set([
        "skill-alpha",
        "skill-beta",
        "srvfx",
        "mcp__srvfx__tool",
        "srvpend",
        "srvfail",
        "mcp__srvpend__ping",
        "fx-reviewer",
        "RULES.md",
        "srvfx-server",
    ]);
    assert!(expected.is_subset(&f["loaded_set_names"]));
    let builtins = set(["Bash", "Read", "Explore", "Edit", "Agent", "Skill"]);
    assert!(f["loaded_set_names"].is_disjoint(&builtins));
    assert_eq!(fx.child.forbids["agent_ids"], set(["fx1"]));
    assert_eq!(fx.child.forbids["tool_use_ids"], set(["toolu_fxc0000001"]));
    assert!(!f.keys().any(|bucket| bucket.starts_with('_')));
    assert_eq!(
        fx.main.mode_counts,
        BTreeMap::from([("acceptEdits".to_string(), 2)])
    );
}

fn set<const N: usize>(values: [&str; N]) -> BTreeSet<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

/// **The privacy invariant of the entire feature.** The fixture plants `ZQXSENTINEL` in every
/// content position — prompt, thinking, tool input, tool result, `toolUseResult` bodies,
/// attachment bodies, summary, unknown line and the sidecar's free-text description — so if any
/// projection kept a session string instead of reducing it to a hash, a length or a boolean, the
/// marker would surface in the debug rendering of the facts, the refs or the cursors. Zero
/// occurrences is the whole claim; the count of planted markers is asserted first so the test
/// cannot pass because the fixture lost them.
#[test]
fn no_content_string_survives_parse() {
    let fx = Fixture::read();
    let marker = sentinel();
    let planted: usize = [main_path(), child_path(), meta_path()]
        .iter()
        .map(|path| {
            String::from_utf8_lossy(&fs::read(path).expect("read fixture"))
                .matches(marker.as_str())
                .count()
        })
        .sum();
    assert!(planted >= 40, "fixture lost its sentinels: {planted}");
    let rendered = [
        format!("{:?}", fx.main),
        format!("{:?}", fx.child),
        format!("{:?}", fx.refs),
        format!("{:?}", fx.main_cursor),
        format!("{:?}", fx.child_cursor),
    ];
    for text in &rendered {
        assert_eq!(text.matches(marker.as_str()).count(), 0);
    }
    // `toolUseResult.outputFile` is a body field with no projection at all; a path out of the
    // user's filesystem must not survive either.
    assert!(!rendered[0].contains("/fixture/out"));
}

// -- truncation and cursors --------------------------------------------------------------------

/// `truncated` says "these numbers are from a run still in progress", and both halves of the test
/// are needed to say it: a file touched moments ago whose last response ended the turn is a
/// finished session, and an open-ended transcript nobody has touched for minutes is an abandoned
/// one. Getting either half wrong would mislabel complete runs as partial.
#[test]
fn truncation_needs_recent_mtime_and_open_stop_reason() {
    let line = br#"{"type":"assistant","timestamp":"2026-08-15T10:00:00.000Z","message":{"id":"msg_fxt0000001","model":"claude-fixture-1","stop_reason":"tool_use","content":[]}}"#;
    let mut bytes = line.to_vec();
    bytes.push(b'\n');
    let (tmp, path) = temp_session(&bytes);
    let ref_ = temp_ref(&path);
    let mtime = mtime_ms(&path);
    let (live, _) = ClaudeCodeSource::with_now_ms(tmp.path().to_path_buf(), mtime + 1_000)
        .read(&ref_, None)
        .expect("read live");
    let (settled, _) = ClaudeCodeSource::with_now_ms(tmp.path().to_path_buf(), mtime + 200_000)
        .read(&ref_, None)
        .expect("read settled");
    assert!(live.truncated);
    assert!(!settled.truncated);
    let fx = Fixture::read();
    let (finished, _) = ClaudeCodeSource::with_now_ms(fixture_home(), fx.now_ms)
        .read(&fx.main_ref, None)
        .expect("read finished");
    assert!(!finished.truncated);
}

/// The cursor is what makes repeated runs cheap and correct: it must sit one byte past the last
/// **complete** line, a resumed read must consume nothing already consumed, and a partial trailing
/// line — the half-written record of a live session — must be neither counted nor advanced past,
/// or the next run would resume in the middle of a JSON object and lose the record for good.
#[test]
fn cursor_resume_and_partial_trailing_line() {
    let fx = Fixture::read();
    let size = fs::metadata(main_path()).expect("stat fixture").len();
    assert_eq!(fx.main_cursor.byte_offset, size);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let ino = fs::metadata(main_path()).expect("stat fixture").ino();
        assert_eq!(fx.main_cursor.inode, Some(ino));
    }
    #[cfg(not(unix))]
    assert_eq!(fx.main_cursor.inode, None);

    let source = ClaudeCodeSource::with_now_ms(fixture_home(), fx.now_ms);
    let (again, cursor2) = source
        .read(&fx.main_ref, Some(&fx.main_cursor))
        .expect("resume");
    assert_eq!(again.lines_seen, 0);
    assert!(again.tool_calls.is_empty());
    assert_eq!(cursor2.byte_offset, size);

    let full = br#"{"type":"user","timestamp":"2026-08-15T10:00:00.000Z","message":{"role":"user","content":"hi"}}"#;
    let mut full = full.to_vec();
    full.push(b'\n');
    let mut bytes = full.clone();
    bytes.extend_from_slice(&full);
    bytes
        .extend_from_slice(br#"{"type":"user","timestamp":"2026-08-15T10:00:01.000Z", "message":"#);
    let (tmp, path) = temp_session(&bytes);
    let ref_ = temp_ref(&path);
    let source = ClaudeCodeSource::with_now_ms(tmp.path().to_path_buf(), 0);
    let (facts, cursor) = source.read(&ref_, None).expect("read partial");
    let partial_cursor = Cursor {
        path: path.clone(),
        byte_offset: full.len() as u64,
        inode: None,
    };
    let (resumed, cursor_b) = source
        .read(&ref_, Some(&partial_cursor))
        .expect("resume partial");
    assert_eq!(facts.lines_seen, 2);
    assert_eq!(facts.user_turns, 2);
    assert_eq!(facts.parse_errors, 0);
    assert_eq!(cursor.byte_offset, 2 * full.len() as u64);
    assert_eq!(resumed.lines_seen, 1);
    assert_eq!(cursor_b.byte_offset, 2 * full.len() as u64);
}
