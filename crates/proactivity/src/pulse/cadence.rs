use std::str::FromStr as _;

use chrono::{Datelike as _, Days, NaiveDateTime, NaiveTime, TimeDelta, Timelike as _, Weekday};

use super::types::{PulseDef, is_within_active_hours, parse_active_hours, parse_schedule_duration};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    Every(TimeDelta),
    OnDays {
        days: Vec<Weekday>,
        min_gap: Option<TimeDelta>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cadence {
    rule: Rule,
    window: Option<(NaiveTime, NaiveTime)>,
}

impl Cadence {
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

    #[must_use]
    pub fn window(&self) -> Option<(NaiveTime, NaiveTime)> {
        self.window
    }

    #[must_use]
    pub fn clock_waits_for_window(&self) -> bool {
        self.window.is_some() && matches!(self.rule, Rule::Every(_))
    }

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
                (now < self.day_slot_end(slot)).then_some(slot)
            }
        }
    }

    #[must_use]
    pub fn missed_slot(&self, anchor: NaiveDateTime, now: NaiveDateTime) -> Option<NaiveDateTime> {
        let Rule::OnDays { days, min_gap } = &self.rule else {
            return None;
        };
        let slot = latest_day_slot(days, self.start_time(), now)?;
        let eligible = slot > anchor && min_gap.is_none_or(|gap| slot >= anchor + gap);
        (eligible && now >= self.day_slot_end(slot)).then_some(slot)
    }

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

    fn start_time(&self) -> NaiveTime {
        self.window.map_or(NaiveTime::MIN, |(start, _end)| start)
    }

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
        assert_eq!(
            cadence.due_slot(at(1, 0, 0), at(9, 5, 59)),
            Some(at(8, 22, 0)),
            "overnight window still open"
        );
        assert_eq!(cadence.due_slot(at(1, 0, 0), at(9, 6, 0)), None);
    }

    #[test]
    fn a_day_run_whose_window_closed_unanswered_is_missed_once_the_anchor_moves() {
        let pulse = PulseDef {
            name: "p".to_string(),
            enabled: true,
            schedule: None,
            days: Some(names(&["sun"])),
            active_hours: Some("09:00-10:00".to_string()),
            agent: None,
            model_tier: None,
            context_from: None,
            include_identity: None,
            tasks: Vec::new(),
        };
        let cadence = Cadence::of(&pulse).unwrap();
        assert_eq!(cadence.missed_slot(at(7, 12, 0), at(8, 9, 30)), None);
        assert_eq!(
            cadence.missed_slot(at(7, 12, 0), at(9, 9, 30)),
            Some(at(8, 9, 0)),
            "Monday: Sunday's window closed without a run"
        );
        assert_eq!(
            cadence.missed_slot(at(8, 9, 0), at(9, 9, 30)),
            None,
            "a run that answered the slot is not missed"
        );
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
