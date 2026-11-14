//! The agent's view of the meeting rooms it participates in, as the prompt needs it.

/// The `ROOMS` block the agent reads into its prompt at every turn entry.
///
/// The background room manager implements it; the agent never sees the store
/// or the room sessions behind it.
pub trait RoomRoster: Send + Sync {
    /// The prompt's `ROOMS` block, or `None` when there are no rooms to show.
    fn prompt_block(&self) -> Option<String>;
}
