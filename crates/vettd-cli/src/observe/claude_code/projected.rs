//! The shape a projected transcript line takes — data only, no logic.
//!
//! These types mirror the dicts `sources/claude_code.py`'s `_project*` functions build, key for
//! key, with the Python's per-branch dicts closed into enums. They are the seam `apply.rs` codes
//! against, so they are re-exported from [`super`] (`claude_code::project`) rather than named
//! through this private submodule.
//!
//! Every field is a hash, a length, a boolean, a count or a closed-vocabulary name. There is no
//! field for message text, thinking, a tool input, a tool result or an attachment body, which is
//! what makes "the raw line is dropped after projection" a property of the type rather than of a
//! reviewer's memory.

use serde_json::Value;
use std::collections::BTreeMap;

/// The four line types the reader consumes (`claude_code.py:40`, `CONSUMED_TYPES`), closed in the
/// type so a typo cannot invent a fifth. A line of any other type is counted as unknown and never
/// projected, which is why [`super::project`] returns `None` for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineKind {
    User,
    Assistant,
    Attachment,
    Summary,
}

impl LineKind {
    /// The `type` of a line, or `None` when the reader does not consume it.
    pub(crate) fn from_type(value: Option<&Value>) -> Option<LineKind> {
        match value.and_then(Value::as_str)? {
            "user" => Some(LineKind::User),
            "assistant" => Some(LineKind::Assistant),
            "attachment" => Some(LineKind::Attachment),
            "summary" => Some(LineKind::Summary),
            _ => None,
        }
    }
}

/// A sha256 and a byte length — everything that is kept about a piece of in-band text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Digest {
    pub sha256: String,
    pub byte_len: i64,
}

/// A `<command-name>` meta line: the command's name plus the digest of the body after the tag
/// (`_command_from_text`, `claude_code.py:281-286`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedCommand {
    pub name: String,
    pub digest: Digest,
    /// `true` when the line also carries `<skill-format>true</skill-format>`, which means the body
    /// is not here yet: it arrives on the following meta line.
    pub skill_format: bool,
}

/// Token usage of one API response (`_project_usage`, `claude_code.py:232-236`).
///
/// Every field is `None` when the provider did not report it, which the envelope must keep distinct
/// from a reported zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectedUsage {
    pub input_tokens: Option<i64>,
    pub cache_creation_input_tokens: Option<i64>,
    pub cache_read_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub thinking_tokens: Option<i64>,
}

/// One content block (`_project_block`, `claude_code.py:239-256`).
///
/// The Python returns a dict that always has `type` and gains the other keys per branch; the port
/// closes that into an enum, so a `tool_result` field cannot be read off a `tool_use`. `Text` is
/// distinct from `Other` because "did this user line contain prose" decides whether it counts as a
/// turn, while `thinking` and every other block type only need to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectedBlock {
    ToolUse {
        id: Option<String>,
        name: Option<String>,
        /// sha256 of the canonicalised `input` — see [`super::sha256_json`]. Local only.
        input_fingerprint: String,
        /// `input.skill` or `input.name`, for `Skill` calls (local only).
        skill: Option<String>,
        /// `input.subagent_type`, for `Agent` spawns (local only).
        agent_type: Option<String>,
    },
    ToolResult {
        tool_use_id: Option<String>,
        is_error: bool,
        /// Whether the result text matched the denial phrases; the text itself is dropped.
        denial: bool,
        /// Whether the result text began with the async-launch acknowledgement.
        async_ack: bool,
    },
    Text,
    Other,
}

/// A projected `message` object (`_project_message`, `claude_code.py:215-229`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectedMessage {
    pub id: Option<String>,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
    pub usage: Option<ProjectedUsage>,
    /// Whether `content` was a bare string rather than a block list — a prose turn either way.
    pub content_is_str: bool,
    pub blocks: Vec<ProjectedBlock>,
    /// Only ever `Some` on a meta line.
    pub command: Option<ProjectedCommand>,
    /// Digest of a meta line's whole text, which is how an injected skill body is captured.
    pub meta_text: Option<Digest>,
    /// Whether the harness injected this line (a notification, a wake, a reminder-only line).
    pub injected: bool,
}

/// The four `toolUseResult` fields the reader keeps (`claude_code.py:178-181`).
///
/// `status` is projected for fidelity with the Python's key list and is read by nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectedToolUseResult {
    pub interrupted: bool,
    pub is_async: bool,
    /// The spawned child's session key (local only).
    pub agent_id: Option<String>,
    pub status: Option<String>,
}

/// A projected `attachment` (`_project_attachment`, `claude_code.py:296-322`).
///
/// As with [`ProjectedBlock`], the Python's per-branch dict becomes an enum. Every `bytes` map holds
/// lengths of lines the reader measured and dropped, never the lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectedAttachment {
    SkillListing {
        names: Vec<String>,
        /// Skill name -> length of its line in the listing (see `listing_bytes` on why that length
        /// is counted in characters).
        listing_bytes: BTreeMap<String, i64>,
    },
    DeferredToolsDelta {
        added: Vec<String>,
        pending: Vec<String>,
        failed: Vec<String>,
        removed: Vec<String>,
        readded: Vec<String>,
        /// MCP server -> summed length of its tool-listing lines.
        schema_bytes: BTreeMap<String, i64>,
    },
    AgentListingDelta {
        types: Vec<String>,
        is_initial: bool,
    },
    McpInstructionsDelta {
        names: Vec<String>,
    },
    NestedMemory {
        /// Basename only: a path would carry this machine's directory layout.
        basename: String,
        sha256: String,
        byte_len: i64,
    },
    /// An attachment subtype outside `CONSUMED_ATTACHMENTS`. Its subtype name is not kept: there is
    /// no field on the wire for it, and the caller only counts it.
    Unconsumed,
}

/// One transcript line, reduced to what the reader may keep (`_project`, `claude_code.py:171-188`).
///
/// The top-level fields are named one for one after the harness keys they come from. `uuid`,
/// `parent_uuid`, `session_id`, `is_sidechain`, `agent_id`, `source_tool_assistant_uuid` and
/// `line_len` are projected for fidelity with the Python and read by nothing today; the identifying
/// ones among them also reach [`Projected::names`], which is the copy the gate actually consumes.
#[derive(Debug, Clone)]
pub(crate) struct Projected {
    pub kind: LineKind,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub timestamp: Option<String>,
    pub session_id: Option<String>,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,
    pub version: Option<String>,
    pub entrypoint: Option<String>,
    pub permission_mode: Option<String>,
    pub effort: Option<String>,
    pub source_tool_assistant_uuid: Option<String>,
    pub is_meta: bool,
    /// Length in bytes of the raw line this was projected from.
    pub line_len: u64,
    /// `(forbids bucket, value)` pairs harvested from the line — local-only names for the gate.
    pub names: Vec<(&'static str, String)>,
    pub message: Option<ProjectedMessage>,
    pub tool_use_result: ProjectedToolUseResult,
    /// `None` when the line had no `attachment` object at all.
    pub attachment: Option<ProjectedAttachment>,
    /// Whether `attributionAgent` equals the agent type the child's sidecar declared.
    pub attribution_matches: bool,
    /// `attributionMcpServer`: the server the harness says produced this response (local only).
    pub mcp_attribution: Option<String>,
}
