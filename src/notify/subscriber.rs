//! Generic subscriber loop for notification channels.

use super::channels::NotificationChannel;
use crate::bus::{BusEvent, Subscriber};

/// Run a subscriber loop that delivers `BusEvent::Notification` events to a channel.
///
/// Non-notification events are silently ignored. Delivery errors are logged
/// but do not stop the loop. The function returns when the subscriber closes.
pub async fn run_notify_subscriber(
    mut subscriber: Subscriber,
    channel: Box<dyn NotificationChannel>,
) {
    let channel_name = channel.name().to_string();
    tracing::info!(channel = %channel_name, "notify subscriber started");

    while let Some(event) = subscriber.recv().await {
        let BusEvent::Notification(ref notification) = event else {
            continue;
        };
        if let Err(e) = channel.deliver(notification).await {
            tracing::warn!(
                channel = %channel_name,
                error = %e,
                "notification delivery failed"
            );
        }
    }

    tracing::info!(channel = %channel_name, "notify subscriber stopped");
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use tokio::sync::Mutex;

    use crate::bus::{BusEvent, NotifyName, TopicId, spawn_broker};
    use crate::bus::{EventTrigger, NotificationEvent};
    use crate::notify::channels::NotificationChannel;

    use super::run_notify_subscriber;

    // -----------------------------------------------------------------------
    // MockChannel
    // -----------------------------------------------------------------------

    struct MockChannel {
        delivered: Arc<Mutex<Vec<NotificationEvent>>>,
        should_fail: Arc<AtomicBool>,
    }

    impl MockChannel {
        fn new(should_fail: Arc<AtomicBool>) -> (Self, Arc<Mutex<Vec<NotificationEvent>>>) {
            let delivered = Arc::new(Mutex::new(Vec::new()));
            let channel = Self {
                delivered: Arc::clone(&delivered),
                should_fail,
            };
            (channel, delivered)
        }
    }

    #[async_trait]
    impl NotificationChannel for MockChannel {
        fn name(&self) -> &'static str {
            "mock"
        }

        fn channel_kind(&self) -> &'static str {
            "mock"
        }

        async fn deliver(&self, notification: &NotificationEvent) -> anyhow::Result<()> {
            if self.should_fail.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("mock delivery failure"));
            }
            self.delivered.lock().await.push(notification.clone());
            Ok(())
        }
    }

    fn make_notification_event() -> BusEvent {
        BusEvent::Notification(NotificationEvent {
            title: "test-title".to_string(),
            content: "test-content".to_string(),
            source: EventTrigger::Pulse,
            timestamp: chrono::NaiveDate::from_ymd_opt(2026, 3, 14)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        })
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn notification_events_are_delivered() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let topic = TopicId::Notify(NotifyName::from("test"));
        let sub = handle.subscribe(topic.clone()).await.unwrap();

        let should_fail = Arc::new(AtomicBool::new(false));
        let (channel, delivered) = MockChannel::new(Arc::clone(&should_fail));

        let loop_task = tokio::spawn(run_notify_subscriber(sub, Box::new(channel)));

        pub_.publish(topic, make_notification_event())
            .await
            .unwrap();

        // Give the loop time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Abort the subscriber loop (it won't exit naturally since the
        // subscriber itself holds a cmd_tx keeping the broker alive)
        loop_task.abort();

        let received = delivered.lock().await;
        assert_eq!(received.len(), 1, "should have delivered one notification");
        assert_eq!(received[0].title, "test-title");
        assert_eq!(received[0].content, "test-content");
    }

    #[tokio::test]
    async fn non_notification_events_are_ignored() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let topic = TopicId::Notify(NotifyName::from("test"));
        let sub = handle.subscribe(topic.clone()).await.unwrap();

        let should_fail = Arc::new(AtomicBool::new(false));
        let (channel, delivered) = MockChannel::new(Arc::clone(&should_fail));

        let loop_task = tokio::spawn(run_notify_subscriber(sub, Box::new(channel)));

        // Publish a non-notification event
        pub_.publish(
            topic,
            BusEvent::TurnStarted {
                correlation_id: "c1".to_string(),
            },
        )
        .await
        .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        loop_task.abort();

        let received = delivered.lock().await;
        assert_eq!(
            received.len(),
            0,
            "non-notification events should be ignored"
        );
    }

    #[tokio::test]
    async fn delivery_errors_do_not_stop_loop() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let topic = TopicId::Notify(NotifyName::from("test"));
        let sub = handle.subscribe(topic.clone()).await.unwrap();

        let should_fail = Arc::new(AtomicBool::new(true));
        let (channel, delivered) = MockChannel::new(Arc::clone(&should_fail));

        let loop_task = tokio::spawn(run_notify_subscriber(sub, Box::new(channel)));

        // First notification — delivery will fail
        pub_.publish(topic.clone(), make_notification_event())
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Now allow delivery to succeed
        should_fail.store(false, Ordering::SeqCst);

        // Second notification — delivery should succeed
        pub_.publish(topic, make_notification_event())
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        loop_task.abort();

        let received = delivered.lock().await;
        assert_eq!(
            received.len(),
            1,
            "only the second notification should be delivered"
        );
    }
}
