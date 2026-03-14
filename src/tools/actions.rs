//! Scheduled action tools: schedule, list, and cancel one-off actions.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::actions::types::ScheduledAction;
use crate::models::ToolDefinition;

use super::background::is_valid_channel;
use super::{Tool, ToolError, ToolResult};

// ─── schedule_action ─────────────────────────────────────────────────────────

/// Tool for scheduling a one-off action.
pub struct ScheduleActionTool {
    store: Arc<Mutex<ActionStore>>,
    notify: Arc<Notify>,
    tz: chrono_tz::Tz,
    valid_external_channels: HashSet<String>,
}

impl ScheduleActionTool {
    /// Create a new `ScheduleActionTool`.
    #[must_use]
    pub fn new(
        store: Arc<Mutex<ActionStore>>,
        notify: Arc<Notify>,
        tz: chrono_tz::Tz,
        valid_external_channels: HashSet<String>,
    ) -> Self {
        Self {
            store,
            notify,
            tz,
            valid_external_channels,
        }
    }
}

#[async_trait]
impl Tool for ScheduleActionTool {
    fn name(&self) -> &'static str {
        "schedule_action"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "schedule_action".to_string(),
            description: "Schedule a one-off action to fire at a specific time. The action runs once and is removed after firing.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Human-readable name for this action"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The prompt to execute when the action fires"
                    },
                    "run_at": {
                        "type": "string",
                        "description": "Always use local time without an offset (e.g. '2026-03-01T09:00:00'). Interpreted in the user's configured timezone."
                    },
                    "agent_name": {
                        "type": "string",
                        "description": "Agent routing: 'main' runs a full wake turn with conversation context; a preset name (e.g. 'memory-agent') spawns a sub-agent using that preset. Omit for default sub-agent behavior."
                    },
                    "model_tier": {
                        "type": "string",
                        "enum": ["small", "medium", "large"],
                        "description": "Model tier override for sub-agent actions. Defaults to medium."
                    },
                    "channels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Result delivery channels for sub-agent actions. Defaults to ['inbox']. Not used when agent_name='main'."
                    }
                },
                "required": ["name", "prompt", "run_at"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = arguments
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("name is required".to_string()))?
            .to_string();

        let prompt = arguments
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("prompt is required".to_string()))?
            .to_string();

        let run_at_str = arguments
            .get("run_at")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("run_at is required".to_string()))?;

        let run_at = parse_run_at(run_at_str, self.tz)?;

        if run_at <= Utc::now() {
            return Err(ToolError::InvalidArguments(
                "run_at must be in the future".to_string(),
            ));
        }

        let agent_name = arguments
            .get("agent_name")
            .and_then(Value::as_str)
            .map(String::from);

        let model_tier = arguments
            .get("model_tier")
            .and_then(Value::as_str)
            .map(String::from);

        let channels: Vec<String> = arguments
            .get("channels")
            .and_then(Value::as_array)
            .map_or_else(
                || vec!["inbox".to_string()],
                |arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                },
            );

        // Mutual exclusion: main-turn actions cannot have channels
        if agent_name.as_deref() == Some("main") && arguments.get("channels").is_some() {
            return Err(ToolError::InvalidArguments(
                "channels cannot be set when agent_name='main' — main-turn actions inject directly into the agent".to_string(),
            ));
        }

        // Validate channels for sub-agent actions
        if agent_name.as_deref() != Some("main") {
            for ch in &channels {
                if !is_valid_channel(ch, &self.valid_external_channels) {
                    return Ok(ToolResult::error(format!(
                        "unknown channel '{ch}'. Valid: inbox or configured external channels."
                    )));
                }
            }
        }

        let id = ActionStore::generate_id();
        let action = ScheduledAction {
            id: id.clone(),
            name: name.clone(),
            prompt,
            run_at,
            agent: agent_name,
            model_tier,
            channels,
            created_at: Utc::now(),
        };

        {
            let mut store = self.store.lock().await;
            store.add(action);
            store.save().await.map_err(|e| {
                tracing::error!(error = %e, "failed to persist action store");
                ToolError::Execution(format!("failed to save action store: {e}"))
            })?;
        }

        self.notify.notify_one();

        let run_at_display = run_at.with_timezone(&self.tz).format("%Y-%m-%dT%H:%M:%S");
        Ok(ToolResult::success(format!(
            "Scheduled '{name}' (id: {id}). Fires at: {run_at_display}"
        )))
    }
}

// ─── list_actions ────────────────────────────────────────────────────────────

/// Tool for listing all pending scheduled actions.
pub struct ListActionsTool {
    store: Arc<Mutex<ActionStore>>,
    tz: chrono_tz::Tz,
}

impl ListActionsTool {
    /// Create a new `ListActionsTool`.
    #[must_use]
    pub fn new(store: Arc<Mutex<ActionStore>>, tz: chrono_tz::Tz) -> Self {
        Self { store, tz }
    }
}

#[async_trait]
impl Tool for ListActionsTool {
    fn name(&self) -> &'static str {
        "list_actions"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_actions".to_string(),
            description:
                "List all pending scheduled actions with their IDs, names, and fire times."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
        let store = self.store.lock().await;
        let actions = store.list();

        if actions.is_empty() {
            return Ok(ToolResult::success("No pending scheduled actions."));
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("{} action(s):", actions.len()));

        for action in actions {
            let run_at = action
                .run_at
                .with_timezone(&self.tz)
                .format("%Y-%m-%dT%H:%M:%S");
            let agent_label = match action.agent.as_deref() {
                Some("main") => " [main turn]".to_string(),
                Some(preset) => format!(" [preset: {preset}]"),
                None => String::new(),
            };
            let channels_label = if action.agent.as_deref() == Some("main") {
                String::new()
            } else {
                format!(" → [{}]", action.channels.join(", "))
            };
            lines.push(format!(
                "  {name} ({id}) — fires: {run_at}{agent}{channels}",
                name = action.name,
                id = action.id,
                agent = agent_label,
                channels = channels_label,
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ─── cancel_action ───────────────────────────────────────────────────────────

/// Tool for cancelling a pending scheduled action.
pub struct CancelActionTool {
    store: Arc<Mutex<ActionStore>>,
    notify: Arc<Notify>,
}

impl CancelActionTool {
    /// Create a new `CancelActionTool`.
    #[must_use]
    pub fn new(store: Arc<Mutex<ActionStore>>, notify: Arc<Notify>) -> Self {
        Self { store, notify }
    }
}

#[async_trait]
impl Tool for CancelActionTool {
    fn name(&self) -> &'static str {
        "cancel_action"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "cancel_action".to_string(),
            description: "Cancel a pending scheduled action by ID.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Action ID to cancel"
                    }
                },
                "required": ["id"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let id = arguments
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("id is required".to_string()))?
            .to_string();

        {
            let mut store = self.store.lock().await;
            if !store.remove(&id) {
                return Ok(ToolResult::error(format!("action '{id}' not found")));
            }
            store.save().await.map_err(|e| {
                tracing::error!(error = %e, "failed to persist action store");
                ToolError::Execution(format!("failed to save action store: {e}"))
            })?;
        }

        self.notify.notify_one();
        Ok(ToolResult::success(format!("Cancelled action '{id}'")))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Parse a datetime string into a UTC `DateTime`.
///
/// Accepts:
/// - Full ISO 8601 with offset: `2026-03-01T09:00:00Z` or `2026-03-01T09:00:00+05:00`
/// - Naive datetime (interpreted in the configured timezone): `2026-03-01T09:00:00`
/// - Naive datetime without seconds: `2026-03-01T09:00`
fn parse_run_at(s: &str, tz: chrono_tz::Tz) -> Result<chrono::DateTime<Utc>, ToolError> {
    // Try RFC 3339 / ISO with offset first
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try naive datetime with seconds
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return naive_to_utc(naive, tz);
    }

    // Try naive datetime without seconds
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return naive_to_utc(naive, tz);
    }

    Err(ToolError::InvalidArguments(format!(
        "invalid run_at datetime '{s}': expected ISO 8601 (e.g. '2026-03-01T09:00:00' or '2026-03-01T09:00:00Z')"
    )))
}

fn naive_to_utc(
    naive: chrono::NaiveDateTime,
    tz: chrono_tz::Tz,
) -> Result<chrono::DateTime<Utc>, ToolError> {
    use chrono::TimeZone;
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Ok(dt.with_timezone(&Utc)),
        chrono::LocalResult::Ambiguous(dt, _) => {
            tracing::warn!(datetime = %naive, timezone = %tz, "datetime is ambiguous during DST transition, using earlier interpretation");
            Ok(dt.with_timezone(&Utc))
        }
        chrono::LocalResult::None => Err(ToolError::InvalidArguments(format!(
            "datetime '{naive}' does not exist in timezone {tz} (DST gap)"
        ))),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn parse_run_at_rfc3339() {
        let dt = parse_run_at("2026-03-01T14:00:00Z", chrono_tz::UTC).unwrap();
        assert_eq!(dt.hour(), 14);
    }

    #[test]
    fn parse_run_at_with_offset() {
        let dt = parse_run_at("2026-03-01T09:00:00-05:00", chrono_tz::UTC).unwrap();
        assert_eq!(dt.hour(), 14, "9am EST = 2pm UTC");
    }

    #[test]
    fn parse_run_at_naive_with_seconds() {
        let dt = parse_run_at("2026-03-01T09:00:00", chrono_tz::Tz::America__New_York).unwrap();
        assert_eq!(dt.hour(), 14, "9am EST = 2pm UTC");
    }

    #[test]
    fn parse_run_at_naive_without_seconds() {
        let dt = parse_run_at("2026-03-01T09:00", chrono_tz::Tz::America__New_York).unwrap();
        assert_eq!(dt.hour(), 14, "9am EST = 2pm UTC");
    }

    #[test]
    fn parse_run_at_invalid() {
        assert!(parse_run_at("not-a-date", chrono_tz::UTC).is_err());
    }

    #[test]
    fn list_actions_tool_has_correct_name() {
        let store = Arc::new(Mutex::new(ActionStore::new_empty("/tmp/test-actions.json")));
        let tool = ListActionsTool::new(store, chrono_tz::UTC);
        assert_eq!(tool.name(), "list_actions");
    }

    use chrono::Timelike;

    #[tokio::test]
    async fn schedule_action_displays_local_time() {
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            "/tmp/test-sched-tz.json",
        )));
        let notify = Arc::new(Notify::new());
        let tz = chrono_tz::Tz::America__New_York;
        let tool = ScheduleActionTool::new(store, notify, tz, HashSet::new());

        // Schedule an action 1 hour from now so it's in the future
        let future = Utc::now() + chrono::Duration::hours(1);
        let naive_local = future
            .with_timezone(&tz)
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();

        let args = serde_json::json!({
            "name": "tz-test",
            "prompt": "hello",
            "run_at": naive_local
        });

        let result = tool.execute(args).await.unwrap();
        let output = result.output;

        // Output should contain the local time, not UTC
        assert!(
            output.contains(&naive_local),
            "expected local time '{naive_local}' in output, got: {output}"
        );
        assert!(
            !output.contains("UTC"),
            "output should not contain 'UTC', got: {output}"
        );
    }

    #[tokio::test]
    async fn list_actions_displays_local_time() {
        let store = Arc::new(Mutex::new(ActionStore::new_empty("/tmp/test-list-tz.json")));
        let tz = chrono_tz::Tz::America__New_York;

        // Add an action with a known UTC time
        let run_at_utc = chrono::DateTime::parse_from_rfc3339("2026-06-15T18:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let action = ScheduledAction {
            id: "action-test1234".to_string(),
            name: "tz-list-test".to_string(),
            prompt: "hello".to_string(),
            run_at: run_at_utc,
            agent: None,
            model_tier: None,
            channels: vec!["inbox".to_string()],
            created_at: Utc::now(),
        };
        store.lock().await.add(action);

        let tool = ListActionsTool::new(store, tz);
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        let output = result.output;

        // 18:00 UTC in June = 14:00 EDT (America/New_York)
        assert!(
            output.contains("2026-06-15T14:00:00"),
            "expected EDT time '2026-06-15T14:00:00' in output, got: {output}"
        );
        assert!(
            !output.contains("UTC"),
            "output should not contain 'UTC', got: {output}"
        );
    }
}
