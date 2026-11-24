use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::bridge::WindowsBridge;
use crate::batch_aggregator;
use residuum_bus::NotificationEvent;

#[must_use]
pub fn spawn(
    rx: mpsc::Receiver<NotificationEvent>,
    bridge: WindowsBridge,
    throttle_window_secs: u64,
) -> JoinHandle<()> {
    residuum_core::util::spawn_in_span(batch_aggregator::run(rx, bridge, throttle_window_secs))
}
