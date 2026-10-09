//! Projection of the content-bearing parts of a line — messages, blocks and attachments.
//!
//! Split out of `project.rs` for the file-length budget. This is where session prose is briefly
//! materialised and immediately reduced: every function below turns text into a hash, a length or a
//! boolean before it returns, and no function returns a string that came out of a transcript.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use super::fingerprint::sha256_json;
use super::projected::{
    Digest, ProjectedAttachment, ProjectedBlock, ProjectedCommand, ProjectedMessage,
    ProjectedToolUseResult, ProjectedUsage,
};
use super::{
    int_or_none, mcp_server, nonempty_str, str_list, str_value, stringify_if_truthy, truthy,
};
use crate::observe::canonical::hex_sha256;

/// Attachment subtypes the reader interprets (`claude_code.py:41-43`, `CONSUMED_ATTACHMENTS`).
/// Anything else becomes [`ProjectedAttachment::Unconsumed`] and is counted, never interpreted.
const CONSUMED_ATTACHMENTS: [&str; 5] = [
    "skill_listing",
    "deferred_tools_delta",
    "agent_listing_delta",
    "mcp_instructions_delta",
    "nested_memory",
];

/// Prefix of the harness's acknowledgement that an agent was launched asynchronously
/// (`claude_code.py:46`). A call whose result starts with it has no latency to measure.
const ASYNC_ACK_PREFIX: &str = "Async agent launched";

/// Leading markers of a `user` line the harness injected rather than a person typed
/// (`claude_code.py:267`). `<system-reminder>` is handled separately in [`is_injected`]: it counts
/// only when it also *closes* the line, because a reminder appended to a real prompt is still a
/// real prompt.
const INJECTED_PREFIXES: [&str; 4] = [
    "<task-notification>",
    "[SYSTEM NOTIFICATION",
    "<wake ",
    "<webhook-payload",
];

/// The phrases that mark a failed tool result as the operator's decision rather than the asset's
/// fault (`claude_code.py:48-49`). Verbatim from the code, which is authoritative over
/// `prototype/CONTRACTS.md`'s variant of the same regex.
static DENIAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"doesn't want to proceed|rejected by the user|denied by the user|permission (?:was )?denied|Request interrupted by user",
    )
    .expect("the denial regex is a literal alternation and always compiles")
});

/// The slash-command name the harness stamps on a meta line (`claude_code.py:50`).
static COMMAND_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<command-name>([^<\s]+)</command-name>")
        .expect("the command-name regex always compiles")
});

/// Project a `message` object (`_project_message`, `claude_code.py:215-229`).
pub(crate) fn project_message(msg: &Map<String, Value>, is_meta: bool) -> ProjectedMessage {
    let content = msg.get("content");
    let blocks = content
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_object)
                .map(project_block)
                .collect()
        })
        .unwrap_or_default();
    ProjectedMessage {
        // Not `nonempty_str`: `claude_code.py:221` keeps an empty id, and it reaches
        // `ToolCall.message_id` unfiltered. `apply.rs` applies the truthiness test instead.
        id: str_value(msg.get("id")),
        model: nonempty_str(msg.get("model")),
        stop_reason: nonempty_str(msg.get("stop_reason")),
        usage: msg
            .get("usage")
            .and_then(Value::as_object)
            .map(project_usage),
        content_is_str: content.is_some_and(Value::is_string),
        blocks,
        command: if is_meta {
            command_from_text(content)
        } else {
            None
        },
        meta_text: if is_meta {
            meta_text_digest(content)
        } else {
            None
        },
        injected: is_injected(content),
    }
}

/// Project a `usage` object (`_project_usage`, `claude_code.py:232-236`).
fn project_usage(usage: &Map<String, Value>) -> ProjectedUsage {
    ProjectedUsage {
        input_tokens: int_or_none(usage.get("input_tokens")),
        cache_creation_input_tokens: int_or_none(usage.get("cache_creation_input_tokens")),
        cache_read_input_tokens: int_or_none(usage.get("cache_read_input_tokens")),
        output_tokens: int_or_none(usage.get("output_tokens")),
        thinking_tokens: usage
            .get("output_tokens_details")
            .and_then(Value::as_object)
            .and_then(|details| int_or_none(details.get("thinking_tokens"))),
    }
}

/// Project one content block (`_project_block`, `claude_code.py:239-256`).
fn project_block(block: &Map<String, Value>) -> ProjectedBlock {
    match block.get("type").and_then(Value::as_str) {
        Some("tool_use") => {
            let input = block.get("input");
            let fields = input.and_then(Value::as_object);
            ProjectedBlock::ToolUse {
                id: nonempty_str(block.get("id")),
                name: nonempty_str(block.get("name")),
                input_fingerprint: sha256_json(input.unwrap_or(&Value::Null)),
                skill: fields.and_then(|fields| {
                    nonempty_str(fields.get("skill")).or_else(|| nonempty_str(fields.get("name")))
                }),
                agent_type: fields.and_then(|fields| nonempty_str(fields.get("subagent_type"))),
            }
        }
        Some("tool_result") => {
            let text = result_text(block.get("content"));
            ProjectedBlock::ToolResult {
                tool_use_id: nonempty_str(block.get("tool_use_id")),
                is_error: truthy(block.get("is_error")),
                denial: DENIAL_RE.is_match(&text),
                async_ack: text.starts_with(ASYNC_ACK_PREFIX),
            }
        }
        Some("text") => ProjectedBlock::Text,
        _ => ProjectedBlock::Other,
    }
}

/// The `toolUseResult` fields the reader keeps (`claude_code.py:178-181`).
pub(crate) fn project_tool_use_result(value: Option<&Value>) -> ProjectedToolUseResult {
    let Some(object) = value.and_then(Value::as_object) else {
        return ProjectedToolUseResult::default();
    };
    ProjectedToolUseResult {
        interrupted: truthy(object.get("interrupted")),
        is_async: truthy(object.get("isAsync")),
        agent_id: stringify_if_truthy(object.get("agentId")),
        status: nonempty_str(object.get("status")),
    }
}

/// The text of a `content` value: the string itself, or the `text` fields of its blocks joined by
/// newlines (`_result_text`, `claude_code.py:259-264`).
///
/// This is the one function that materialises session prose. Every caller reduces its result to a
/// hash, a length or a boolean before returning, and the string is dropped at the end of the call.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Whether a `user` line was injected by the harness rather than typed by a person (`_is_injected`,
/// `claude_code.py:270-278`).
///
/// Decided on the leading bytes of the text, which are then discarded. A `<system-reminder>` counts
/// only when it also closes the line, so a reminder appended to a real prompt leaves the turn a
/// turn.
fn is_injected(content: Option<&Value>) -> bool {
    let text = result_text(content);
    // Python's `str.lstrip()` strips by `str.isspace()`, Rust's `trim_start` by the Unicode
    // White_Space property; they differ only on C1 control characters, which no prompt begins with.
    let text = text.trim_start();
    if text.is_empty() {
        return false;
    }
    if text.starts_with("<system-reminder>") && text.trim_end().ends_with("</system-reminder>") {
        return true;
    }
    INJECTED_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

/// The command name and body digest of a meta line, when it names one (`_command_from_text`,
/// `claude_code.py:281-286`).
///
/// The body is everything after the closing tag; `byte_len` is its UTF-8 length.
fn command_from_text(content: Option<&Value>) -> Option<ProjectedCommand> {
    let text = result_text(content);
    let matched = COMMAND_NAME_RE.captures(&text)?;
    let whole = matched.get(0).expect("group 0 always exists");
    Some(ProjectedCommand {
        name: matched
            .get(1)
            .expect("the pattern has one capture group")
            .as_str()
            .to_string(),
        digest: digest_of(&text[whole.end()..]),
        skill_format: text.contains("<skill-format>true</skill-format>"),
    })
}

/// Hash and length of a meta line's whole text, or `None` when it has none (`_meta_text_digest`,
/// `claude_code.py:289-293`).
///
/// The harness injects a skill body as its own meta line right after the command line; this is how
/// that body is captured without keeping it.
fn meta_text_digest(content: Option<&Value>) -> Option<Digest> {
    let text = result_text(content);
    if text.is_empty() {
        return None;
    }
    Some(digest_of(&text))
}

/// sha256 and UTF-8 length of `text`.
fn digest_of(text: &str) -> Digest {
    Digest {
        sha256: hex_sha256(text.as_bytes()),
        byte_len: text.len() as i64,
    }
}

/// Project an `attachment` (`_project_attachment`, `claude_code.py:296-322`).
pub(crate) fn project_attachment(attachment: &Map<String, Value>) -> ProjectedAttachment {
    let kind = attachment.get("type").and_then(Value::as_str).unwrap_or("");
    if !CONSUMED_ATTACHMENTS.contains(&kind) {
        return ProjectedAttachment::Unconsumed;
    }
    match kind {
        "skill_listing" => {
            let names = str_list(attachment.get("names"));
            ProjectedAttachment::SkillListing {
                listing_bytes: listing_bytes(&names, attachment.get("content")),
                names,
            }
        }
        "deferred_tools_delta" => project_deferred_tools(attachment),
        "agent_listing_delta" => ProjectedAttachment::AgentListingDelta {
            types: str_list(attachment.get("addedTypes")),
            is_initial: truthy(attachment.get("isInitial")),
        },
        "mcp_instructions_delta" => ProjectedAttachment::McpInstructionsDelta {
            names: str_list(attachment.get("addedNames")),
        },
        _ => project_nested_memory(attachment),
    }
}

/// The `deferred_tools_delta` branch of [`project_attachment`].
fn project_deferred_tools(attachment: &Map<String, Value>) -> ProjectedAttachment {
    let added = str_list(attachment.get("addedNames"));
    let schema_bytes = schema_bytes(&added, &str_list(attachment.get("addedLines")));
    ProjectedAttachment::DeferredToolsDelta {
        added,
        pending: str_list(attachment.get("pendingMcpServers")),
        failed: str_list(attachment.get("failedMcpServers")),
        removed: str_list(attachment.get("removedNames")),
        readded: str_list(attachment.get("readdedNames")),
        schema_bytes,
    }
}

/// The `nested_memory` branch of [`project_attachment`].
///
/// Harness 2.1.x nests the body one level deeper (`{path, type, content, contentDiffersFromDisk}`);
/// both shapes are read, and only the basename of the path survives.
fn project_nested_memory(attachment: &Map<String, Value>) -> ProjectedAttachment {
    let content = match attachment.get("content") {
        Some(Value::Object(inner)) => inner.get("content"),
        other => other,
    };
    let body = content.and_then(Value::as_str).unwrap_or("");
    let path = attachment.get("path").and_then(Value::as_str).unwrap_or("");
    // `os.path.basename` on the collector's platform, except that both separators are honoured
    // everywhere: a Windows harness writes `C:\...\CLAUDE.md`, and splitting on `/` alone would put
    // that whole path into a name field. Never `std::path::Path`, whose answer depends on the
    // machine reading the log rather than on the machine that wrote it.
    let basename = path.rsplit(['/', '\\']).next().unwrap_or("");
    ProjectedAttachment::NestedMemory {
        basename: if basename.is_empty() {
            "memory".to_string()
        } else {
            basename.to_string()
        },
        sha256: hex_sha256(body.as_bytes()),
        byte_len: body.len() as i64,
    }
}

/// Length of each named skill's line in a skill listing (`claude_code.py:301-303`).
///
/// Every name gets an entry, `0` when no line matches. **These are character counts, not bytes**:
/// the Python measures `str` values with `len()`, and `listing_bytes` feeds a context-size estimate
/// that must match the prototype's numbers, so the misleading name is kept and the semantics with
/// it.
fn listing_bytes(names: &[String], content: Option<&Value>) -> BTreeMap<String, i64> {
    let content = content.and_then(Value::as_str).unwrap_or("");
    let lines: Vec<&str> = if content.is_empty() {
        Vec::new()
    } else {
        content.split('\n').collect()
    };
    names
        .iter()
        .map(|name| {
            let prefix = format!("- {name}:");
            let line = lines
                .iter()
                .find(|line| line.starts_with(&prefix))
                .copied()
                .unwrap_or("");
            (name.clone(), line.chars().count() as i64)
        })
        .collect()
}

/// Lengths of the tool-listing lines, summed per MCP server (`_schema_bytes`,
/// `claude_code.py:325-334`).
///
/// Lines align with names by index; when they do not, the first line with the name as its prefix is
/// used. Character counts, for the same reason as [`listing_bytes`].
pub(crate) fn schema_bytes(names: &[String], lines: &[String]) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for (index, name) in names.iter().enumerate() {
        let Some(server) = mcp_server(name) else {
            continue;
        };
        let line = lines
            .get(index)
            .filter(|line| line.starts_with(name))
            .or_else(|| lines.iter().find(|line| line.starts_with(name)))
            .map_or("", String::as_str);
        *out.entry(server.to_string()).or_insert(0) += line.chars().count() as i64;
    }
    out
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
