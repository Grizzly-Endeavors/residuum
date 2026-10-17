//! The factory that builds each session run's tool registry.
//!
//! Background decides when a session forks and what it is scoped to; which
//! tools it gets, and the shared handles they hold, belong to whoever
//! implements [`SessionToolFactory`] (the gateway hands background
//! `tools::SessionTools`).

use crate::agent::{HopCounter, ToolRegistry};
use crate::bus::session::SessionCategory;
use crate::bus::{ConversationTarget, EventTrigger, SessionAddress};
use crate::skills::SharedSkillState;

/// What one session run's tools are scoped to; everything else they need is
/// the factory's.
pub struct SessionToolScope {
    /// This session's own address, recorded as the spawner on any session it
    /// forks in turn and reported as the sender of its `message_agent` calls.
    pub own_address: SessionAddress,
    /// This session's own depth from the main agent (main = 0).
    pub own_depth: u32,
    /// This session's category, reported alongside `own_address` by
    /// `message_agent`.
    pub category: SessionCategory,
    /// What triggered this session.
    pub trigger: EventTrigger,
    /// The conversation this session replies to, for a conversation-triggered
    /// session; `None` for every other trigger. Its endpoint gates
    /// `a2a_task_update`.
    pub conversation_target: Option<ConversationTarget>,
    /// This session's current-turn hop counter, shared with the tools that
    /// compute outgoing hop counts so they read the value the runtime updates.
    pub hop_counter: HopCounter,
    /// The session's own skill state, prepared from main's by
    /// `prepare_session_skill_state`.
    pub skill_state: SharedSkillState,
}

/// Builds the tool registry of each session run.
pub trait SessionToolFactory: Send + Sync {
    /// A fresh registry for one session run scoped to `scope`.
    fn build(&self, scope: SessionToolScope) -> ToolRegistry;
}
