//! `--source local`: Claude Code and Codex session logs, read offline and
//! read-only. Files are opened for reading only; nothing is written and
//! no socket is opened. See openspec/changes/add-local-source/design.md
//! for the log shapes these rules depend on.

use crate::data::{civil_from_days, Data, Share, Source, BUCKETS};
use crate::fixture::{Request, Session, Used, Window};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
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
    /// History's read reports (files read, files found) here as it goes.
    pub(crate) progress: Option<Box<dyn FnMut(usize, usize) + Send>>,
    pub(crate) files: (usize, usize),
}

impl Found {
    /// A reader found `n` more files it will open.
    pub(crate) fn opening(&mut self, n: usize) {
        self.files.1 += n;
        self.report();
    }

    /// A reader is opening its next file.
    pub(crate) fn opened(&mut self) {
        self.files.0 += 1;
        self.report();
    }

    fn report(&mut self) {
        let (read, found) = self.files;
        if let Some(progress) = self.progress.as_mut() {
            progress(read, found);
        }
    }

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

/// What a read covers.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Span {
    /// The dashboard: the 7 UTC days ending today, from files modified
    /// since the cutoff (none in tests, which use fixed dates).
    Week(Option<SystemTime>),
    /// History: every file, every request, one slice per whole session.
    All,
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

/// A file's lines, parsed one at a time so only one line is held in
/// memory. `broken` turns true when the file can't be read or any line
/// fails to parse (a cut-off last line included); unparsable lines are
/// skipped, never fatal.
pub(crate) struct Lines {
    reader: Option<BufReader<File>>,
    buf: Vec<u8>,
    pub(crate) broken: bool,
}

pub(crate) fn lines_of(path: &Path) -> Lines {
    // Read-only: File::open opens the file without write access.
    let reader = File::open(path).ok().map(BufReader::new);
    Lines {
        broken: reader.is_none(),
        reader,
        buf: Vec::new(),
    }
}

impl Iterator for Lines {
    type Item = Value;

    fn next(&mut self) -> Option<Value> {
        let reader = self.reader.as_mut()?;
        loop {
            self.buf.clear();
            match reader.read_until(b'\n', &mut self.buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => {
                    self.broken = true;
                    break;
                }
            }
            // Line endings as `str::lines`: "\n" or "\r\n".
            let mut line = &self.buf[..];
            if let Some(l) = line.strip_suffix(b"\n") {
                line = l.strip_suffix(b"\r").unwrap_or(l);
            }
            let text = String::from_utf8_lossy(line);
            if text.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&text) {
                Ok(v) => return Some(v),
                Err(_) => self.broken = true,
            }
        }
        self.reader = None;
        None
    }
}

/// A whole file's lines at once, for files that are read more than once
/// over (Codex rollouts, the session index).
pub(crate) fn read_lines(path: &Path) -> (Vec<Value>, bool) {
    let mut lines = lines_of(path);
    let all = lines.by_ref().collect();
    (all, lines.broken)
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
    /// The statusline relay's file (`--relay-file`, else the default).
    pub(crate) relay: Option<PathBuf>,
}

impl Roots {
    fn from_env(relay: Option<PathBuf>) -> Self {
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
            relay: relay.or_else(crate::relay::default_file),
        }
    }
}

/// The hand-written logs under `tests/data/`, read as of 2026-11-25 12:00
/// UTC with no relay file.
#[cfg(test)]
pub(crate) fn test_data() -> Data {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data");
    let roots = Roots {
        claude: Some(dir.join("claude")),
        codex: Some(dir.join("codex")),
        relay: None,
    };
    let now = parse_ts("2026-11-25T12:00:00Z").unwrap();
    read(&roots, now, Span::Week(None), &mut Vec::new())
}

/// History over the hand-written logs under `tests/data/history/`: an old
/// unpriced session and one that spans the period's start.
#[cfg(test)]
pub(crate) fn test_history() -> Data {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("history");
    let roots = Roots {
        claude: Some(dir.join("claude")),
        codex: None,
        relay: None,
    };
    let now = parse_ts("2026-11-25T12:00:00Z").unwrap();
    read(&roots, now, Span::All, &mut Vec::new())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Reads local logs as of the system clock. Notes go to stderr.
pub(crate) fn load(relay_file: Option<PathBuf>) -> Data {
    eprintln!("Reading local logs…");
    let cutoff = SystemTime::now().checked_sub(Duration::from_secs((HISTORY_DAYS * DAY) as u64));
    let mut notes = Vec::new();
    let data = read(
        &Roots::from_env(relay_file),
        unix_now(),
        Span::Week(cutoff),
        &mut notes,
    );
    for note in notes {
        eprintln!("{note}");
    }
    data
}

/// History: every session in the logs on disk, however old, newest
/// first. `progress` hears (files read, files found) as reading goes. Its
/// notes are dropped: the dashboard's read already printed them.
pub(crate) fn history(progress: Option<Box<dyn FnMut(usize, usize) + Send>>) -> Data {
    let roots = Roots {
        relay: None,
        ..Roots::from_env(None)
    };
    let found = Found {
        progress,
        ..Found::default()
    };
    read_into(&roots, unix_now(), Span::All, &mut Vec::new(), found)
}

/// Reads both agents' logs into the views' data over `span`.
pub(crate) fn read(roots: &Roots, now: i64, span: Span, notes: &mut Vec<String>) -> Data {
    read_into(roots, now, span, notes, Found::default())
}

fn read_into(
    roots: &Roots,
    now: i64,
    span: Span,
    notes: &mut Vec<String>,
    mut found: Found,
) -> Data {
    let history = History {
        cutoff: match span {
            Span::Week(cutoff) => cutoff,
            Span::All => None,
        },
        since: now - HISTORY_DAYS * DAY,
    };
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
    let relay = roots
        .relay
        .as_deref()
        .and_then(crate::relay::records)
        .map(|records| {
            records
                .into_iter()
                .filter_map(|r| {
                    let minutes = match r.w.as_str() {
                        "five_hour" => 300,
                        "seven_day" => 10_080,
                        _ => return None,
                    };
                    Some(Reading {
                        ts: r.t,
                        minutes,
                        percent: r.pct,
                        resets_at: r.resets_at,
                    })
                })
                .collect::<Vec<_>>()
        });
    assemble(found, hits, relay, now, span == Span::All, notes)
}

/// Builds the views' data. The dashboard (`all` false) gets one slice per
/// session and UTC day in the period; History (`all`) one slice per whole
/// session, on the day it started, newest first.
fn assemble(
    found: Found,
    hits: Vec<(&'static str, u32)>,
    relay: Option<Vec<Reading>>,
    now: i64,
    all: bool,
    notes: &mut Vec<String>,
) -> Data {
    let today = now.div_euclid(DAY);
    let first = today - 6;
    let midnight = today * DAY;
    let Found {
        mut requests,
        sessions: metas,
        uses,
        readings,
        ..
    } = found;
    // A session's requests in time order. The sort is stable, so ties keep
    // the readers' order: Claude by request id, Codex by file and line.
    requests.sort_by_key(|r| r.ts);
    // Each session's start time, over all its requests in the period.
    let mut start: HashMap<(&str, &str), i64> = HashMap::new();
    let in_period = |ts: i64| all || (first..=today).contains(&ts.div_euclid(DAY));
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
    // The day a slice is on: its own on the dashboard, the session's start
    // day in History.
    let day_of = |key: (&str, &str), ts: i64| if all { start[&key] } else { ts }.div_euclid(DAY);
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
        let day = day_of((agent, session), ts);
        let agent = if agent == "Claude" { "Claude" } else { "Codex" };
        slices
            .entry((day, group[&(agent, session)]))
            .or_insert_with(|| slice(agent, session, day));
    }
    for r in requests.iter().filter(|r| in_period(r.ts)) {
        let g = group[&(r.agent, r.session.as_str())];
        let day = day_of((r.agent, r.session.as_str()), r.ts);
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
            let day = day_of((u.agent, u.session.as_str()), u.ts);
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
    let mut sessions: Vec<Session> = slices.into_values().collect();
    if all {
        let started = |s: &Session| start[&keys[s.group]];
        sessions.sort_by(|a, b| {
            started(b)
                .cmp(&started(a))
                .then(a.agent.cmp(b.agent))
                .then(a.name.cmp(b.name))
        });
    }
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
    let mut windows = claude_windows(relay.as_deref(), midnight);
    windows.extend(codex::windows(&readings, midnight, notes));
    let weekly_ends = relay
        .as_deref()
        .map(|r| weekly_ends(r, now))
        .filter(|e| !e.is_empty());
    Data {
        source: Source::Local,
        sessions,
        days: (first..=today).map(|d| d as u32).collect(),
        today: today as u32,
        now: ((now - midnight) / 60) as i32,
        windows,
        weekly_ends,
        limit_hits: hits.iter().map(|h| h.1).sum(),
        limit_hits_by_agent: Some(hits),
        token_sources,
        now_utc: Some(utc(now)),
    }
}

const NOT_SET_UP: &str = "relay not set up";
const NO_READING: &str = "no relay reading yet";

/// Claude windows come only from the statusline relay: with no relay file
/// there is no reading and no guess.
fn claude_windows(relay: Option<&[Reading]>, midnight: i64) -> Vec<Window> {
    let mut windows = windows_for("Claude", "relay", relay.unwrap_or_default(), midnight);
    for w in &mut windows {
        if w.readings.is_empty() {
            w.note = Some(if relay.is_some() {
                NO_READING
            } else {
                NOT_SET_UP
            });
        }
    }
    windows
}

/// The 5-hour and weekly rows for one agent. A window is found by its
/// length. Its current instance ends at the latest `resets_at` (which
/// drifts by seconds between events) and starts that many minutes earlier;
/// every reading in that span counts, as a running maximum, because
/// concurrent sessions report slightly older shares.
pub(crate) fn windows_for(
    agent: &'static str,
    source: &'static str,
    readings: &[Reading],
    midnight: i64,
) -> Vec<Window> {
    [("5-hour", 300u32), ("weekly", 10_080)]
        .into_iter()
        .map(|(name, minutes)| {
            let mine: Vec<&Reading> = readings.iter().filter(|r| r.minutes == minutes).collect();
            let empty = Window {
                agent,
                name,
                source,
                minutes,
                start: None,
                readings: Cow::Owned(Vec::new()),
                note: None,
                precise: Vec::new(),
            };
            let Some(end) = mine.iter().map(|r| r.resets_at).max() else {
                return empty;
            };
            let start = end - i64::from(minutes) * 60;
            let mut current: Vec<&Reading> = mine
                .into_iter()
                .filter(|r| (start..=end).contains(&r.ts))
                .collect();
            current.sort_by_key(|r| r.ts);
            // One minute grid: the start rounds up, so a reset is never
            // shown early, and each reading keeps its own UTC minute and day.
            let start_minute = (start - midnight).div_euclid(60)
                + i64::from((start - midnight).rem_euclid(60) > 0);
            let mut high = 0.0f64;
            let grid = midnight + start_minute * 60;
            let (points, precise): (Vec<_>, Vec<_>) = current
                .iter()
                .map(|r| {
                    high = high.max(r.percent);
                    let minute = (r.ts - midnight).div_euclid(60) - start_minute;
                    let exact = ((r.ts - grid) as f64 / 60.0).max(0.0);
                    ((minute.max(0) as u32, high.round() as u32), (exact, high))
                })
                .unzip();
            Window {
                start: Some(start_minute as i32),
                readings: Cow::Owned(points),
                precise,
                ..empty
            }
        })
        .collect()
}

/// How the last (up to) 4 Claude weekly windows that have ended finished:
/// the highest share recorded before each reset, oldest first. Resets
/// within an hour of each other are one window, since `resets_at` can
/// drift by seconds.
fn weekly_ends(relay: &[Reading], now: i64) -> Vec<u32> {
    let mut weekly: Vec<&Reading> = relay.iter().filter(|r| r.minutes == 10_080).collect();
    weekly.sort_by_key(|r| r.resets_at);
    let mut ends: Vec<(i64, f64)> = Vec::new();
    for r in weekly {
        match ends.last_mut() {
            Some((end, high)) if r.resets_at - *end < 3600 => {
                *end = (*end).max(r.resets_at);
                *high = high.max(r.percent);
            }
            _ => ends.push((r.resets_at, r.percent)),
        }
    }
    let ended: Vec<u32> = ends
        .into_iter()
        .filter(|(end, _)| *end <= now)
        .map(|(_, high)| high.round() as u32)
        .collect();
    ended[ended.len().saturating_sub(4)..].to_vec()
}

/// `history[]` for `--json --history`: History's sessions, newest first,
/// under the same rules as `sessions[]`.
pub(crate) fn history_export(history: &Data) -> Value {
    sessions_json(&history.sessions).into()
}

/// Sessions as the export holds them: no titles, names, paths or IDs.
fn sessions_json(sessions: &[Session]) -> Vec<Value> {
    sessions
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
        .collect()
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
    let sessions = sessions_json(&data.sessions);
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
        "claude_weekly_ends_percent": data.weekly_ends,
        "limit_hits": data.limit_hits,
        "limit_hits_by_agent": by_agent,
        "token_sources": token_sources,
        "sessions": sessions,
    })
}
