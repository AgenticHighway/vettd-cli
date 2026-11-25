//! Seeded, deterministic stress corpus: about 2,000 synthetic sessions over
//! Nov 12-18 for layout and timing work. The copy coverage of about 10% is a
//! labelled assumption, not a measured figure. Artifacts are matched by
//! component only, not agent or workspace, so a Codex session may record a
//! Claude-scoped copy.

use super::{Request, Session, Use, AGENTS, COMPONENTS, PLACEMENTS};

const SEED: u64 = 0x5EED_1A75_2026_1118;

const STRESS_WORKSPACES: [&str; 6] = [
    "api",
    "web",
    "billing-service",
    "mobile-app",
    "docs-site",
    "platform/services/internal-tooling/analytics-pipeline/ingest-workers/batch-reprocessing-stress",
];

const NAMES: [&str; 8] = [
    "Index rebuild",
    "Release checks",
    "Fix flaky test",
    "Refactor auth",
    "Update deps",
    "Write docs",
    "Triage issues",
    "Review PR",
];

const MODELS: [&str; 3] = [
    "Small",
    "Large",
    "synthetic-large-reasoning-model-preview-2026-11-01-long-context",
];

const RATES: [[u32; 4]; 3] = [[5, 1, 8, 20], [15, 2, 20, 60], [5, 1, 0, 20]];

/// One signal per use, accounting for all of its calls: index `count - 1`.
const SKILL_SIGNALS: [&[(&str, u32)]; 5] = [
    &[("Skill tool", 1)],
    &[("Skill tool", 2)],
    &[("Skill tool", 3)],
    &[("Skill tool", 4)],
    &[("Skill tool", 5)],
];
const TOOL_SIGNALS: [&[(&str, u32)]; 5] = [
    &[("Tool call", 1)],
    &[("Tool call", 2)],
    &[("Tool call", 3)],
    &[("Tool call", 4)],
    &[("Tool call", 5)],
];

/// Components a session can call directly. The qa-pack plugin manifest (5)
/// is left out: its use is recorded on its children, lint-guide and triage.
const CALLABLE: [usize; 9] = [0, 1, 2, 3, 4, 6, 7, 8, 9];

/// splitmix64: a tiny generator whose output depends only on `SEED`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

pub fn stress_sessions() -> Vec<Session> {
    let mut rng = Rng(SEED);
    let mut sessions = Vec::new();
    for day in 12..=18 {
        let n = 272 + rng.below(28);
        for _ in 0..n {
            // Each stress session is its own session: none spans days.
            let group = sessions.len();
            sessions.push(Session {
                group,
                ..session(&mut rng, day)
            });
        }
    }
    sessions
}

fn session(rng: &mut Rng, day: u32) -> Session {
    let workspace = rng.pick(&STRESS_WORKSPACES);
    let agent = rng.pick(&AGENTS);
    let name = rng.pick(&NAMES);
    let incomplete = rng.below(100) == 0;

    let request_count = 1 + rng.below(4);
    let mut requests = Vec::new();
    for i in 0..request_count {
        requests.push(request(rng, i == 0));
    }

    let use_count = 1 + rng.below(3);
    let mut observed = Vec::new();
    for _ in 0..use_count {
        observed.push(observed_use(rng));
    }

    Session {
        day,
        name,
        workspace,
        agent,
        incomplete,
        requests,
        observed,
        group: 0,
        used: Vec::new(),
        sources: Vec::new(),
    }
}

fn request(rng: &mut Rng, main: bool) -> Request {
    let worker = if main { "Main" } else { "Subagent" };
    let (model, milli_usd_per_ktok) = if rng.below(100) == 0 {
        ("Unpriced", None)
    } else {
        (rng.pick(&MODELS), Some(rng.pick(&RATES)))
    };
    let tokens = [
        rng.below(200_001) as u32,
        rng.below(1_000_001) as u32,
        rng.below(100_001) as u32,
        rng.below(50_001) as u32,
    ];
    Request {
        worker,
        model,
        tokens,
        milli_usd_per_ktok,
        exact: None,
    }
}

fn observed_use(rng: &mut Rng) -> Use {
    let component = rng.pick(&CALLABLE);
    let skill = COMPONENTS[component].kind == "skill";
    let (artifact, version) = if skill && rng.below(10) == 0 {
        let (artifact, version) = placement_copy(rng, component);
        (Some(artifact), version)
    } else {
        (None, None)
    };
    let count = 1 + rng.below(5) as u32;
    let signals = if skill { SKILL_SIGNALS } else { TOOL_SIGNALS };
    Use {
        component,
        version,
        artifact,
        count,
        context_cents: rng.below(10) as u32,
        signals: signals[count as usize - 1],
    }
}

/// One installed copy of `component` that has a scanned artifact.
fn placement_copy(rng: &mut Rng, component: usize) -> (&'static str, Option<&'static str>) {
    let copies: Vec<_> = PLACEMENTS
        .iter()
        .filter(|p| p.component == component)
        .filter_map(|p| p.artifact.map(|artifact| (artifact, p.version)))
        .collect();
    debug_assert!(
        !copies.is_empty(),
        "no placement with an artifact for component {component}"
    );
    rng.pick(&copies)
}
