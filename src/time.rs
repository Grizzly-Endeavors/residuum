//! Central time helper for timezone-aware local time.

use chrono::{Datelike, NaiveDateTime, TimeDelta};
use tracing::warn;

/// Get the current local time for the configured timezone.
///
/// Converts UTC wall-clock time to the correct local time via `chrono_tz::Tz`,
/// then drops the offset to produce a plain `NaiveDateTime`. DST transitions
/// are handled at the moment of this call.
#[must_use]
pub fn now_local(tz: chrono_tz::Tz) -> NaiveDateTime {
    chrono::Utc::now().with_timezone(&tz).naive_local()
}

/// English ordinal suffix for a day-of-month number.
#[must_use]
pub fn ordinal_suffix(day: u32) -> &'static str {
    match day {
        11..=13 => "th",
        _ if day % 10 == 1 => "st",
        _ if day % 10 == 2 => "nd",
        _ if day % 10 == 3 => "rd",
        _ => "th",
    }
}

/// Format a datetime for display in the time context tag.
///
/// Produces `"Sunday Feb 22nd 2026 | 17:00"`.
#[must_use]
pub fn format_display_datetime(dt: NaiveDateTime) -> String {
    let suffix = ordinal_suffix(dt.day());
    format!(
        "{} {}{suffix} {}",
        dt.format("%A %b"),
        dt.day(),
        dt.format("%Y | %H:%M")
    )
}

/// Format a time delta as a human-readable relative string.
///
/// Produces `"just now"`, `"5 mins ago"`, `"2 hours ago"`, or `"3 days ago"`.
#[must_use]
pub fn format_relative_time(delta: TimeDelta) -> String {
    let total_secs = delta.num_seconds();
    if total_secs < 60 {
        if total_secs < 0 {
            warn!(
                total_secs,
                "format_relative_time called with negative delta"
            );
        }
        return "just now".to_string();
    }

    let mins = delta.num_minutes();
    if mins < 60 {
        let label = if mins == 1 { "min" } else { "mins" };
        return format!("{mins} {label} ago");
    }

    let hours = delta.num_hours();
    if hours < 24 {
        let label = if hours == 1 { "hour" } else { "hours" };
        return format!("{hours} {label} ago");
    }

    let days = delta.num_days();
    let label = if days == 1 { "day" } else { "days" };
    format!("{days} {label} ago")
}

const MINUTE_FMT: &str = "%Y-%m-%dT%H:%M";

fn parse_minute(s: &str) -> Result<NaiveDateTime, String> {
    NaiveDateTime::parse_from_str(s, MINUTE_FMT).map_err(|e| format!("invalid datetime {s:?}: {e}"))
}

/// Serde module for `NaiveDateTime` using minute-precision (`YYYY-MM-DDTHH:MM`).
pub mod minute_format {
    use chrono::NaiveDateTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    /// # Errors
    /// Returns a serializer error if the underlying serializer fails.
    pub fn serialize<S>(dt: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&dt.format(super::MINUTE_FMT).to_string())
    }

    /// # Errors
    /// Returns a deserializer error if the string is missing or not in `YYYY-MM-DDTHH:MM` format.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveDateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        super::parse_minute(&s).map_err(serde::de::Error::custom)
    }
}

/// Serde module for `Option<NaiveDateTime>` using minute-precision.
pub mod minute_format_opt {
    use chrono::NaiveDateTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    /// # Errors
    /// Returns a serializer error if the underlying serializer fails.
    pub fn serialize<S>(dt: &Option<NaiveDateTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match dt {
            Some(dt) => super::minute_format::serialize(dt, serializer),
            None => serializer.serialize_none(),
        }
    }

    /// # Errors
    /// Returns a deserializer error if the string is present but not in `YYYY-MM-DDTHH:MM` format.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<NaiveDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(s) => super::parse_minute(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(year: i32, month: u32, day: u32, hour: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, min, 0)
            .unwrap()
    }

    #[test]
    fn ordinal_suffix_st() {
        assert_eq!(ordinal_suffix(1), "st");
        assert_eq!(ordinal_suffix(21), "st");
        assert_eq!(ordinal_suffix(31), "st");
    }

    #[test]
    fn ordinal_suffix_nd() {
        assert_eq!(ordinal_suffix(2), "nd");
        assert_eq!(ordinal_suffix(22), "nd");
    }

    #[test]
    fn ordinal_suffix_rd() {
        assert_eq!(ordinal_suffix(3), "rd");
        assert_eq!(ordinal_suffix(23), "rd");
    }

    #[test]
    fn ordinal_suffix_th() {
        assert_eq!(ordinal_suffix(4), "th");
        assert_eq!(ordinal_suffix(10), "th");
        assert_eq!(ordinal_suffix(11), "th");
        assert_eq!(ordinal_suffix(12), "th");
        assert_eq!(ordinal_suffix(13), "th");
        assert_eq!(ordinal_suffix(14), "th");
        assert_eq!(ordinal_suffix(20), "th");
    }

    #[test]
    fn format_display_datetime_typical() {
        // Sunday Feb 22nd 2026 at 17:00
        let t = dt(2026, 2, 22, 17, 0);
        assert_eq!(format_display_datetime(t), "Sunday Feb 22nd 2026 | 17:00");
    }

    #[test]
    fn format_display_datetime_first_day() {
        let t = dt(2026, 1, 1, 9, 5);
        assert_eq!(format_display_datetime(t), "Thursday Jan 1st 2026 | 09:05");
    }

    #[test]
    fn format_relative_time_just_now() {
        assert_eq!(format_relative_time(TimeDelta::seconds(0)), "just now");
        assert_eq!(format_relative_time(TimeDelta::seconds(59)), "just now");
    }

    #[test]
    fn format_relative_time_minutes() {
        assert_eq!(format_relative_time(TimeDelta::minutes(1)), "1 min ago");
        assert_eq!(format_relative_time(TimeDelta::minutes(15)), "15 mins ago");
        assert_eq!(format_relative_time(TimeDelta::minutes(59)), "59 mins ago");
    }

    #[test]
    fn format_relative_time_hours() {
        assert_eq!(format_relative_time(TimeDelta::hours(1)), "1 hour ago");
        assert_eq!(format_relative_time(TimeDelta::hours(23)), "23 hours ago");
    }

    #[test]
    fn format_relative_time_days() {
        assert_eq!(format_relative_time(TimeDelta::days(1)), "1 day ago");
        assert_eq!(format_relative_time(TimeDelta::days(7)), "7 days ago");
    }
}
