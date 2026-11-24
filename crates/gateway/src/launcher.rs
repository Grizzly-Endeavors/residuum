use async_trait::async_trait;

use residuum_core::util::FatalError;
use residuum_hub::launch::{AgentLauncher, AgentStartInputs, StartedAgent};

#[derive(Debug, Default, Clone, Copy)]
pub struct GatewayLauncher;

impl GatewayLauncher {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AgentLauncher for GatewayLauncher {
    async fn start(&self, inputs: AgentStartInputs) -> Result<StartedAgent, FatalError> {
        Box::pin(super::event_loop::start_agent(inputs)).await
    }
}
