//! Cron schedule evaluation: determines when each job next fires.

use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::error::IronclawError;

use super::types::{CronJob, CronSchedule};

/// Minimum gap in milliseconds between same-job re-fires to prevent
/// same-second rescheduling loops.
const MIN_REFIRE_GAP_MS: i64 = 2_000;

/// Compute when a job should next run given the current time.
///
/// Returns `None` for one-shot `At` jobs that have already completed
/// (i.e., `last_run_at` is set to a time >= `at`).
///
/// # Errors
/// Returns `IronclawError::Scheduling` if the cron expression cannot be parsed.
pub fn compute_next_run(
    job: &CronJob,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, IronclawError> {
    match &job.schedule {
        CronSchedule::At { at } => {
            // One-shot: due if not yet run
            if job.state.last_run_at.is_some() {
                Ok(None) // already ran
            } else {
                Ok(Some(*at))
            }
        }

        CronSchedule::Every {
            every_ms,
            anchor_ms,
        } => {
            let interval_ms = (*every_ms).cast_signed();
            if interval_ms <= 0 {
                return Err(IronclawError::Scheduling(format!(
                    "invalid every_ms {} for job '{}'",
                    every_ms, job.name
                )));
            }

            let anchor = DateTime::from_timestamp_millis(*anchor_ms).ok_or_else(|| {
                IronclawError::Scheduling(format!(
                    "invalid anchor_ms {} for job '{}'",
                    anchor_ms, job.name
                ))
            })?;

            let now_ms = now.timestamp_millis();
            let anchor_ms_val = anchor.timestamp_millis();
            let elapsed_ms = now_ms - anchor_ms_val;

            // Number of full periods elapsed (ceiling division; fire immediately if not yet past anchor)
            let periods = if elapsed_ms <= 0 {
                0_i64
            } else {
                (elapsed_ms + interval_ms - 1) / interval_ms
            };
            let next_ms = anchor_ms_val + periods * interval_ms;

            // Clamp to at least MIN_REFIRE_GAP_MS from now
            let min_next_ms = now_ms + MIN_REFIRE_GAP_MS;
            let final_ms = next_ms.max(min_next_ms);

            let next = DateTime::from_timestamp_millis(final_ms).ok_or_else(|| {
                IronclawError::Scheduling(format!(
                    "computed next_run overflows for job '{}'",
                    job.name
                ))
            })?;

            Ok(Some(next))
        }

        CronSchedule::Cron { expr, tz } => {
            if tz != "UTC" && !tz.is_empty() {
                tracing::warn!(
                    job = %job.name,
                    tz = %tz,
                    "non-UTC timezones are not supported in Phase 3; using UTC"
                );
            }

            let schedule = cron::Schedule::from_str(expr).map_err(|e| {
                IronclawError::Scheduling(format!(
                    "invalid cron expression '{}' for job '{}': {e}",
                    expr, job.name
                ))
            })?;

            let next = schedule.after(&now).next();
            Ok(next)
        }
    }
}

/// Compute the next run for a job and apply error backoff if needed.
///
/// If the job had consecutive errors, the natural next run is pushed forward
/// by the corresponding backoff duration.
///
/// # Errors
/// Returns `IronclawError::Scheduling` if `compute_next_run` fails.
pub fn compute_next_run_with_backoff(
    job: &CronJob,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, IronclawError> {
    let natural_next = compute_next_run(job, now)?;

    let Some(next) = natural_next else {
        return Ok(None);
    };

    if job.state.consecutive_errors == 0 {
        return Ok(Some(next));
    }

    let backoff_ms = backoff_duration_ms(job.state.consecutive_errors);
    let backoff_next =
        DateTime::from_timestamp_millis(now.timestamp_millis() + backoff_ms.cast_signed())
            .unwrap_or(next);

    Ok(Some(next.max(backoff_next)))
}

/// Error backoff durations: [30s, 60s, 5m, 15m, 1h] in milliseconds.
fn backoff_duration_ms(consecutive_errors: u32) -> u64 {
    const BACKOFFS: [u64; 5] = [30_000, 60_000, 300_000, 900_000, 3_600_000];
    let idx = (consecutive_errors as usize)
        .saturating_sub(1)
        .min(BACKOFFS.len() - 1);
    BACKOFFS.get(idx).copied().unwrap_or(3_600_000)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use chrono::TimeZone;

    use crate::cron::types::{CronJobState, CronPayload, CronSchedule, Delivery};

    fn base_job(schedule: CronSchedule) -> CronJob {
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        CronJob {
            id: "cron-test".to_string(),
            name: "test".to_string(),
            description: None,
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule,
            delivery: Delivery::UserVisible,
            payload: CronPayload::SystemEvent {
                text: "test".to_string(),
            },
            state: CronJobState::default(),
        }
    }

    #[test]
    fn at_schedule_future_returns_target() {
        let target = Utc.with_ymd_and_hms(2026, 2, 20, 12, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        let job = base_job(CronSchedule::At { at: target });
        let next = compute_next_run(&job, now).unwrap();
        assert_eq!(next, Some(target), "At job should return its target time");
    }

    #[test]
    fn at_schedule_after_run_returns_none() {
        let target = Utc.with_ymd_and_hms(2026, 2, 19, 11, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        let mut job = base_job(CronSchedule::At { at: target });
        job.state.last_run_at = Some(now);
        let next = compute_next_run(&job, now).unwrap();
        assert_eq!(next, None, "At job that already ran should return None");
    }

    #[test]
    fn every_schedule_computes_next() {
        // 1-hour interval anchored at epoch
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 30, 0).unwrap();
        let job = base_job(CronSchedule::Every {
            every_ms: 3_600_000,
            anchor_ms: 0,
        });
        let next = compute_next_run(&job, now).unwrap().unwrap();
        // Should be the next top-of-hour (13:00 UTC) or at minimum now + 2s
        assert!(next > now, "next should be in the future");
    }

    #[test]
    fn every_schedule_min_refire_gap() {
        // Very short interval (100ms) - next should be at least 2s away
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        let job = base_job(CronSchedule::Every {
            every_ms: 100,
            anchor_ms: 0,
        });
        let next = compute_next_run(&job, now).unwrap().unwrap();
        let gap_ms = next.timestamp_millis() - now.timestamp_millis();
        assert!(
            gap_ms >= MIN_REFIRE_GAP_MS,
            "next should be at least MIN_REFIRE_GAP_MS away"
        );
    }

    #[test]
    fn cron_expression_next_occurrence() {
        // Fire at 9:00 AM every day
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 8, 0, 0).unwrap();
        let job = base_job(CronSchedule::Cron {
            expr: "0 0 9 * * *".to_string(),
            tz: "UTC".to_string(),
        });
        let next = compute_next_run(&job, now).unwrap().unwrap();
        assert_eq!(
            next.time(),
            chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            "should fire at 9:00"
        );
    }

    #[test]
    fn invalid_cron_expression_errors() {
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        let job = base_job(CronSchedule::Cron {
            expr: "not a cron".to_string(),
            tz: "UTC".to_string(),
        });
        assert!(
            compute_next_run(&job, now).is_err(),
            "invalid cron expr should error"
        );
    }

    #[test]
    fn backoff_duration_first_error() {
        assert_eq!(backoff_duration_ms(1), 30_000, "first error: 30s backoff");
    }

    #[test]
    fn backoff_duration_caps_at_max() {
        assert_eq!(backoff_duration_ms(100), 3_600_000, "should cap at 1h");
    }
}
