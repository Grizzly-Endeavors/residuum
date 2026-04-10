use std::path::Path;

use chrono::{Duration, NaiveDateTime, NaiveTime};
use serde::Deserialize;

use anyhow::bail;

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
    /// Optional agent routing: `"main"` for a full wake turn, or a preset name.
    #[serde(default)]
    pub agent: Option<String>,
    /// Maximum number of times this pulse fires within its active period.
    /// When set, firings are spaced evenly across the active window.
    #[serde(default)]
    pub trigger_count: Option<u32>,
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
}

/// Parse a schedule duration string like "30m", "2h", "24h", "1d", "60s".
///
/// # Errors
///
/// Returns an error if the string is empty, has no unit suffix,
/// or contains a non-numeric value before the suffix.
pub fn parse_schedule_duration(s: &str) -> anyhow::Result<Duration> {
    if s.is_empty() {
        bail!("schedule duration cannot be empty");
    }
    let Some(last_byte_idx) = s.char_indices().next_back().map(|(i, _)| i) else {
        bail!("schedule duration cannot be empty");
    };
    let (num_part, unit) = s.split_at(last_byte_idx);
    let value: i64 = num_part.parse().map_err(|_parse_err| {
        anyhow::anyhow!("invalid schedule duration '{s}': expected number followed by s/m/h/d",)
    })?;
    if value <= 0 {
        bail!("schedule duration must be positive, got '{s}'");
    }
    match unit {
        "s" => Ok(Duration::seconds(value)),
        "m" => Ok(Duration::minutes(value)),
        "h" => Ok(Duration::hours(value)),
        "d" => Ok(Duration::days(value)),
        other => bail!("unknown duration unit '{other}' in '{s}': expected s, m, h, or d",),
    }
}

/// Parse an active-hours window string like "08:00-18:00".
///
/// Returns `(start_time, end_time)` as `NaiveTime` values in the configured timezone.
///
/// # Errors
///
/// Returns an error if the string is malformed or contains
/// out-of-range hour/minute values.
pub fn parse_active_hours(s: &str) -> anyhow::Result<(NaiveTime, NaiveTime)> {
    let (start_str, end_str) = s
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("invalid active_hours '{s}': expected 'HH:MM-HH:MM'",))?;
    let start = parse_naive_time(start_str, s)?;
    let end = parse_naive_time(end_str, s)?;
    Ok((start, end))
}

fn parse_naive_time(t: &str, context: &str) -> anyhow::Result<NaiveTime> {
    let (hour_str, min_str) = t.split_once(':').ok_or_else(|| {
        anyhow::anyhow!("invalid time '{t}' in active_hours '{context}': expected HH:MM",)
    })?;
    let hour: u32 = hour_str.parse().map_err(|_parse_err| {
        anyhow::anyhow!("invalid hour '{hour_str}' in active_hours '{context}'",)
    })?;
    let min: u32 = min_str.parse().map_err(|_parse_err| {
        anyhow::anyhow!("invalid minute '{min_str}' in active_hours '{context}'",)
    })?;
    NaiveTime::from_hms_opt(hour, min, 0)
        .ok_or_else(|| anyhow::anyhow!("out-of-range time '{t}' in active_hours '{context}'",))
}

/// Check whether `now` falls within the active hours window (inclusive of start, exclusive of end).
///
/// Handles overnight windows (e.g. "22:00-06:00") where `start > end`.
#[must_use]
pub fn is_within_active_hours(now: NaiveDateTime, start: NaiveTime, end: NaiveTime) -> bool {
    let now_time = now.time();
    if start <= end {
        now_time >= start && now_time < end
    } else {
        // Overnight window (e.g. 22:00-06:00)
        now_time >= start || now_time < end
    }
}

/// Read a file and parse its contents, returning `None` on missing file or errors.
///
/// Logs a warning on parse or read failures; silently returns `None` for missing files.
pub(crate) fn read_and_parse<T, E>(path: &Path, parse: impl Fn(&str) -> Result<T, E>) -> Option<T>
where
    E: std::fmt::Display,
{
    match std::fs::read_to_string(path) {
        Ok(contents) => match parse(&contents) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to parse file");
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read file");
            None
        }
    }
}

/// Load HEARTBEAT.yml from the given path.
///
/// On parse error, logs a warning and returns `None` so the caller keeps the last good config.
/// Returns `None` if the file does not exist.
#[must_use]
pub(crate) fn load_heartbeat(path: &Path) -> Option<HeartbeatConfig> {
    let cfg = read_and_parse(path, |s| serde_yaml_ng::from_str::<HeartbeatConfig>(s))?;
    tracing::trace!(path = %path.display(), pulses = cfg.pulses.len(), "loaded HEARTBEAT.yml");
    Some(cfg)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
        let noon = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert!(
            is_within_active_hours(noon, start, end),
            "noon should be inside 08:00-18:00"
        );
    }

    #[test]
    fn is_within_active_hours_outside() {
        let late = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert!(
            !is_within_active_hours(late, start, end),
            "22:00 should be outside 08:00-18:00"
        );
    }

    #[test]
    fn is_within_active_hours_boundary_start_inclusive() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(8, 0, 0)
            .unwrap();
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert!(
            is_within_active_hours(t, start, end),
            "start time should be inclusive"
        );
    }

    #[test]
    fn is_within_active_hours_boundary_end_exclusive() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert!(
            !is_within_active_hours(t, start, end),
            "end time should be exclusive"
        );
    }

    #[test]
    fn is_within_active_hours_overnight_inside_before_midnight() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(23, 0, 0)
            .unwrap();
        let start = NaiveTime::from_hms_opt(22, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
        assert!(
            is_within_active_hours(t, start, end),
            "23:00 should be inside overnight window 22:00-06:00"
        );
    }

    #[test]
    fn is_within_active_hours_overnight_outside() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let start = NaiveTime::from_hms_opt(22, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
        assert!(
            !is_within_active_hours(t, start, end),
            "12:00 should be outside overnight window 22:00-06:00"
        );
    }

    #[test]
    fn is_within_active_hours_overnight_inside_after_midnight() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(1, 0, 0)
            .unwrap();
        let start = NaiveTime::from_hms_opt(22, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
        assert!(
            is_within_active_hours(t, start, end),
            "01:00 should be inside overnight window 22:00-06:00"
        );
    }

    #[test]
    fn parse_schedule_duration_zero_fails() {
        assert!(
            parse_schedule_duration("0m").is_err(),
            "zero duration should fail"
        );
    }

    #[test]
    fn parse_schedule_duration_negative_fails() {
        assert!(
            parse_schedule_duration("-1h").is_err(),
            "negative duration should fail"
        );
    }

    #[test]
    fn heartbeat_config_empty_pulses() {
        let yaml = "pulses: []";
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(cfg.pulses.is_empty(), "empty pulses should parse");
    }

    #[test]
    fn heartbeat_config_missing_pulses_key() {
        let yaml = "{}";
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
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
"#;
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.pulses.len(), 1, "should parse one pulse");
        let pulse = cfg.pulses.first().unwrap();
        assert_eq!(pulse.name, "email_check", "pulse name should match");
        assert!(pulse.enabled, "enabled should default to true");
        assert_eq!(pulse.tasks.len(), 1, "should have one task");
    }

    #[test]
    fn pulse_def_agent_field_present() {
        let yaml = r#"
pulses:
  - name: daily_plan
    schedule: "24h"
    agent: main
    tasks:
      - name: plan
        prompt: "Plan the day."
"#;
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let pulse = cfg.pulses.first().unwrap();
        assert_eq!(
            pulse.agent.as_deref(),
            Some("main"),
            "agent should be 'main'"
        );
    }

    #[test]
    fn pulse_def_agent_field_preset_name() {
        let yaml = r#"
pulses:
  - name: email_triage
    schedule: "30m"
    agent: memory-agent
    tasks:
      - name: check
        prompt: "Check email."
"#;
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let pulse = cfg.pulses.first().unwrap();
        assert_eq!(
            pulse.agent.as_deref(),
            Some("memory-agent"),
            "agent should be 'memory-agent'"
        );
    }

    #[test]
    fn pulse_def_agent_field_absent() {
        let yaml = r#"
pulses:
  - name: basic
    schedule: "1h"
    tasks: []
"#;
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let pulse = cfg.pulses.first().unwrap();
        assert!(
            pulse.agent.is_none(),
            "agent should default to None when absent"
        );
    }

    #[test]
    fn pulse_def_trigger_count_present() {
        let yaml = r#"
pulses:
  - name: limited
    schedule: "1h"
    active_hours: "09:00-17:00"
    trigger_count: 3
    tasks: []
"#;
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let pulse = cfg.pulses.first().unwrap();
        assert_eq!(pulse.trigger_count, Some(3), "trigger_count should be 3");
    }

    #[test]
    fn pulse_def_trigger_count_absent() {
        let yaml = r#"
pulses:
  - name: unlimited
    schedule: "1h"
    tasks: []
"#;
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let pulse = cfg.pulses.first().unwrap();
        assert!(
            pulse.trigger_count.is_none(),
            "trigger_count should default to None"
        );
    }

    #[test]
    fn pulse_def_enabled_defaults_true() {
        let yaml = r#"
pulses:
  - name: test
    schedule: "1h"
    tasks: []
"#;
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let pulse = cfg.pulses.first().unwrap();
        assert!(pulse.enabled, "enabled should default to true when absent");
    }

    #[test]
    fn load_heartbeat_missing_file_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        assert!(
            load_heartbeat(&path).is_none(),
            "missing file should return None"
        );
    }

    #[test]
    fn load_heartbeat_invalid_yaml_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();
        assert!(
            load_heartbeat(&path).is_none(),
            "invalid YAML should return None"
        );
    }
}
