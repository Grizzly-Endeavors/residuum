use residuum_inference::Message;

pub(super) const REFLECTION_CONTENT_PROMPT: &str =
    include_str!("../../../../assets/workspace-bootstrap/memory/REFLECTOR.md");

pub(super) const REFLECTION_FORMAT_SPEC: &str = r#"For each observation:
- "content" (string): the merged observation as a complete, self-contained sentence
- "timestamp": timestamp at minute precision (YYYY-MM-DDTHH:MM) — use the most recent timestamp from the source observations being merged
- "visibility" ("user" or "background"): use "background" only if all source observations were background"#;

/// JSON schema for the reflector response, used with structured output mode.
///
/// Returns a schema requiring `observations` (array of items with content, timestamp,
/// visibility). Wrapped in an object because `OpenAI` structured output
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
                        "visibility": { "type": "string", "enum": ["user", "background"] }
                    },
                    "required": ["content", "timestamp", "visibility"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["observations"],
        "additionalProperties": false
    })
}

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
