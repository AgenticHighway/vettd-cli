//! `--source local`: Claude Code and Codex session logs, read offline and
//! read-only. Files are opened for reading only; nothing is written and
//! no socket is opened. See openspec/changes/add-local-source/design.md
//! for the log shapes these rules depend on.

use crate::data::{civil_from_days, Data, Share, Source, BUCKETS};
use crate::fixture::{Request, Session, Used, Window};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod claude;
mod codex;
#[cfg(test)]
mod tests;

const DAY: i64 = 86_400;
/// Files older than this are never opened; newer ones feed limit hits
/// and Codex windows even when no request falls in the period.
const HISTORY_DAYS: i64 = 28;

/// One request as read, before it is placed on a day.
pub(crate) struct Req {
    pub(crate) agent: &'static str,
    pub(crate) session: String,
    pub(crate) ts: i64,
    pub(crate) worker: &'static str,
    pub(crate) model: &'static str,
    pub(crate) tokens: [u32; 4],
    pub(crate) cost: Option<[f64; 4]>,
    pub(crate) sources: [u64; BUCKETS.len()],
}

#[derive(Default)]
pub(crate) struct Meta {
    pub(crate) workspace: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) partial: bool,
    /// The latest request that had no usable usage: the session is shown
    /// on its day, incomplete, even with no counted request.
    pub(crate) missing: Option<i64>,
}

pub(crate) struct UseEvent {
    pub(crate) agent: &'static str,
    pub(crate) session: String,
    pub(crate) ts: i64,
    pub(crate) kind: &'static str,
    pub(crate) name: String,
    pub(crate) copy: &'static str,
    pub(crate) estimated: bool,
}

/// One Codex `rate_limits` side: when it was read, its length, share and
/// reset time (Unix seconds).
pub(crate) struct Reading {
    pub(crate) ts: i64,
    pub(crate) minutes: u32,
    pub(crate) percent: f64,
    pub(crate) resets_at: i64,
}

/// Everything one agent's reader found.
#[derive(Default)]
pub(crate) struct Found {
    pub(crate) requests: Vec<Req>,
    pub(crate) sessions: HashMap<(&'static str, String), Meta>,
    pub(crate) uses: Vec<UseEvent>,
    pub(crate) limit_hits: u32,
    pub(crate) readings: Vec<Reading>,
}

impl Found {
    pub(crate) fn meta(&mut self, agent: &'static str, session: &str) -> &mut Meta {
        self.sessions
            .entry((agent, session.to_string()))
            .or_default()
    }
}

/// Which files to open: those modified since `cutoff`, or all of them
/// when there is none (tests use fixed dates on hand-written files).
#[derive(Clone, Copy)]
pub(crate) struct History {
    pub(crate) cutoff: Option<SystemTime>,
    /// Unix seconds: limit hits before this aren't counted.
    pub(crate) since: i64,
}

impl History {
    pub(crate) fn opens(&self, path: &Path) -> bool {
        let Some(cutoff) = self.cutoff else {
            return true;
        };
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified >= cutoff)
    }
}

/// Process-lifetime names for the views' `&'static str` fields. Local
/// data is read once at start, so the set stays small.
pub(crate) fn intern(s: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES.get_or_init(Default::default).lock().unwrap();
    if let Some(&name) = names.get(s) {
        return name;
    }
    let name: &'static str = Box::leak(s.to_string().into_boxed_str());
    names.insert(name);
    name
}

/// RFC 3339 UTC or offset timestamp to Unix seconds. Fractions are dropped.
pub(crate) fn parse_ts(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6)?.parse::<i64>().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    let days = crate::data::days_from_civil(y, mo as u32, d as u32);
    Some(days * DAY + h * 3600 + mi * 60 + sec - offset)
}

pub(crate) fn ts_of(line: &Value) -> Option<i64> {
    line.get("timestamp")?.as_str().and_then(parse_ts)
}

/// "09:12": a session with no title is named by when it started.
fn hhmm(ts: i64) -> String {
    let rest = ts.rem_euclid(DAY);
    format!("{:02}:{:02}", rest / 3600, rest % 3600 / 60)
}

fn utc(ts: i64) -> String {
    let (y, m, d) = civil_from_days(ts.div_euclid(DAY));
    format!(
        "{y:04}-{m:02}-{d:02}T{}:{:02}Z",
        hhmm(ts),
        ts.rem_euclid(60)
    )
}

/// The last component of a working directory, on either separator. Never
/// a Claude project folder name, which spells out the full path.
pub(crate) fn workspace_of(cwd: &str) -> Option<String> {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Characters in every string inside a value: a block's size as the model
/// would read it, without keys, `type` tags or JSON punctuation.
pub(crate) fn text_chars(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.chars().count() as u64,
        Value::Array(items) => items.iter().map(text_chars).sum(),
        Value::Object(map) => map
            .iter()
            .filter(|(k, _)| k.as_str() != "type")
            .map(|(_, v)| text_chars(v))
            .sum(),
        _ => 0,
    }
}

pub(crate) fn u64_at(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// A usage object with every named count present as a number. A null or
/// partial usage is missing, not zero.
pub(crate) fn usage_with<'a>(usage: Option<&'a Value>, keys: &[&str]) -> Option<&'a Value> {
    usage.filter(|u| keys.iter().all(|k| u.get(*k).is_some_and(Value::is_u64)))
}

impl Meta {
    pub(crate) fn missing_at(&mut self, ts: Option<i64>) {
        self.partial = true;
        if let Some(ts) = ts {
            self.missing = Some(self.missing.map_or(ts, |m| m.max(ts)));
        }
    }
}

/// A file's lines, and whether any failed to parse (a cut-off last line
/// included). Unparsable lines are skipped, never fatal.
pub(crate) fn read_lines(path: &Path) -> (Vec<Value>, bool) {
    // Read-only: std::fs::read opens the file without write access.
    let Ok(bytes) = std::fs::read(path) else {
        return (Vec::new(), true);
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut broken = false;
    let lines = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str(l) {
            Ok(v) => Some(v),
            Err(_) => {
                broken = true;
                None
            }
        })
        .collect();
    (lines, broken)
}

pub(crate) fn jsonl_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .collect();
    files.sort();
    files
}

pub(crate) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// Splits `growth` tokens across buckets in proportion to their characters.
pub(crate) fn split(growth: u64, chars: &[u64; BUCKETS.len()]) -> [u64; BUCKETS.len()] {
    let total: u128 = chars.iter().map(|&c| u128::from(c)).sum();
    if total == 0 {
        return [0; BUCKETS.len()];
    }
    chars.map(|c| (u128::from(growth) * u128::from(c) / total) as u64)
}

/// Shares in basis points that sum to 10,000 (largest remainder), or all
/// zero when there is nothing to split.
pub(crate) fn basis_points(tokens: &[u64]) -> Vec<u32> {
    let total: u128 = tokens.iter().map(|&t| u128::from(t)).sum();
    if total == 0 {
        return vec![0; tokens.len()];
    }
    let exact: Vec<u128> = tokens.iter().map(|&t| u128::from(t) * 10_000).collect();
    let mut bp: Vec<u32> = exact.iter().map(|e| (e / total) as u32).collect();
    let mut order: Vec<usize> = (0..tokens.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(exact[i] % total));
    let short = 10_000 - bp.iter().sum::<u32>();
    for &i in order.iter().take(short as usize) {
        bp[i] += 1;
    }
    bp
}

/// Where each agent keeps its logs: `CLAUDE_CONFIG_DIR` / `CODEX_HOME`,
/// else `~/.claude` / `~/.codex`.
pub(crate) struct Roots {
    pub(crate) claude: Option<PathBuf>,
    pub(crate) codex: Option<PathBuf>,
}

impl Roots {
    fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        let pick = |var: &str, dir: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join(dir)))
        };
        Self {
            claude: pick("CLAUDE_CONFIG_DIR", ".claude"),
            codex: pick("CODEX_HOME", ".codex"),
        }
    }
}

/// The hand-written logs under `tests/data/`, read as of 2026-11-25 12:00
/// UTC.
#[cfg(test)]
pub(crate) fn test_data() -> Data {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data");
    let roots = Roots {
        claude: Some(dir.join("claude")),
        codex: Some(dir.join("codex")),
    };
    let now = parse_ts("2026-11-25T12:00:00Z").unwrap();
    read(&roots, now, None, &mut Vec::new())
}

/// Reads local logs as of the system clock. Notes go to stderr.
pub(crate) fn load() -> Data {
    eprintln!("Reading local logs…");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let cutoff = SystemTime::now().checked_sub(Duration::from_secs((HISTORY_DAYS * DAY) as u64));
    let mut notes = Vec::new();
    let data = read(&Roots::from_env(), now, cutoff, &mut notes);
    for note in notes {
        eprintln!("{note}");
    }
    data
}

/// Reads both agents' logs into the views' data. `cutoff` limits which
/// files are opened by modification time.
pub(crate) fn read(
    roots: &Roots,
    now: i64,
    cutoff: Option<SystemTime>,
    notes: &mut Vec<String>,
) -> Data {
    let history = History {
        cutoff,
        since: now - HISTORY_DAYS * DAY,
    };
    let mut found = Found::default();
    let mut hits = Vec::new();
    for (agent, root, sub, reader) in [
        (
            "Claude",
            &roots.claude,
            "projects",
            claude::read as fn(&Path, History, &mut Found),
        ),
        ("Codex", &roots.codex, "sessions", codex::read),
    ] {
        let before = found.limit_hits;
        match root.as_ref().map(|r| r.join(sub)).filter(|d| d.is_dir()) {
            Some(_) => reader(root.as_ref().unwrap(), history, &mut found),
            None => notes.push(format!(
                "{agent}: no {sub} directory found{}; no {agent} sessions",
                root.as_ref()
                    .map(|r| format!(" under {}", r.display()))
                    .unwrap_or_default()
            )),
        }
        hits.push((agent, found.limit_hits - before));
    }
    assemble(found, hits, now, notes)
}

fn assemble(
    found: Found,
    hits: Vec<(&'static str, u32)>,
    now: i64,
    notes: &mut Vec<String>,
) -> Data {
    let today = now.div_euclid(DAY);
    let first = today - 6;
    let midnight = today * DAY;
    let Found {
        requests,
        sessions: metas,
        uses,
        readings,
        ..
    } = found;
    // Each session's start time, over all its requests in the period.
    let mut start: HashMap<(&str, &str), i64> = HashMap::new();
    let in_period = |ts: i64| (first..=today).contains(&ts.div_euclid(DAY));
    for r in requests.iter().filter(|r| in_period(r.ts)) {
        let e = start.entry((r.agent, &r.session)).or_insert(r.ts);
        *e = (*e).min(r.ts);
    }
    let missing: Vec<((&str, &str), i64)> = metas
        .iter()
        .filter_map(|((agent, session), m)| Some(((*agent, session.as_str()), m.missing?)))
        .filter(|&(_, ts)| in_period(ts))
        .collect();
    for &(key, ts) in &missing {
        start.entry(key).or_insert(ts);
    }
    // Sessions sorted by start, so their order (and group numbers) don't
    // depend on file order.
    let mut keys: Vec<(&str, &str)> = start.keys().copied().collect();
    keys.sort_by_key(|k| (start[k], k.0, k.1));
    let group: HashMap<(&str, &str), usize> =
        keys.iter().enumerate().map(|(i, &k)| (k, i)).collect();
    // One slice per session and UTC day.
    let mut slices: BTreeMap<(i64, usize), Session> = BTreeMap::new();
    let slice = |agent: &'static str, session: &str, day: i64| {
        let meta = metas.get(&(agent, session.to_string()));
        Session {
            day: day as u32,
            name: intern(
                &meta
                    .and_then(|m| m.title.clone())
                    .unwrap_or_else(|| hhmm(start[&(agent, session)])),
            ),
            workspace: intern(
                meta.and_then(|m| m.workspace.as_deref())
                    .unwrap_or("unknown"),
            ),
            agent,
            incomplete: meta.is_some_and(|m| m.partial),
            requests: Vec::new(),
            observed: Vec::new(),
            group: group[&(agent, session)],
            used: Vec::new(),
            sources: vec![0; BUCKETS.len()],
        }
    };
    for &((agent, session), ts) in &missing {
        let day = ts.div_euclid(DAY);
        let agent = if agent == "Claude" { "Claude" } else { "Codex" };
        slices
            .entry((day, group[&(agent, session)]))
            .or_insert_with(|| slice(agent, session, day));
    }
    for r in requests.iter().filter(|r| in_period(r.ts)) {
        let g = group[&(r.agent, r.session.as_str())];
        let day = r.ts.div_euclid(DAY);
        let s = slices
            .entry((day, g))
            .or_insert_with(|| slice(r.agent, &r.session, day));
        s.requests.push(Request {
            worker: r.worker,
            model: r.model,
            tokens: r.tokens,
            milli_usd_per_ktok: None,
            exact: r.cost,
        });
        for (total, add) in s.sources.iter_mut().zip(r.sources) {
            *total += add;
        }
    }
    // Uses land on the slice of the day they happened.
    let mut counted: BTreeMap<(i64, usize, &str, &str, &str, bool), u32> = BTreeMap::new();
    for u in uses.iter().filter(|u| in_period(u.ts)) {
        if let Some(&g) = group.get(&(u.agent, u.session.as_str())) {
            let day = u.ts.div_euclid(DAY);
            if slices.contains_key(&(day, g)) {
                *counted
                    .entry((day, g, u.kind, u.name.as_str(), u.copy, u.estimated))
                    .or_insert(0) += 1;
            }
        }
    }
    for ((day, g, kind, name, copy, estimated), count) in counted {
        slices.get_mut(&(day, g)).unwrap().used.push(Used {
            kind,
            name: intern(name),
            count,
            copy,
            estimated,
        });
    }
    let sessions: Vec<Session> = slices.into_values().collect();
    let totals: Vec<u64> = (0..BUCKETS.len())
        .map(|i| sessions.iter().map(|s| s.sources[i]).sum())
        .collect();
    let token_sources = BUCKETS
        .iter()
        .zip(basis_points(&totals))
        .zip(&totals)
        .map(|((&name, basis_points), &tokens)| Share {
            name,
            basis_points,
            tokens: Some(tokens),
        })
        .collect();
    let mut windows = vec![
        relay_window("5-hour", 300),
        relay_window("weekly", 7 * 1440),
    ];
    windows.extend(codex::windows(&readings, midnight, notes));
    Data {
        source: Source::Local,
        sessions,
        days: (first..=today).map(|d| d as u32).collect(),
        today: today as u32,
        now: ((now - midnight) / 60) as i32,
        windows,
        weekly_ends: None,
        limit_hits: hits.iter().map(|h| h.1).sum(),
        limit_hits_by_agent: Some(hits),
        token_sources,
        now_utc: Some(utc(now)),
    }
}

/// Claude windows reach Lens only through the statusline relay, which
/// this change doesn't add: no reading, no guess.
fn relay_window(name: &'static str, minutes: u32) -> Window {
    Window {
        agent: "Claude",
        name,
        source: "needs relay",
        minutes,
        start: None,
        readings: Cow::Owned(Vec::new()),
    }
}

/// The local export: aggregates only. No titles, names, paths, prompts or
/// IDs; sessions are numbered in start order instead.
pub(crate) fn export(data: &Data) -> Value {
    let token_sources: Vec<_> = data
        .token_sources
        .iter()
        .map(|s| {
            serde_json::json!({
                "source": s.name,
                "basis_points": s.basis_points,
                "estimated_tokens": s.tokens,
            })
        })
        .collect();
    let sessions: Vec<_> = data
        .sessions
        .iter()
        .map(|s| {
            let uses = |kind: &str| -> u32 {
                s.used
                    .iter()
                    .filter(|u| u.kind == kind)
                    .map(|u| u.count)
                    .sum()
            };
            let (y, m, d) = civil_from_days(i64::from(s.day));
            serde_json::json!({
                "session": s.group,
                "day": s.day,
                "date": format!("{y:04}-{m:02}-{d:02}"),
                "workspace": s.workspace,
                "agent": s.agent,
                "incomplete": s.incomplete,
                "skill_uses": uses("skill"),
                "mcp_uses": uses("MCP"),
                "requests": s.requests.iter().map(|r| serde_json::json!({
                    "worker": r.worker,
                    "model": r.model,
                    "tokens": r.tokens,
                    "api_equivalent_cents": r.exact,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut workspaces: Vec<&str> = data.sessions.iter().map(|s| s.workspace).collect();
    workspaces.sort_unstable();
    workspaces.dedup();
    let by_agent: serde_json::Map<String, Value> = data
        .limit_hits_by_agent
        .iter()
        .flatten()
        .map(|&(agent, n)| (agent.to_string(), n.into()))
        .collect();
    serde_json::json!({
        "source": "local",
        "synthetic": false,
        "period": data.period(),
        "now": data.now_utc,
        "snapshot": crate::fixture::SNAPSHOT,
        "workspaces": workspaces,
        "agents": crate::fixture::AGENTS,
        "components": crate::fixture::COMPONENTS,
        "placements": crate::fixture::PLACEMENTS,
        "scans": crate::fixture::SCANS,
        "synthetic_sections": ["snapshot", "components", "placements", "scans"],
        "estimated_sections": ["token_sources", "sessions.skill_uses (Codex)"],
        "windows": data.windows,
        "now_minutes_from_nov_18": Value::Null,
        "claude_weekly_ends_percent": Value::Null,
        "limit_hits": data.limit_hits,
        "limit_hits_by_agent": by_agent,
        "token_sources": token_sources,
        "sessions": sessions,
    })
}
