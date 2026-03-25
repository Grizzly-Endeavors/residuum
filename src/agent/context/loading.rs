//! Context loading: reads observations, recent context, project/skill/subagent data from disk or shared state.

use std::path::Path;

use anyhow::Context;

use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::subagents::SubagentPresetIndex;

/// Load and format observations from the observation log JSON file.
///
/// Returns the formatted observation text, or `None` if the file is missing or empty.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
#[tracing::instrument(skip_all, fields(path = %path.display()))]
pub(crate) async fn load_observations(path: &Path) -> anyhow::Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) if !content.trim().is_empty() => {
            let log: crate::memory::types::ObservationLog = serde_json::from_str(&content)
                .with_context(|| {
                    format!(
                        "corrupt observation log on disk at {} \
                         (a .json.bak backup may exist alongside it with a valid prior version)",
                        path.display()
                    )
                })?;
            let formatted = log.display_formatted();
            if formatted.is_empty() {
                Ok(None)
            } else {
                tracing::debug!(len = formatted.len(), "loaded observations");
                Ok(Some(formatted))
            }
        }
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context(format!("failed to read observations at {}", path.display())),
    }
}

/// Load the narrative context from the `recent_context.json` file.
///
/// Returns the narrative string, or `None` if the file is missing or empty.
///
/// # Errors
/// Returns an error if the file exists but cannot be parsed.
#[tracing::instrument(skip_all, fields(path = %path.display()))]
pub(crate) async fn load_recent_context_narrative(path: &Path) -> anyhow::Result<Option<String>> {
    let result = crate::memory::recent_context::load_recent_context(path)
        .await?
        .map(|ctx| ctx.narrative);
    if let Some(narrative) = &result {
        tracing::debug!(len = narrative.len(), "loaded recent context narrative");
    }
    Ok(result)
}

/// Build formatted strings for project context from shared project state.
///
/// Returns `(index_text, active_context_text)` — each `Option<String>`.
pub(crate) async fn build_project_context_strings(
    project_state: &SharedProjectState,
) -> (Option<String>, Option<String>) {
    let state = project_state.lock().await;
    let formatted = state.format_index_for_prompt();
    let index_text = (!formatted.is_empty()).then_some(formatted);
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
    let formatted = state.format_index_for_prompt();
    let index_text = (!formatted.is_empty()).then_some(formatted);
    let active_text = state.format_active_for_prompt();
    (index_text, active_text)
}

/// Scan the subagents directory and format the index for the system prompt.
///
/// Returns `None` if the formatted index is empty (the scan succeeded but produced no output),
/// or if the scan itself fails (logged at `warn` level).
#[tracing::instrument(skip_all, fields(dir = %subagents_dir.display()))]
pub(crate) async fn build_subagents_context_string(subagents_dir: &Path) -> Option<String> {
    match SubagentPresetIndex::scan(subagents_dir).await {
        Ok(index) => {
            let formatted = index.format_for_prompt();
            (!formatted.is_empty()).then_some(formatted)
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to scan subagent presets");
            None
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_observations_not_found_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = load_observations(&path).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn load_observations_empty_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observations.json");
        tokio::fs::write(&path, "   ").await.unwrap();
        let result = load_observations(&path).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn load_observations_corrupt_json_error_mentions_bak() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observations.json");
        tokio::fs::write(&path, "not valid json").await.unwrap();
        let result = load_observations(&path).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains(".json.bak"),
            "error should mention .json.bak backup, got: {err}"
        );
    }

    #[tokio::test]
    async fn load_observations_valid_file_returns_some() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observations.json");
        let json = r#"{"observations":[{"timestamp":"2024-02-19T00:00","project_context":"test","visibility":"user","content":"test observation"}]}"#;
        tokio::fs::write(&path, json).await.unwrap();
        let result = load_observations(&path).await.unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().contains("test observation"));
    }
}
