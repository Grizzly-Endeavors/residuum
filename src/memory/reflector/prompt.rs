//! Prompt construction for the reflector LLM call.

use crate::models::Message;

/// User-customizable content guidance — default when `memory/REFLECTOR.md` is absent.
///
/// The workspace bootstrap writes this same content to disk so users can customise
/// it without recompiling. The format spec is always appended by code.
pub(super) const REFLECTION_CONTENT_PROMPT: &str = "You are a memory reorganization system. Given a list of observations, merge and deduplicate them to reduce size while preserving all important information.

Rules:
- Merge related observations into single, precise sentences
- Do NOT summarize — preserve specific details
- Remove redundant or duplicate observations
- Each output object should have a complete, self-contained content sentence";

/// Output format spec — always appended by code, never stored in editable files.
///
/// This is injected unconditionally so editing `REFLECTOR.md` cannot break JSON parsing.
/// The structural JSON requirements are enforced by the model's structured output mode
/// (JSON schema), so this spec focuses on semantic field guidance only.
pub(super) const REFLECTION_FORMAT_SPEC: &str = r#"For each observation:
- "content" (string): the merged observation as a complete, self-contained sentence
- "timestamp": timestamp at minute precision (YYYY-MM-DDTHH:MM) — use the most recent timestamp from the source observations being merged
- "project_context" (string): use the most relevant context from the source observations
- "visibility" ("user" or "background"): use "background" only if all source observations were background"#;

/// JSON schema for the reflector response, used with structured output mode.
///
/// Returns a schema requiring `observations` (array of items with content, timestamp,
/// `project_context`, visibility). Wrapped in an object because `OpenAI` structured output
/// doesn't support top-level arrays.
#[must_use]
pub(super) fn reflector_response_schema() -> serde_json::Value {
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
                        "project_context": { "type": "string" },
                        "visibility": { "type": "string", "enum": ["user", "background"] }
                    },
                    "required": ["content", "timestamp", "project_context", "visibility"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["observations"],
        "additionalProperties": false
    })
}

/// Build the reflection prompt with the serialized observation list.
///
/// Injects the format spec alongside user-customizable content guidance so the
/// format requirement cannot be lost by editing the disk file.
pub(super) fn build_reflection_prompt(
    serialized_observations: &str,
    content_guidance: &str,
) -> Vec<Message> {
    let system = format!("{content_guidance}\n\n{REFLECTION_FORMAT_SPEC}");
    vec![
        Message::system(system),
        Message::user(format!(
            "Reorganize and compress these observations:\n\n{serialized_observations}"
        )),
    ]
}
