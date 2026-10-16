//! Test helpers for code that drains bus interrupts.

use tokio::sync::mpsc;

use super::interrupt::Interrupt;

/// Create a dead-end receiver that will never receive any messages.
///
/// For tests that run a turn or a session without an interrupt source of its
/// own. Unbounded, matching the main agent's own channel shape: nothing is
/// ever sent through it, so a session test using it in place of its own
/// bounded channel still works, since `InterruptSource` is implemented for
/// both channel shapes.
#[must_use]
pub(crate) fn dead_interrupt_rx() -> mpsc::UnboundedReceiver<Interrupt> {
    let (_tx, rx) = mpsc::unbounded_channel::<Interrupt>();
    rx
}
