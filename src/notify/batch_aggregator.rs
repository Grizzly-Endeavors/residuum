//! Shared debounce-window batch aggregator for native notification channels.
//!
//! Collects notifications during a configurable window and flushes them as
//! individual or summary notifications to prevent flooding. Platform
//! channels (macOS, Windows) each implement [`NotificationBridge`] to plug
//! their delivery mechanism into the shared debounce/flush logic here.

use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::bus::NotificationEvent;

/// Platform-specific delivery for an already-batched notification.
///
/// Implementors own only the platform's notification API; the debounce
/// window, the 3-notification flush cap (SC-004), and the summary-body
/// fallback all live in [`run`] and are shared across platforms.
#[async_trait]
pub trait NotificationBridge: Send + Sync {
    /// Deliver a single notification.
    async fn deliver_individual(&self, notif: &NotificationEvent);

    /// Deliver a rolled-up summary for the notifications that didn't get
    /// individual delivery. `urgent` is set if any summarized notification
    /// was urgent, so the platform can escalate delivery priority.
    async fn deliver_summary(&self, title: &str, body: &str, urgent: bool);

    /// Channel display name used in summary titles (e.g. `"Residuum"`).
    fn app_name(&self) -> &str;

    /// Platform label for tracing messages (e.g. `"macOS"`, `"Windows"`).
    fn platform_label(&self) -> &'static str;
}

/// Run the batch aggregator loop: buffer incoming notifications for
/// `throttle_window_secs` after the first arrival, then flush the batch.
pub async fn run<B: NotificationBridge>(
    mut rx: mpsc::Receiver<NotificationEvent>,
    bridge: B,
    throttle_window_secs: u64,
) {
    let throttle_duration = Duration::from_secs(throttle_window_secs);
    let mut buffer: Vec<NotificationEvent> = Vec::new();
    let mut window_deadline: Option<Instant> = None;

    loop {
        tokio::select! {
            maybe_notif = rx.recv() => {
                if let Some(notif) = maybe_notif {
                    buffer.push(notif);
                    if window_deadline.is_none() {
                        window_deadline = Some(Instant::now() + throttle_duration);
                    }
                } else {
                    // Channel closed — flush remaining and exit
                    if !buffer.is_empty() {
                        flush(&bridge, &buffer).await;
                    }
                    tracing::info!(
                        platform = bridge.platform_label(),
                        "notification aggregator shutting down"
                    );
                    return;
                }
            }
            () = async {
                match window_deadline {
                    Some(d) => sleep_until(d).await,
                    None => std::future::pending().await,
                }
            } => {
                if !buffer.is_empty() {
                    flush(&bridge, &buffer).await;
                    buffer.clear();
                }
                window_deadline = None;
            }
        }
    }
}

/// Cap at 3 notifications per flush (SC-004) to avoid flooding the
/// platform's notification surface when many tasks complete in one window.
/// - 1-3 items: deliver each individually
/// - 4+  items: deliver top 2 + 1 summary
async fn flush<B: NotificationBridge>(bridge: &B, buffer: &[NotificationEvent]) {
    debug_assert!(!buffer.is_empty(), "flush called with an empty buffer");
    let count = buffer.len();
    tracing::info!(
        count,
        platform = bridge.platform_label(),
        "flushing notification batch"
    );

    if count <= 3 {
        for notif in buffer {
            bridge.deliver_individual(notif).await;
        }
    } else {
        for notif in buffer.iter().take(2) {
            bridge.deliver_individual(notif).await;
        }

        let tail = buffer.get(2..).unwrap_or_default();
        let summarized = tail.len();
        let summary_title = format!("{} \u{2014} {count} results", bridge.app_name());
        let summary_body = build_summary_body(tail);
        let urgent = tail.iter().any(|n| n.urgent);

        bridge
            .deliver_summary(&summary_title, &summary_body, urgent)
            .await;

        tracing::debug!(
            individual = 2,
            summarized,
            "posted batch: 2 individual + summary"
        );
    }
}

#[must_use]
pub fn build_summary_body(buffer: &[NotificationEvent]) -> String {
    let joined = buffer
        .iter()
        .map(|n| n.title.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    truncate_body(&joined, 200)
}

#[must_use]
pub fn truncate_body(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let mut result: String = s.chars().take(max_len - 1).collect();
        result.push('\u{2026}');
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_notification(task_name: &str) -> NotificationEvent {
        NotificationEvent {
            title: task_name.to_string(),
            content: format!("Summary for {task_name}"),
            source: crate::bus::EventTrigger::Pulse,
            urgent: false,
            timestamp: chrono::NaiveDate::from_ymd_opt(2026, 3, 14)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        }
    }

    // ── Truncation tests ────────────────────────────────────────────────

    #[test]
    fn truncate_body_short_string() {
        let result = truncate_body("hello", 200);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_body_exact_length() {
        let s = "a".repeat(200);
        let result = truncate_body(&s, 200);
        assert_eq!(result.len(), 200);
        assert!(!result.contains('\u{2026}'));
    }

    #[test]
    fn truncate_body_over_limit() {
        let s = "a".repeat(250);
        let result = truncate_body(&s, 200);
        assert!(
            result.chars().count() <= 200,
            "truncated body should not exceed 200 chars"
        );
        assert!(result.ends_with('\u{2026}'), "should end with ellipsis");
    }

    #[test]
    fn truncate_body_unicode_within_char_limit() {
        // 10 emoji (4 bytes each) = 40 bytes but only 10 chars
        let s = "\u{1F980}".repeat(10);
        let result = truncate_body(&s, 200);
        assert_eq!(result, s, "should not truncate — char count is under limit");
        assert!(!result.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_body_unicode_over_byte_limit_but_within_char_limit() {
        // 60 emoji (4 bytes each) = 240 bytes but only 60 chars — exceeds the
        // 200 byte-length guard but stays under the 200 char-count limit, so
        // this only passes untruncated once the guard counts chars, not bytes.
        let s = "\u{1F980}".repeat(60);
        let result = truncate_body(&s, 200);
        assert_eq!(
            result, s,
            "should not truncate — char count is under limit even though byte count exceeds it"
        );
        assert!(!result.ends_with('\u{2026}'));
    }

    // ── Summary body tests ──────────────────────────────────────────────

    #[test]
    fn build_summary_body_empty() {
        let body = build_summary_body(&[]);
        assert_eq!(body, "");
    }

    #[test]
    fn build_summary_body_single_item() {
        let buffer = vec![make_notification("email_check")];
        let body = build_summary_body(&buffer);
        assert_eq!(body, "email_check");
    }

    #[test]
    fn build_summary_body_multiple_items() {
        let buffer = vec![
            make_notification("email_check"),
            make_notification("deploy_status"),
            make_notification("backup"),
        ];
        let body = build_summary_body(&buffer);
        assert_eq!(body, "email_check\ndeploy_status\nbackup");
    }
}
