use std::path::Path;

use crate::agent::Agent;
use crate::agent::context::ProjectsContext;
use crate::channels::null::NullDisplay;
use crate::error::IronclawError;
use crate::models::{Message, ModelProvider};

use super::alerts::load_alerts;
use super::types::{AlertLevel, PulseDef};

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
    /// The highest alert level across pulse tasks (meaningful only when `!is_heartbeat_ok`).
    pub alert_level: AlertLevel,
}

/// Execute a pulse check using the given agent.
///
/// Builds a prompt from the pulse tasks and optional Alerts.md content,
/// runs `agent.run_system_turn`, and handles alert delivery for the CLI.
///
/// If `provider_override` is `Some`, that provider is used instead of the
/// agent's default for this turn.
///
/// # Errors
///
/// Returns `IronclawError` if loading alerts or running the agent turn fails.
pub async fn execute_pulse(
    pulse: &PulseDef,
    agent: &Agent,
    alerts_path: &Path,
    provider_override: Option<&dyn ModelProvider>,
    projects_ctx: &ProjectsContext<'_>,
) -> Result<PulseResult, IronclawError> {
    let alerts_content = load_alerts(alerts_path).await?;

    let mut parts = Vec::new();
    parts.push(format!(
        "You are running a scheduled pulse check: {}",
        pulse.name
    ));

    if let Some(ref alerts) = alerts_content {
        parts.push(alerts.clone());
    }

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
        .run_system_turn(&prompt, &display, provider_override, projects_ctx)
        .await?;

    let is_heartbeat_ok = result.response.contains("HEARTBEAT_OK");

    // Compute the highest alert level across tasks
    let alert_level =
        pulse
            .tasks
            .iter()
            .fold(AlertLevel::Low, |acc, task| match (acc, task.alert) {
                (AlertLevel::High | AlertLevel::Medium | AlertLevel::Low, AlertLevel::High)
                | (AlertLevel::High, AlertLevel::Medium | AlertLevel::Low) => AlertLevel::High,
                (AlertLevel::Medium | AlertLevel::Low, AlertLevel::Medium)
                | (AlertLevel::Medium, AlertLevel::Low) => AlertLevel::Medium,
                (AlertLevel::Low, AlertLevel::Low) => AlertLevel::Low,
            });

    if is_heartbeat_ok {
        tracing::info!(pulse = %pulse.name, "pulse check: HEARTBEAT_OK");
    } else if matches!(alert_level, AlertLevel::Low) {
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
        alert_level,
    })
}
