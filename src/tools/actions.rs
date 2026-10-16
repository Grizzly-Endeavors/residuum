//! Scheduled action tools: schedule, list, and cancel one-off actions.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::actions::types::ScheduledAction;
use crate::config::{ModelSelector, SharedModelCatalog};
use crate::inference::ToolDefinition;

use crate::agent::Tool;
use crate::inference::{ToolError, ToolResult};

// ─── schedule_action ─────────────────────────────────────────────────────────

/// Tool for scheduling a one-off action.
pub struct ScheduleActionTool {
    store: Arc<Mutex<ActionStore>>,
    notify: Arc<Notify>,
    tz: chrono_tz::Tz,
    /// The live catalog an explicit `model_tier` is checked against.
    models: SharedModelCatalog,
}

impl ScheduleActionTool {
    /// Create a new `ScheduleActionTool`.
    #[must_use]
    pub fn new(
        store: Arc<Mutex<ActionStore>>,
        notify: Arc<Notify>,
        tz: chrono_tz::Tz,
        models: SharedModelCatalog,
    ) -> Self {
        Self {
            store,
            notify,
            tz,
            models,
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
            name: self.name().to_string(),
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
                        "description": "A skill name (e.g. 'memory-analyst') to fork the session with that skill as its role. Omit to fork a plain session with no skill."
                    },
                    "model_tier": {
                        "type": "string",
                        "description": "The model the action's session runs on: a tier (\"small\", \"medium\", or \"large\") or an explicit model id in provider/model form, such as \"anthropic/claude-haiku-4-5\" (the format providers.toml uses). An explicit model runs that one model with no failover. Defaults to \"medium\"."
                    }
                },
                "required": ["name", "prompt", "run_at"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = super::require_str(&arguments, "name")?.to_string();
        let prompt = super::require_str(&arguments, "prompt")?.to_string();
        let run_at_str = super::require_str(&arguments, "run_at")?;

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

        if agent_name
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case("main"))
        {
            return Err(ToolError::InvalidArguments(
                "\"main\" is no longer a valid agent_name — every session fork already carries \
                 the main agent's identity and memory snapshot, so name a skill instead, or \
                 omit agent_name to run on the prompt alone."
                    .to_string(),
            ));
        }

        let model_tier = match arguments.get("model_tier").and_then(Value::as_str) {
            Some(raw) => {
                let model = raw
                    .parse::<ModelSelector>()
                    .map_err(ToolError::InvalidArguments)?;
                self.models
                    .read()
                    .await
                    .check(&model)
                    .map_err(ToolError::InvalidArguments)?;
                Some(model.to_string())
            }
            None => None,
        };

        let id = ScheduledAction::generate_id();
        let action = ScheduledAction {
            id: id.clone(),
            name: name.clone(),
            prompt,
            run_at,
            agent: agent_name,
            model_tier,
            created_at: Utc::now(),
        };

        {
            let mut store = self.store.lock().await;
            store.add(action);
            save_action_store(&mut store).await?;
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
            name: self.name().to_string(),
            description:
                "List all pending scheduled actions with their IDs, names, fire times, and \
                 models, and say which are held because their model can't run."
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
                Some(skill) => format!(" [skill: {skill}]"),
                None => String::new(),
            };
            let model_label = match action.model_tier.as_deref() {
                Some(model) => format!(" [model: {model}]"),
                None => String::new(),
            };
            let held_label = match store.model_problem(&action.id) {
                Some(reason) => format!(" — held, not running: {reason}"),
                None => String::new(),
            };
            lines.push(format!(
                "  {name} ({id}) — fires: {run_at}{agent_label}{model_label}{held_label}",
                name = action.name,
                id = action.id,
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
            name: self.name().to_string(),
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
        let id = super::require_str(&arguments, "id")?.to_string();

        {
            let mut store = self.store.lock().await;
            if !store.remove(&id) {
                return Ok(ToolResult::error(format!("action '{id}' not found")));
            }
            save_action_store(&mut store).await?;
        }

        self.notify.notify_one();
        Ok(ToolResult::success(format!("Cancelled action '{id}'")))
    }
}

async fn save_action_store(store: &mut ActionStore) -> Result<(), ToolError> {
    store.save().await.map_err(|e| {
        tracing::error!(error = %e, "failed to persist action store");
        ToolError::Execution(format!("failed to save action store: {e}"))
    })
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
mod tests {
    use super::*;

    #[test]
    fn parse_run_at_rfc3339() {
        let dt = parse_run_at("2026-03-01T14:00:00Z", chrono_tz::UTC).unwrap();
        assert_eq!(
            dt,
            chrono::DateTime::parse_from_rfc3339("2026-03-01T14:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn parse_run_at_with_offset() {
        let dt = parse_run_at("2026-03-01T09:00:00-05:00", chrono_tz::UTC).unwrap();
        assert_eq!(
            dt,
            chrono::DateTime::parse_from_rfc3339("2026-03-01T14:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            "9am EST = 2pm UTC"
        );
    }

    #[test]
    fn parse_run_at_naive_with_seconds() {
        let dt = parse_run_at("2026-03-01T09:00:00", chrono_tz::Tz::America__New_York).unwrap();
        assert_eq!(
            dt,
            chrono::DateTime::parse_from_rfc3339("2026-03-01T14:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            "9am EST = 2pm UTC"
        );
    }

    #[test]
    fn parse_run_at_naive_without_seconds() {
        let dt = parse_run_at("2026-03-01T09:00", chrono_tz::Tz::America__New_York).unwrap();
        assert_eq!(
            dt,
            chrono::DateTime::parse_from_rfc3339("2026-03-01T14:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            "9am EST = 2pm UTC"
        );
    }

    #[test]
    fn parse_run_at_dst_gap() {
        // America/New_York spring-forward on 2026-03-08: 2:00 AM → 3:00 AM
        // 2:30 AM does not exist in that timezone
        let result = parse_run_at("2026-03-08T02:30:00", chrono_tz::Tz::America__New_York);
        assert!(result.is_err(), "datetime in DST gap should fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("DST gap") || err.contains("does not exist"),
            "error should mention DST: {err}"
        );
    }

    #[test]
    fn parse_run_at_invalid() {
        assert!(parse_run_at("not-a-date", chrono_tz::UTC).is_err());
    }

    #[tokio::test]
    async fn schedule_action_past_run_at() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-past.json"),
        )));
        let notify = Arc::new(Notify::new());
        let tool = ScheduleActionTool::new(
            store,
            notify,
            chrono_tz::UTC,
            crate::config::ModelCatalog::default().into_shared(),
        );
        let result = tool
            .execute(serde_json::json!({
                "name": "past",
                "prompt": "too late",
                "run_at": "2020-01-01T00:00:00Z"
            }))
            .await;
        assert!(result.is_err(), "past run_at should return ToolError");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("future"), "error should mention future: {err}");
    }

    #[tokio::test]
    async fn schedule_action_rejects_agent_main() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-main.json"),
        )));
        let notify = Arc::new(Notify::new());
        let tool = ScheduleActionTool::new(
            store,
            notify,
            chrono_tz::UTC,
            crate::config::ModelCatalog::default().into_shared(),
        );
        let result = tool
            .execute(serde_json::json!({
                "name": "wake main",
                "prompt": "do the thing",
                "run_at": "2099-01-01T00:00:00Z",
                "agent_name": "main"
            }))
            .await;
        assert!(result.is_err(), "agent_name: main should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no longer"),
            "error should explain why, got: {err}"
        );
    }

    #[tokio::test]
    async fn schedule_action_rejects_agent_main_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-main-case.json"),
        )));
        let notify = Arc::new(Notify::new());
        let tool = ScheduleActionTool::new(
            store,
            notify,
            chrono_tz::UTC,
            crate::config::ModelCatalog::default().into_shared(),
        );
        let result = tool
            .execute(serde_json::json!({
                "name": "wake main",
                "prompt": "do the thing",
                "run_at": "2099-01-01T00:00:00Z",
                "agent_name": "MAIN"
            }))
            .await;
        assert!(result.is_err(), "agent_name: MAIN should also be rejected");
    }

    async fn schedule_with_model(
        model_tier: &str,
    ) -> (Result<ToolResult, ToolError>, Vec<ScheduledAction>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-model.json"),
        )));
        let tool = ScheduleActionTool::new(
            Arc::clone(&store),
            Arc::new(Notify::new()),
            chrono_tz::UTC,
            crate::config::ModelCatalog::default().into_shared(),
        );
        let result = tool
            .execute(serde_json::json!({
                "name": "with a model",
                "prompt": "do the thing",
                "run_at": "2099-01-01T00:00:00",
                "model_tier": model_tier
            }))
            .await;
        let stored = store.lock().await.list().to_vec();
        (result, stored)
    }

    #[tokio::test]
    async fn schedule_action_stores_a_tier_or_explicit_model() {
        for model in ["large", "anthropic/claude-haiku-4-5"] {
            let (result, stored) = schedule_with_model(model).await;
            assert!(!result.unwrap().is_error);
            assert_eq!(stored.first().unwrap().model_tier.as_deref(), Some(model));
        }
    }

    #[tokio::test]
    async fn schedule_action_rejects_a_bad_model_naming_it() {
        let (result, stored) = schedule_with_model("lagre").await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("'lagre'"), "{err}");
        assert!(stored.is_empty(), "nothing is scheduled on a bad model");

        let (unknown, unknown_stored) = schedule_with_model("nowhere/some-model").await;
        let unknown_err = unknown.unwrap_err().to_string();
        assert!(
            unknown_err.contains("nowhere/some-model")
                && unknown_err.contains("not a known provider"),
            "{unknown_err}"
        );
        assert!(
            unknown_stored.is_empty(),
            "nothing is scheduled on an unknown provider"
        );
    }

    #[tokio::test]
    async fn list_actions_shows_the_model_and_a_held_action_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-list-held.json"),
        )));
        {
            let mut locked = store.lock().await;
            locked.add(ScheduledAction {
                id: "action-held0001".to_string(),
                name: "local summary".to_string(),
                prompt: "summarize".to_string(),
                run_at: Utc::now() - chrono::Duration::minutes(5),
                agent: None,
                model_tier: Some("lcoal/qwen3:8b".to_string()),
                created_at: Utc::now(),
            });
            let _fresh = locked.record_model_problems(std::collections::HashMap::from([(
                "action-held0001".to_string(),
                "'lcoal' is not a known provider".to_string(),
            )]));
        }

        let result = ListActionsTool::new(store, chrono_tz::UTC)
            .execute(serde_json::json!({}))
            .await
            .unwrap();

        assert!(
            result.output.contains("[model: lcoal/qwen3:8b]"),
            "{}",
            result.output
        );
        assert!(
            result
                .output
                .contains("held, not running: 'lcoal' is not a known provider"),
            "{}",
            result.output
        );
    }

    #[tokio::test]
    async fn cancel_action_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-cancel.json"),
        )));
        let notify = Arc::new(Notify::new());

        // Add an action directly
        let action = ScheduledAction {
            id: "action-cancel01".to_string(),
            name: "to-cancel".to_string(),
            prompt: "cancel me".to_string(),
            run_at: Utc::now() + chrono::Duration::hours(1),
            agent: None,
            model_tier: None,
            created_at: Utc::now(),
        };
        store.lock().await.add(action);

        let tool = CancelActionTool::new(Arc::clone(&store), Arc::clone(&notify));
        let result = tool
            .execute(serde_json::json!({"id": "action-cancel01"}))
            .await
            .unwrap();
        assert!(!result.is_error, "cancel should succeed: {}", result.output);
        assert!(
            result.output.contains("action-cancel01"),
            "output should mention the cancelled action id"
        );
        assert!(
            store.lock().await.list().is_empty(),
            "action should be removed from store"
        );
    }

    #[tokio::test]
    async fn cancel_action_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-cancel-notfound.json"),
        )));
        let notify = Arc::new(Notify::new());
        let tool = CancelActionTool::new(store, notify);
        let result = tool
            .execute(serde_json::json!({"id": "action-missing01"}))
            .await
            .unwrap();
        assert!(
            result.is_error,
            "cancelling nonexistent action should be error result"
        );
        assert!(
            result.output.contains("not found"),
            "error should mention not found: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn schedule_action_displays_local_time() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-sched-tz.json"),
        )));
        let notify = Arc::new(Notify::new());
        let tz = chrono_tz::Tz::America__New_York;
        let tool = ScheduleActionTool::new(
            store,
            notify,
            tz,
            crate::config::ModelCatalog::default().into_shared(),
        );

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
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("test-list-tz.json"),
        )));
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
