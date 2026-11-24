use residuum_agent::{HopCounter, ToolRegistry};
use residuum_bus::session::SessionCategory;
use residuum_bus::{ConversationTarget, EventTrigger, SessionAddress};
use residuum_skills::SharedSkillState;

pub struct SessionToolScope {
    pub own_address: SessionAddress,
    pub own_depth: u32,
    pub category: SessionCategory,
    pub trigger: EventTrigger,
    pub conversation_target: Option<ConversationTarget>,
    pub hop_counter: HopCounter,
    pub skill_state: SharedSkillState,
}

pub trait SessionToolFactory: Send + Sync {
    fn build(&self, scope: SessionToolScope) -> ToolRegistry;
}
