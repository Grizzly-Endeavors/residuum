use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Days, DurationRound as _, NaiveDateTime, NaiveTime, TimeDelta, Utc};

use super::cadence::Cadence;
use super::types::{PulseDef, is_within_active_hours};
use residuum_bus::PulseFireOutcome;
use residuum_core::time::local_to_instant;

#[must_use]
pub fn next_run_at(
    pulse: &PulseDef,
    anchor: Option<NaiveDateTime>,
    now: DateTime<Utc>,
    tz: chrono_tz::Tz,
) -> Option<DateTime<Utc>> {
    if !pulse.enabled {
        return None;
    }
    let cadence = Cadence::of(pulse).ok()?;
    let window = cadence.window();

    let this_minute = now.duration_trunc(TimeDelta::minutes(1)).ok()?;
    let now_local = this_minute.with_timezone(&tz).naive_local();
    let anchor = match (anchor, window) {
        (Some(anchor), _) => {
            if cadence.due_slot(anchor, now_local).is_some() {
                return Some(this_minute);
            }
            anchor
        }
        (None, Some((start, end))) if cadence.clock_waits_for_window() => {
            next_active_moment(now_local, start, end)?
        }
        (None, _) => now_local,
    };
    let scheduled = cadence.next_slot(anchor, now_local)?;
    let at = match window {
        Some((start, end)) => next_active_moment(scheduled, start, end)?,
        None => scheduled,
    };
    if at <= now_local {
        return Some(this_minute);
    }
    Some(local_to_instant(tz, at).with_timezone(&Utc))
}

fn next_active_moment(
    at: NaiveDateTime,
    start: NaiveTime,
    end: NaiveTime,
) -> Option<NaiveDateTime> {
    if start == end {
        return None;
    }
    if is_within_active_hours(at, start, end) {
        return Some(at);
    }
    let opens_today = at.date().and_time(start);
    if at < opens_today {
        Some(opens_today)
    } else {
        opens_today.checked_add_days(Days::new(1))
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct RecordedPulseState {
    #[serde(default, alias = "last_run")]
    pub anchor: HashMap<String, NaiveDateTime>,
    #[serde(default)]
    pub last_fire: HashMap<String, PulseFireOutcome>,
}

#[must_use]
pub fn load_recorded_state(path: &Path) -> RecordedPulseState {
    super::types::read_and_parse(path, |contents| {
        serde_json::from_str::<RecordedPulseState>(contents)
    })
    .unwrap_or_default()
}

#[must_use]
pub fn load_anchors(path: &Path) -> HashMap<String, NaiveDateTime> {
    load_recorded_state(path).anchor
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone as _};

    use super::*;

    fn pulse(schedule: &str, active_hours: Option<&str>) -> PulseDef {
        PulseDef {
            name: "p".to_string(),
            enabled: true,
            schedule: Some(schedule.to_string()),
            days: None,
            active_hours: active_hours.map(str::to_string),
            agent: None,
            model_tier: None,
            context_from: None,
            include_identity: None,
            tasks: Vec::new(),
        }
    }

    fn local(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.from_utc_datetime(&local(day, hour, minute))
    }

    #[test]
    fn a_pulse_runs_when_its_schedule_has_passed_since_its_last_run() {
        let at = next_run_at(
            &pulse("1h", None),
            Some(local(1, 12, 0)),
            utc(1, 12, 20),
            chrono_tz::UTC,
        );
        assert_eq!(at, Some(utc(1, 13, 0)));
    }

    #[test]
    fn a_pulse_that_is_due_already_is_reported_at_the_start_of_the_current_minute() {
        let now = Utc.with_ymd_and_hms(2026, 3, 1, 15, 42, 37).unwrap();
        assert_eq!(
            next_run_at(
                &pulse("1h", None),
                Some(local(1, 9, 0)),
                now,
                chrono_tz::UTC
            ),
            Some(utc(1, 15, 42))
        );
    }

    #[test]
    fn a_pulse_that_never_ran_first_runs_one_schedule_after_now() {
        let now = Utc.with_ymd_and_hms(2026, 3, 1, 15, 42, 37).unwrap();
        assert_eq!(
            next_run_at(&pulse("1h", None), None, now, chrono_tz::UTC),
            Some(utc(1, 16, 42))
        );
        assert_eq!(
            next_run_at(&pulse("7d", None), None, now, chrono_tz::UTC),
            Some(utc(8, 15, 42))
        );
    }

    #[test]
    fn a_run_that_falls_after_the_active_hours_close_waits_for_them_to_open_again() {
        let evening = next_run_at(
            &pulse("1h", Some("09:00-17:00")),
            Some(local(1, 16, 30)),
            utc(1, 16, 45),
            chrono_tz::UTC,
        );
        assert_eq!(evening, Some(utc(2, 9, 0)), "17:30 is after closing");

        let early = next_run_at(
            &pulse("1h", Some("09:00-17:00")),
            Some(local(1, 6, 0)),
            utc(1, 6, 30),
            chrono_tz::UTC,
        );
        assert_eq!(early, Some(utc(1, 9, 0)), "07:00 is before opening, today");
    }

    #[test]
    fn a_run_inside_the_active_hours_is_not_moved() {
        let at = next_run_at(
            &pulse("2h", Some("09:00-17:00")),
            Some(local(1, 10, 0)),
            utc(1, 10, 5),
            chrono_tz::UTC,
        );
        assert_eq!(at, Some(utc(1, 12, 0)));
    }

    #[test]
    fn a_pulse_that_never_ran_is_first_seen_when_the_active_hours_open() {
        let at = next_run_at(
            &pulse("1h", Some("09:00-17:00")),
            None,
            utc(1, 20, 0),
            chrono_tz::UTC,
        );
        assert_eq!(
            at,
            Some(utc(2, 10, 0)),
            "first seen at 09:00 tomorrow, runs an hour later"
        );
    }

    #[test]
    fn an_overnight_window_opens_in_the_evening_of_the_same_day() {
        let window = Some("22:00-06:00");
        assert_eq!(
            next_run_at(&pulse("1h", window), None, utc(1, 14, 0), chrono_tz::UTC),
            Some(utc(1, 23, 0)),
            "in the afternoon, first seen at tonight's opening, runs an hour later"
        );
        assert_eq!(
            next_run_at(&pulse("1h", window), None, utc(1, 23, 0), chrono_tz::UTC),
            Some(utc(2, 0, 0)),
            "inside the window already, so first seen now"
        );
        assert_eq!(
            next_run_at(
                &pulse("1h", window),
                Some(local(1, 5, 30)),
                utc(1, 5, 40),
                chrono_tz::UTC
            ),
            Some(utc(1, 22, 0)),
            "06:30 is after the window closed"
        );
    }

    #[test]
    fn the_closing_time_is_outside_the_window_and_the_opening_time_is_inside() {
        let closing = next_run_at(
            &pulse("1h", Some("09:00-17:00")),
            Some(local(1, 16, 0)),
            utc(1, 16, 1),
            chrono_tz::UTC,
        );
        assert_eq!(closing, Some(utc(2, 9, 0)), "17:00 is the closing time");

        let opening = next_run_at(
            &pulse("1h", Some("09:00-17:00")),
            Some(local(1, 8, 0)),
            utc(1, 8, 1),
            chrono_tz::UTC,
        );
        assert_eq!(opening, Some(utc(1, 9, 0)), "09:00 is the opening time");
    }

    #[test]
    fn a_pulse_that_will_not_run_has_no_next_run() {
        let now = utc(1, 12, 0);
        let mut disabled = pulse("1h", None);
        disabled.enabled = false;
        assert_eq!(next_run_at(&disabled, None, now, chrono_tz::UTC), None);
        assert_eq!(
            next_run_at(&pulse("soon", None), None, now, chrono_tz::UTC),
            None,
            "an unreadable schedule"
        );
        assert_eq!(
            next_run_at(&pulse("1h", Some("9-5")), None, now, chrono_tz::UTC),
            None,
            "unreadable active hours"
        );
        assert_eq!(
            next_run_at(&pulse("1h", Some("09:00-09:00")), None, now, chrono_tz::UTC),
            None,
            "a window that never opens"
        );
    }

    #[test]
    fn the_schedule_and_the_active_hours_are_read_in_the_hub_timezone() {
        let new_york: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let at = next_run_at(
            &pulse("1h", Some("09:00-17:00")),
            None,
            utc(2, 0, 30),
            new_york,
        );
        assert_eq!(
            at,
            Some(utc(2, 15, 0)),
            "first seen at 09:00 in New York (14:00 UTC) the next morning, runs an hour later"
        );
        let inside = next_run_at(
            &pulse("1h", Some("09:00-17:00")),
            None,
            utc(1, 20, 7),
            new_york,
        );
        assert_eq!(
            inside,
            Some(utc(1, 21, 7)),
            "15:07 in New York is inside the window, so it is first seen now and runs an hour later"
        );
    }

    #[test]
    fn a_run_inside_a_spring_forward_gap_lands_later_by_the_gap() {
        let new_york: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let at = next_run_at(
            &pulse("30m", None),
            Some(local(8, 2, 0)),
            Utc.with_ymd_and_hms(2026, 3, 8, 6, 0, 0).unwrap(),
            new_york,
        );
        assert_eq!(
            at,
            Some(Utc.with_ymd_and_hms(2026, 3, 8, 7, 30, 0).unwrap())
        );
    }

    #[test]
    fn last_runs_are_read_from_the_state_file_and_a_missing_or_broken_one_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pulse_state.json");
        assert!(load_anchors(&path).is_empty(), "no file");

        std::fs::write(
            &path,
            r#"{"last_run":{"a":"2026-03-01T12:00:00","b":"2026-03-02T01:30:15.250"},"last_output":{"a":"x"}}"#,
        )
        .unwrap();
        let runs = load_anchors(&path);
        assert_eq!(runs.get("a"), Some(&local(1, 12, 0)));
        assert_eq!(runs.len(), 2);

        std::fs::write(&path, "{ not json").unwrap();
        assert!(load_anchors(&path).is_empty(), "a broken file");
    }

    #[test]
    fn last_fires_are_read_from_the_state_file_and_an_older_file_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pulse_state.json");
        std::fs::write(&path, r#"{"last_run":{"a":"2026-03-01T12:00:00"}}"#).unwrap();
        let older = load_recorded_state(&path);
        assert!(older.last_fire.is_empty());
        assert_eq!(
            older.anchor.len(),
            1,
            "the rest of an older file still loads"
        );

        std::fs::write(
            &path,
            r#"{"last_run":{},"last_fire":{"a":{"status":"stopped","at":"2026-03-01T12:00:05Z"}}}"#,
        )
        .unwrap();
        let state = load_recorded_state(&path);
        let fire = state.last_fire.get("a").expect("a's last fire");
        assert_eq!(fire.status, residuum_bus::PulseFireStatus::Stopped);
        assert_eq!(fire.error, None);
        assert_eq!(fire.at, Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 5).unwrap());
    }

    fn on_days(days: &[&str], active_hours: Option<&str>) -> PulseDef {
        PulseDef {
            schedule: None,
            days: Some(days.iter().map(|d| (*d).to_string()).collect()),
            ..pulse("1h", active_hours)
        }
    }

    #[test]
    fn a_day_pulse_is_next_at_the_window_start_on_its_next_listed_day() {
        let sunday = on_days(&["sun"], Some("09:00-10:00"));
        assert_eq!(
            next_run_at(
                &sunday,
                Some(local(7, 9, 30)),
                utc(7, 12, 0),
                chrono_tz::UTC
            ),
            Some(utc(8, 9, 0)),
            "Saturday noon, anchored to Saturday 09:30: next is Sunday 09:00"
        );
        assert_eq!(
            next_run_at(&sunday, Some(local(8, 9, 0)), utc(8, 9, 30), chrono_tz::UTC),
            Some(utc(15, 9, 0)),
            "after Sunday's run, the next is a week on"
        );
    }

    #[test]
    fn a_day_pulse_never_seen_outside_its_window_runs_on_its_first_listed_day() {
        let sunday = on_days(&["sun"], Some("17:30-18:30"));
        assert_eq!(
            next_run_at(&sunday, None, utc(7, 12, 0), chrono_tz::UTC),
            Some(utc(8, 17, 30)),
            "Saturday noon: Sunday at the window start"
        );
        assert_eq!(
            next_run_at(&sunday, None, utc(8, 9, 0), chrono_tz::UTC),
            Some(utc(8, 17, 30)),
            "Sunday morning, before the window"
        );
    }

    #[test]
    fn a_day_pulse_due_inside_its_open_window_is_next_this_minute() {
        let sunday = on_days(&["sun"], Some("09:00-10:00"));
        assert_eq!(
            next_run_at(
                &sunday,
                Some(local(7, 9, 30)),
                utc(8, 9, 20),
                chrono_tz::UTC
            ),
            Some(utc(8, 9, 20)),
        );
    }

    #[test]
    fn a_day_pulse_never_seen_is_first_seen_now_and_runs_on_its_next_listed_day() {
        let sunday = on_days(&["sun"], None);
        assert_eq!(
            next_run_at(&sunday, None, utc(7, 12, 0), chrono_tz::UTC),
            Some(utc(8, 0, 0)),
            "midnight on the coming Sunday"
        );
    }

    #[test]
    fn a_deferred_weekly_run_keeps_the_weekday_it_was_anchored_to() {
        let weekly = PulseDef {
            active_hours: Some("17:00-23:00".to_string()),
            ..pulse("7d", None)
        };
        assert_eq!(
            next_run_at(
                &weekly,
                Some(local(15, 17, 30)),
                utc(23, 9, 0),
                chrono_tz::UTC
            ),
            Some(utc(23, 17, 0)),
            "the overdue run lands when the window reopens"
        );
        assert_eq!(
            next_run_at(
                &weekly,
                Some(local(22, 17, 30)),
                utc(23, 17, 1),
                chrono_tz::UTC
            ),
            Some(utc(29, 17, 30)),
        );
    }
}
