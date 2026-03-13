//! Context loading: reads observations, recent context, project/skill/subagent data from disk or shared state.

use std::path::Path;

use crate::error::ResiduumError;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::subagents::SubagentPresetIndex;

/// Load and format observations from the observation log JSON file.
///
/// Returns the formatted observation text, or `None` if the file is missing or empty.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub(crate) async fn load_observations(path: &Path) -> Result<Option<String>, ResiduumError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) if !content.trim().is_empty() => {
            let log: crate::memory::types::ObservationLog = serde_json::from_str(&content)
                .map_err(|e| {
                    ResiduumError::Memory(format!(
                        "failed to parse observations at {}: {e}",
                        path.display()
                    ))
                })?;
            let formatted = log.display_formatted();
            if formatted.is_empty() {
                Ok(None)
            } else {
                Ok(Some(formatted))
            }
        }
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ResiduumError::Memory(format!(
            "failed to read observations at {}: {e}",
            path.display()
        ))),
    }
}

/// Load the narrative context from the `recent_context.json` file.
///
/// Returns the narrative string, or `None` if the file is missing or empty.
///
/// # Errors
/// Returns an error if the file exists but cannot be parsed.
pub(crate) async fn load_recent_context_narrative(
    path: &Path,
) -> Result<Option<String>, ResiduumError> {
    Ok(crate::memory::recent_context::load_recent_context(path)
        .await?
        .map(|ctx| ctx.narrative))
}

/// Build formatted strings for project context from shared project state.
///
/// Returns `(index_text, active_context_text)` — each `Option<String>`.
pub(crate) async fn build_project_context_strings(
    project_state: &SharedProjectState,
) -> (Option<String>, Option<String>) {
    let state = project_state.lock().await;
    let index_text = Some(state.format_index_for_prompt());
    let active_text = state.format_active_context_for_prompt();
    (index_text, active_text)
}

/// Build formatted strings for skills context from shared skill state.
///
/// Returns `(index_text, active_instructions_text)` — each `Option<String>`.
pub(crate) async fn build_skill_context_strings(
    skill_state: &SharedSkillState,
) -> (Option<String>, Option<String>) {
    let state = skill_state.lock().await;
    let index_text = {
        let formatted = state.format_index_for_prompt();
        if formatted.is_empty() {
            None
        } else {
            Some(formatted)
        }
    };
    let active_text = state.format_active_for_prompt();
    (index_text, active_text)
}

/// Scan the subagents directory and format the index for the system prompt.
///
/// Returns `None` if the index is empty (shouldn't happen — built-in presets are always present).
pub(crate) async fn build_subagents_context_string(subagents_dir: &Path) -> Option<String> {
    match SubagentPresetIndex::scan(subagents_dir).await {
        Ok(index) => {
            let formatted = index.format_for_prompt();
            if formatted.is_empty() {
                None
            } else {
                Some(formatted)
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to scan subagent presets");
            None
        }
    }
}
