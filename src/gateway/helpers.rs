//! Gateway-specific helpers: workspace naming and project context labels.

use crate::projects::activation::SharedProjectState;
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
