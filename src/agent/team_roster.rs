//! The agent's view of its team, as the prompt needs it.

/// The `TEAM` block the agent reads into its prompt at every turn entry.
///
/// The background team implements it; the agent never sees the router or
/// the directory behind it.
pub trait TeamRoster: Send + Sync {
    /// The prompt's `TEAM` block, or `None` when there is no team to show.
    fn prompt_block(&self) -> Option<String>;
}
