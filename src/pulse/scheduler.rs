use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use chrono::{Duration, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};

use super::types::{
    HeartbeatConfig, PulseDef, is_within_active_hours, parse_active_hours, parse_schedule_duration,
};

/// On-disk format for `pulse_state.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PulseState {
    #[serde(default)]
    last_run: HashMap<String, NaiveDateTime>,
    #[serde(default)]
    run_counts: HashMap<String, u32>,
}

/// Tracks per-pulse last-run times and determines which pulses are due.
///
/// When constructed with `with_state_path`, timestamps are persisted to
/// `pulse_state.json` and survive restarts. Without a state path, timestamps
/// are in-memory only (backward-compatible).
pub struct PulseScheduler {
    last_run: HashMap<String, NaiveDateTime>,
    run_counts: HashMap<String, u32>,
    state_path: Option<PathBuf>,
}

impl Default for PulseScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl PulseScheduler {
    /// Create a new scheduler with no run history and no persistence.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_run: HashMap::new(),
            run_counts: HashMap::new(),
            state_path: None,
        }
    }

    /// Create a scheduler that persists state to the given path.
    ///
    /// Loads existing state from disk if the file exists. Missing or corrupt
    /// files are treated as empty state (logged as a warning for corrupt files).
    #[must_use]
    pub fn with_state_path(path: &Path) -> Self {
        let state = load_state(path);
        Self {
            last_run: state.last_run,
            run_counts: state.run_counts,
            state_path: Some(path.to_path_buf()),
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
    /// - Either it has never run, or `now - last_run >= effective_interval`
    /// - If `active_hours` is set, `now` falls within the window
    /// - If `trigger_count` is set, the count has not been exhausted in the current active period
    ///
    /// Due pulses have their `last_run` updated to `now` and persisted (if a state path is set).
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

            // Parse active hours window (needed for trigger_count spacing)
            let active_window = pulse.active_hours.as_ref().and_then(|hours_str| {
                match parse_active_hours(hours_str) {
                    Ok(window) => Some(window),
                    Err(e) => {
                        tracing::warn!(
                            pulse = %pulse.name,
                            active_hours = %hours_str,
                            error = %e,
                            "invalid active_hours, skipping pulse"
                        );
                        None
                    }
                }
            });

            // Skip if active_hours was set but failed to parse
            if pulse.active_hours.is_some() && active_window.is_none() {
                continue;
            }

            // Check active hours if configured
            if let Some((start, end)) = active_window {
                if !is_within_active_hours(now, start, end) {
                    continue;
                }

                // Reset run_count when active period rolls over
                if let Some(trigger_count) = pulse.trigger_count {
                    self.maybe_reset_run_count(&pulse.name, now, start, end);

                    // Check if trigger_count exhausted
                    let current_count = self.run_counts.get(&pulse.name).copied().unwrap_or(0);
                    if current_count >= trigger_count {
                        continue;
                    }
                }
            }

            // Compute effective interval: if trigger_count is set, space evenly across active period
            let effective_interval = match pulse.trigger_count {
                Some(tc) if tc > 0 => {
                    let active_duration = active_window.map_or_else(
                        || Duration::hours(24),
                        |(start, end)| active_period_duration(start, end),
                    );
                    let spacing = if let Ok(tc_i32) = i32::try_from(tc) {
                        active_duration / tc_i32
                    } else {
                        tracing::warn!(
                            pulse = %pulse.name,
                            trigger_count = tc,
                            "trigger_count exceeds i32::MAX, spacing collapsed to near-zero; pulse will fire at schedule rate"
                        );
                        active_duration / i32::MAX
                    };
                    let jittered = apply_jitter(spacing, &pulse.name, now);
                    // Effective interval is max(schedule_duration, spacing_with_jitter)
                    if jittered > duration {
                        jittered
                    } else {
                        duration
                    }
                }
                _ => duration,
            };

            // Check if due: fire immediately if never run, otherwise after the effective interval
            let is_due = match self.last_run.get(&pulse.name) {
                None => true,
                Some(last) => (now - *last) >= effective_interval,
            };

            if is_due {
                self.last_run.insert(pulse.name.clone(), now);
                if pulse.trigger_count.is_some() {
                    let count = self.run_counts.entry(pulse.name.clone()).or_insert(0);
                    *count += 1;
                }
                due.push(pulse);
            }
        }

        if !due.is_empty()
            && let Err(e) = self.save_state()
        {
            tracing::warn!(
                pulses = ?due.iter().map(|p| &p.name).collect::<Vec<_>>(),
                error = %e,
                "failed to persist pulse state; these pulses may re-fire on restart"
            );
        }

        due
    }

    /// Reset run count if the current active window start differs from
    /// the window that contained the last run.
    fn maybe_reset_run_count(
        &mut self,
        pulse_name: &str,
        now: NaiveDateTime,
        window_start: NaiveTime,
        window_end: NaiveTime,
    ) {
        let Some(last) = self.last_run.get(pulse_name) else {
            return;
        };

        // If the last run was outside the current active window, or on a different
        // calendar day (for non-overnight windows), reset the count.
        if !is_within_active_hours(*last, window_start, window_end) || now.date() != last.date() {
            self.run_counts.remove(pulse_name);
        }
    }

    /// Persist current state to disk (no-op if no state path is configured).
    fn save_state(&self) -> Result<(), Box<dyn std::error::Error>> {
        let Some(ref path) = self.state_path else {
            return Ok(());
        };

        let state = PulseState {
            last_run: self.last_run.clone(),
            run_counts: self.run_counts.clone(),
        };

        match serde_json::to_string_pretty(&state) {
            Ok(json) => {
                let tmp = path.with_extension("json.tmp");
                if let Err(err) = std::fs::write(&tmp, &json) {
                    tracing::warn!(
                        path = %tmp.display(),
                        error = %err,
                        "failed to write pulse state temp file"
                    );
                    return Err(err.into());
                }
                if let Err(err) = std::fs::rename(&tmp, path) {
                    tracing::warn!(
                        tmp = %tmp.display(),
                        path = %path.display(),
                        error = %err,
                        "failed to rename pulse state file"
                    );
                    return Err(err.into());
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to serialize pulse state");
                return Err(err.into());
            }
        }
        Ok(())
    }
}

/// Load pulse state from disk; returns default on missing or corrupt file.
fn load_state(path: &Path) -> PulseState {
    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<PulseState>(&contents) {
            Ok(state) => state,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "corrupt pulse_state.json, starting with empty state"
                );
                PulseState::default()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => PulseState::default(),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to read pulse_state.json, starting with empty state"
            );
            PulseState::default()
        }
    }
}

/// Compute the duration of an active-hours window.
fn active_period_duration(start: NaiveTime, end: NaiveTime) -> Duration {
    if end > start {
        end - start
    } else {
        // Overnight window (e.g. 22:00-06:00): 24h - (start - end)
        Duration::hours(24) - (start - end)
    }
}

/// Apply ±15% jitter to a duration using a deterministic seed from
/// pulse name and current date (for reproducibility in tests).
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "intentional float arithmetic for jitter; precision loss is acceptable for scheduling"
)]
fn apply_jitter(base: Duration, pulse_name: &str, now: NaiveDateTime) -> Duration {
    let mut hasher = DefaultHasher::new();
    pulse_name.hash(&mut hasher);
    now.date().hash(&mut hasher);
    let hash = hasher.finish();

    // Map hash to [-0.15, +0.15] range
    let fraction = (hash % 3001) as f64 / 10000.0 - 0.15;
    let base_secs = base.num_seconds() as f64;
    let jittered = base_secs * (1.0 + fraction);

    Duration::seconds(jittered as i64)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::shadow_unrelated,
    reason = "test code reuses 'due' across sequential assertions in the same test"
)]
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

    // ── Persistence tests ─────────────────────────────────────────────

    #[test]
    fn persistence_round_trip() {
        let dir = tempdir().unwrap();
        let hb_path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);
        let state_path = dir.path().join("pulse_state.json");

        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        // First scheduler fires and persists
        {
            let mut sched = PulseScheduler::with_state_path(&state_path);
            let due = sched.due_pulses(now, &hb_path);
            assert_eq!(due.len(), 1, "should fire on first run");
        }

        // Second scheduler loads persisted state — should NOT re-fire
        {
            let mut sched = PulseScheduler::with_state_path(&state_path);
            let thirty_min_later = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
                .unwrap()
                .and_hms_opt(12, 30, 0)
                .unwrap();
            let due = sched.due_pulses(thirty_min_later, &hb_path);
            assert!(due.is_empty(), "should not re-fire from persisted state");
        }
    }

    #[test]
    fn persistence_missing_file_starts_empty() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("pulse_state.json");

        // No file exists yet — should start with empty state
        let sched = PulseScheduler::with_state_path(&state_path);
        assert!(sched.last_run.is_empty(), "missing file means empty state");
    }

    #[test]
    fn persistence_corrupt_file_recovers() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("pulse_state.json");
        std::fs::write(&state_path, "not valid json {{{").unwrap();

        // Corrupt file — should recover with empty state
        let sched = PulseScheduler::with_state_path(&state_path);
        assert!(
            sched.last_run.is_empty(),
            "corrupt file should recover to empty state"
        );
    }

    #[test]
    fn state_file_format_matches_spec() {
        let dir = tempdir().unwrap();
        let hb_path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);
        let state_path = dir.path().join("pulse_state.json");

        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 28)
            .unwrap()
            .and_hms_opt(14, 30, 0)
            .unwrap();

        let mut sched = PulseScheduler::with_state_path(&state_path);
        let _due = sched.due_pulses(now, &hb_path);

        let contents = std::fs::read_to_string(&state_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert!(
            parsed.get("last_run").is_some(),
            "state file should have last_run key"
        );
        let last_run = parsed.get("last_run").unwrap().as_object().unwrap();
        assert!(
            last_run.contains_key("test_pulse"),
            "should contain the pulse name"
        );
    }

    // ── Trigger count tests ───────────────────────────────────────────

    #[test]
    fn trigger_count_limits_firings() {
        let yaml = r#"
pulses:
  - name: limited_pulse
    enabled: true
    schedule: "10m"
    active_hours: "09:00-17:00"
    trigger_count: 2
    tasks: []
"#;
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), yaml);
        let mut scheduler = PulseScheduler::new();

        // 8h window, trigger_count=2 → spacing ~4h (with jitter)
        // Fire 1: 09:00
        let t1 = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(t1, &path);
        assert_eq!(due.len(), 1, "first firing should succeed");
        assert_eq!(scheduler.run_counts.get("limited_pulse").copied(), Some(1));

        // Fire 2: well after spacing (~5h later)
        let t2 = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(14, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(t2, &path);
        assert_eq!(due.len(), 1, "second firing should succeed");
        assert_eq!(scheduler.run_counts.get("limited_pulse").copied(), Some(2));

        // Fire 3: should be blocked (count exhausted)
        let t3 = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(16, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(t3, &path);
        assert!(
            due.is_empty(),
            "should not fire after trigger_count exhausted"
        );
    }

    #[test]
    fn trigger_count_spacing_enforced() {
        let yaml = r#"
pulses:
  - name: spaced_pulse
    enabled: true
    schedule: "1m"
    active_hours: "09:00-17:00"
    trigger_count: 2
    tasks: []
"#;
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), yaml);
        let mut scheduler = PulseScheduler::new();

        // 8h window / 2 = 4h spacing (schedule is only 1m, so spacing dominates)
        let t1 = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(t1, &path);
        assert_eq!(due.len(), 1, "first should fire");

        // 30 minutes later — spacing should block even though schedule (1m) has passed
        let t2 = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(9, 30, 0)
            .unwrap();
        let due = scheduler.due_pulses(t2, &path);
        assert!(due.is_empty(), "spacing should prevent early re-fire");
    }

    #[test]
    fn trigger_count_resets_on_period_rollover() {
        let yaml = r#"
pulses:
  - name: daily_pulse
    enabled: true
    schedule: "10m"
    active_hours: "09:00-17:00"
    trigger_count: 1
    tasks: []
"#;
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), yaml);
        let mut scheduler = PulseScheduler::new();

        // Day 1: fire once
        let day1 = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(day1, &path);
        assert_eq!(due.len(), 1, "should fire on day 1");

        // Day 1 later: exhausted
        let day1_later = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(15, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(day1_later, &path);
        assert!(due.is_empty(), "should be exhausted on day 1");

        // Day 2: count should reset
        let day2 = chrono::NaiveDate::from_ymd_opt(2026, 2, 20)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(day2, &path);
        assert_eq!(due.len(), 1, "should fire again after period rollover");
    }

    #[test]
    fn trigger_count_persisted_in_state_file() {
        let yaml = r#"
pulses:
  - name: counted_pulse
    enabled: true
    schedule: "10m"
    active_hours: "09:00-17:00"
    trigger_count: 3
    tasks: []
"#;
        let dir = tempdir().unwrap();
        let hb_path = write_heartbeat(dir.path(), yaml);
        let state_path = dir.path().join("pulse_state.json");

        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();

        let mut sched = PulseScheduler::with_state_path(&state_path);
        let _due = sched.due_pulses(now, &hb_path);

        let contents = std::fs::read_to_string(&state_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert!(
            parsed.get("run_counts").is_some(),
            "state file should have run_counts key"
        );
        let counts = parsed.get("run_counts").unwrap().as_object().unwrap();
        assert_eq!(
            counts
                .get("counted_pulse")
                .and_then(serde_json::Value::as_u64),
            Some(1),
            "run count should be 1 after first fire"
        );
    }

    // ── Helper function tests ─────────────────────────────────────────

    #[test]
    fn active_period_duration_normal_window() {
        let start = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(17, 0, 0).unwrap();
        assert_eq!(
            active_period_duration(start, end),
            Duration::hours(8),
            "09:00-17:00 should be 8h"
        );
    }

    #[test]
    fn active_period_duration_overnight_window() {
        let start = NaiveTime::from_hms_opt(22, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
        assert_eq!(
            active_period_duration(start, end),
            Duration::hours(8),
            "22:00-06:00 should be 8h"
        );
    }

    #[test]
    fn apply_jitter_deterministic() {
        let base = Duration::hours(4);
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let j1 = apply_jitter(base, "test_pulse", now);
        let j2 = apply_jitter(base, "test_pulse", now);
        assert_eq!(j1, j2, "same inputs should produce same jitter");

        // Jitter should be within ±15% of base (4h = 14400s)
        let base_secs = base.num_seconds();
        let min_secs = base_secs * 85 / 100;
        let max_secs = base_secs * 115 / 100;
        assert!(
            j1.num_seconds() >= min_secs && j1.num_seconds() <= max_secs,
            "jittered duration {} should be within ±15% of {} (range {}-{})",
            j1.num_seconds(),
            base_secs,
            min_secs,
            max_secs,
        );
    }
}
