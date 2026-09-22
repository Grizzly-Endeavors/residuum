//! Interrupt channel types for mid-turn message injection.

use tokio::sync::mpsc;

use crate::bus::AgentMessageEvent;
use crate::interfaces::types::InboundMessage;

/// An interrupt that can be injected into an in-progress agent turn.
#[derive(Clone)]
pub enum Interrupt {
    /// A user message arrived while the agent was processing a turn.
    UserMessage(InboundMessage),
    /// A message from another agent (main or a session), addressed to this
    /// agent by the `message_agent` tool, arrived while a turn was running.
    /// Drained at the tool loop's next checkpoint, the same way a user
    /// message is. This is also the delivery path a session's runtime uses
    /// to wake it while idle (see `crate::background::messaging`).
    AgentMessage(AgentMessageEvent),
    /// The subconscious classifier found a course correction to inject.
    Subconscious(String),
    /// The user asked to stop the current turn.
    ///
    /// Observed at the tool loop's checkpoint (between iterations) so a stop
    /// that lands while a tool is running still lets that tool finish before
    /// the turn ends. An in-flight model call is cancelled separately and
    /// immediately via the turn's `CancellationToken` — this variant only
    /// carries the "record that it happened" half of a stop.
    Stopped,
}

/// Create a dead-end receiver that will never receive any messages.
///
/// Used by system turns and tests that don't participate in interrupts.
#[must_use]
pub fn dead_interrupt_rx() -> mpsc::Receiver<Interrupt> {
    let (_tx, rx) = mpsc::channel::<Interrupt>(1);
    rx
}
