//! Prompt construction for the observer LLM call.

use crate::memory::recent_messages::RecentMessage;
use crate::memory::types::Visibility;
use crate::models::Message;

/// User-customizable content guidance — default when `memory/OBSERVER.md` is absent.
///
/// The workspace bootstrap writes this same content to disk so users can customise
/// it without recompiling. The format spec is always appended by code.
pub(super) const EXTRACTION_CONTENT_PROMPT: &str =
    "You are a memory extraction system. Given a conversation segment, extract key observations.

For each observation, capture:
- Key decisions made and their rationale
- Problems encountered and their solutions
- Corrections or mistakes that were fixed
- Important technical details or patterns discovered
- Action items or next steps identified

Each observation should be a complete sentence useful as future context. Be specific and concise.";

/// Output format spec — always appended by code, never stored in editable files.
///
/// This is injected unconditionally so editing `OBSERVER.md` cannot break JSON parsing.
/// The structural JSON requirements are enforced by the model's structured output mode
/// (JSON schema), so this spec focuses on semantic field guidance only.
pub(super) const EXTRACTION_FORMAT_SPEC: &str = r#"For each observation:
- "content": a complete, self-contained observation sentence
- "timestamp": timestamp at minute precision (YYYY-MM-DDTHH:MM) matching the most relevant message
- "visibility": "user" if the observation involves a user-visible turn, "background" if from a system/background turn

"narrative": a 2-4 sentence summary of what was being discussed and where things left off,
written as if briefing someone who needs to continue the conversation. Include the current
topic, any open questions, and the overall direction of the conversation.
Set to empty string if there is insufficient context for a meaningful narrative."#;

/// JSON schema for the observer response, used with structured output mode.
///
/// Returns a schema requiring `observations` (array of items with content, timestamp,
/// visibility) and `narrative` (string, required but empty string maps to `None`).
#[must_use]
pub(super) fn observer_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "observations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string" },
                        "timestamp": { "type": "string" },
                        "visibility": { "type": "string", "enum": ["user", "background"] }
                    },
                    "required": ["content", "timestamp", "visibility"],
                    "additionalProperties": false
                }
            },
            "narrative": { "type": "string" }
        },
        "required": ["observations", "narrative"],
        "additionalProperties": false
    })
}

/// Format a single `RecentMessage` for the extraction prompt transcript.
///
/// Includes timestamp, role, project context, visibility, content, and any
/// tool calls or tool call IDs, so the observer LLM has full context.
pub(super) fn format_recent_message(rm: &RecentMessage) -> String {
    let role = rm.message.role.as_str();
    let timestamp = rm.timestamp.format("%Y-%m-%dT%H:%M:%S").to_string();
    let visibility = match rm.visibility {
        Visibility::User => "user",
        Visibility::Background => "background",
    };
    let tool_call_id_part = rm
        .message
        .tool_call_id
        .as_deref()
        .map_or_else(String::new, |id| format!(" (call: {id})"));

    let header = format!(
        "[{timestamp}] [{role}]{tool_call_id_part} (project: {}, visibility: {visibility}):",
        rm.project_context
    );

    let mut parts = vec![header];

    if !rm.message.content.is_empty() {
        parts.push(rm.message.content.clone());
    }

    if let Some(tool_calls) = &rm.message.tool_calls {
        let mut tc_lines = vec!["  tool_calls:".to_string()];
        for tc in tool_calls {
            tc_lines.push(format!(
                "    - {}({}) [id: {}]",
                tc.name, tc.arguments, tc.id
            ));
        }
        parts.push(tc_lines.join("\n"));
    }

    parts.join("\n")
}

/// Build the extraction prompt for the observer model.
///
/// Injects the format spec alongside user-customizable content guidance so the
/// format requirement cannot be lost by editing the disk file.
pub(super) fn build_extraction_prompt(
    recent_messages: &[RecentMessage],
    content_guidance: &str,
) -> Vec<Message> {
    let system = format!("{content_guidance}\n\n{EXTRACTION_FORMAT_SPEC}");
    let transcript = recent_messages
        .iter()
        .map(format_recent_message)
        .collect::<Vec<_>>()
        .join("\n\n");

    vec![
        Message::system(system),
        Message::user(format!(
            "Extract observations from this conversation segment:\n\n{transcript}"
        )),
    ]
}
