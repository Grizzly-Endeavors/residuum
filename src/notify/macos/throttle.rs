//! Batch aggregator for throttling macOS notifications.
//!
//! Collects notifications during a configurable window and flushes them
//! as individual or summary macOS notifications to prevent flooding.

use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};

use super::MacosChannelConfig;
use super::bridge::MacosBridge;
use crate::bus::NotificationEvent;

#[must_use]
pub fn spawn(
    rx: mpsc::Receiver<NotificationEvent>,
    bridge: MacosBridge,
    config: MacosChannelConfig,
) -> JoinHandle<()> {
    tokio::spawn(run(rx, bridge, config))
}

async fn run(
    mut rx: mpsc::Receiver<NotificationEvent>,
    bridge: MacosBridge,
    config: MacosChannelConfig,
) {
    let throttle_duration = Duration::from_secs(config.throttle_window_secs);
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
                        flush(&bridge, &buffer, &config).await;
                    }
                    tracing::info!("macOS notification aggregator shutting down");
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
                    flush(&bridge, &buffer, &config).await;
                    buffer.clear();
                }
                window_deadline = None;
            }
        }
    }
}

/// Cap at 3 macOS notifications per flush (SC-004) to avoid flooding
/// Notification Center when many tasks complete in one window.
/// - 1-3 items: deliver each individually
/// - 4+  items: deliver top 2 + 1 summary
async fn flush(bridge: &MacosBridge, buffer: &[NotificationEvent], config: &MacosChannelConfig) {
    debug_assert!(!buffer.is_empty());
    let count = buffer.len();
    tracing::info!(count, "flushing macOS notification batch");

    if count <= 3 {
        for notif in buffer {
            deliver_individual(bridge, notif, config).await;
        }
    } else {
        for notif in buffer.iter().take(2) {
            deliver_individual(bridge, notif, config).await;
        }

        let summarized = count - 2;
        let summary_title = format!("{} \u{2014} {count} results", config.app_name);
        let summary_body = build_summary_body(&buffer[2..]);

        if let Err(e) = bridge
            .post_summary(&summary_title, &summary_body, config.default_priority)
            .await
        {
            tracing::warn!(error = %e, "failed to post summary notification");
        }

        tracing::debug!(
            individual = 2,
            summarized,
            "posted batch: 2 individual + summary"
        );
    }
}

async fn deliver_individual(
    bridge: &MacosBridge,
    notif: &NotificationEvent,
    config: &MacosChannelConfig,
) {
    let id = format!("residuum-{}", uuid::Uuid::new_v4());
    let text = super::bridge::NotificationText {
        title: config.app_name.clone(),
        subtitle: notif.title.replace('_', " "),
        body: truncate_body(&notif.content, 200),
    };
    let category = config.default_category;

    if let Err(e) = bridge
        .post_notification(
            &id,
            text,
            category.as_category_id(),
            config.default_priority,
            config.sound,
            category.as_category_id(),
        )
        .await
    {
        tracing::warn!(
            task = %notif.title,
            error = %e,
            "failed to post individual notification"
        );
    }
}

fn build_summary_body(buffer: &[NotificationEvent]) -> String {
    let joined = buffer
        .iter()
        .map(|n| n.title.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    truncate_body(&joined, 200)
}

fn truncate_body(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut result: String = s.chars().take(max_len - 1).collect();
        result.push('\u{2026}');
        result
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    fn make_notification(task_name: &str) -> NotificationEvent {
        NotificationEvent {
            title: task_name.to_string(),
            content: format!("Summary for {task_name}"),
            source: crate::bus::EventTrigger::Pulse,
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
