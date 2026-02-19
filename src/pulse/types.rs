use chrono::{DateTime, Duration, NaiveTime, Utc};
use serde::Deserialize;

use crate::error::IronclawError;

/// Top-level HEARTBEAT.yml structure.
#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default)]
    pub pulses: Vec<PulseDef>,
}

/// One pulse definition from HEARTBEAT.yml.
#[derive(Debug, Clone, Deserialize)]
pub struct PulseDef {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub schedule: String,
    pub active_hours: Option<String>,
    #[serde(default)]
    pub tasks: Vec<PulseTask>,
}

fn default_enabled() -> bool {
    true
}

/// One task within a pulse.
#[derive(Debug, Clone, Deserialize)]
pub struct PulseTask {
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub alert: AlertLevel,
}

/// Alert urgency level.
#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AlertLevel {
    /// Highest urgency — shown prominently to the user.
    High,
    /// Normal urgency — shown to the user.
    #[default]
    Medium,
    /// Low urgency — logged at info level only.
    Low,
}

/// Parse a schedule duration string like "30m", "2h", "24h", "1d", "60s".
///
/// # Errors
///
/// Returns `IronclawError::Scheduling` if the string is empty, has no unit suffix,
/// or contains a non-numeric value before the suffix.
pub fn parse_schedule_duration(s: &str) -> Result<Duration, IronclawError> {
    if s.is_empty() {
        return Err(IronclawError::Scheduling(
            "schedule duration cannot be empty".to_string(),
        ));
    }
    let (num_part, unit) = s.split_at(s.len() - 1);
    let value: i64 = num_part.parse().map_err(|_parse_err| {
        IronclawError::Scheduling(format!(
            "invalid schedule duration '{s}': expected number followed by s/m/h/d",
        ))
    })?;
    match unit {
        "s" => Ok(Duration::seconds(value)),
        "m" => Ok(Duration::seconds(value * 60)),
        "h" => Ok(Duration::seconds(value * 3600)),
        "d" => Ok(Duration::seconds(value * 86_400)),
        other => Err(IronclawError::Scheduling(format!(
            "unknown duration unit '{other}' in '{s}': expected s, m, h, or d",
        ))),
    }
}

/// Parse an active-hours window string like "08:00-18:00".
///
/// Returns `(start_time, end_time)` as `NaiveTime` values in UTC.
///
/// # Errors
///
/// Returns `IronclawError::Scheduling` if the string is malformed or contains
/// out-of-range hour/minute values.
pub fn parse_active_hours(s: &str) -> Result<(NaiveTime, NaiveTime), IronclawError> {
    let (start_str, end_str) = s.split_once('-').ok_or_else(|| {
        IronclawError::Scheduling(format!(
            "invalid active_hours '{s}': expected 'HH:MM-HH:MM'",
        ))
    })?;
    let start = parse_naive_time(start_str, s)?;
    let end = parse_naive_time(end_str, s)?;
    Ok((start, end))
}

fn parse_naive_time(t: &str, context: &str) -> Result<NaiveTime, IronclawError> {
    let (hour_str, min_str) = t.split_once(':').ok_or_else(|| {
        IronclawError::Scheduling(format!(
            "invalid time '{t}' in active_hours '{context}': expected HH:MM",
        ))
    })?;
    let hour: u32 = hour_str.parse().map_err(|_parse_err| {
        IronclawError::Scheduling(format!(
            "invalid hour '{hour_str}' in active_hours '{context}'",
        ))
    })?;
    let min: u32 = min_str.parse().map_err(|_parse_err| {
        IronclawError::Scheduling(format!(
            "invalid minute '{min_str}' in active_hours '{context}'",
        ))
    })?;
    NaiveTime::from_hms_opt(hour, min, 0).ok_or_else(|| {
        IronclawError::Scheduling(format!(
            "out-of-range time '{t}' in active_hours '{context}'",
        ))
    })
}

/// Check whether `now` falls within the active hours window (inclusive of start, exclusive of end).
///
/// Handles overnight windows (e.g. "22:00-06:00") where `start > end`.
#[must_use]
pub fn is_within_active_hours(now: DateTime<Utc>, start: NaiveTime, end: NaiveTime) -> bool {
    let now_time = now.time();
    if start <= end {
        now_time >= start && now_time < end
    } else {
        // Overnight window (e.g. 22:00-06:00)
        now_time >= start || now_time < end
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parse_schedule_duration_minutes() {
        let d = parse_schedule_duration("30m").unwrap();
        assert_eq!(d, Duration::seconds(1800), "30m should be 1800 seconds");
    }

    #[test]
    fn parse_schedule_duration_hours() {
        let d = parse_schedule_duration("2h").unwrap();
        assert_eq!(d, Duration::seconds(7200), "2h should be 7200 seconds");
    }

    #[test]
    fn parse_schedule_duration_days() {
        let d = parse_schedule_duration("1d").unwrap();
        assert_eq!(d, Duration::seconds(86_400), "1d should be 86400 seconds");
    }

    #[test]
    fn parse_schedule_duration_seconds() {
        let d = parse_schedule_duration("60s").unwrap();
        assert_eq!(d, Duration::seconds(60), "60s should be 60 seconds");
    }

    #[test]
    fn parse_schedule_duration_empty_fails() {
        assert!(
            parse_schedule_duration("").is_err(),
            "empty string should fail"
        );
    }

    #[test]
    fn parse_schedule_duration_invalid_fails() {
        assert!(
            parse_schedule_duration("xyz").is_err(),
            "non-numeric should fail"
        );
        assert!(
            parse_schedule_duration("10x").is_err(),
            "unknown unit should fail"
        );
    }

    #[test]
    fn parse_active_hours_valid() {
        let (start, end) = parse_active_hours("08:00-18:00").unwrap();
        assert_eq!(
            start,
            NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
            "start should be 08:00"
        );
        assert_eq!(
            end,
            NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
            "end should be 18:00"
        );
    }

    #[test]
    fn parse_active_hours_full_day() {
        let (start, end) = parse_active_hours("00:00-23:59").unwrap();
        assert_eq!(
            start,
            NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            "start at midnight"
        );
        assert_eq!(
            end,
            NaiveTime::from_hms_opt(23, 59, 0).unwrap(),
            "end at 23:59"
        );
    }

    #[test]
    fn parse_active_hours_missing_dash_fails() {
        assert!(
            parse_active_hours("08:00").is_err(),
            "missing dash should fail"
        );
    }

    #[test]
    fn parse_active_hours_bad_format_fails() {
        assert!(parse_active_hours("bad").is_err(), "bad format should fail");
    }

    #[test]
    fn is_within_active_hours_inside() {
        let noon = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert!(
            is_within_active_hours(noon, start, end),
            "noon should be inside 08:00-18:00"
        );
    }

    #[test]
    fn is_within_active_hours_outside() {
        let late = Utc.with_ymd_and_hms(2026, 2, 19, 22, 0, 0).unwrap();
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert!(
            !is_within_active_hours(late, start, end),
            "22:00 should be outside 08:00-18:00"
        );
    }

    #[test]
    fn heartbeat_config_empty_pulses() {
        let yaml = "pulses: []";
        let cfg: HeartbeatConfig = serde_yml::from_str(yaml).unwrap();
        assert!(cfg.pulses.is_empty(), "empty pulses should parse");
    }

    #[test]
    fn heartbeat_config_missing_pulses_key() {
        let yaml = "{}";
        let cfg: HeartbeatConfig = serde_yml::from_str(yaml).unwrap();
        assert!(
            cfg.pulses.is_empty(),
            "missing pulses key should default to empty"
        );
    }

    #[test]
    fn heartbeat_config_one_pulse() {
        let yaml = r#"
pulses:
  - name: email_check
    schedule: "30m"
    tasks:
      - name: check_inbox
        prompt: "Check email"
        alert: high
"#;
        let cfg: HeartbeatConfig = serde_yml::from_str(yaml).unwrap();
        assert_eq!(cfg.pulses.len(), 1, "should parse one pulse");
        let pulse = cfg.pulses.first().unwrap();
        assert_eq!(pulse.name, "email_check", "pulse name should match");
        assert!(pulse.enabled, "enabled should default to true");
        assert_eq!(pulse.tasks.len(), 1, "should have one task");
    }

    #[test]
    fn pulse_def_enabled_defaults_true() {
        let yaml = r#"
pulses:
  - name: test
    schedule: "1h"
    tasks: []
"#;
        let cfg: HeartbeatConfig = serde_yml::from_str(yaml).unwrap();
        let pulse = cfg.pulses.first().unwrap();
        assert!(pulse.enabled, "enabled should default to true when absent");
    }
}
