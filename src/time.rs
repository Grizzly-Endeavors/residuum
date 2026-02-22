//! Central time helper for timezone-aware local time.

/// Get the current local time for the configured timezone.
///
/// Converts UTC wall-clock time to the correct local time via `chrono_tz::Tz`,
/// then drops the offset to produce a plain `NaiveDateTime`. DST transitions
/// are handled at the moment of this call.
#[must_use]
pub fn now_local(tz: chrono_tz::Tz) -> chrono::NaiveDateTime {
    chrono::Utc::now().with_timezone(&tz).naive_local()
}

const MINUTE_FMT: &str = "%Y-%m-%dT%H:%M";

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
        NaiveDateTime::parse_from_str(&s, super::MINUTE_FMT).map_err(serde::de::Error::custom)
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
            Some(dt) => serializer.serialize_str(&dt.format(super::MINUTE_FMT).to_string()),
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
            Some(s) => NaiveDateTime::parse_from_str(&s, super::MINUTE_FMT)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}
