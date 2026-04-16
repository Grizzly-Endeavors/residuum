//! `file_bug_report` tool — lets the agent file a structured bug
//! report against itself when it observes broken behavior.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::models::ToolDefinition;
use crate::tracing_service::{BugReport, ClientContext, Severity, TracingService};

use super::{Tool, ToolError, ToolResult, require_str};

/// Built-in tool that submits a bug report through the tracing service.
pub(crate) struct FileBugReportTool {
    service: Arc<TracingService>,
    /// Snapshot of the runtime client context (version, model, OS, etc.).
    /// Built at registration time; refreshed when the gateway is rebuilt
    /// on config reload.
    client_context: Arc<ClientContext>,
}

impl FileBugReportTool {
    pub(crate) fn new(service: Arc<TracingService>, client_context: Arc<ClientContext>) -> Self {
        Self {
            service,
            client_context,
        }
    }
}

#[async_trait]
impl Tool for FileBugReportTool {
    fn name(&self) -> &'static str {
        "file_bug_report"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "File a structured bug report when you observe broken behavior in \
                          residuum itself (a crash, a tool returning the wrong result, a model \
                          reply that violates a hard constraint, etc.). Use this for things \
                          that are clearly wrong, not for confusion or usability friction — \
                          use submit_feedback for those. Returns a public reference ID."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "what_happened": {
                        "type": "string",
                        "description": "What actually happened (the broken behavior)"
                    },
                    "what_expected": {
                        "type": "string",
                        "description": "What should have happened instead"
                    },
                    "what_doing": {
                        "type": "string",
                        "description": "What you (or the user) were doing when it happened"
                    },
                    "severity": {
                        "type": "string",
                        "enum": ["broken", "wrong", "annoying"],
                        "description": "broken = non-functional; wrong = wrong result; annoying = functional but bad UX"
                    }
                },
                "required": ["what_happened", "what_expected", "what_doing", "severity"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let what_happened = require_str(&arguments, "what_happened")?.to_string();
        let what_expected = require_str(&arguments, "what_expected")?.to_string();
        let what_doing = require_str(&arguments, "what_doing")?.to_string();
        let severity = match require_str(&arguments, "severity")? {
            "broken" => Severity::Broken,
            "wrong" => Severity::Wrong,
            "annoying" => Severity::Annoying,
            other => {
                return Err(ToolError::InvalidArguments(format!(
                    "invalid severity '{other}': must be broken, wrong, or annoying"
                )));
            }
        };

        let report = BugReport {
            what_happened,
            what_expected,
            what_doing,
            severity,
            client: (*self.client_context).clone(),
        };

        match self.service.send_bug_report(report).await {
            Ok(receipt) => Ok(ToolResult::success(format!(
                "bug report submitted: {}",
                receipt.public_id
            ))),
            Err(e) => Ok(ToolResult::error(format!(
                "bug report submission failed: {e}"
            ))),
        }
    }
}
