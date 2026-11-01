//! The status line each user message in the main conversation carries.

use chrono::NaiveDateTime;
use residuum_bus::MessageEvent;
use residuum_config::WEB_UI_ENDPOINT;
use residuum_core::time::{format_display_datetime, format_relative_time, now_local};
use residuum_inference::{Message, Role};

/// Stamps each user message entering the main conversation with its status
/// line: `[Received: …][Last Message: …][Message Source: …]`, computed
/// at the message's arrival and stored on it ([`Message::status_line`]).
///
/// A stored line is never recomputed, so replaying history renders the same
/// bytes on every turn.
#[derive(Debug)]
pub struct StatusClock {
    tz: chrono_tz::Tz,
    /// When the previous user message arrived, for the next `[Last Message]`.
    last_message_at: Option<NaiveDateTime>,
}

impl StatusClock {
    /// A clock in `tz` that has seen no message yet.
    #[must_use]
    pub(crate) fn new(tz: chrono_tz::Tz) -> Self {
        Self {
            tz,
            last_message_at: None,
        }
    }

    /// Render later lines in `tz`.
    pub(crate) fn set_timezone(&mut self, tz: chrono_tz::Tz) {
        self.tz = tz;
    }

    /// Set when the previous user message arrived (at startup, from the
    /// persisted history), for the next line's `[Last Message]`.
    pub(crate) fn set_last_message_at(&mut self, at: Option<NaiveDateTime>) {
        self.last_message_at = at;
    }

    /// Stamp `message`, arriving now from the interface `source`, with its
    /// status line, and record its arrival for the next line.
    pub(crate) fn stamp(&mut self, message: &mut Message, source: Option<&str>) {
        let now = now_local(self.tz);
        message.status_line = Some(render_status_line(now, self.last_message_at, source));
        self.last_message_at = Some(now);
    }

    /// The history entries for `message` arriving now
    /// ([`MessageEvent::into_history_messages`]), its user message stamped.
    #[must_use]
    pub(crate) fn admit(&mut self, message: MessageEvent) -> Vec<Message> {
        let source = message.origin.endpoint.clone();
        let mut entries = message.into_history_messages();
        if let Some(user) = entries.iter_mut().rfind(|m| m.role == Role::User) {
            self.stamp(user, Some(&source));
        }
        entries
    }
}

/// Render the status line for a message that arrived at `now`, the previous
/// one at `last_message_at`, from the interface `source`.
fn render_status_line(
    now: NaiveDateTime,
    last_message_at: Option<NaiveDateTime>,
    source: Option<&str>,
) -> String {
    use std::fmt::Write as _;

    let mut tag = format!("[Received: {}]", format_display_datetime(now));

    if let Some(prev) = last_message_at {
        let relative = format_relative_time(now - prev);
        _ = write!(tag, "[Last Message: {relative}]");
    }

    if let Some(source) = source {
        // The web UI's endpoint is named `ws` inside the harness; the agent
        // knows that interface as the web UI.
        let source = if source == WEB_UI_ENDPOINT {
            "web"
        } else {
            source
        };
        _ = write!(tag, "[Message Source: {source}]");
    }

    tag
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    fn dt(year: i32, month: u32, day: u32, hour: u32, min: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, min, 0)
            .unwrap()
    }

    #[test]
    fn line_without_a_previous_message_has_no_last_message() {
        let line = render_status_line(dt(2026, 2, 22, 17, 0), None, None);
        assert!(line.starts_with("[Received:"), "got: {line}");
        assert!(!line.contains("[Last Message:"), "got: {line}");
    }

    #[test]
    fn line_with_a_previous_message_says_how_long_ago() {
        let line = render_status_line(dt(2026, 2, 22, 17, 0), Some(dt(2026, 2, 22, 16, 45)), None);
        assert!(line.contains("[Last Message:"), "got: {line}");
    }

    #[test]
    fn line_names_the_source_interface() {
        let line = render_status_line(dt(2026, 2, 22, 17, 0), None, Some("discord"));
        assert!(line.ends_with("[Message Source: discord]"), "got: {line}");
    }

    #[test]
    fn web_ui_endpoint_reads_as_web() {
        let line = render_status_line(dt(2026, 2, 22, 17, 0), None, Some(WEB_UI_ENDPOINT));
        assert!(line.ends_with("[Message Source: web]"), "got: {line}");
    }

    #[test]
    fn stamping_records_the_arrival_for_the_next_line() {
        let mut clock = StatusClock::new(chrono_tz::UTC);
        let mut first = Message::user("first");
        let mut second = Message::user("second");
        clock.stamp(&mut first, Some("discord"));
        clock.stamp(&mut second, None);

        let first_line = first.status_line.unwrap();
        let second_line = second.status_line.unwrap();
        assert!(!first_line.contains("[Last Message:"), "got: {first_line}");
        assert!(second_line.contains("[Last Message:"), "got: {second_line}");
        assert!(
            !second_line.contains("[Message Source:"),
            "got: {second_line}"
        );
    }

    #[test]
    fn admitting_a_message_stamps_its_user_entry_only() {
        let mut clock = StatusClock::new(chrono_tz::UTC);
        let mut event = MessageEvent::from_background("hello".to_string());
        event.origin.endpoint = "telegram".to_string();
        event.context = Some("earlier conversation".to_string());

        let entries = clock.admit(event);

        assert_eq!(entries.len(), 2, "context entry, then the user message");
        assert!(entries[0].status_line.is_none(), "context is not stamped");
        assert!(
            entries[1]
                .status_line
                .as_deref()
                .is_some_and(|l| l.ends_with("[Message Source: telegram]")),
            "user message carries its line: {:?}",
            entries[1].status_line
        );
    }
}
