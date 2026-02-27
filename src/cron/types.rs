//! Cron job types: schedules, payloads, state, and job records.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// A scheduled job managed by the cron system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique job identifier (`cron-{hex}`).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description of what this job does.
    pub description: Option<String>,
    /// Whether this job is active.
    pub enabled: bool,
    /// Delete the job record after it successfully runs once.
    pub delete_after_run: bool,
    /// When the job was created.
    #[serde(with = "crate::time::minute_format")]
    pub created_at: NaiveDateTime,
    /// When the job was last modified.
    #[serde(with = "crate::time::minute_format")]
    pub updated_at: NaiveDateTime,
    /// When and how often to run.
    pub schedule: CronSchedule,
    /// What the job delivers when it fires.
    pub payload: CronPayload,
    /// Runtime state (next run, last run, errors).
    pub state: CronJobState,
}

/// Scheduling strategy for a cron job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CronSchedule {
    /// Fire once at a specific local datetime.
    At {
        /// Local datetime to fire.
        #[serde(with = "crate::time::minute_format")]
        at: NaiveDateTime,
    },
    /// Fire repeatedly on a fixed interval anchored to an epoch.
    Every {
        /// Interval in milliseconds.
        every_ms: u64,
        /// Anchor epoch (milliseconds since Unix epoch).
        anchor_ms: i64,
    },
    /// Fire on a cron expression schedule.
    Cron {
        /// Standard cron expression (5 or 6 fields).
        expr: String,
        /// IANA timezone for cron evaluation.
        tz: String,
    },
}

/// What a job does when it fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CronPayload {
    /// A plain text message queued into the main conversation.
    SystemEvent {
        /// The event text to inject.
        text: String,
    },
    /// Run an isolated agent turn with this message as the prompt.
    AgentTurn {
        /// The message/prompt for the agent turn.
        message: String,
        /// Optional agent routing: `"main"` for a full wake turn, or a preset name.
        #[serde(default)]
        agent: Option<String>,
    },
}

/// Runtime state tracked per job.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CronJobState {
    /// When this job is next scheduled to run.
    #[serde(default, with = "crate::time::minute_format_opt")]
    pub next_run_at: Option<NaiveDateTime>,
    /// When this job last ran.
    #[serde(default, with = "crate::time::minute_format_opt")]
    pub last_run_at: Option<NaiveDateTime>,
    /// Status of the last run.
    pub last_status: Option<RunStatus>,
    /// Error message from the last failed run, if any.
    pub last_error: Option<String>,
    /// Number of consecutive failed runs (used for backoff).
    pub consecutive_errors: u32,
}

/// Result status of a job run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Job ran successfully.
    Ok,
    /// Job encountered an error.
    Error,
    /// Job was skipped (e.g. scheduler overlap guard).
    Skipped,
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::panic,
    reason = "test assertions use panic for unreachable variants"
)]
mod tests {
    use super::*;

    fn sample_job() -> CronJob {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        CronJob {
            id: "cron-deadbeef".to_string(),
            name: "test job".to_string(),
            description: None,
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule: CronSchedule::At { at: now },
            payload: CronPayload::SystemEvent {
                text: "hello".to_string(),
            },
            state: CronJobState::default(),
        }
    }

    #[test]
    fn cron_job_serializes_roundtrip() {
        let job = sample_job();
        let json = serde_json::to_string(&job).unwrap();
        let back: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, job.id, "id should round-trip");
        assert_eq!(back.name, job.name, "name should round-trip");
    }

    #[test]
    fn cron_schedule_at_tagged() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let sched = CronSchedule::At { at };
        let json = serde_json::to_string(&sched).unwrap();
        assert!(json.contains("\"type\":\"at\""), "should use 'at' tag");
    }

    #[test]
    fn cron_schedule_every_tagged() {
        let sched = CronSchedule::Every {
            every_ms: 3_600_000,
            anchor_ms: 0,
        };
        let json = serde_json::to_string(&sched).unwrap();
        assert!(
            json.contains("\"type\":\"every\""),
            "should use 'every' tag"
        );
    }

    #[test]
    fn cron_schedule_cron_tagged() {
        let sched = CronSchedule::Cron {
            expr: "0 30 9 * * *".to_string(),
            tz: "UTC".to_string(),
        };
        let json = serde_json::to_string(&sched).unwrap();
        assert!(json.contains("\"type\":\"cron\""), "should use 'cron' tag");
    }

    #[test]
    fn agent_turn_with_agent_field_roundtrip() {
        let payload = CronPayload::AgentTurn {
            message: "check email".to_string(),
            agent: Some("memory-agent".to_string()),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let back: CronPayload = serde_json::from_str(&json).unwrap();
        match back {
            CronPayload::AgentTurn { message, agent } => {
                assert_eq!(message, "check email");
                assert_eq!(agent.as_deref(), Some("memory-agent"));
            }
            CronPayload::SystemEvent { .. } => panic!("expected AgentTurn"),
        }
    }

    #[test]
    fn agent_turn_with_main_agent_roundtrip() {
        let payload = CronPayload::AgentTurn {
            message: "daily plan".to_string(),
            agent: Some("main".to_string()),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let back: CronPayload = serde_json::from_str(&json).unwrap();
        match back {
            CronPayload::AgentTurn { message, agent } => {
                assert_eq!(message, "daily plan");
                assert_eq!(agent.as_deref(), Some("main"));
            }
            CronPayload::SystemEvent { .. } => panic!("expected AgentTurn"),
        }
    }

    #[test]
    fn agent_turn_without_agent_field_backward_compat() {
        // Simulates loading a jobs.json that predates the `agent` field
        let json = r#"{"type":"agent_turn","message":"do something"}"#;
        let payload: CronPayload = serde_json::from_str(json).unwrap();
        match payload {
            CronPayload::AgentTurn { message, agent } => {
                assert_eq!(message, "do something");
                assert!(agent.is_none(), "agent should default to None");
            }
            CronPayload::SystemEvent { .. } => panic!("expected AgentTurn"),
        }
    }

    #[test]
    fn cron_job_state_defaults() {
        let state = CronJobState::default();
        assert!(
            state.next_run_at.is_none(),
            "next_run_at should default to None"
        );
        assert_eq!(
            state.consecutive_errors, 0,
            "consecutive_errors should default to 0"
        );
    }
}
