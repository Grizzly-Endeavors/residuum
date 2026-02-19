//! Cron job types: schedules, payloads, state, and job records.

use chrono::{DateTime, Utc};
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
    pub created_at: DateTime<Utc>,
    /// When the job was last modified.
    pub updated_at: DateTime<Utc>,
    /// When and how often to run.
    pub schedule: CronSchedule,
    /// Whether to run in the main session or an isolated turn.
    pub session_target: SessionTarget,
    /// What the job delivers when it fires.
    pub payload: CronPayload,
    /// Runtime state (next run, last run, errors).
    pub state: CronJobState,
}

/// Scheduling strategy for a cron job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CronSchedule {
    /// Fire once at a specific UTC timestamp.
    At {
        /// UTC datetime to fire.
        at: DateTime<Utc>,
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
        /// Timezone string (reserved for Phase 4; "UTC" used for all in Phase 3).
        tz: String,
    },
}

/// Where the job's output is delivered.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionTarget {
    /// Inject into the main user session at the next user turn.
    Main,
    /// Run as an isolated agent turn (does not affect main session).
    Isolated,
}

/// What a job does when it fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CronPayload {
    /// A plain text message queued into the main session.
    SystemEvent {
        /// The event text to inject.
        text: String,
    },
    /// Run an isolated agent turn with this message as the prompt.
    AgentTurn {
        /// The message/prompt for the agent turn.
        message: String,
    },
}

/// Runtime state tracked per job.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CronJobState {
    /// When this job is next scheduled to run.
    pub next_run_at: Option<DateTime<Utc>>,
    /// When this job last ran.
    pub last_run_at: Option<DateTime<Utc>>,
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
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_job() -> CronJob {
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        CronJob {
            id: "cron-deadbeef".to_string(),
            name: "test job".to_string(),
            description: None,
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule: CronSchedule::At { at: now },
            session_target: SessionTarget::Main,
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
        let at = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
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
    fn session_target_serializes() {
        let json = serde_json::to_string(&SessionTarget::Main).unwrap();
        assert_eq!(json, "\"main\"", "main should serialize as 'main'");
        let json2 = serde_json::to_string(&SessionTarget::Isolated).unwrap();
        assert_eq!(
            json2, "\"isolated\"",
            "isolated should serialize as 'isolated'"
        );
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
