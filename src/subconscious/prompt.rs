//! Prompt construction for the subconscious classifier LLM call.

use super::EvalPhase;
use crate::models::{Message, Role};

/// User-customizable check guidance — default when `SUBCONSCIOUS.md` is absent.
///
/// The workspace bootstrap writes this same content to disk so users can
/// customise it without recompiling. The format spec is always appended by code.
pub(super) const DEFAULT_SUBCONSCIOUS_PROMPT: &str =
    include_str!("../../assets/workspace-bootstrap/SUBCONSCIOUS.md");

/// Output format spec — always appended by code, never stored in editable files.
///
/// Injected unconditionally so editing `SUBCONSCIOUS.md` cannot break JSON
/// parsing. Structural requirements are enforced by structured output mode
/// (JSON schema); this spec covers semantic field guidance only.
pub(super) const FORMAT_SPEC: &str = r#"Report findings using these fields:
- "kind": "violation" if the agent acted against an instruction, "omission" if it failed to do something it should have
- "severity": "act" only when the problem needs correction right now (e.g. an unsaved user preference); "note" when the agent just needs to know for next time
- "instruction": one or two sentences of corrective guidance addressed to the agent, specific enough to act on without re-reading the transcript

Return an empty "findings" array whenever the agent's behavior is acceptable.
That is the expected result for most turns — do not invent findings, and never
report the same problem twice."#;

/// JSON schema for the subconscious response, used with structured output mode.
#[must_use]
pub(super) fn subconscious_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["violation", "omission"] },
                        "severity": { "type": "string", "enum": ["note", "act"] },
                        "instruction": { "type": "string" }
                    },
                    "required": ["kind", "severity", "instruction"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["findings"],
        "additionalProperties": false
    })
}

/// Maximum characters of a tool result included in the transcript.
///
/// Tool output dominates transcript size but rarely matters for judging the
/// agent's behavior; a prefix is enough for the classifier to see what the
/// agent acted on.
const MAX_TOOL_RESULT_CHARS: usize = 1_000;

/// Format a single turn message for the classifier transcript.
fn format_message(msg: &Message) -> String {
    let role = msg.role.as_str();
    let call_part = msg
        .tool_call_id
        .as_deref()
        .map_or_else(String::new, |id| format!(" (call: {id})"));

    let mut parts = vec![format!("[{role}]{call_part}:")];

    if !msg.content.is_empty() {
        if msg.role == Role::Tool && msg.content.len() > MAX_TOOL_RESULT_CHARS {
            let mut boundary = MAX_TOOL_RESULT_CHARS;
            while !msg.content.is_char_boundary(boundary) {
                boundary -= 1;
            }
            let (head, _) = msg.content.split_at(boundary);
            parts.push(format!(
                "{head}\n  [... tool result truncated, {} chars total]",
                msg.content.len()
            ));
        } else {
            parts.push(msg.content.clone());
        }
    }

    if let Some(tool_calls) = &msg.tool_calls {
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

/// Format the turn transcript, dropping the oldest messages to fit the token cap.
///
/// The tail is kept because the most recent behavior is what the classifier
/// judges; a dropped head is announced so the model knows context is missing.
pub(super) fn format_turn_transcript(transcript: &[Message], max_tokens: usize) -> String {
    let formatted: Vec<String> = transcript.iter().map(format_message).collect();

    let mut start = 0;
    let mut total: usize = formatted
        .iter()
        .map(|s| crate::memory::tokens::estimate_tokens(s))
        .sum();
    while start < formatted.len().saturating_sub(1) && total > max_tokens {
        total -=
            crate::memory::tokens::estimate_tokens(formatted.get(start).map_or("", String::as_str));
        start += 1;
    }

    let body = formatted.get(start..).unwrap_or_default().join("\n\n");
    if start > 0 {
        format!("[... {start} earlier messages omitted to fit context ...]\n\n{body}")
    } else {
        body
    }
}

/// The phase-specific question posed to the classifier.
fn phase_question(phase: EvalPhase) -> &'static str {
    match phase {
        EvalPhase::MidTurn => {
            "The agent is still working on this turn. Is it currently violating one of its \
             instructions or heading in a direction an instruction forbids? Ignore work that is \
             simply unfinished."
        }
        EvalPhase::EndOfTurn => {
            "The agent has finished this turn. Did it omit anything it should have done — for \
             example, failing to persist a stated user preference to MEMORY.md — or violate one \
             of its instructions?"
        }
    }
}

/// Build the evaluation prompt for the classifier model.
///
/// Injects the format spec alongside the user-customizable policy so the
/// format requirement cannot be lost by editing the disk file.
pub(super) fn build_eval_prompt(
    policy: &str,
    identity: &str,
    transcript: &str,
    phase: EvalPhase,
) -> Vec<Message> {
    let identity_section = if identity.is_empty() {
        String::new()
    } else {
        format!("\n\n# Agent Instructions (what the agent is supposed to follow)\n\n{identity}")
    };
    let system = format!("{policy}{identity_section}\n\n{FORMAT_SPEC}");

    vec![
        Message::system(system),
        Message::user(format!(
            "{}\n\n# Turn Transcript\n\n{transcript}",
            phase_question(phase)
        )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ToolCall;

    #[test]
    fn format_message_includes_tool_calls() {
        let msg = Message::assistant(
            "checking".to_string(),
            Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "write_file".to_string(),
                arguments: serde_json::json!({"path": "MEMORY.md"}),
            }]),
        );
        let formatted = format_message(&msg);
        assert!(formatted.contains("write_file"), "tool name included");
        assert!(formatted.contains("call_1"), "tool call id included");
        assert!(formatted.contains("MEMORY.md"), "arguments included");
    }

    #[test]
    fn format_message_truncates_long_tool_results() {
        let msg = Message::tool("x".repeat(5_000), "call_1");
        let formatted = format_message(&msg);
        assert!(
            formatted.len() < 2_000,
            "tool result should be truncated, got {} chars",
            formatted.len()
        );
        assert!(
            formatted.contains("truncated"),
            "truncation should be announced"
        );
        assert!(
            formatted.contains("5000 chars total"),
            "original size should be reported"
        );
    }

    #[test]
    fn format_message_truncation_respects_char_boundaries() {
        // Multi-byte characters straddling the cut must not panic.
        let msg = Message::tool("é".repeat(3_000), "call_1");
        let formatted = format_message(&msg);
        assert!(formatted.contains("truncated"), "should truncate");
    }

    #[test]
    fn transcript_cap_drops_head_not_tail() {
        let transcript = vec![
            Message::user(format!("OLDEST {}", "a".repeat(4_000))),
            Message::user(format!("MIDDLE {}", "b".repeat(4_000))),
            Message::user("NEWEST short message".to_string()),
        ];
        let out = format_turn_transcript(&transcript, 1_100);
        assert!(!out.contains("OLDEST"), "oldest message should be dropped");
        assert!(out.contains("NEWEST"), "newest message must survive");
        assert!(out.contains("omitted to fit context"), "drop is announced");
    }

    #[test]
    fn transcript_under_cap_is_untouched() {
        let transcript = vec![Message::user("hello"), Message::user("world")];
        let out = format_turn_transcript(&transcript, 10_000);
        assert!(out.contains("hello") && out.contains("world"));
        assert!(!out.contains("omitted"), "nothing should be dropped");
    }

    #[test]
    fn transcript_keeps_last_message_even_over_cap() {
        let transcript = vec![Message::user("x".repeat(8_000))];
        let out = format_turn_transcript(&transcript, 10);
        assert!(
            out.contains(&"x".repeat(100)),
            "sole message survives even when over cap"
        );
    }

    #[test]
    fn eval_prompt_always_includes_format_spec() {
        let prompt = build_eval_prompt("policy text", "", "transcript", EvalPhase::MidTurn);
        let system = prompt.first().map_or("", |m| m.content.as_str());
        assert!(system.contains(FORMAT_SPEC), "format spec must be present");
        assert!(system.contains("policy text"), "policy must be present");
    }

    #[test]
    fn eval_prompt_includes_identity_when_present() {
        let prompt = build_eval_prompt("p", "## SOUL.md\n\nBe kind.", "t", EvalPhase::EndOfTurn);
        let system = prompt.first().map_or("", |m| m.content.as_str());
        assert!(system.contains("Be kind."), "identity content included");
        assert!(
            system.contains("Agent Instructions"),
            "identity section labelled"
        );
    }

    #[test]
    fn eval_prompt_phase_questions_differ() {
        let mid = build_eval_prompt("p", "", "t", EvalPhase::MidTurn);
        let end = build_eval_prompt("p", "", "t", EvalPhase::EndOfTurn);
        let mid_user = mid.get(1).map_or("", |m| m.content.as_str());
        let end_user = end.get(1).map_or("", |m| m.content.as_str());
        assert!(mid_user.contains("still working"), "mid-turn question");
        assert!(
            end_user.contains("finished this turn"),
            "end-of-turn question"
        );
        assert_ne!(mid_user, end_user);
    }

    #[test]
    fn default_prompt_is_nonempty() {
        assert!(
            !DEFAULT_SUBCONSCIOUS_PROMPT.trim().is_empty(),
            "bundled SUBCONSCIOUS.md must have content"
        );
    }
}
