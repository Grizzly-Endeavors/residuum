//! Interrupt channel types for mid-turn message injection.

use tokio::sync::mpsc;

use crate::bus::AgentMessageEvent;
use crate::bus::MessageEvent;

/// An interrupt that can be injected into an in-progress agent turn.
#[derive(Clone)]
pub enum Interrupt {
    /// A user message arrived while the agent was processing a turn.
    UserMessage(MessageEvent),
    /// A message from another agent (main or a session), addressed to this
    /// agent by the `message_agent` tool, arrived while a turn was running.
    /// Drained at the tool loop's next checkpoint, the same way a user
    /// message is. This is also the delivery path a session's runtime uses
    /// to wake it while idle (the background messaging module).
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

/// A source of queued [`Interrupt`]s that the turn loop can drain at a
/// checkpoint, abstracting over the two channel shapes in use:
///
/// - A session's interrupt channel is deliberately bounded: a stuck session
///   should report itself busy (`DeliverOutcome::Full`) rather than accept
///   unbounded backlog.
/// - The main agent's interrupt channel is unbounded: a mid-turn user message
///   must never be silently dropped for want of queue space, since there is
///   no "busy" signal to give the user back — the message would just vanish.
///
/// The agent's turn loop only ever drains what's already queued, so this
/// needs nothing beyond a non-blocking receive.
pub trait InterruptSource: Send {
    /// Non-blocking: take the next already-queued interrupt, if any.
    ///
    /// # Errors
    ///
    /// Returns `TryRecvError::Empty` when nothing is queued, and
    /// `TryRecvError::Disconnected` when nothing more ever will be.
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
