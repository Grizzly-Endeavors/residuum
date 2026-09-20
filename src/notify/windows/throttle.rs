//! Batch aggregator wiring for the Windows notification channel.
//!
//! The debounce-window batching logic itself lives in
//! [`crate::notify::batch_aggregator`], shared with the macOS channel;
//! this module just spawns it with the Windows bridge.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::bridge::WindowsBridge;
use crate::bus::NotificationEvent;
use crate::notify::batch_aggregator;

#[must_use]
pub fn spawn(
    rx: mpsc::Receiver<NotificationEvent>,
    bridge: WindowsBridge,
    throttle_window_secs: u64,
) -> JoinHandle<()> {
    tokio::spawn(batch_aggregator::run(rx, bridge, throttle_window_secs))
}
