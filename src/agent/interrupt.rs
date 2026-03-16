//! Interrupt channel types for mid-turn message injection.

use tokio::sync::mpsc;

use crate::bus::AgentResultEvent;
use crate::interfaces::types::InboundMessage;

/// An interrupt that can be injected into an in-progress agent turn.
#[derive(Clone)]
pub enum Interrupt {
    /// A user message arrived while the agent was processing a turn.
    UserMessage(InboundMessage),
    /// A background task completed and its result should be injected.
    BackgroundResult(AgentResultEvent),
}

/// Create a dead-end receiver that will never receive any messages.
///
/// Used by system turns and tests that don't participate in interrupts.
#[must_use]
pub fn dead_interrupt_rx() -> mpsc::Receiver<Interrupt> {
    let (_tx, rx) = mpsc::channel::<Interrupt>(1);
    rx
}
