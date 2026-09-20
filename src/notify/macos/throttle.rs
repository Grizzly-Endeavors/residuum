//! Batch aggregator wiring for the macOS notification channel.
//!
//! The debounce-window batching logic itself lives in
//! [`crate::notify::batch_aggregator`], shared with the Windows channel;
//! this module just spawns it with the macOS bridge.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::bridge::MacosBridge;
use crate::bus::NotificationEvent;
use crate::notify::batch_aggregator;

#[must_use]
pub fn spawn(
    rx: mpsc::Receiver<NotificationEvent>,
    bridge: MacosBridge,
    throttle_window_secs: u64,
) -> JoinHandle<()> {
    tokio::spawn(batch_aggregator::run(rx, bridge, throttle_window_secs))
}
