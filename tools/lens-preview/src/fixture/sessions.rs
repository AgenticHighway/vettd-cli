//! Synthetic sessions: one Claude index rebuild in api and one Codex release
//! check in web on each day of Nov 12-18, except Nov 13.

use serde::Serialize;

#[derive(Serialize)]
pub struct Use {
    pub component: usize,
    pub version: Option<&'static str>,
    pub artifact: Option<&'static str>,
    pub count: u32,
    pub context_cents: u32,
    pub signals: &'static [(&'static str, u32)],
}

#[derive(Serialize)]
pub struct Request {
    pub worker: &'static str,
    pub model: &'static str,
    pub tokens: [u32; 4],
    pub milli_usd_per_ktok: Option<[u32; 4]>,
}

impl Request {
    pub fn cents(&self) -> [u32; 4] {
        self.milli_usd_per_ktok.map_or([0; 4], |rates| {
            std::array::from_fn(|i| self.tokens[i] * rates[i] / 10_000)
        })
    }
}

#[derive(Serialize)]
pub struct Session {
    pub day: u32,
    pub name: &'static str,
    pub workspace: &'static str,
    pub agent: &'static str,
    pub incomplete: bool,
    pub requests: Vec<Request>,
    pub observed: Vec<Use>,
}

impl Session {
    pub fn cents(&self) -> u32 {
        self.requests.iter().flat_map(Request::cents).sum()
    }
}

/// review-kit and read-file use in the api index rebuild. The recorded
/// review-kit copy changes on Nov 15, and the Nov 16 spike costs more context.
fn rebuild_uses(day: u32) -> Vec<Use> {
    let (version, artifact) = match day {
        15 | 16 => (Some("1.4"), Some("sha:1c4e")),
        17 | 18 => (Some("1.5"), Some("sha:7b20")),
        _ => (None, None),
    };
    let mut uses = vec![
        Use {
            component: 0,
            version,
            artifact,
            count: 2,
            context_cents: if day == 16 { 46 } else { 4 },
            signals: &[("Skill tool", 1), ("Slash invocation", 1)],
        },
        Use {
            component: 2,
            version: None,
            artifact: None,
            count: 5,
            context_cents: 6,
            signals: &[("File tool call", 5)],
        },
    ];
    if day == 17 {
        uses.push(Use {
            component: 6,
            version: None,
            artifact: Some("sha:3f6b"),
            count: 1,
            context_cents: 1,
            signals: &[("Skill tool", 1)],
        });
    }
    uses
}

/// The api index rebuild. Nov 16 is the spike: the Large model, a cold
/// cache and a subagent.
fn index_rebuild(day: u32) -> Session {
    let spike = day == 16;
    let mut requests = vec![Request {
        worker: "Main",
        model: if spike { "Large" } else { "Small" },
        tokens: if spike {
            [220_000, 40_000, 90_000, 36_000]
        } else {
            [20_000 + 2_000 * (day - 12), 140_000, 10_000, 8_000]
        },
        milli_usd_per_ktok: Some(if spike {
            [15, 2, 20, 60]
        } else {
            [5, 1, 8, 20]
        }),
    }];
    if spike {
        requests.push(Request {
            worker: "Subagent",
            model: "Small",
            tokens: [80_000, 40_000, 0, 21_000],
            milli_usd_per_ktok: Some([5, 1, 8, 20]),
        });
    }
    Session {
        day,
        name: "Index rebuild",
        workspace: "api",
        agent: "Claude",
        incomplete: false,
        requests,
        observed: rebuild_uses(day),
    }
}

/// The web release check on Codex. Nov 18 is incomplete and unpriced.
fn release_checks(day: u32) -> Session {
    let mut codex = Session {
        day,
        name: "Release checks",
        workspace: "web",
        agent: "Codex",
        incomplete: day == 18,
        requests: vec![Request {
            worker: "Main",
            model: if day == 18 { "Unpriced" } else { "Small" },
            tokens: [30_000, 90_000, 0, 4_000],
            milli_usd_per_ktok: (day != 18).then_some([5, 1, 0, 20]),
        }],
        observed: vec![Use {
            component: 1,
            version: None,
            artifact: None,
            count: 12,
            context_cents: if day == 18 { 0 } else { 5 },
            signals: &[("Exec wrapper call", 12)],
        }],
    };
    if day == 16 || day == 17 {
        codex.observed.push(Use {
            component: 0,
            version: Some("1.5"),
            artifact: Some("sha:e913"),
            count: 1,
            context_cents: 3,
            signals: &[("Skill invocation", 1)],
        });
    }
    if day == 16 {
        codex.observed.push(Use {
            component: 3,
            version: None,
            artifact: None,
            count: 2,
            context_cents: 4,
            signals: &[("MCP search", 1), ("MCP get_issue", 1)],
        });
    }
    codex
}

pub fn sessions() -> Vec<Session> {
    (12..=18)
        .filter(|day| *day != 13)
        .flat_map(|day| [index_rebuild(day), release_checks(day)])
        .collect()
}
