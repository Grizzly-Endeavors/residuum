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
/// Unbounded, matching the main agent's own channel shape — nothing is ever
/// sent through it, so a session test using it in place of its own bounded
/// channel still works, since [`InterruptSource`] is implemented for both.
#[must_use]
pub fn dead_interrupt_rx() -> mpsc::UnboundedReceiver<Interrupt> {
    let (_tx, rx) = mpsc::unbounded_channel::<Interrupt>();
    rx
}

/// A source of queued [`Interrupt`]s that the turn loop can drain at a
/// checkpoint, abstracting over the two channel shapes in use:
///
/// - A session's interrupt channel (`crate::background::registry`) is
///   deliberately bounded: a stuck session should report itself busy
///   (`DeliverOutcome::Full`) rather than accept unbounded backlog.
/// - The main agent's interrupt channel (`crate::gateway::event_loop::turns`)
///   is unbounded: a mid-turn user message must never be silently dropped
///   for want of queue space, since there is no "busy" signal to give the
///   user back — the message would just vanish.
///
/// The turn loop (`crate::agent::turn::execute_turn`) only ever drains
/// what's already queued, so this needs nothing beyond a non-blocking
/// receive.
pub(crate) trait InterruptSource: Send {
    /// Non-blocking: take the next already-queued interrupt, if any.
    fn try_recv(&mut self) -> Result<Interrupt, mpsc::error::TryRecvError>;
}

impl InterruptSource for mpsc::Receiver<Interrupt> {
    fn try_recv(&mut self) -> Result<Interrupt, mpsc::error::TryRecvError> {
        mpsc::Receiver::try_recv(self)
    }
}

impl InterruptSource for mpsc::UnboundedReceiver<Interrupt> {
    fn try_recv(&mut self) -> Result<Interrupt, mpsc::error::TryRecvError> {
        mpsc::UnboundedReceiver::try_recv(self)
    }
}
