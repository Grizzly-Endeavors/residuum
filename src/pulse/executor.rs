use crate::agent::Agent;
use crate::agent::context::{ProjectsContext, SkillsContext};
use crate::channels::null::NullDisplay;
use crate::error::IronclawError;
use crate::models::{Message, ModelProvider};

use super::types::PulseDef;

/// Result of executing a pulse check.
pub struct PulseResult {
    /// Name of the pulse that was executed.
    pub pulse_name: String,
    /// The agent's text response.
    pub response: String,
    /// All ephemeral messages from the agent turn (for memory pipeline).
    pub messages: Vec<Message>,
    /// True if the response contains the `HEARTBEAT_OK` sentinel.
    pub is_heartbeat_ok: bool,
}

/// Execute a pulse check using the given agent.
///
/// Builds a prompt from the pulse tasks, runs `agent.run_system_turn`,
/// and logs the result.
///
/// If `provider_override` is `Some`, that provider is used instead of the
/// agent's default for this turn.
///
/// # Errors
///
/// Returns `IronclawError` if running the agent turn fails.
pub async fn execute_pulse(
    pulse: &PulseDef,
    agent: &Agent,
    provider_override: Option<&dyn ModelProvider>,
    projects_ctx: &ProjectsContext<'_>,
) -> Result<PulseResult, IronclawError> {
    let mut parts = Vec::new();
    parts.push(format!(
        "You are running a scheduled pulse check: {}",
        pulse.name
    ));

    parts.push("## Tasks".to_string());

    for task in &pulse.tasks {
        parts.push(format!("### {}\n{}", task.name, task.prompt));
    }

    parts.push(
        "Complete all tasks above. If nothing noteworthy was found across all tasks, \
         respond with exactly: HEARTBEAT_OK"
            .to_string(),
    );

    let prompt = parts.join("\n\n");

    let display = NullDisplay;
    let result = agent
        .run_system_turn(
            &prompt,
            &display,
            provider_override,
            projects_ctx,
            &SkillsContext::none(),
        )
        .await?;

    let is_heartbeat_ok = result.response.contains("HEARTBEAT_OK");

    if is_heartbeat_ok {
        tracing::info!(pulse = %pulse.name, "pulse check: HEARTBEAT_OK");
    } else {
        tracing::info!(
            pulse = %pulse.name,
            response = %result.response,
            "pulse finding"
        );
    }

    Ok(PulseResult {
        pulse_name: pulse.name.clone(),
        response: result.response,
        messages: result.messages,
        is_heartbeat_ok,
    })
}
