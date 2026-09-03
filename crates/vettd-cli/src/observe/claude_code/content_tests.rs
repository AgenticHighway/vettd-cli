//! Tests for `content.rs` — the content-bearing half of the projection.
//!
//! Every test drives the real entry point, `project`, rather than calling the private helpers, so a
//! projection that stops reaching one of them fails here instead of passing vacuously.

use super::*;
use crate::observe::claude_code::project::project;
use serde_json::json;

/// Project `raw` as a line the reader consumes.
fn projected(raw: Value) -> super::super::Projected {
    project(&raw, 0, None).expect("the fixture line has a consumed type")
}

/// Project a `user` line whose message content is `content`.
fn user_content(content: Value) -> ProjectedMessage {
    projected(json!({"type": "user", "message": {"content": content}}))
        .message
        .expect("the line has a message object")
}

/// Project a meta `user` line whose message content is the string `text`.
fn meta_line(text: &str) -> ProjectedMessage {
    projected(json!({"type": "user", "isMeta": true, "message": {"content": text}}))
        .message
        .expect("the line has a message object")
}

/// The first `tool_result` block of a `user` line whose content is `content`.
fn tool_result(content: Value) -> ProjectedBlock {
    user_content(json!([{"type": "tool_result", "tool_use_id": "t", "content": content}]))
        .blocks
        .remove(0)
}

/// A tool_use block keeps only ids, names and a hash — never the input. The invariant is the
/// privacy one: a tool input is arbitrary user data (file paths, commands, prompts) and the only
/// thing the observer may remember about it is whether it was the same input twice.
#[test]
fn a_tool_use_block_keeps_a_hash_and_never_the_input() {
    let message = user_content(json!([{
        "type": "tool_use", "id": "t1", "name": "Bash",
        "input": {"command": "rm -rf /secret/path", "b": 1.5},
    }]));
    let ProjectedBlock::ToolUse {
        id,
        name,
        input_fingerprint,
        ..
    } = &message.blocks[0]
    else {
        panic!("expected a tool_use block");
    };
    assert_eq!((id.as_deref(), name.as_deref()), (Some("t1"), Some("Bash")));
    assert_eq!(input_fingerprint.len(), 64);
    let rendered = format!("{message:?}");
    assert!(!rendered.contains("secret") && !rendered.contains("rm -rf"));
}

/// `Skill` inputs name the skill under `skill` or, failing that, `name`, and an `Agent` input names
/// its subagent type. Attribution depends on these three fields; nothing else about the input
/// survives, which is why they are read here and not later.
#[test]
fn tool_use_names_the_skill_or_the_subagent_type() {
    let blocks = user_content(json!([
        {"type": "tool_use", "id": "a", "name": "Skill", "input": {"skill": "alpha", "name": "beta"}},
        {"type": "tool_use", "id": "b", "name": "Skill", "input": {"name": "beta"}},
        {"type": "tool_use", "id": "c", "name": "Skill", "input": {"skill": ""}},
        {"type": "tool_use", "id": "d", "name": "Agent", "input": {"subagent_type": "rev"}},
    ]))
    .blocks;
    let named: Vec<(Option<String>, Option<String>)> = blocks
        .iter()
        .map(|block| match block {
            ProjectedBlock::ToolUse {
                skill, agent_type, ..
            } => (skill.clone(), agent_type.clone()),
            _ => panic!("expected tool_use blocks"),
        })
        .collect();
    assert_eq!(named[0], (Some("alpha".to_string()), None));
    assert_eq!(named[1], (Some("beta".to_string()), None));
    assert_eq!(named[2], (None, None));
    assert_eq!(named[3], (None, Some("rev".to_string())));
}

/// Every phrase of `DENIAL_RE` (`claude_code.py:48-49`) is recognised anywhere in a result, and an
/// ordinary failure is not. The invariant: a denial is the operator's decision and must never move
/// an asset's observed non-success rate, so the split has to happen before the class is assigned.
#[test]
fn the_denial_phrases_are_the_prototypes_and_only_those() {
    let denials = [
        "the user doesn't want to proceed",
        "Tool call rejected by the user",
        "denied by the user",
        "permission denied",
        "permission was denied",
        "Request interrupted by user",
    ];
    for text in denials {
        let ProjectedBlock::ToolResult { denial, .. } = tool_result(json!(text)) else {
            panic!("expected a tool_result block");
        };
        assert!(denial, "expected {text:?} to read as a denial");
    }
    for text in ["command not found", "Permission Denied", "user rejected it"] {
        let ProjectedBlock::ToolResult { denial, .. } = tool_result(json!(text)) else {
            panic!("expected a tool_result block");
        };
        assert!(!denial, "expected {text:?} not to read as a denial");
    }
}

/// A tool_result reduces to four booleans and an id — the text is measured and dropped. Without
/// this the observer would be storing tool output, which is the single largest body of session
/// content there is.
#[test]
fn a_tool_result_keeps_no_text() {
    let message = user_content(json!([{
        "type": "tool_result", "tool_use_id": "t1", "is_error": "truthy string",
        "content": [{"type": "text", "text": "Async agent launched: SECRETVALUE"}, {"other": 1}],
    }]));
    assert_eq!(
        message.blocks[0],
        ProjectedBlock::ToolResult {
            tool_use_id: Some("t1".to_string()),
            is_error: true,
            denial: false,
            async_ack: true,
        }
    );
    assert!(!format!("{message:?}").contains("SECRETVALUE"));
}

/// Harness-injected user lines are recognised by their opening marker, and a `<system-reminder>`
/// counts only when it also closes the line. The invariant is that `user_turns` measures people: a
/// wake, a notification or a bare reminder is the machinery talking to itself.
#[test]
fn injected_lines_are_recognised_by_their_opening_marker() {
    let injected = [
        "<task-notification>done</task-notification>",
        "[SYSTEM NOTIFICATION] something",
        "<wake reason=\"x\">",
        "<webhook-payload>{}</webhook-payload>",
        "<system-reminder>be brief</system-reminder>",
        "   \n<system-reminder>be brief</system-reminder>  \n",
    ];
    for text in injected {
        assert!(
            user_content(json!(text)).injected,
            "expected {text:?} to read as injected"
        );
    }
    let genuine = [
        "",
        "   ",
        "please fix the build",
        "<system-reminder>be brief</system-reminder> and also fix the build",
        "here is a <task-notification> tag mid-sentence",
    ];
    for text in genuine {
        assert!(
            !user_content(json!(text)).injected,
            "expected {text:?} to read as a person's turn"
        );
    }
}

/// A `<command-name>` meta line yields the command name plus a digest of the body *after* the
/// closing tag, and notices `<skill-format>true</skill-format>`. The digest is what lets an invoked
/// skill be identified by content hash without the body ever being stored.
#[test]
fn command_from_text_digests_only_the_body_after_the_tag() {
    let body = "\n# alpha\nStep one.\n";
    let text = format!(
        "<command-message>ignored</command-message>\n<command-name>alpha</command-name>{body}"
    );
    let command = meta_line(&text).command.expect("the line names a command");
    assert_eq!(command.name, "alpha");
    assert!(!command.skill_format);
    assert_eq!(command.digest.byte_len, body.len() as i64);
    assert_eq!(command.digest.sha256, hex_sha256(body.as_bytes()));
    let deferred =
        meta_line("<command-name>beta</command-name>\n<skill-format>true</skill-format>")
            .command
            .expect("the line names a command");
    assert!(deferred.skill_format);
    assert!(meta_line("no command here").command.is_none());
    assert!(user_content(json!("<command-name>gamma</command-name>"))
        .command
        .is_none());
}

/// A meta line's whole text is reduced to a digest, and an empty one to nothing. This is how the
/// second half of the two-line skill dance is captured: the body is hashed and dropped in the same
/// expression.
#[test]
fn meta_text_digest_covers_the_whole_line_or_nothing() {
    let text = "# alpha body\nStep one.";
    let digest = meta_line(text)
        .meta_text
        .expect("a non-empty meta line has a digest");
    assert_eq!(digest.byte_len, text.len() as i64);
    assert_eq!(digest.sha256, hex_sha256(text.as_bytes()));
    assert!(meta_line("").meta_text.is_none());
    assert!(user_content(json!("not meta")).meta_text.is_none());
}

/// A skill listing keeps one length per name, counted in **characters** and zero for a name with no
/// line. The number feeds the prototype's context-cost estimate, so counting UTF-8 bytes instead
/// would silently change every reported cost for a non-ASCII listing.
#[test]
fn listing_bytes_counts_characters_and_defaults_to_zero() {
    let raw = json!({"type": "attachment", "attachment": {
        "type": "skill_listing",
        "names": ["alpha", "beta", 7],
        "content": "Available skills:\n- alpha: héllo\n",
    }});
    let Some(ProjectedAttachment::SkillListing {
        names,
        listing_bytes,
    }) = projected(raw).attachment
    else {
        panic!("expected a skill_listing attachment");
    };
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(
        listing_bytes["alpha"],
        "- alpha: héllo".chars().count() as i64
    );
    assert_eq!(listing_bytes["alpha"], 14);
    assert_eq!("- alpha: héllo".len(), 15);
    assert_eq!(listing_bytes["beta"], 0);
}

/// Tool-schema lengths are summed per MCP server, aligned by index and falling back to a prefix
/// match when the harness's two lists disagree. Non-MCP tools contribute nothing, because the
/// number exists to size an MCP server's contribution to the context.
#[test]
fn schema_bytes_sums_per_server_and_survives_misaligned_lines() {
    let names = ["mcp__s1__a", "mcp__s1__b", "Bash", "mcp__s2__c"].map(str::to_string);
    let lines = ["mcp__s1__a: xé", "MISALIGNED", "Bash: shell"].map(str::to_string);
    let bytes = schema_bytes(&names, &lines);
    // s1: "mcp__s1__a: xé" is 14 characters (15 UTF-8 bytes) and lands by index; mcp__s1__b's own
    // index holds MISALIGNED, so the prefix search runs and finds nothing, adding 0. s2 has no line
    // at all and is still present, with 0.
    assert_eq!(bytes["s1"], 14);
    assert_eq!(bytes["s2"], 0);
    assert!(!bytes.contains_key("Bash"));
}

/// A `nested_memory` attachment keeps a basename, a hash and a length — never a path and never the
/// body. A path would carry this machine's directory layout off it, which is exactly what the
/// basename rule exists to prevent.
#[test]
fn nested_memory_keeps_a_basename_and_a_hash() {
    let cases = [
        (json!("/home/someone/project/RULES.md"), "RULES.md"),
        (json!("C:\\Users\\someone\\RULES.md"), "RULES.md"),
        (json!("RULES.md"), "RULES.md"),
        (json!("/trailing/slash/"), "memory"),
        (json!(""), "memory"),
        (json!(7), "memory"),
    ];
    for (path, expected) in cases {
        let raw = json!({"type": "attachment", "attachment": {
            "type": "nested_memory", "path": path, "content": {"content": "be brief\n"}}});
        let Some(ProjectedAttachment::NestedMemory {
            basename,
            sha256,
            byte_len,
        }) = projected(raw).attachment
        else {
            panic!("expected a nested_memory attachment");
        };
        assert_eq!(basename, expected);
        assert_eq!(byte_len, 9);
        assert_eq!(sha256, hex_sha256(b"be brief\n"));
    }
}

/// An attachment subtype outside `CONSUMED_ATTACHMENTS` is projected as `Unconsumed` and carries
/// nothing at all — not even its subtype name. An uninterpreted attachment is counted, never
/// interpreted, so a new harness attachment cannot leak by being passed through.
#[test]
fn an_unconsumed_attachment_carries_nothing() {
    let raw = json!({"type": "attachment", "attachment": {
        "type": "diagnostics", "content": "SECRETVALUE", "path": "/secret"}});
    let unconsumed = projected(raw);
    assert_eq!(unconsumed.attachment, Some(ProjectedAttachment::Unconsumed));
    assert!(!format!("{unconsumed:?}").contains("SECRET"));
    assert_eq!(projected(json!({"type": "attachment"})).attachment, None);
}

/// Usage keeps integers only: a float, a bool or a string token count is "not reported" rather than
/// a guess. The envelope's token totals are summed and bounded by the gate, and a coerced value
/// would be indistinguishable from an observed one.
#[test]
fn usage_keeps_integers_and_treats_everything_else_as_unreported() {
    assert!(user_content(json!(null)).usage.is_none());
    let raw = json!({"type": "assistant", "message": {"usage": {
        "input_tokens": 10, "output_tokens": 2.5, "cache_read_input_tokens": true,
        "output_tokens_details": {"thinking_tokens": 7}}}});
    let usage = projected(raw)
        .message
        .and_then(|message| message.usage)
        .expect("the line has a usage object");
    assert_eq!(
        usage,
        ProjectedUsage {
            input_tokens: Some(10),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            output_tokens: None,
            thinking_tokens: Some(7),
        }
    );
}

/// A `toolUseResult` keeps four fields and coerces its agent id to a string; a missing object is
/// all-false rather than absent. `agentId` is the only link from a spawn to the child transcript it
/// produced, so a numeric one must not be dropped.
#[test]
fn tool_use_result_keeps_four_fields_and_stringifies_the_agent_id() {
    let raw = json!({"type": "user", "toolUseResult": {
        "interrupted": 1, "isAsync": "yes", "agentId": 77, "status": "done", "prompt": "SECRETVALUE"}});
    let line = projected(raw);
    assert_eq!(
        line.tool_use_result,
        ProjectedToolUseResult {
            interrupted: true,
            is_async: true,
            agent_id: Some("77".to_string()),
            status: Some("done".to_string()),
        }
    );
    assert!(!format!("{line:?}").contains("SECRETVALUE"));
    assert_eq!(
        projected(json!({"type": "user"})).tool_use_result,
        ProjectedToolUseResult::default()
    );
}
