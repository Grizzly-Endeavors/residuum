//! The gateway's [`AgentLauncher`]: how the hub starts an agent's runtime.

use async_trait::async_trait;

use residuum_core::util::FatalError;
use residuum_hub::launch::{AgentLauncher, AgentStartInputs, StartedAgent};

/// Starts each agent on the gateway's runtime: its adapters, watchers, A2A
/// serving state and event loop.
#[derive(Debug, Default, Clone, Copy)]
pub struct GatewayLauncher;

impl GatewayLauncher {
    /// The launcher the hub process runs its agents with.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AgentLauncher for GatewayLauncher {
    async fn start(&self, inputs: AgentStartInputs) -> Result<StartedAgent, FatalError> {
        // Box::pin keeps the start-up future, which is large, off the
        // caller's stack.
        Box::pin(super::event_loop::start_agent(inputs)).await
    }
}
