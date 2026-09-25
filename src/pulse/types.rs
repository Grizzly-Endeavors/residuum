use std::collections::HashSet;
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
    /// Optional agent routing: a skill name to run a session with that skill
    /// activated. `"main"` is rejected at load — see [`validate_pulse`].
    #[serde(default)]
    pub agent: Option<String>,
    /// Model tier for the session. Defaults to `small`.
    #[serde(default)]
    pub model_tier: Option<String>,
    /// Removed field, kept here only so its presence in HEARTBEAT.yml can be
    /// detected and rejected at load rather than silently ignored. Forks
    /// always carry the full agent identity now, so this option no longer
    /// does anything.
    #[serde(default)]
    pub include_identity: Option<bool>,
    #[serde(default)]
    pub tasks: Vec<PulseTask>,
}

/// Validate a pulse definition against removed options.
///
/// # Errors
/// Returns an error naming the pulse and the field to remove if it uses
/// `agent: "main"` or sets `include_identity` at all (forks always carry the
/// main agent's identity now, so the field is never silently reinterpreted).
pub fn validate_pulse(pulse: &PulseDef) -> Result<(), String> {
    if pulse.agent.as_deref() == Some("main") {
        return Err(format!(
            "pulse '{}' uses agent: \"main\", which is no longer supported — every session fork \
             already carries the main agent's identity and memory snapshot, so remove the \
             `agent: main` line (or set it to a skill name to give the session a role)",
            pulse.name
        ));
    }
    if pulse.include_identity.is_some() {
        return Err(format!(
            "pulse '{}' sets include_identity, which has been removed — every session fork \
             already carries SOUL.md and AGENTS.md, so remove the `include_identity` line",
            pulse.name
        ));
    }
    Ok(())
}

fn default_enabled() -> bool {
    true
}

/// What kind of problem a `HeartbeatProblem` represents, which decides both
/// the log level and which doc a notice about it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProblemKind {
    /// A pulse used an option removed by the agent sessions overhaul
    /// (`agent: "main"` or `include_identity`). Logged at `error!`; the
    /// owner notice links the migration guide.
    RemovedOption,
    /// Anything else wrong with the file that isn't about a removed option
    /// (a duplicate pulse name, an unparseable `schedule` or
    /// `active_hours`). Logged at `warn!`; the owner notice links the
    /// heartbeats reference doc instead.
    Malformed,
}

/// A single problem found while loading or evaluating a HEARTBEAT.yml pulse
/// — a pulse using a removed option, a duplicate pulse name, or an
/// unparseable `schedule`/`active_hours` string.
///
/// Carries the already-formatted message so the log line and the owner
/// notice built from it stay in sync with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeartbeatProblem {
    pub name: String,
    pub message: String,
    pub kind: ProblemKind,
}

/// Build an owner-facing notice naming every current HEARTBEAT.yml problem.
///
/// Links the migration guide if any problem is a removed option, or the
/// heartbeats reference doc otherwise (a duplicate name or bad
/// `schedule`/`active_hours` string has nothing to do with the migration).
///
/// Callers are responsible for only calling this when the problem set has
/// actually changed since the last check — see `PulseScheduler` — so the
/// notice fires once per distinct problem rather than on every reload of an
/// unchanged file.
#[must_use]
pub(crate) fn heartbeat_problems_notice(problems: &[HeartbeatProblem]) -> String {
    let details = problems
        .iter()
        .map(|p| format!("- {}", p.message))
        .collect::<Vec<_>>()
        .join("\n");
    let guide = if problems
        .iter()
        .any(|p| p.kind == ProblemKind::RemovedOption)
    {
        format!(
            "See {} for how to update them.",
            crate::util::MIGRATION_GUIDE_URL
        )
    } else {
        format!(
            "See {} for the HEARTBEAT.yml format.",
            crate::util::HEARTBEATS_REFERENCE_URL
        )
    };
    format!(
        "HEARTBEAT.yml: {count} problem{plural} found and will not run until fixed:\n{details}\n{guide}",
        count = problems.len(),
        plural = if problems.len() == 1 { "" } else { "s" },
    )
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
    let Some(last_byte_idx) = s.char_indices().next_back().map(|(i, _)| i) else {
        bail!("schedule duration cannot be empty");
    };
    let (num_part, unit) = s.split_at(last_byte_idx);
    let value: i64 = num_part.parse().map_err(|_parse_err| {
        anyhow::anyhow!("invalid schedule duration '{s}': expected number followed by s/m/h/d")
    })?;
    if value <= 0 {
        bail!("schedule duration must be positive, got '{s}'");
    }
    match unit {
        "s" => Ok(Duration::seconds(value)),
        "m" => Ok(Duration::minutes(value)),
        "h" => Ok(Duration::hours(value)),
        "d" => Ok(Duration::days(value)),
        other => bail!("unknown duration unit '{other}' in '{s}': expected s, m, h, or d"),
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
        .ok_or_else(|| anyhow::anyhow!("invalid active_hours '{s}': expected 'HH:MM-HH:MM'"))?;
    let start = parse_naive_time(start_str, s)?;
    let end = parse_naive_time(end_str, s)?;
    Ok((start, end))
}

fn parse_naive_time(t: &str, context: &str) -> anyhow::Result<NaiveTime> {
    let (hour_str, min_str) = t.split_once(':').ok_or_else(|| {
        anyhow::anyhow!("invalid time '{t}' in active_hours '{context}': expected HH:MM")
    })?;
    let hour: u32 = hour_str.parse().map_err(|_parse_err| {
        anyhow::anyhow!("invalid hour '{hour_str}' in active_hours '{context}'")
    })?;
    let min: u32 = min_str.parse().map_err(|_parse_err| {
        anyhow::anyhow!("invalid minute '{min_str}' in active_hours '{context}'")
    })?;
    NaiveTime::from_hms_opt(hour, min, 0)
        .ok_or_else(|| anyhow::anyhow!("out-of-range time '{t}' in active_hours '{context}'"))
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

/// Deserialize one entry of HEARTBEAT.yml's `pulses` list from its raw YAML
/// value, given its index in the list.
///
/// Exposed as its own function — not inlined into [`load_heartbeat`] — so
/// other tooling (e.g. a HEARTBEAT.yml editor-side linter) can reuse the same
/// per-pulse diagnostics without re-parsing or re-validating the whole
/// document just to check one entry.
///
/// On failure, the returned [`HeartbeatProblem`] names the pulse by its
/// `name` field when the entry has a readable one, otherwise by its position
/// (`entry #N`), and includes a source line/column when `serde_yaml_ng`
/// reports one.
///
/// Deserializes by re-serializing this one entry back to a YAML string and
/// parsing that, rather than deserializing the already-parsed `Value`
/// directly: a `Value` carries no span info once parsed, so a `from_value`
/// error never has a location, while re-parsed text does (relative to this
/// entry's own re-serialized snippet, not the original file's line numbers,
/// but still enough to point at the right field within the pulse). Falls
/// back to deserializing the `Value` directly if re-serializing it fails,
/// which loses the location but still catches the error.
pub(crate) fn deserialize_pulse_entry(
    index: usize,
    value: &serde_yaml_ng::Value,
) -> Result<PulseDef, HeartbeatProblem> {
    let result = match serde_yaml_ng::to_string(value) {
        Ok(entry_yaml) => serde_yaml_ng::from_str::<PulseDef>(&entry_yaml),
        Err(_) => serde_yaml_ng::from_value::<PulseDef>(value.clone()),
    };
    result.map_err(|e| {
        let name = value
            .as_mapping()
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let label = name
            .clone()
            .map_or_else(|| format!("entry #{}", index + 1), |n| format!("'{n}'"));
        let location = e
            .location()
            .map(|l| format!(" (line {}, column {})", l.line(), l.column()))
            .unwrap_or_default();
        HeartbeatProblem {
            name: name.unwrap_or_else(|| format!("entry #{}", index + 1)),
            message: format!(
                "pulse {label} in HEARTBEAT.yml failed to load{location}: {e} — skipping until \
                 fixed"
            ),
            kind: ProblemKind::Malformed,
        }
    })
}

/// Describe a YAML value's kind in plain words, for a problem message naming
/// what was found where a list was expected.
fn yaml_kind(value: &serde_yaml_ng::Value) -> &'static str {
    match value {
        serde_yaml_ng::Value::Null => "nothing",
        serde_yaml_ng::Value::Bool(_) => "a true/false value",
        serde_yaml_ng::Value::Number(_) => "a number",
        serde_yaml_ng::Value::String(_) => "text",
        serde_yaml_ng::Value::Sequence(_) => "a list",
        serde_yaml_ng::Value::Mapping(_) => "a mapping",
        serde_yaml_ng::Value::Tagged(_) => "a tagged value",
    }
}

/// Load HEARTBEAT.yml from the given path.
///
/// Returns `None` only when the file does not exist. Every other problem
/// degrades instead of stopping every pulse:
///
/// - A single pulse entry that fails to deserialize (see
///   [`deserialize_pulse_entry`]) is dropped; every other pulse in the file
///   still loads.
/// - A whole-document YAML syntax error, or a top-level `pulses` key that
///   isn't a list, keeps `last_good_pulses` — the last set that loaded
///   successfully — running rather than firing nothing until the file is
///   fixed.
///
/// `last_parse_error` carries the most recently logged syntax-error message
/// across calls (this is hot-reloaded on every scheduler tick). A syntax
/// error is logged at `warn` only the first time it's seen or when the error
/// text changes; an identical, still-broken file logs at `debug` instead so a
/// typo in HEARTBEAT.yml doesn't repeat the same warning forever.
///
/// Duplicate pulse names are dropped, keeping the first occurrence: `PulseScheduler`
/// keys its per-pulse state by name, so two pulses sharing a name would otherwise
/// silently collapse into one scheduler-state entry.
///
/// `problems` is cleared and refilled with every pulse dropped this call — for
/// failing to deserialize, using a removed option (see `validate_pulse`), or
/// duplicating an earlier pulse's name. This function itself doesn't log or
/// notify about them — every call re-validates the whole file, so a caller
/// hot-reloading on a timer (`PulseScheduler`) is responsible for comparing
/// against the previous call's result and only logging/notifying when it
/// actually changed.
#[must_use]
pub(crate) fn load_heartbeat(
    path: &Path,
    last_parse_error: &mut Option<String>,
    problems: &mut Vec<HeartbeatProblem>,
    last_good_pulses: &[PulseDef],
) -> Option<HeartbeatConfig> {
    problems.clear();
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            *last_parse_error = None;
            return None;
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read file");
            return None;
        }
    };

    let root: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            let message = e.to_string();
            if last_parse_error.as_deref() == Some(message.as_str()) {
                tracing::debug!(path = %path.display(), error = %message, "HEARTBEAT.yml still fails to parse");
            } else {
                tracing::warn!(path = %path.display(), error = %message, "HEARTBEAT.yml has a syntax error; keeping the last working set of pulses running until this is fixed");
                *last_parse_error = Some(message);
            }
            // Whole document is unparseable — keep the last known-good,
            // already-validated pulse set running rather than stopping every
            // pulse until the syntax error is fixed.
            return Some(HeartbeatConfig {
                pulses: last_good_pulses.to_vec(),
            });
        }
    };

    if last_parse_error.take().is_some() {
        tracing::info!(path = %path.display(), "HEARTBEAT.yml parses again after previous errors");
    }

    let pulses_value = root
        .as_mapping()
        .and_then(|m| m.get("pulses"))
        .cloned()
        .unwrap_or(serde_yaml_ng::Value::Sequence(Vec::new()));

    let entries = match pulses_value {
        serde_yaml_ng::Value::Sequence(seq) => seq,
        serde_yaml_ng::Value::Null => Vec::new(),
        other @ (serde_yaml_ng::Value::Bool(_)
        | serde_yaml_ng::Value::Number(_)
        | serde_yaml_ng::Value::String(_)
        | serde_yaml_ng::Value::Mapping(_)
        | serde_yaml_ng::Value::Tagged(_)) => {
            problems.push(HeartbeatProblem {
                name: "HEARTBEAT.yml".to_string(),
                message: format!(
                    "HEARTBEAT.yml's top-level 'pulses' key must be a list, found {} instead; \
                     keeping the last working set of pulses running until this is fixed",
                    yaml_kind(&other)
                ),
                kind: ProblemKind::Malformed,
            });
            return Some(HeartbeatConfig {
                pulses: last_good_pulses.to_vec(),
            });
        }
    };

    let mut pulses = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        match deserialize_pulse_entry(index, entry) {
            Ok(pulse) => pulses.push(pulse),
            Err(problem) => problems.push(problem),
        }
    }

    problems.extend(dedupe_pulse_names(&mut pulses));
    problems.extend(reject_invalid_pulses(&mut pulses));

    tracing::trace!(path = %path.display(), pulses = pulses.len(), "loaded HEARTBEAT.yml");
    Some(HeartbeatConfig { pulses })
}

/// Drop pulses that use a removed option (`agent: "main"` or
/// `include_identity`), returning what was dropped and why.
///
/// Never silently reinterpreted: the pulse is skipped entirely, not routed
/// as if the option were absent. Logging and owner notification are the
/// caller's responsibility (see `load_heartbeat`'s doc comment).
fn reject_invalid_pulses(pulses: &mut Vec<PulseDef>) -> Vec<HeartbeatProblem> {
    let mut problems = Vec::new();
    pulses.retain(|pulse| match validate_pulse(pulse) {
        Ok(()) => true,
        Err(message) => {
            problems.push(HeartbeatProblem {
                name: pulse.name.clone(),
                message,
                kind: ProblemKind::RemovedOption,
            });
            false
        }
    });
    problems
}

/// Drop pulses whose name duplicates an earlier one in the list, keeping the
/// first, and return one problem per dropped duplicate.
///
/// A HEARTBEAT.yml typo (copy-pasted pulse block with an unchanged `name`)
/// should be visible rather than silently overwriting scheduler state for
/// the earlier pulse of the same name. Logging and owner notification are
/// the caller's responsibility (see `load_heartbeat`'s doc comment).
fn dedupe_pulse_names(pulses: &mut Vec<PulseDef>) -> Vec<HeartbeatProblem> {
    let mut seen = HashSet::new();
    let mut problems = Vec::new();
    pulses.retain(|pulse| {
        if seen.insert(pulse.name.clone()) {
            true
        } else {
            problems.push(HeartbeatProblem {
                name: pulse.name.clone(),
                message: format!(
                    "pulse '{}' is defined more than once in HEARTBEAT.yml; only the first \
                     definition is used, the later one is ignored",
                    pulse.name
                ),
                kind: ProblemKind::Malformed,
            });
            false
        }
    });
    problems
}

#[cfg(test)]
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
    fn pulse_def_agent_field_skill_name() {
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
        let mut last_error = None;
        let mut problems = Vec::new();
        assert!(
            load_heartbeat(&path, &mut last_error, &mut problems, &[]).is_none(),
            "missing file should return None"
        );
    }

    #[test]
    fn bundled_heartbeat_asset_parses_with_builtin_pulses() {
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(include_str!(
            "../../assets/workspace-bootstrap/HEARTBEAT.yml"
        ))
        .unwrap();
        let names: Vec<&str> = cfg.pulses.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["reflection", "memory_tending", "wiki_lint"]);
        let agents: Vec<Option<&str>> = cfg.pulses.iter().map(|p| p.agent.as_deref()).collect();
        assert_eq!(
            agents,
            [Some("introspection"), Some("wiki"), Some("wiki")],
            "each built-in pulse runs as a bundled role skill"
        );
        for pulse in &cfg.pulses {
            assert!(pulse.enabled, "built-in pulses ship enabled");
            validate_pulse(pulse).unwrap_or_else(|e| {
                panic!(
                    "bundled pulse '{}' must not use a removed option: {e}",
                    pulse.name
                )
            });
            parse_schedule_duration(&pulse.schedule).unwrap();
            if let Some(hours) = &pulse.active_hours {
                parse_active_hours(hours).unwrap();
            }
        }
    }

    #[test]
    fn load_heartbeat_invalid_yaml_with_no_prior_good_config_loads_no_pulses() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &[])
            .expect("a syntax error is never None — it falls back to last_good_pulses");
        assert!(
            cfg.pulses.is_empty(),
            "with nothing known-good yet, the fallback set is empty"
        );
        assert!(last_error.is_some(), "the syntax error should be recorded");
    }

    #[test]
    fn load_heartbeat_invalid_yaml_keeps_the_last_good_pulses_running() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();

        let last_good = vec![PulseDef {
            name: "still_running".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            model_tier: None,
            include_identity: None,
            tasks: vec![],
        }];

        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &last_good)
            .expect("a syntax error keeps the last good pulse set, never None");
        let names: Vec<&str> = cfg.pulses.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["still_running"],
            "a whole-document syntax error must keep the last known-good pulses running \
             instead of stopping every pulse"
        );
    }

    #[test]
    fn load_heartbeat_repeated_identical_error_not_rewarned() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();

        assert!(load_heartbeat(&path, &mut last_error, &mut problems, &[]).is_some());
        assert!(
            last_error.is_some(),
            "first parse failure should record the error text"
        );
        let recorded = last_error.clone();

        // Same broken file parsed again on a later tick: the recorded error is
        // unchanged, so the caller can tell this isn't a new failure worth a
        // fresh warning (see load_heartbeat's dedup behavior).
        assert!(load_heartbeat(&path, &mut last_error, &mut problems, &[]).is_some());
        assert_eq!(
            last_error, recorded,
            "identical repeated parse error should not change the recorded message"
        );
    }

    #[test]
    fn load_heartbeat_recovering_clears_last_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        assert!(load_heartbeat(&path, &mut last_error, &mut problems, &[]).is_some());
        assert!(last_error.is_some(), "broken file should record an error");

        std::fs::write(&path, SIMPLE_VALID_HEARTBEAT).unwrap();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &[]);
        assert!(cfg.is_some(), "fixed file should parse successfully");
        assert!(
            last_error.is_none(),
            "successful parse should clear the recorded error"
        );
    }

    #[test]
    fn load_heartbeat_one_invalid_pulse_entry_is_dropped_but_others_still_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        // `schedule` must be a string; giving it a mapping makes this one
        // pulse entry fail to deserialize, without touching the document's
        // own YAML syntax (which is fine) or the other, valid pulse.
        let yaml = r#"
pulses:
  - name: bad_pulse
    schedule: {not: a-string}
    tasks: []
  - name: good_pulse
    schedule: "1h"
    tasks: []
"#;
        std::fs::write(&path, yaml).unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &[]).unwrap();
        let names: Vec<&str> = cfg.pulses.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["good_pulse"],
            "the pulse that fails to deserialize should be dropped, keeping the rest"
        );
        assert_eq!(
            problems.len(),
            1,
            "the dropped pulse should be reported as one problem"
        );
        assert_eq!(problems.first().unwrap().name, "bad_pulse");
        assert_eq!(problems.first().unwrap().kind, ProblemKind::Malformed);
        assert!(last_error.is_none(), "the document itself parses fine");
    }

    #[test]
    fn load_heartbeat_unnamed_invalid_pulse_entry_is_labeled_by_position() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let yaml = "
pulses:
  - schedule: {not: a-string}
    tasks: []
";
        std::fs::write(&path, yaml).unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &[]).unwrap();
        assert!(cfg.pulses.is_empty());
        assert_eq!(problems.len(), 1);
        assert_eq!(
            problems.first().unwrap().name,
            "entry #1",
            "a pulse entry with no readable name should be labeled by position"
        );
    }

    #[test]
    fn load_heartbeat_unknown_top_level_keys_do_not_take_down_the_pulses_list() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let yaml = r#"
some_future_field: true
pulses:
  - name: test_pulse
    schedule: "1h"
    tasks: []
"#;
        std::fs::write(&path, yaml).unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &[]).unwrap();
        assert_eq!(
            cfg.pulses.len(),
            1,
            "unknown top-level keys should be ignored"
        );
        assert!(problems.is_empty());
    }

    #[test]
    fn load_heartbeat_non_list_pulses_key_falls_back_to_last_good() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "pulses: \"oops, not a list\"").unwrap();
        let last_good = vec![PulseDef {
            name: "still_running".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            model_tier: None,
            include_identity: None,
            tasks: vec![],
        }];
        let mut last_error = None;
        let mut problems = Vec::new();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &last_good).unwrap();
        let names: Vec<&str> = cfg.pulses.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["still_running"]);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems.first().unwrap().kind, ProblemKind::Malformed);
    }

    #[test]
    fn deserialize_pulse_entry_reports_yaml_location_when_available() {
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "
name: bad_pulse
schedule: {not: a-string}
tasks: []
",
        )
        .unwrap();
        let problem = deserialize_pulse_entry(0, &value).unwrap_err();
        assert_eq!(problem.name, "bad_pulse");
        assert!(
            problem.message.contains("line"),
            "problem message should include a source location when serde_yaml_ng reports one: \
             {}",
            problem.message
        );
    }

    const SIMPLE_VALID_HEARTBEAT: &str = r#"
pulses:
  - name: test_pulse
    schedule: "1h"
    tasks: []
"#;

    #[test]
    fn load_heartbeat_dedupes_duplicate_pulse_names() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let yaml = r#"
pulses:
  - name: dup
    schedule: "1h"
    tasks:
      - name: first
        prompt: "first"
  - name: unique
    schedule: "2h"
    tasks: []
  - name: dup
    schedule: "3h"
    tasks:
      - name: second
        prompt: "second"
"#;
        std::fs::write(&path, yaml).unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &[]).unwrap();
        let names: Vec<&str> = cfg.pulses.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["dup", "unique"],
            "second pulse named 'dup' should be dropped, keeping the first occurrence"
        );
        assert_eq!(
            cfg.pulses.first().unwrap().schedule,
            "1h",
            "the surviving 'dup' pulse should be the first one in the file"
        );
    }

    #[test]
    fn load_heartbeat_drops_pulse_with_agent_main_but_keeps_the_rest() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let yaml = r#"
pulses:
  - name: wake_main
    schedule: "1h"
    agent: main
    tasks: []
  - name: keep_me
    schedule: "2h"
    tasks: []
"#;
        std::fs::write(&path, yaml).unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &[]).unwrap();
        let names: Vec<&str> = cfg.pulses.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["keep_me"],
            "the agent: main pulse should be dropped, never silently reinterpreted, \
             while the rest of the file still loads"
        );
        assert_eq!(
            problems.len(),
            1,
            "load_heartbeat should report exactly the one rejected pulse"
        );
        assert_eq!(problems.first().unwrap().name, "wake_main");
        assert_eq!(problems.first().unwrap().kind, ProblemKind::RemovedOption);
        assert!(
            problems.first().unwrap().message.contains("agent: main"),
            "rejected pulse message should name the offending option"
        );
    }

    #[test]
    fn load_heartbeat_drops_pulse_with_include_identity() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let yaml = r#"
pulses:
  - name: legacy_identity
    schedule: "1h"
    include_identity: true
    tasks: []
"#;
        std::fs::write(&path, yaml).unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        let cfg = load_heartbeat(&path, &mut last_error, &mut problems, &[]).unwrap();
        assert!(
            cfg.pulses.is_empty(),
            "a pulse setting include_identity (removed) must not load"
        );
        assert_eq!(problems.len(), 1, "the rejection should be reported");
        assert_eq!(problems.first().unwrap().name, "legacy_identity");
        assert_eq!(problems.first().unwrap().kind, ProblemKind::RemovedOption);
        assert!(
            problems
                .first()
                .unwrap()
                .message
                .contains("include_identity"),
            "rejected pulse message should name the offending option"
        );
    }

    #[test]
    fn load_heartbeat_valid_pulse_reports_no_rejections() {
        let dir = tempdir().unwrap();
        let path = write_heartbeat_for_test(dir.path(), SIMPLE_VALID_HEARTBEAT);
        let mut last_error = None;
        let mut problems = Vec::new();
        load_heartbeat(&path, &mut last_error, &mut problems, &[]).unwrap();
        assert!(
            problems.is_empty(),
            "a valid HEARTBEAT.yml should report no problems"
        );
    }

    #[test]
    fn load_heartbeat_reports_duplicate_pulse_name_as_malformed_problem() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let yaml = r#"
pulses:
  - name: dup
    schedule: "1h"
    tasks: []
  - name: dup
    schedule: "2h"
    tasks: []
"#;
        std::fs::write(&path, yaml).unwrap();
        let mut last_error = None;
        let mut problems = Vec::new();
        load_heartbeat(&path, &mut last_error, &mut problems, &[]).unwrap();
        assert_eq!(problems.len(), 1, "the duplicate should be reported");
        assert_eq!(problems.first().unwrap().name, "dup");
        assert_eq!(problems.first().unwrap().kind, ProblemKind::Malformed);
    }

    #[test]
    fn heartbeat_problems_notice_names_each_pulse_and_links_the_migration_guide() {
        let problems = vec![
            HeartbeatProblem {
                name: "wake_main".to_string(),
                message: "pulse 'wake_main' uses agent: \"main\", which is no longer supported"
                    .to_string(),
                kind: ProblemKind::RemovedOption,
            },
            HeartbeatProblem {
                name: "legacy_identity".to_string(),
                message: "pulse 'legacy_identity' sets include_identity, which has been removed"
                    .to_string(),
                kind: ProblemKind::RemovedOption,
            },
        ];
        let notice = heartbeat_problems_notice(&problems);
        assert!(
            notice.contains("wake_main"),
            "notice should name the first rejected pulse"
        );
        assert!(
            notice.contains("legacy_identity"),
            "notice should name the second rejected pulse"
        );
        assert!(
            notice.contains("migrating-to-agent-sessions.md"),
            "notice should point at the migration guide when a removed option is involved"
        );
        assert!(
            notice.contains('2'),
            "notice should mention how many problems were found"
        );
    }

    #[test]
    fn heartbeat_problems_notice_links_the_heartbeats_doc_when_nothing_is_a_removed_option() {
        let problems = vec![HeartbeatProblem {
            name: "dup".to_string(),
            message: "pulse 'dup' is defined more than once in HEARTBEAT.yml".to_string(),
            kind: ProblemKind::Malformed,
        }];
        let notice = heartbeat_problems_notice(&problems);
        assert!(notice.contains("dup"));
        assert!(
            !notice.contains("migrating-to-agent-sessions.md"),
            "a malformed-only notice shouldn't point at the unrelated migration guide"
        );
        assert!(
            notice.contains("heartbeats.md"),
            "notice should point at the heartbeats reference doc instead"
        );
    }

    #[test]
    fn heartbeat_problems_notice_links_the_migration_guide_when_problems_are_mixed() {
        let problems = vec![
            HeartbeatProblem {
                name: "dup".to_string(),
                message: "pulse 'dup' is defined more than once in HEARTBEAT.yml".to_string(),
                kind: ProblemKind::Malformed,
            },
            HeartbeatProblem {
                name: "wake_main".to_string(),
                message: "pulse 'wake_main' uses agent: \"main\", which is no longer supported"
                    .to_string(),
                kind: ProblemKind::RemovedOption,
            },
        ];
        let notice = heartbeat_problems_notice(&problems);
        assert!(notice.contains("dup"));
        assert!(notice.contains("wake_main"));
        assert!(
            notice.contains("migrating-to-agent-sessions.md"),
            "one removed-option problem should be enough to link the migration guide"
        );
    }

    fn write_heartbeat_for_test(dir: &Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("HEARTBEAT.yml");
        std::fs::write(&path, content).unwrap();
        path
    }
}
