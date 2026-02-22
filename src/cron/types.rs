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
    pub created_at: NaiveDateTime,
    /// When the job was last modified.
    pub updated_at: NaiveDateTime,
    /// When and how often to run.
    pub schedule: CronSchedule,
    /// How the job's output is delivered: visible to the user or in the background.
    pub delivery: Delivery,
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

/// How a job's output is delivered.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Print to CLI and queue for the next user turn (user sees it).
    #[serde(alias = "main")]
    UserVisible,
    /// Run silently in the background (feeds memory pipeline only).
    #[serde(alias = "isolated")]
    Background,
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
    pub next_run_at: Option<NaiveDateTime>,
    /// When this job last ran.
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
            delivery: Delivery::UserVisible,
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
    fn delivery_serializes() {
        let json = serde_json::to_string(&Delivery::UserVisible).unwrap();
        assert_eq!(
            json, "\"user_visible\"",
            "user_visible should serialize as 'user_visible'"
        );
        let json2 = serde_json::to_string(&Delivery::Background).unwrap();
        assert_eq!(
            json2, "\"background\"",
            "background should serialize as 'background'"
        );
    }

    #[test]
    fn delivery_legacy_aliases_deserialize() {
        let main: Delivery = serde_json::from_str("\"main\"").unwrap();
        assert_eq!(
            main,
            Delivery::UserVisible,
            "'main' should deserialize to UserVisible"
        );
        let isolated: Delivery = serde_json::from_str("\"isolated\"").unwrap();
        assert_eq!(
            isolated,
            Delivery::Background,
            "'isolated' should deserialize to Background"
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
