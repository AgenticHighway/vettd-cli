//! What the views read: the sessions plus everything around them that
//! depends on where the data came from (period, "now", plan windows,
//! limit hits and the token-source split).

use crate::fixture::{self, Session, Window};
use std::ops::Deref;

/// Where context tokens came from, estimated from block sizes on local
/// data (design.md, Decision 13). Claude and Codex share one list; Codex
/// `exec` output stays "Tool output" because splitting it needs the
/// command text.
pub(crate) const BUCKETS: [&str; 9] = [
    "File reads",
    "Shell output",
    "Tool output",
    "MCP tools",
    "Skills",
    "Instructions and files",
    "Hooks",
    "Prompts and replies",
    "Other tools",
];

/// Claude records left out of the estimate: whether they reach the model
/// is unverified.
pub(crate) const NOT_COUNTED: &str = "prompt_snapshot, deferred_tools_*, hook_success, reminders";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Source {
    Fixture,
    Local,
}

impl Source {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "fixture" => Some(Source::Fixture),
            "local" => Some(Source::Local),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Source::Fixture => "fixture",
            Source::Local => "local",
        }
    }
}

/// One row of "Where tokens went": a source, its share in basis points
/// and, on local data, its estimated tokens.
pub(crate) struct Share {
    pub(crate) name: &'static str,
    pub(crate) basis_points: u32,
    pub(crate) tokens: Option<u64>,
}

pub(crate) struct Data {
    pub(crate) source: Source,
    pub(crate) sessions: Vec<Session>,
    /// The period's day keys, oldest first. See `day_label`.
    pub(crate) days: Vec<u32>,
    /// The day "now" falls on; plan times are minutes from its midnight.
    pub(crate) today: u32,
    pub(crate) now: i32,
    pub(crate) windows: Vec<Window>,
    /// Claude weekly window endings: fixture only, the relay on local.
    pub(crate) weekly_ends: Option<Vec<u32>>,
    pub(crate) limit_hits: u32,
    pub(crate) limit_hits_by_agent: Option<Vec<(&'static str, u32)>>,
    pub(crate) token_sources: Vec<Share>,
    /// "now" as a UTC timestamp, local data only.
    pub(crate) now_utc: Option<String>,
}

impl Deref for Data {
    type Target = [Session];

    fn deref(&self) -> &[Session] {
        &self.sessions
    }
}

impl Data {
    pub(crate) fn fixture() -> Self {
        Self {
            source: Source::Fixture,
            sessions: fixture::sessions(),
            days: (12..=18).collect(),
            today: 18,
            now: fixture::NOW,
            windows: fixture::WINDOWS.to_vec(),
            weekly_ends: Some(fixture::CLAUDE_WEEKLY_ENDS.to_vec()),
            limit_hits: fixture::LIMIT_HITS,
            limit_hits_by_agent: None,
            token_sources: fixture::SOURCE_SHARES
                .iter()
                .map(|&(name, basis_points)| Share {
                    name,
                    basis_points,
                    tokens: None,
                })
                .collect(),
            now_utc: None,
        }
    }

    /// The seeded stress corpus on the default plan windows and token
    /// sources, which no stress session records.
    pub(crate) fn stress() -> Self {
        Self {
            sessions: fixture::stress_sessions(),
            ..Self::fixture()
        }
    }

    pub(crate) fn local(&self) -> bool {
        self.source == Source::Local
    }

    /// How dollar figures are labelled: what the fixture's rates price, or
    /// what local usage would cost on the API (it may be paid by a plan).
    pub(crate) fn dollars(&self) -> &'static str {
        if self.local() {
            "API-equivalent"
        } else {
            "priced"
        }
    }

    pub(crate) fn unit(&self, count: bool) -> &'static str {
        match (count, self.local()) {
            (true, _) => "Requests",
            (false, false) => "Priced USD",
            (false, true) => "API-equivalent USD",
        }
    }

    /// Distinct sessions: a local session that crosses midnight is one
    /// session shown on each of its days.
    pub(crate) fn session_count(&self) -> usize {
        let mut groups: Vec<usize> = self.sessions.iter().map(|s| s.group).collect();
        groups.sort_unstable();
        groups.dedup();
        groups.len()
    }

    pub(crate) fn period(&self) -> String {
        period_label(self.days[0], *self.days.last().unwrap())
    }
}

/// Days from 1970-01-01 to a civil (year, month, day). Howard Hinnant's
/// algorithm, valid for the proleptic Gregorian calendar.
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Fixture day keys are days of November 2026 (12-18). Local day keys are
/// days since the Unix epoch, so they sort by date across month ends.
const FIXTURE_DAYS: u32 = 32;

fn civil(day: u32) -> (i64, u32, u32) {
    if day < FIXTURE_DAYS {
        (2026, 11, day)
    } else {
        civil_from_days(i64::from(day))
    }
}

/// "Nov 18", or "Dec 1": every label names its month.
pub(crate) fn day_label(day: u32) -> String {
    let (_, m, d) = civil(day);
    format!("{} {d}", MONTHS[m as usize - 1])
}

/// "Nov 12-18, 2026 UTC", or "Nov 26-Dec 2, 2026 UTC" across a month end.
pub(crate) fn period_label(from: u32, to: u32) -> String {
    let ((_, m0, d0), (y1, m1, d1)) = (civil(from), civil(to));
    let end = if m0 == m1 {
        d1.to_string()
    } else {
        format!("{} {d1}", MONTHS[m1 as usize - 1])
    };
    format!("{} {d0}-{end}, {y1} UTC", MONTHS[m0 as usize - 1])
}
