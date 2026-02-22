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
