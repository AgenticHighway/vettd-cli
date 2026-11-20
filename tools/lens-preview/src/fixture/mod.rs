use serde::Serialize;

pub const PERIOD: &str = "Nov 12-18, 2026 UTC";
pub const SNAPSHOT: &str = "Snapshot Nov 18 09:00 UTC";
pub const PARTS: [&str; 4] = ["Fresh input", "Cache read", "Cache write", "Output"];
pub const WORKSPACES: [&str; 2] = ["api", "web"];
pub const AGENTS: [&str; 2] = ["Claude", "Codex"];
pub const KINDS: [&str; 7] = [
    "skill", "tool", "MCP", "plugin", "command", "hook", "unknown",
];

/// A component groups copies that share a native kind and display name.
/// It is a browsing aid only: evidence and actions use exact placements.
#[derive(Serialize)]
pub struct Component {
    pub kind: &'static str,
    pub name: &'static str,
}

const fn component(kind: &'static str, name: &'static str) -> Component {
    Component { kind, name }
}

pub const COMPONENTS: [Component; 10] = [
    component("skill", "review-kit"),
    component("tool", "exec"),
    component("tool", "read-file"),
    component("MCP", "issue-index"),
    component("skill", "release-notes"),
    component("plugin", "qa-pack"),
    component("skill", "lint-guide"),
    component("command", "triage"),
    component("hook", "PreToolUse/Bash"),
    component("unknown", "tools.yaml"),
];

/// A configured copy in the inventory snapshot. `workspace: None` means the
/// copy is not tied to one workspace (user scope or built-in).
#[derive(Serialize)]
pub struct Placement {
    pub component: usize,
    pub agent: Option<&'static str>,
    pub workspace: Option<&'static str>,
    pub scope: &'static str,
    pub locator: &'static str,
    pub version: Option<&'static str>,
    pub artifact: Option<&'static str>,
    pub format: &'static str,
    pub parent: Option<usize>,
    pub native: &'static [&'static str],
    pub can_disable: bool,
}

const SKILL_SPEC: &str = "Agent Skills SKILL.md";
const BASE: Placement = Placement {
    component: 0,
    agent: Some("Claude"),
    workspace: None,
    scope: "Workspace",
    locator: "",
    version: None,
    artifact: None,
    format: SKILL_SPEC,
    parent: None,
    native: &[],
    can_disable: false,
};

pub const PLACEMENTS: [Placement; 12] = [
    Placement {
        workspace: Some("api"),
        locator: "api/.claude/skills/review-kit/SKILL.md",
        version: Some("1.5"),
        artifact: Some("sha:7b20"),
        can_disable: true,
        ..BASE
    },
    Placement {
        scope: "User",
        locator: "~/.claude/skills/review-kit/SKILL.md",
        version: Some("1.4"),
        artifact: Some("sha:1c4e"),
        ..BASE
    },
    Placement {
        agent: Some("Codex"),
        workspace: Some("web"),
        locator: "web/.agents/skills/review-kit/SKILL.md",
        version: Some("1.5"),
        artifact: Some("sha:e913"),
        can_disable: true,
        ..BASE
    },
    Placement {
        component: 1,
        agent: Some("Codex"),
        scope: "Built-in",
        locator: "Codex exec wrapper",
        format: "Host built-in tool",
        native: &["Inner commands: not recorded"],
        ..BASE
    },
    Placement {
        component: 2,
        scope: "Built-in",
        locator: "Claude read-file tool",
        format: "Host built-in tool",
        ..BASE
    },
    Placement {
        component: 3,
        agent: Some("Codex"),
        workspace: Some("web"),
        locator: "web/.codex/config.toml mcp_servers",
        format: "Codex config (host format)",
        native: &[
            "Connection: configured, stdio",
            "Capabilities: not probed",
            "Server identity: unverified",
        ],
        ..BASE
    },
    Placement {
        component: 4,
        scope: "User",
        locator: "~/.claude/skills/release-notes/SKILL.md",
        version: Some("2.0"),
        artifact: Some("sha:52aa"),
        ..BASE
    },
    Placement {
        component: 5,
        workspace: Some("api"),
        locator: "api/.claude/plugins/qa-pack/plugin.json",
        version: Some("0.3"),
        artifact: Some("sha:90d1"),
        format: "Agent Plugins manifest",
        native: &["Use is recorded on contained items"],
        ..BASE
    },
    Placement {
        component: 6,
        workspace: Some("api"),
        scope: "Plugin",
        locator: "qa-pack/skills/lint-guide/SKILL.md",
        artifact: Some("sha:3f6b"),
        parent: Some(7),
        ..BASE
    },
    Placement {
        component: 7,
        workspace: Some("api"),
        scope: "Plugin",
        locator: "qa-pack/commands/triage.md",
        artifact: Some("sha:c822"),
        format: "Claude Code command (host)",
        parent: Some(7),
        native: &["Entry point: /qa-pack:triage"],
        ..BASE
    },
    Placement {
        component: 8,
        workspace: Some("api"),
        locator: "api/.claude/settings.json hooks",
        artifact: Some("sha:0be4"),
        format: "Claude Code hooks (host)",
        native: &[
            "Event: PreToolUse",
            "Matcher: Bash",
            "Settings layer: project",
        ],
        ..BASE
    },
    Placement {
        component: 9,
        agent: None,
        workspace: Some("web"),
        scope: "Unknown",
        locator: "web/.agent-tools/tools.yaml",
        artifact: Some("sha:d71c"),
        format: "Unrecognized",
        native: &[
            "Kind, host and scope: unknown",
            "Not executed during discovery",
        ],
        ..BASE
    },
];

/// Where tokens entered context, in basis points of the corpus total.
/// Illustrative: the fixture's requests don't record their sources. Hooks
/// stay at zero because they run outside the model.
pub const SOURCE_SHARES: [(&str, u32); 6] = [
    ("File reads", 3_800),
    ("Skills", 2_400),
    ("Bash output", 1_800),
    ("MCP tools", 1_100),
    ("Prompts and replies", 900),
    ("Hooks", 0),
];

/// "Now" for plan windows: Nov 18 11:45, in minutes from Nov 18 00:00.
pub const NOW: i32 = 11 * 60 + 45;

/// One provider usage window, as its source reports it. Codex writes these
/// to its session logs (`rate_limits.primary` / `secondary`); Claude sends
/// them only to the statusline (`rate_limits.five_hour` / `seven_day`), so
/// they reach Lens through the opt-in relay.
#[derive(Serialize)]
pub struct Window {
    pub agent: &'static str,
    pub name: &'static str,
    pub source: &'static str,
    pub minutes: u32,
    /// Window start, in minutes from Nov 18 00:00.
    pub start: i32,
    /// (minutes since start, used percent) as each reading arrived.
    pub readings: &'static [(u32, u32)],
}

pub const WINDOWS: [Window; 4] = [
    Window {
        agent: "Claude",
        name: "5-hour",
        source: "relay",
        minutes: 300,
        start: 9 * 60,
        readings: &[(30, 9), (90, 31), (165, 62)],
    },
    Window {
        agent: "Claude",
        name: "weekly",
        source: "relay",
        minutes: 7 * 1440,
        start: -2 * 1440 + 9 * 60,
        readings: &[(1440, 25), (3045, 31)],
    },
    // One reading so far: too few for a pace.
    Window {
        agent: "Codex",
        name: "5-hour",
        source: "logs",
        minutes: 300,
        start: 10 * 60,
        readings: &[(105, 12)],
    },
    Window {
        agent: "Codex",
        name: "weekly",
        source: "logs",
        minutes: 7 * 1440,
        start: -3 * 1440 + 22 * 60 + 10,
        readings: &[(1000, 33), (3695, 44)],
    },
];

/// Used share at each of the last four Claude weekly resets, oldest first.
pub const CLAUDE_WEEKLY_ENDS: [u32; 4] = [41, 63, 38, 55];
/// Requests refused with a structured `rate_limit` error in those weeks.
pub const LIMIT_HITS: u32 = 3;

/// Scan evidence is keyed by artifact identity, never by version label.
pub const SCANS: [(&str, &str); 2] = [
    ("sha:1c4e", "Fixture scan RK-14: A"),
    ("sha:7b20", "Fixture scan RK-15: B"),
];

mod sessions;
pub use sessions::*;
mod stress;
pub use stress::*;
