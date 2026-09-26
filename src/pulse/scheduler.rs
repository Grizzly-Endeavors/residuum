use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use super::types::{
    HeartbeatProblem, ProblemKind, PulseDef, heartbeat_problems_notice, is_within_active_hours,
    load_heartbeat, parse_active_hours, parse_schedule_duration, read_and_parse,
};

/// Tracks per-pulse last-run times and determines which pulses are due.
///
/// When constructed with `with_state_path`, timestamps are persisted to
/// `pulse_state.json` and survive restarts. Without a state path, timestamps
/// are in-memory only (backward-compatible).
#[derive(Debug, Serialize, Deserialize)]
pub struct PulseScheduler {
    #[serde(default)]
    last_run: HashMap<String, NaiveDateTime>,
    #[serde(skip)]
    state_path: Option<PathBuf>,
    /// Most recently logged HEARTBEAT.yml parse-error message, so `due_pulses`
    /// doesn't re-warn on an identical error every tick (ticks run every 60s).
    #[serde(skip)]
    last_heartbeat_parse_error: Option<String>,
    /// Every problem found on the most recent tick's HEARTBEAT.yml (sorted by
    /// name then message) — a pulse using a removed option, a duplicate
    /// pulse name, or an unparseable `schedule`/`active_hours` string — so
    /// `due_pulses` only re-logs and re-notifies when this set actually
    /// changes rather than on every tick of an unchanged, still-broken file.
    #[serde(skip)]
    last_heartbeat_problems: Vec<HeartbeatProblem>,
    /// Owner-facing notice queued by `due_pulses` when the problem set
    /// changed on the most recent tick; taken (and cleared) by
    /// `take_problem_notice`.
    #[serde(skip)]
    pending_problem_notice: Option<String>,
    /// The last pulse set that loaded successfully (parsed and passed
    /// per-pulse/dedup validation), so a later whole-document YAML syntax
    /// error can keep these pulses running instead of firing nothing. Not
    /// persisted: in-memory only, since it's rebuilt from HEARTBEAT.yml on
    /// every successful tick and a restart always re-reads the file fresh.
    #[serde(skip)]
    last_good_pulses: Vec<PulseDef>,
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
            state_path: None,
            last_heartbeat_parse_error: None,
            last_heartbeat_problems: Vec::new(),
            pending_problem_notice: None,
            last_good_pulses: Vec::new(),
        }
    }

    /// Create a scheduler that persists state to the given path.
    ///
    /// Loads existing state from disk if the file exists. Missing or corrupt
    /// files are treated as empty state (logged as a warning for corrupt files).
    #[must_use]
    pub fn with_state_path(path: &Path) -> Self {
        let mut scheduler = load_state(path);
        scheduler.state_path = Some(path.to_path_buf());
        scheduler
    }

    /// Find pulses that are due at `now`, hot-reloading HEARTBEAT.yml each call.
    ///
    /// A pulse is due when all of the following hold:
    /// - `enabled == true`
    /// - Its schedule duration can be parsed
    /// - Either it has never run, or `now - last_run >= schedule duration`
    /// - If `active_hours` is set, `now` falls within the window
    ///
    /// Due pulses have their `last_run` updated to `now` and persisted (if a state path is set).
    #[must_use]
    #[tracing::instrument(skip_all, fields(heartbeat_path = %heartbeat_path.display()))]
    pub fn due_pulses(&mut self, now: NaiveDateTime, heartbeat_path: &Path) -> Vec<PulseDef> {
        let mut problems = Vec::new();
        let Some(heartbeat) = load_heartbeat(
            heartbeat_path,
            &mut self.last_heartbeat_parse_error,
            &mut problems,
            &self.last_good_pulses,
        ) else {
            return Vec::new();
        };

        // Remember this tick's validated pulse set (whether freshly parsed,
        // or the previous good set echoed back by a syntax-error fallback —
        // either way it's what should keep running if the file breaks on a
        // later tick) before the loop below consumes `heartbeat.pulses`.
        self.last_good_pulses.clone_from(&heartbeat.pulses);

        let current_pulse_names: HashSet<String> =
            heartbeat.pulses.iter().map(|p| p.name.clone()).collect();
        let pruned = self.prune_removed_pulses(&current_pulse_names);

        let mut due = Vec::new();

        for pulse in heartbeat.pulses {
            if !pulse.enabled {
                continue;
            }

            let duration = match parse_schedule_duration(&pulse.schedule) {
                Ok(d) => d,
                Err(e) => {
                    problems.push(HeartbeatProblem {
                        name: pulse.name.clone(),
                        message: format!(
                            "pulse '{}' has an invalid schedule '{}' ({e}) — skipping until fixed",
                            pulse.name, pulse.schedule
                        ),
                        kind: ProblemKind::Malformed,
                    });
                    continue;
                }
            };

            // Parse active hours window
            let active_window = match pulse.active_hours.as_deref() {
                None => None,
                Some(hours_str) => match parse_active_hours(hours_str) {
                    Ok(window) => Some(window),
                    Err(e) => {
                        problems.push(HeartbeatProblem {
                            name: pulse.name.clone(),
                            message: format!(
                                "pulse '{}' has an invalid active_hours '{hours_str}' ({e}) — \
                                 skipping until fixed",
                                pulse.name
                            ),
                            kind: ProblemKind::Malformed,
                        });
                        None
                    }
                },
            };

            // Skip if active_hours was set but failed to parse
            if pulse.active_hours.is_some() && active_window.is_none() {
                continue;
            }

            // Check active hours if configured
            if let Some((start, end)) = active_window
                && !is_within_active_hours(now, start, end)
            {
                tracing::trace!(pulse = %pulse.name, "skipped: outside active hours");
                continue;
            }

            // Check if due: fire immediately if never run, otherwise after the schedule duration
            let is_due = match self.last_run.get(&pulse.name) {
                None => true,
                Some(last) => (now - *last) >= duration,
            };

            if is_due {
                tracing::debug!(pulse = %pulse.name, "pulse due, queuing execution");
                self.last_run.insert(pulse.name.clone(), now);
                due.push(pulse);
            }
        }

        self.record_heartbeat_problems(problems);

        if (pruned || !due.is_empty())
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

    /// Record every problem found while loading and evaluating the current
    /// tick's HEARTBEAT.yml — a pulse using a removed option, a duplicate
    /// pulse name, or an unparseable `schedule`/`active_hours` string — logging
    /// one line per problem and queuing an owner notice, but only when this
    /// set differs from the last time it was checked. An unchanged, still-
    /// broken HEARTBEAT.yml therefore logs and notifies exactly once, not on
    /// every once-a-minute scheduler tick.
    fn record_heartbeat_problems(&mut self, mut problems: Vec<HeartbeatProblem>) {
        problems.sort_by(|a, b| (&a.name, &a.message).cmp(&(&b.name, &b.message)));
        if problems == self.last_heartbeat_problems {
            return;
        }
        if problems.is_empty() {
            tracing::info!("previously reported HEARTBEAT.yml problems are now resolved");
        } else {
            for problem in &problems {
                match problem.kind {
                    ProblemKind::RemovedOption => {
                        tracing::error!(pulse = %problem.name, "{}", problem.message);
                    }
                    ProblemKind::Malformed => {
                        tracing::warn!(pulse = %problem.name, "{}", problem.message);
                    }
                }
            }
            self.pending_problem_notice = Some(heartbeat_problems_notice(&problems));
        }
        self.last_heartbeat_problems = problems;
    }

    /// Take the owner-facing notice queued by the most recent `due_pulses`
    /// call, if the set of HEARTBEAT.yml problems changed on that tick.
    /// Returns `None` on every tick where nothing new needs telling —
    /// including every tick of an unchanged, still-broken file.
    #[must_use]
    pub fn take_problem_notice(&mut self) -> Option<String> {
        self.pending_problem_notice.take()
    }

    /// Every problem found while loading and evaluating the most recent
    /// tick's `HEARTBEAT.yml`, regardless of whether it's already been
    /// notified (unlike [`Self::take_problem_notice`], which only reports a
    /// change). Used to report a specific tick's full outcome — e.g. to the
    /// agent whose own edit that tick is reloading — where "nothing new
    /// since last time" isn't the question being asked.
    #[must_use]
    pub(crate) fn current_problems(&self) -> &[HeartbeatProblem] {
        &self.last_heartbeat_problems
    }

    /// The most recent whole-document parse failure recorded for
    /// `HEARTBEAT.yml`, if the file is currently unparseable. `None` once a
    /// later tick parses successfully.
    #[must_use]
    pub(crate) fn last_parse_error(&self) -> Option<&str> {
        self.last_heartbeat_parse_error.as_deref()
    }

    /// Remove `last_run` entries for pulses no longer present in
    /// `HEARTBEAT.yml` (deleted or renamed), so `pulse_state.json` doesn't grow
    /// unboundedly with unexplainable stale keys. Returns whether anything was removed.
    fn prune_removed_pulses(&mut self, current_pulse_names: &HashSet<String>) -> bool {
        let last_run_before = self.last_run.len();

        self.last_run
            .retain(|name, _| current_pulse_names.contains(name));

        let pruned = self.last_run.len() != last_run_before;
        if pruned {
            tracing::debug!(
                removed_last_run = last_run_before - self.last_run.len(),
                "pruned pulse state for pulses no longer in HEARTBEAT.yml"
            );
        }
        pruned
    }

    /// Persist current state to disk (no-op if no state path is configured).
    fn save_state(&self) -> Result<(), Box<dyn std::error::Error>> {
        let Some(ref path) = self.state_path else {
            return Ok(());
        };
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, path)?;
        tracing::trace!(path = %path.display(), "pulse state saved");
        Ok(())
    }
}

/// Load pulse state from disk; returns default on missing or corrupt file.
fn load_state(path: &Path) -> PulseScheduler {
    let Some(state) = read_and_parse(path, |s| serde_json::from_str::<PulseScheduler>(s)) else {
        return PulseScheduler::new();
    };
    tracing::debug!(path = %path.display(), pulses = state.last_run.len(), "loaded pulse state");
    state
}

#[cfg(test)]
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
    fn due_pulses_missing_heartbeat_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(now, &path);
        assert!(due.is_empty(), "missing HEARTBEAT.yml should return empty");
    }

    #[test]
    fn due_pulses_skips_invalid_schedule() {
        let yaml = r#"
pulses:
  - name: bad_schedule
    enabled: true
    schedule: "10x"
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
        assert!(
            due.is_empty(),
            "pulse with invalid schedule should be skipped"
        );
    }

    #[test]
    fn due_pulses_skips_invalid_active_hours() {
        let yaml = r#"
pulses:
  - name: bad_hours
    enabled: true
    schedule: "1h"
    active_hours: "not-valid"
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
        assert!(
            due.is_empty(),
            "pulse with invalid active_hours should be skipped"
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
        let hb_path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);

        let mut sched = PulseScheduler::with_state_path(&state_path);
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = sched.due_pulses(now, &hb_path);
        assert_eq!(
            due.len(),
            1,
            "missing state file means empty state, pulse should fire"
        );
    }

    #[test]
    fn persistence_corrupt_file_recovers() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("pulse_state.json");
        std::fs::write(&state_path, "not valid json {{{").unwrap();
        let hb_path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);

        let mut sched = PulseScheduler::with_state_path(&state_path);
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = sched.due_pulses(now, &hb_path);
        assert_eq!(
            due.len(),
            1,
            "corrupt state file should recover to empty state, pulse should fire"
        );
    }

    #[test]
    fn persistence_ignores_unknown_fields() {
        // Unknown top-level keys in pulse_state.json (e.g. from an older
        // scheduler schema) must not break deserialization; `last_run` should
        // still load correctly and the unrecognized key is silently ignored.
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("pulse_state.json");
        std::fs::write(
            &state_path,
            r#"{"last_run":{"test_pulse":"2026-02-19T12:00:00"},"run_counts":{"test_pulse":2}}"#,
        )
        .unwrap();
        let hb_path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);

        let mut sched = PulseScheduler::with_state_path(&state_path);
        let thirty_min_later = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap();
        let due = sched.due_pulses(thirty_min_later, &hb_path);
        assert!(
            due.is_empty(),
            "last_run from the legacy state file should still be honored"
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

    #[test]
    fn due_pulses_prunes_state_for_pulses_removed_from_heartbeat() {
        let dir = tempdir().unwrap();
        let hb_path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT); // only "test_pulse"
        let state_path = dir.path().join("pulse_state.json");

        let earlier = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(11, 0, 0)
            .unwrap();

        // Seed state (via the real persistence path) as if a pulse named
        // "removed_pulse" used to exist in HEARTBEAT.yml and has since been
        // deleted or renamed, plus a still-valid entry for "test_pulse".
        {
            let mut sched = PulseScheduler::new();
            sched.last_run.insert("removed_pulse".to_string(), earlier);
            sched.last_run.insert("test_pulse".to_string(), earlier);
            sched.state_path = Some(state_path.clone());
            sched.save_state().unwrap();
        }

        let mut sched = PulseScheduler::with_state_path(&state_path);
        assert!(
            sched.last_run.contains_key("removed_pulse"),
            "sanity check: stale entry should be loaded from disk"
        );

        let tick_time = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = sched.due_pulses(tick_time, &hb_path);
        assert_eq!(
            due.len(),
            1,
            "test_pulse should still fire (1h since last run)"
        );

        assert!(
            !sched.last_run.contains_key("removed_pulse"),
            "stale last_run entry should be pruned in memory"
        );
        assert!(
            sched.last_run.contains_key("test_pulse"),
            "state for pulses still in HEARTBEAT.yml should be preserved"
        );

        // Reload from disk to confirm the prune was persisted, not just in-memory.
        let contents = std::fs::read_to_string(&state_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let last_run = parsed.get("last_run").unwrap().as_object().unwrap();
        assert!(
            !last_run.contains_key("removed_pulse"),
            "stale entry should not survive save/reload"
        );
        assert!(
            last_run.contains_key("test_pulse"),
            "current pulse entry should survive save/reload"
        );
    }

    // ── HEARTBEAT.yml problem notice tests ───────────────────────────────────

    const AGENT_MAIN_HEARTBEAT: &str = r#"
pulses:
  - name: wake_main
    schedule: "1h"
    agent: main
    tasks: []
"#;

    #[test]
    fn due_pulses_queues_a_notice_the_first_time_a_pulse_is_rejected() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), AGENT_MAIN_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let due = scheduler.due_pulses(now, &path);
        assert!(due.is_empty(), "a rejected pulse should never become due");
        let notice = scheduler.take_problem_notice();
        assert!(
            notice.is_some(),
            "the first tick that sees a rejected pulse should queue a notice"
        );
        assert!(
            notice.unwrap().contains("wake_main"),
            "notice should name the rejected pulse"
        );
    }

    #[test]
    fn due_pulses_does_not_requeue_notice_on_unchanged_ticks() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), AGENT_MAIN_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let first_due = scheduler.due_pulses(now, &path);
        assert!(
            first_due.is_empty(),
            "a rejected pulse should never become due"
        );
        assert!(
            scheduler.take_problem_notice().is_some(),
            "first tick should queue a notice"
        );

        // Several more ticks over an unchanged, still-invalid file: none of
        // them should queue a fresh notice (this is the ~29x-per-run log
        // spam this scheduler is meant to prevent).
        for minute in 1..=5 {
            let later = now + chrono::Duration::minutes(minute);
            let later_due = scheduler.due_pulses(later, &path);
            assert!(
                later_due.is_empty(),
                "a rejected pulse should never become due"
            );
            assert!(
                scheduler.take_problem_notice().is_none(),
                "tick {minute} over an unchanged file should not requeue the notice"
            );
        }
    }

    #[test]
    fn due_pulses_requeues_notice_after_the_file_changes_and_is_still_invalid() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), AGENT_MAIN_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let first_due = scheduler.due_pulses(now, &path);
        assert!(
            first_due.is_empty(),
            "a rejected pulse should never become due"
        );
        assert!(scheduler.take_problem_notice().is_some());

        let later = now + chrono::Duration::minutes(1);
        let second_due = scheduler.due_pulses(later, &path);
        assert!(
            second_due.is_empty(),
            "a rejected pulse should never become due"
        );
        assert!(
            scheduler.take_problem_notice().is_none(),
            "sanity check: unchanged file should not requeue"
        );

        // Edit the file: still invalid overall, but now a second, distinct
        // pulse is rejected too — a genuinely new problem, so it should
        // queue a fresh notice even though `wake_main` was already known.
        let edited = r#"
pulses:
  - name: wake_main
    schedule: "1h"
    agent: main
    tasks: []
  - name: legacy_identity
    schedule: "1h"
    include_identity: true
    tasks: []
"#;
        std::fs::write(&path, edited).unwrap();
        let even_later = now + chrono::Duration::minutes(2);
        let third_due = scheduler.due_pulses(even_later, &path);
        assert!(
            third_due.is_empty(),
            "a rejected pulse should never become due"
        );
        let notice = scheduler.take_problem_notice();
        assert!(
            notice.is_some(),
            "a changed rejection set should queue a new notice"
        );
        let notice = notice.unwrap();
        assert!(notice.contains("wake_main"));
        assert!(notice.contains("legacy_identity"));
    }

    #[test]
    fn due_pulses_queues_no_notice_when_nothing_is_rejected() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let due = scheduler.due_pulses(now, &path);
        assert_eq!(due.len(), 1, "the valid pulse should still fire normally");
        assert!(
            scheduler.take_problem_notice().is_none(),
            "a valid HEARTBEAT.yml should never queue a rejection notice"
        );
    }

    // ── Duplicate pulse names get the same once-per-change treatment ────

    const DUPLICATE_NAME_HEARTBEAT: &str = r#"
pulses:
  - name: dup
    schedule: "1h"
    tasks: []
  - name: dup
    schedule: "2h"
    tasks: []
"#;

    #[test]
    fn due_pulses_queues_a_notice_the_first_time_a_name_is_duplicated() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), DUPLICATE_NAME_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let due = scheduler.due_pulses(now, &path);
        assert_eq!(due.len(), 1, "the surviving 'dup' pulse should still fire");
        let notice = scheduler.take_problem_notice();
        assert!(
            notice.is_some(),
            "the first tick that sees a duplicate name should queue a notice"
        );
        assert!(
            notice.unwrap().contains("dup"),
            "notice should name the duplicated pulse"
        );
    }

    #[test]
    fn due_pulses_does_not_requeue_duplicate_name_notice_on_unchanged_ticks() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), DUPLICATE_NAME_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let first_due = scheduler.due_pulses(now, &path);
        assert_eq!(first_due.len(), 1, "the surviving 'dup' pulse should fire");
        assert!(
            scheduler.take_problem_notice().is_some(),
            "first tick should queue a notice"
        );

        // The surviving 'dup' pulse has a 1h schedule, so it won't be due
        // again on these later ticks — only the duplicate-name problem is
        // under test here, and it should stay silent while nothing changes.
        for minute in 1..=5 {
            let later = now + chrono::Duration::minutes(minute);
            let later_due = scheduler.due_pulses(later, &path);
            assert!(later_due.is_empty(), "not due again within the 1h schedule");
            assert!(
                scheduler.take_problem_notice().is_none(),
                "tick {minute} over an unchanged duplicate should not requeue the notice"
            );
        }
    }

    #[test]
    fn due_pulses_requeues_duplicate_name_notice_after_a_new_duplicate_appears() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), DUPLICATE_NAME_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let first_due = scheduler.due_pulses(now, &path);
        assert_eq!(first_due.len(), 1, "the surviving 'dup' pulse should fire");
        assert!(scheduler.take_problem_notice().is_some());

        let later = now + chrono::Duration::minutes(1);
        let second_due = scheduler.due_pulses(later, &path);
        assert!(
            second_due.is_empty(),
            "not due again within the 1h schedule"
        );
        assert!(
            scheduler.take_problem_notice().is_none(),
            "sanity check: unchanged file should not requeue"
        );

        // A second, distinct duplicate name appears — a genuinely new
        // problem, so it should queue a fresh notice.
        let edited = r#"
pulses:
  - name: dup
    schedule: "1h"
    tasks: []
  - name: dup
    schedule: "2h"
    tasks: []
  - name: also-dup
    schedule: "1h"
    tasks: []
  - name: also-dup
    schedule: "2h"
    tasks: []
"#;
        std::fs::write(&path, edited).unwrap();
        let even_later = now + chrono::Duration::minutes(2);
        let third_due = scheduler.due_pulses(even_later, &path);
        assert_eq!(
            third_due.len(),
            1,
            "the new 'also-dup' survivor should fire for the first time"
        );
        let notice = scheduler.take_problem_notice();
        assert!(
            notice.is_some(),
            "a changed duplicate set should queue a new notice"
        );
        let notice = notice.unwrap();
        assert!(notice.contains("dup"));
        assert!(notice.contains("also-dup"));
    }

    // ── Invalid schedule / active_hours get the same treatment ──────────

    const INVALID_SCHEDULE_HEARTBEAT: &str = r#"
pulses:
  - name: bad_schedule
    enabled: true
    schedule: "not-a-duration"
    tasks: []
"#;

    #[test]
    fn due_pulses_dedupes_invalid_schedule_notice_across_ticks() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), INVALID_SCHEDULE_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let due = scheduler.due_pulses(now, &path);
        assert!(due.is_empty(), "a pulse with a bad schedule never fires");
        let notice = scheduler.take_problem_notice();
        assert!(notice.is_some(), "first tick should queue a notice");
        assert!(notice.unwrap().contains("bad_schedule"));

        for minute in 1..=5 {
            let later = now + chrono::Duration::minutes(minute);
            let later_due = scheduler.due_pulses(later, &path);
            assert!(later_due.is_empty(), "a bad schedule never becomes due");
            assert!(
                scheduler.take_problem_notice().is_none(),
                "tick {minute} over an unchanged bad schedule should not requeue"
            );
        }
    }

    const INVALID_ACTIVE_HOURS_HEARTBEAT: &str = r#"
pulses:
  - name: bad_hours
    enabled: true
    schedule: "1h"
    active_hours: "not-valid"
    tasks: []
"#;

    #[test]
    fn due_pulses_dedupes_invalid_active_hours_notice_across_ticks() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), INVALID_ACTIVE_HOURS_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let due = scheduler.due_pulses(now, &path);
        assert!(due.is_empty(), "a pulse with bad active_hours never fires");
        let notice = scheduler.take_problem_notice();
        assert!(notice.is_some(), "first tick should queue a notice");
        assert!(notice.unwrap().contains("bad_hours"));

        for minute in 1..=5 {
            let later = now + chrono::Duration::minutes(minute);
            let later_due = scheduler.due_pulses(later, &path);
            assert!(later_due.is_empty(), "bad active_hours never becomes due");
            assert!(
                scheduler.take_problem_notice().is_none(),
                "tick {minute} over unchanged bad active_hours should not requeue"
            );
        }
    }

    // ── Whole-document syntax error: last good pulses keep running ─────

    #[test]
    fn syntax_error_keeps_the_last_good_pulses_running_and_notices_once() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat(dir.path(), SIMPLE_HEARTBEAT);
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        // First tick loads the valid file and fires the pulse once.
        let first_due = scheduler.due_pulses(now, &path);
        assert_eq!(first_due.len(), 1, "test_pulse should fire on first run");
        assert!(
            scheduler.take_problem_notice().is_none(),
            "a valid file should queue no notice"
        );

        // The file goes bad (a syntax error, not just one bad pulse) —
        // due_pulses must not stop scheduling test_pulse because of it.
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();
        let later = now + chrono::Duration::hours(2);
        let due_while_broken = scheduler.due_pulses(later, &path);
        assert_eq!(
            due_while_broken.len(),
            1,
            "test_pulse (the last known-good set) should still fire on schedule even while \
             HEARTBEAT.yml is syntactically broken"
        );
        let notice = scheduler.take_problem_notice();
        assert!(
            notice.is_some(),
            "the first tick that sees a new syntax error should queue a notice"
        );
        assert!(notice.unwrap().contains("syntax error"));

        // Further ticks over the same unchanged syntax error must not
        // requeue the notice.
        for minute in 1..=5 {
            let even_later = later + chrono::Duration::minutes(minute);
            let repeat_due = scheduler.due_pulses(even_later, &path);
            assert!(
                repeat_due.is_empty(),
                "not due again within the 1h schedule from its last (fallback) run"
            );
            assert!(
                scheduler.take_problem_notice().is_none(),
                "tick {minute} over an unchanged syntax error should not requeue the notice"
            );
        }
    }

    #[test]
    fn syntax_error_with_no_prior_good_pulses_fires_nothing_but_still_notices() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();
        let mut scheduler = PulseScheduler::new();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 19)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();

        let due = scheduler.due_pulses(now, &path);
        assert!(
            due.is_empty(),
            "with nothing known-good yet, a syntax error fires nothing"
        );
        assert!(scheduler.take_problem_notice().is_some());
    }
}
