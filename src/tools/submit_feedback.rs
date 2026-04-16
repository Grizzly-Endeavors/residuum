//! `submit_feedback` tool — lets the agent send short, free-form
//! feedback when it notices confusion, usability problems, or
//! patterns in its own behavior that seem off.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::models::ToolDefinition;
use crate::tracing_service::{Feedback, TracingService, client_context};

use super::{Tool, ToolError, ToolResult, require_str};

/// Built-in tool that submits a feedback message through the tracing service.
pub(crate) struct SubmitFeedbackTool {
    service: Arc<TracingService>,
}

impl SubmitFeedbackTool {
    pub(crate) fn new(service: Arc<TracingService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for SubmitFeedbackTool {
    fn name(&self) -> &'static str {
        "submit_feedback"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Submit short, free-form feedback to the developer. Use this when you \
                          notice confusion, usability friction, surprising behavior, or \
                          patterns in your own actions that seem worth surfacing — anything \
                          that isn't unambiguously broken (use file_bug_report for that). \
                          You are encouraged to use this proactively. Returns a public \
                          reference ID."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "The feedback message"
                    },
                    "category": {
                        "type": "string",
                        "description": "Optional free-form category tag (e.g. 'ui', 'docs', 'tools')"
                    }
                },
                "required": ["message"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let message = require_str(&arguments, "message")?.to_string();
        let category = arguments
            .get("category")
            .and_then(Value::as_str)
            .map(str::to_string);

        let feedback = Feedback {
            message,
            category,
            client: client_context::gather_for_feedback(),
        };

        match self.service.send_feedback(feedback).await {
            Ok(receipt) => Ok(ToolResult::success(format!(
                "feedback submitted: {}",
                receipt.public_id
            ))),
            Err(e) => Ok(ToolResult::error(format!(
                "feedback submission failed: {e}"
            ))),
        }
    }
}
