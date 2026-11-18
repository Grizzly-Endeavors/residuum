//! When a pulse is due: the one rule set behind the scheduler's ticks and the
//! next-run calculation, so the two can't disagree.
//!
//! A pulse's clock is an *anchor*: the scheduled time its last run answered,
//! or the moment the scheduler first saw it. Runs are scheduled from the
//! anchor, never from when a run actually fired, so a run delayed by a
//! stopped gateway or by `active_hours` doesn't move the pulse's later runs.

use std::str::FromStr as _;

use chrono::{Datelike as _, Days, NaiveDateTime, NaiveTime, TimeDelta, Timelike as _, Weekday};

use super::types::{PulseDef, is_within_active_hours, parse_active_hours, parse_schedule_duration};

/// How a pulse's runs are spaced.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    /// Every `interval` after the anchor.
    Every(TimeDelta),
    /// On each of `days`, at the start of the active-hours window (midnight
    /// without one). `min_gap`, from `schedule`, is the least time between two
    /// runs.
    OnDays {
        days: Vec<Weekday>,
        min_gap: Option<TimeDelta>,
    },
}

/// A pulse's `schedule`, `days` and `active_hours`, read once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cadence {
    rule: Rule,
    window: Option<(NaiveTime, NaiveTime)>,
}

impl Cadence {
    /// Read `pulse`'s timing settings.
    ///
    /// # Errors
    /// Returns a message naming the pulse and the bad value when its
    /// `schedule`, `days` or `active_hours` can't be read, or when it sets
    /// neither `schedule` nor `days`.
    pub fn of(pulse: &PulseDef) -> Result<Self, String> {
        let window = match pulse.active_hours.as_deref() {
            None => None,
            Some(hours) => Some(parse_active_hours(hours).map_err(|e| {
                format!(
                    "pulse '{}' has an invalid active_hours '{hours}' ({e}) — skipping until \
                     fixed",
                    pulse.name
                )
            })?),
        };
        let schedule = pulse
            .schedule
            .as_deref()
            .map(|s| {
                parse_schedule_duration(s).map_err(|e| {
                    format!(
                        "pulse '{}' has an invalid schedule '{s}' ({e}) — skipping until fixed",
                        pulse.name
                    )
                })
            })
            .transpose()?;
        let rule = match (pulse.days.as_deref(), schedule) {
            (Some(days), min_gap) => Rule::OnDays {
                days: parse_weekdays(days).map_err(|e| format!("pulse '{}' {e}", pulse.name))?,
                min_gap,
            },
            (None, Some(interval)) => Rule::Every(interval),
            (None, None) => {
                return Err(format!(
                    "pulse '{}' needs a `schedule` or `days` — skipping until fixed",
                    pulse.name
                ));
            }
        };
        Ok(Self { rule, window })
    }

    /// The active-hours window, if the pulse has one.
    #[must_use]
    pub fn window(&self) -> Option<(NaiveTime, NaiveTime)> {
        self.window
    }

    /// The scheduled time a run at `now` answers, if the pulse is due at `now`
    /// given its `anchor`. The caller fires it once and stores the returned
    /// time as the new anchor.
    ///
    /// An interval pulse that missed several intervals is due once: the
    /// returned time is the latest interval boundary at or before `now`, so
    /// the next run is the first one after it.
    #[must_use]
    pub fn due_slot(&self, anchor: NaiveDateTime, now: NaiveDateTime) -> Option<NaiveDateTime> {
        if let Some((start, end)) = self.window
            && !is_within_active_hours(now, start, end)
        {
            return None;
        }
        match &self.rule {
            Rule::Every(interval) => {
                let elapsed = now - anchor;
                if elapsed < *interval {
                    return None;
                }
                let steps = elapsed.num_seconds() / interval.num_seconds();
                Some(anchor + TimeDelta::seconds(interval.num_seconds() * steps))
            }
            Rule::OnDays { days, min_gap } => {
                let slot = latest_day_slot(days, self.start_time(), now)?;
                if slot <= anchor || min_gap.is_some_and(|gap| slot < anchor + gap) {
                    return None;
                }
                // A day's run can start late, but not after its window closes.
                (now < self.day_slot_end(slot)).then_some(slot)
            }
        }
    }

    /// The first moment the pulse is scheduled to run at or after `now`,
    /// before `active_hours` defers it. `None` only when a day pulse's
    /// window never opens (an empty window).
    #[must_use]
    pub fn next_slot(&self, anchor: NaiveDateTime, now: NaiveDateTime) -> Option<NaiveDateTime> {
        match &self.rule {
            Rule::Every(interval) => Some((anchor + *interval).max(now)),
            Rule::OnDays { days, min_gap } => {
                let floor = min_gap.map_or(now, |gap| now.max(anchor + gap));
                first_day_slot_from(days, self.start_time(), floor, anchor)
            }
        }
    }

    /// The time a day's run starts: the window's start, or midnight.
    fn start_time(&self) -> NaiveTime {
        self.window.map_or(NaiveTime::MIN, |(start, _end)| start)
    }

    /// The moment a day's run can no longer start: its window's close, or the
    /// next midnight without a window. An empty window closes at its start.
    fn day_slot_end(&self, slot: NaiveDateTime) -> NaiveDateTime {
        match self.window {
            Some((start, end)) => {
                let len = (i64::from(end.num_seconds_from_midnight())
                    - i64::from(start.num_seconds_from_midnight()))
                .rem_euclid(SECONDS_PER_DAY);
                slot + TimeDelta::seconds(len)
            }
            None => slot
                .date()
                .checked_add_days(Days::new(1))
                .map_or(NaiveDateTime::MAX, |next| next.and_time(NaiveTime::MIN)),
        }
    }
}

const SECONDS_PER_DAY: i64 = 86_400;

/// Parse a pulse's `days`: weekday names, short or full, in any case.
///
/// # Errors
/// Returns a message naming the bad value, or that the list is empty.
pub fn parse_weekdays(days: &[String]) -> Result<Vec<Weekday>, String> {
    if days.is_empty() {
        return Err(
            "sets `days` to an empty list — list the weekdays it runs on, or remove the line"
                .to_string(),
        );
    }
    days.iter()
        .map(|day| {
            Weekday::from_str(day.trim()).map_err(|_parse_err| {
                format!(
                    "has an invalid day '{day}' in `days` — use mon, tue, wed, thu, fri, sat or sun"
                )
            })
        })
        .collect()
}

/// The latest occurrence of a listed weekday at `start` that is at or before
/// `now`.
fn latest_day_slot(
    days: &[Weekday],
    start: NaiveTime,
    now: NaiveDateTime,
) -> Option<NaiveDateTime> {
    let mut date = now.date();
    for _ in 0..7 {
        let slot = date.and_time(start);
        if days.contains(&date.weekday()) && slot <= now {
            return Some(slot);
        }
        date = date.pred_opt()?;
    }
    None
}

/// The first occurrence of a listed weekday at `start` that is at or after
/// `floor` and after `anchor`. `floor`'s own day may have passed its slot, so
/// the next listed day can be seven days after it: eight days are searched.
fn first_day_slot_from(
    days: &[Weekday],
    start: NaiveTime,
    floor: NaiveDateTime,
    anchor: NaiveDateTime,
) -> Option<NaiveDateTime> {
    let mut date = floor.date();
    for _ in 0..=7 {
        let slot = date.and_time(start);
        if days.contains(&date.weekday()) && slot >= floor && slot > anchor {
            return Some(slot);
        }
        date = date.succ_opt()?;
    }
    None
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|d| (*d).to_string()).collect()
    }

    fn at(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    #[test]
    fn weekdays_read_short_and_full_names_in_any_case() {
        assert_eq!(
            parse_weekdays(&names(&["sun", "Monday", "FRI", "wed"])).unwrap(),
            vec![Weekday::Sun, Weekday::Mon, Weekday::Fri, Weekday::Wed]
        );
    }

    #[test]
    fn a_weekday_name_that_is_not_a_day_is_rejected_with_the_value() {
        let err = parse_weekdays(&names(&["sun", "funday"])).unwrap_err();
        assert!(err.contains("'funday'"), "{err}");
    }

    #[test]
    fn an_empty_day_list_is_rejected() {
        assert!(parse_weekdays(&[]).is_err());
    }

    #[test]
    fn a_day_run_window_closes_at_its_end_on_the_same_day() {
        let pulse = PulseDef {
            name: "p".to_string(),
            enabled: true,
            schedule: None,
            days: Some(names(&["sun"])),
            active_hours: Some("22:00-06:00".to_string()),
            agent: None,
            model_tier: None,
            context_from: None,
            include_identity: None,
            tasks: Vec::new(),
        };
        let cadence = Cadence::of(&pulse).unwrap();
        // Sunday 2026-03-08 22:00 runs and may start until Monday 06:00.
        assert_eq!(
            cadence.due_slot(at(1, 0, 0), at(9, 5, 59)),
            Some(at(8, 22, 0)),
            "overnight window still open"
        );
        assert_eq!(cadence.due_slot(at(1, 0, 0), at(9, 6, 0)), None);
    }

    #[test]
    fn an_interval_pulse_is_not_due_before_one_interval_has_passed() {
        let pulse = PulseDef {
            name: "p".to_string(),
            enabled: true,
            schedule: Some("1h".to_string()),
            days: None,
            active_hours: None,
            agent: None,
            model_tier: None,
            context_from: None,
            include_identity: None,
            tasks: Vec::new(),
        };
        let cadence = Cadence::of(&pulse).unwrap();
        assert_eq!(cadence.due_slot(at(1, 12, 0), at(1, 12, 59)), None);
        assert_eq!(
            cadence.due_slot(at(1, 12, 0), at(1, 13, 0)),
            Some(at(1, 13, 0))
        );
    }
}
