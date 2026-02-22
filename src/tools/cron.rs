//! Cron management tools: add, list, update, and remove scheduled jobs.

use std::sync::Arc;

use async_trait::async_trait;

use serde_json::Value;
use tokio::sync::{Mutex, Notify};

use crate::cron::executor::initialize_next_run;
use crate::cron::store::CronStore;
use crate::cron::types::{CronJob, CronJobState, CronPayload, CronSchedule, Delivery, RunStatus};
use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

// ─── cron_add ────────────────────────────────────────────────────────────────

/// Tool for creating a new scheduled job.
pub struct CronAddTool {
    store: Arc<Mutex<CronStore>>,
    notify: Arc<Notify>,
    tz: chrono_tz::Tz,
}

impl CronAddTool {
    /// Create a new `CronAddTool`.
    #[must_use]
    pub fn new(store: Arc<Mutex<CronStore>>, notify: Arc<Notify>, tz: chrono_tz::Tz) -> Self {
        Self { store, notify, tz }
    }
}

#[async_trait]
impl Tool for CronAddTool {
    fn name(&self) -> &'static str {
        "cron_add"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "cron_add".to_string(),
            description: "Create a new scheduled cron job. The job will persist across restarts."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Human-readable name for this job"
                    },
                    "schedule_type": {
                        "type": "string",
                        "enum": ["at", "every", "cron"],
                        "description": "'at' = one-shot at a UTC datetime; 'every' = repeating interval; 'cron' = 6-field cron expression"
                    },
                    "schedule_at": {
                        "type": "string",
                        "description": "Local datetime (YYYY-MM-DDTHH:MM:SS), required when schedule_type='at'"
                    },
                    "schedule_every_ms": {
                        "type": "integer",
                        "description": "Interval in milliseconds, required when schedule_type='every'"
                    },
                    "schedule_anchor_ms": {
                        "type": "integer",
                        "description": "Anchor epoch ms, default 0 = Unix epoch; optional when schedule_type='every'"
                    },
                    "schedule_expr": {
                        "type": "string",
                        "description": "6-field cron expression including seconds, e.g. '0 30 9 * * *'; required when schedule_type='cron'"
                    },
                    "schedule_tz": {
                        "type": "string",
                        "description": "IANA timezone for cron expression evaluation; defaults to the configured timezone"
                    },
                    "delivery": {
                        "type": "string",
                        "enum": ["user_visible", "background"],
                        "description": "'user_visible' prints to CLI and queues for next user turn; 'background' runs silently for memory"
                    },
                    "payload_type": {
                        "type": "string",
                        "enum": ["system_event", "agent_turn"],
                        "description": "'system_event' = inject text into main conversation; 'agent_turn' = run a background agent turn"
                    },
                    "payload_text": {
                        "type": "string",
                        "description": "Text to announce/inject, required when payload_type='system_event'"
                    },
                    "payload_message": {
                        "type": "string",
                        "description": "Prompt for the isolated agent turn, required when payload_type='agent_turn'"
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional description of what this job does"
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "Whether to start the job enabled (default true)"
                    },
                    "delete_after_run": {
                        "type": "boolean",
                        "description": "Delete the job after it runs once (useful for one-shots)"
                    }
                },
                "required": ["name", "schedule_type", "delivery", "payload_type"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("name is required".to_string()))?
            .to_string();

        let schedule = parse_schedule(&arguments, self.tz)?;
        let delivery = parse_delivery(&arguments)?;
        let payload = parse_payload(&arguments)?;

        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let enabled = arguments
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let delete_after_run = arguments
            .get("delete_after_run")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let now = crate::time::now_local(self.tz);
        let id = CronStore::generate_id();

        let mut job = CronJob {
            id: id.clone(),
            name: name.clone(),
            description,
            enabled,
            delete_after_run,
            created_at: now,
            updated_at: now,
            schedule,
            delivery,
            payload,
            state: CronJobState::default(),
        };

        initialize_next_run(&mut job, now, self.tz)
            .map_err(|e| ToolError::Execution(format!("failed to compute next run: {e}")))?;

        let next_run = job.state.next_run_at;

        // Hold lock only for the sync mutation, release before notify
        {
            let mut store = self.store.lock().await;
            store.add_job(job);
            store
                .save()
                .await
                .map_err(|e| ToolError::Execution(format!("failed to save cron store: {e}")))?;
        }

        self.notify.notify_one();

        let next_str = next_run.map_or_else(
            || "never".to_string(),
            |t| t.format("%Y-%m-%dT%H:%M:%S").to_string(),
        );
        Ok(ToolResult::success(format!(
            "Created job '{name}' with id {id}. Next run: {next_str}"
        )))
    }
}

// ─── cron_list ───────────────────────────────────────────────────────────────

/// Tool for listing all scheduled jobs.
pub struct CronListTool {
    store: Arc<Mutex<CronStore>>,
}

impl CronListTool {
    /// Create a new `CronListTool`.
    #[must_use]
    pub fn new(store: Arc<Mutex<CronStore>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &'static str {
        "cron_list"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "cron_list".to_string(),
            description: "List all scheduled cron jobs with their status and next run time."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "include_disabled": {
                        "type": "boolean",
                        "description": "Include disabled jobs in the list (default false)"
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let include_disabled = arguments
            .get("include_disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let store = self.store.lock().await;
        let jobs: Vec<&CronJob> = store
            .list_jobs()
            .iter()
            .filter(|j| include_disabled || j.enabled)
            .collect();

        if jobs.is_empty() {
            return Ok(ToolResult::success("No cron jobs found."));
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("{} job(s):", jobs.len()));

        for job in jobs {
            let status = match job.state.last_status {
                Some(RunStatus::Ok) => "ok",
                Some(RunStatus::Error) => "error",
                Some(RunStatus::Skipped) => "skipped",
                None => "never run",
            };
            let next = job.state.next_run_at.map_or_else(
                || "—".to_string(),
                |t| t.format("%Y-%m-%dT%H:%M:%S").to_string(),
            );
            let enabled_str = if job.enabled { "enabled" } else { "disabled" };
            lines.push(format!(
                "  [{enabled_str}] {} ({}) — last: {status} — next: {next}",
                job.name, job.id
            ));
            if let Some(ref desc) = job.description {
                lines.push(format!("    {desc}"));
            }
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ─── cron_update ─────────────────────────────────────────────────────────────

/// Tool for updating an existing scheduled job.
pub struct CronUpdateTool {
    store: Arc<Mutex<CronStore>>,
    notify: Arc<Notify>,
    tz: chrono_tz::Tz,
}

impl CronUpdateTool {
    /// Create a new `CronUpdateTool`.
    #[must_use]
    pub fn new(store: Arc<Mutex<CronStore>>, notify: Arc<Notify>, tz: chrono_tz::Tz) -> Self {
        Self { store, notify, tz }
    }
}

#[async_trait]
impl Tool for CronUpdateTool {
    fn name(&self) -> &'static str {
        "cron_update"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "cron_update".to_string(),
            description: "Update an existing cron job by ID. Only provided fields are changed."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Job ID to update"
                    },
                    "name": {"type": "string"},
                    "description": {"type": "string"},
                    "enabled": {"type": "boolean"},
                    "delete_after_run": {"type": "boolean"},
                    "schedule_type": {
                        "type": "string",
                        "enum": ["at", "every", "cron"],
                        "description": "New schedule type — providing this replaces the existing schedule"
                    },
                    "schedule_at": {
                        "type": "string",
                        "description": "Local datetime (YYYY-MM-DDTHH:MM:SS), required when schedule_type='at'"
                    },
                    "schedule_every_ms": {
                        "type": "integer",
                        "description": "Interval in milliseconds, required when schedule_type='every'"
                    },
                    "schedule_anchor_ms": {
                        "type": "integer",
                        "description": "Anchor epoch ms, optional when schedule_type='every'"
                    },
                    "schedule_expr": {
                        "type": "string",
                        "description": "6-field cron expression, required when schedule_type='cron'"
                    },
                    "schedule_tz": {
                        "type": "string",
                        "description": "IANA timezone for cron expression evaluation; defaults to the configured timezone"
                    },
                    "delivery": {"type": "string", "enum": ["user_visible", "background"]},
                    "payload_type": {
                        "type": "string",
                        "enum": ["system_event", "agent_turn"],
                        "description": "New payload type — providing this replaces the existing payload"
                    },
                    "payload_text": {
                        "type": "string",
                        "description": "Text to inject, required when payload_type='system_event'"
                    },
                    "payload_message": {
                        "type": "string",
                        "description": "Agent prompt, required when payload_type='agent_turn'"
                    }
                },
                "required": ["id"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let id = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("id is required".to_string()))?
            .to_string();

        let now = crate::time::now_local(self.tz);

        {
            let mut store = self.store.lock().await;

            let job = store
                .get_job_mut(&id)
                .ok_or_else(|| ToolError::Execution(format!("job '{id}' not found")))?;

            if let Some(name) = arguments.get("name").and_then(|v| v.as_str()) {
                name.clone_into(&mut job.name);
            }
            if let Some(desc) = arguments.get("description").and_then(|v| v.as_str()) {
                job.description = Some(desc.to_string());
            }
            if let Some(enabled) = arguments.get("enabled").and_then(Value::as_bool) {
                job.enabled = enabled;
            }
            if let Some(dar) = arguments.get("delete_after_run").and_then(Value::as_bool) {
                job.delete_after_run = dar;
            }
            if arguments.get("schedule_type").is_some() {
                job.schedule = parse_schedule(&arguments, self.tz)?;
                // Recompute next_run when schedule changes
                initialize_next_run(job, now, self.tz).map_err(|e| {
                    ToolError::Execution(format!("failed to compute next run: {e}"))
                })?;
            }
            if arguments.get("delivery").is_some() {
                job.delivery = parse_delivery(&arguments)?;
            }
            if arguments.get("payload_type").is_some() {
                job.payload = parse_payload(&arguments)?;
            }

            job.updated_at = now;

            store
                .save()
                .await
                .map_err(|e| ToolError::Execution(format!("failed to save cron store: {e}")))?;
        }

        self.notify.notify_one();
        Ok(ToolResult::success(format!("Updated job '{id}'")))
    }
}

// ─── cron_remove ─────────────────────────────────────────────────────────────

/// Tool for removing a scheduled job.
pub struct CronRemoveTool {
    store: Arc<Mutex<CronStore>>,
    notify: Arc<Notify>,
}

impl CronRemoveTool {
    /// Create a new `CronRemoveTool`.
    #[must_use]
    pub fn new(store: Arc<Mutex<CronStore>>, notify: Arc<Notify>) -> Self {
        Self { store, notify }
    }
}

#[async_trait]
impl Tool for CronRemoveTool {
    fn name(&self) -> &'static str {
        "cron_remove"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "cron_remove".to_string(),
            description: "Remove a scheduled cron job by ID.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Job ID to remove"
                    }
                },
                "required": ["id"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let id = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("id is required".to_string()))?
            .to_string();

        {
            let mut store = self.store.lock().await;
            if !store.remove_job(&id) {
                return Ok(ToolResult::error(format!("job '{id}' not found")));
            }
            store
                .save()
                .await
                .map_err(|e| ToolError::Execution(format!("failed to save cron store: {e}")))?;
        }

        self.notify.notify_one();
        Ok(ToolResult::success(format!("Removed job '{id}'")))
    }
}

// ─── Argument parsers ────────────────────────────────────────────────────────

fn parse_schedule(args: &Value, default_tz: chrono_tz::Tz) -> Result<CronSchedule, ToolError> {
    let sched_type = args
        .get("schedule_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArguments("schedule_type is required".to_string()))?;

    match sched_type {
        "at" => {
            let at_str = args
                .get("schedule_at")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "schedule_at is required when schedule_type='at'".to_string(),
                    )
                })?;
            let at =
                chrono::NaiveDateTime::parse_from_str(at_str, "%Y-%m-%dT%H:%M").map_err(|e| {
                    ToolError::InvalidArguments(format!(
                        "invalid 'at' datetime '{at_str}' (expected YYYY-MM-DDTHH:MM): {e}"
                    ))
                })?;
            Ok(CronSchedule::At { at })
        }
        "every" => {
            let every_ms = args
                .get("schedule_every_ms")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "schedule_every_ms is required when schedule_type='every'".to_string(),
                    )
                })?;
            let anchor_ms = args
                .get("schedule_anchor_ms")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            Ok(CronSchedule::Every {
                every_ms,
                anchor_ms,
            })
        }
        "cron" => {
            let expr = args
                .get("schedule_expr")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "schedule_expr is required when schedule_type='cron'".to_string(),
                    )
                })?
                .to_string();
            let tz = args
                .get("schedule_tz")
                .and_then(|v| v.as_str())
                .unwrap_or(default_tz.name())
                .to_string();
            Ok(CronSchedule::Cron { expr, tz })
        }
        other => Err(ToolError::InvalidArguments(format!(
            "unknown schedule type '{other}': expected at, every, or cron"
        ))),
    }
}

fn parse_delivery(args: &Value) -> Result<Delivery, ToolError> {
    let value = args
        .get("delivery")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArguments("delivery is required".to_string()))?;

    match value {
        "user_visible" => Ok(Delivery::UserVisible),
        "background" => Ok(Delivery::Background),
        other => Err(ToolError::InvalidArguments(format!(
            "unknown delivery '{other}': expected 'user_visible' or 'background'"
        ))),
    }
}

fn parse_payload(args: &Value) -> Result<CronPayload, ToolError> {
    let payload_type = args
        .get("payload_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArguments("payload_type is required".to_string()))?;

    match payload_type {
        "system_event" => {
            let text = args
                .get("payload_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "payload_text is required when payload_type='system_event'".to_string(),
                    )
                })?
                .to_string();
            Ok(CronPayload::SystemEvent { text })
        }
        "agent_turn" => {
            let message = args
                .get("payload_message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "payload_message is required when payload_type='agent_turn'".to_string(),
                    )
                })?
                .to_string();
            Ok(CronPayload::AgentTurn { message })
        }
        other => Err(ToolError::InvalidArguments(format!(
            "unknown payload type '{other}': expected 'system_event' or 'agent_turn'"
        ))),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    fn make_store_and_notify() -> (Arc<Mutex<CronStore>>, Arc<Notify>) {
        let store = CronStore::new_empty("/tmp/test-jobs.json");
        (Arc::new(Mutex::new(store)), Arc::new(Notify::new()))
    }

    #[test]
    fn parse_schedule_at_valid() {
        let args = serde_json::json!({
            "schedule_type": "at",
            "schedule_at": "2026-02-19T12:00",
            "delivery": "user_visible",
            "payload_type": "system_event",
            "payload_text": "hi"
        });
        let sched = parse_schedule(&args, chrono_tz::UTC).unwrap();
        assert!(
            matches!(sched, CronSchedule::At { .. }),
            "should parse At schedule"
        );
    }

    #[test]
    fn parse_schedule_every_valid() {
        let args = serde_json::json!({
            "schedule_type": "every",
            "schedule_every_ms": 3_600_000
        });
        let sched = parse_schedule(&args, chrono_tz::UTC).unwrap();
        assert!(
            matches!(
                sched,
                CronSchedule::Every {
                    every_ms: 3_600_000,
                    anchor_ms: 0
                }
            ),
            "should parse Every schedule"
        );
    }

    #[test]
    fn parse_schedule_cron_valid() {
        let args = serde_json::json!({
            "schedule_type": "cron",
            "schedule_expr": "0 0 9 * * *"
        });
        let sched = parse_schedule(&args, chrono_tz::UTC).unwrap();
        assert!(
            matches!(sched, CronSchedule::Cron { .. }),
            "should parse Cron schedule"
        );
    }

    #[test]
    fn parse_schedule_unknown_type_errors() {
        let args = serde_json::json!({"schedule_type": "unknown"});
        assert!(
            parse_schedule(&args, chrono_tz::UTC).is_err(),
            "unknown schedule type should error"
        );
    }

    #[test]
    fn parse_delivery_user_visible() {
        let args = serde_json::json!({"delivery": "user_visible"});
        assert_eq!(
            parse_delivery(&args).unwrap(),
            Delivery::UserVisible,
            "should parse user_visible"
        );
    }

    #[test]
    fn parse_delivery_background() {
        let args = serde_json::json!({"delivery": "background"});
        assert_eq!(
            parse_delivery(&args).unwrap(),
            Delivery::Background,
            "should parse background"
        );
    }

    #[test]
    fn parse_payload_system_event() {
        let args = serde_json::json!({"payload_type": "system_event", "payload_text": "hello"});
        let p = parse_payload(&args).unwrap();
        assert!(
            matches!(p, CronPayload::SystemEvent { .. }),
            "should parse SystemEvent"
        );
    }

    #[test]
    fn parse_payload_agent_turn() {
        let args =
            serde_json::json!({"payload_type": "agent_turn", "payload_message": "check email"});
        let p = parse_payload(&args).unwrap();
        assert!(
            matches!(p, CronPayload::AgentTurn { .. }),
            "should parse AgentTurn"
        );
    }

    #[test]
    fn cron_list_tool_has_correct_name() {
        let (store, _) = make_store_and_notify();
        let tool = CronListTool::new(store);
        assert_eq!(tool.name(), "cron_list", "name should be cron_list");
    }
}
