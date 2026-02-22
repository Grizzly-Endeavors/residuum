use std::collections::HashMap;
use std::path::Path;

use chrono::NaiveDateTime;

use super::types::{
    HeartbeatConfig, PulseDef, is_within_active_hours, parse_active_hours, parse_schedule_duration,
};

/// Tracks per-pulse last-run times and determines which pulses are due.
///
/// Timestamps are in-memory only and reset on restart, so pulses fire
/// immediately on first run after startup.
pub struct PulseScheduler {
    last_run: HashMap<String, NaiveDateTime>,
}

impl Default for PulseScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl PulseScheduler {
    /// Create a new scheduler with no run history.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_run: HashMap::new(),
        }
    }

    /// Load HEARTBEAT.yml from the given path.
    ///
    /// On parse error, logs a warning and returns `None` so the caller keeps the last good config.
    /// Returns `None` if the file does not exist.
    #[must_use]
    pub fn load_heartbeat(path: &Path) -> Option<HeartbeatConfig> {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_yml::from_str::<HeartbeatConfig>(&contents) {
                Ok(cfg) => Some(cfg),
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to parse HEARTBEAT.yml, keeping last good config"
                    );
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to read HEARTBEAT.yml"
                );
                None
            }
        }
    }

    /// Find pulses that are due at `now`, hot-reloading HEARTBEAT.yml each call.
    ///
    /// A pulse is due when all of the following hold:
    /// - `enabled == true`
    /// - Its schedule duration can be parsed
    /// - Either it has never run, or `now - last_run >= schedule_duration`
    /// - If `active_hours` is set, `now` falls within the window
    ///
    /// Due pulses have their `last_run` updated to `now`.
    #[must_use]
    pub fn due_pulses(&mut self, now: NaiveDateTime, heartbeat_path: &Path) -> Vec<PulseDef> {
        let Some(heartbeat) = Self::load_heartbeat(heartbeat_path) else {
            return Vec::new();
        };

        let mut due = Vec::new();

        for pulse in heartbeat.pulses {
            if !pulse.enabled {
                continue;
            }

            let duration = match parse_schedule_duration(&pulse.schedule) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        pulse = %pulse.name,
                        schedule = %pulse.schedule,
                        error = %e,
                        "invalid schedule, skipping pulse"
                    );
                    continue;
                }
            };

            // Check active hours if configured
            if let Some(ref hours_str) = pulse.active_hours {
                match parse_active_hours(hours_str) {
                    Ok((start, end)) => {
                        if !is_within_active_hours(now, start, end) {
                            continue;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            pulse = %pulse.name,
                            active_hours = %hours_str,
                            error = %e,
                            "invalid active_hours, skipping pulse"
                        );
                        continue;
                    }
                }
            }

            // Check if due: fire immediately if never run, otherwise after the schedule period
            let is_due = match self.last_run.get(&pulse.name) {
                None => true,
                Some(last) => (now - *last) >= duration,
            };

            if is_due {
                self.last_run.insert(pulse.name.clone(), now);
                due.push(pulse);
            }
        }

        due
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_heartbeat(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("HEARTBEAT.yml");
        std::fs::write(&path, content).unwrap();
        path
    }

    const SIMPLE_HEARTBEAT: &str = r#"
pulses:
  - name: test_pulse
    enabled: true
    schedule: "1h"
    tasks:
      - name: check
        prompt: "Do a check"
        alert: low
"#;

    #[test]
    fn due_pulses_fires_immediately_when_never_run() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(now, &path);
        assert_eq!(due.len(), 1, "should fire on first run");
        assert_eq!(due.first().unwrap().name, "test_pulse", "name should match");
    }

    #[test]
    fn due_pulses_does_not_refire_when_recent() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        // First run marks it as run
        let first = scheduler.due_pulses(now, &path);
        assert_eq!(first.len(), 1, "should fire on first run");

        // 30 minutes later — not yet due (schedule is 1h)
        let later = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap();
        let due = scheduler.due_pulses(later, &path);
        assert!(due.is_empty(), "should not refire within schedule period");
    }

    #[test]
    fn due_pulses_skips_disabled() {
        let yaml = r#"
pulses:
  - name: disabled_pulse
    enabled: false
    schedule: "1h"
    tasks: []
"#;
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), yaml);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(now, &path);
        assert!(due.is_empty(), "disabled pulse should not fire");
    }

    #[test]
    fn due_pulses_respects_active_hours_outside_window() {
        let yaml = r#"
pulses:
  - name: daytime_pulse
    enabled: true
    schedule: "1h"
    active_hours: "09:00-17:00"
    tasks: []
"#;
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), yaml);
        let mut scheduler = PulseScheduler::new();
        // 22:00 UTC — outside 09:00-17:00
        let night = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(night, &path);
        assert!(due.is_empty(), "pulse should not fire outside active hours");
    }

    #[test]
    fn due_pulses_respects_active_hours_inside_window() {
        let yaml = r#"
pulses:
  - name: daytime_pulse
    enabled: true
    schedule: "1h"
    active_hours: "09:00-17:00"
    tasks: []
"#;
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), yaml);
        let mut scheduler = PulseScheduler::new();
        // 12:00 UTC — inside 09:00-17:00
        let day = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(day, &path);
        assert_eq!(due.len(), 1, "pulse should fire inside active hours");
    }

    #[test]
    fn load_heartbeat_missing_file_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        assert!(
            PulseScheduler::load_heartbeat(&path).is_none(),
            "missing file should return None"
        );
    }

    #[test]
    fn load_heartbeat_invalid_yaml_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();
        assert!(
            PulseScheduler::load_heartbeat(&path).is_none(),
            "invalid YAML should return None"
        );
    }
}
