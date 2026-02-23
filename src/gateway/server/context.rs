//! Context-building helpers: project index strings, skill strings, and context labels.

use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

/// Derive the workspace name from the root directory for use as project context.
pub(super) fn workspace_name(layout: &WorkspaceLayout) -> String {
    layout
        .root()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Build formatted strings for project context from shared project state.
///
/// Returns `(index_text, active_context_text)` — each `Option<String>`.
pub(super) async fn build_project_context_strings(
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
pub(super) async fn build_skill_context_strings(
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

/// Derive the project context label for memory tagging.
///
/// Uses the active project name if one is active, otherwise falls back to the
/// workspace directory name.
pub(super) async fn project_context_label(
    project_state: &SharedProjectState,
    layout: &WorkspaceLayout,
) -> String {
    let state = project_state.lock().await;
    state
        .active_project_name()
        .map_or_else(|| workspace_name(layout), str::to_string)
}
