//! Write-scoping policy for file tools.
//!
//! Enforces that writes to `projects/` land inside the active project directory,
//! and that `archive/` is always read-only. Workspace-level writes (memory/,
//! MEMORY.md, daily logs, etc.) are unrestricted.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;

/// Shared path policy, checked by `WriteTool` and `EditTool` before every write.
pub type SharedPathPolicy = Arc<RwLock<PathPolicy>>;

/// Write-scoping policy based on workspace layout and active project.
pub struct PathPolicy {
    /// Root of the workspace (contains `projects/`, `archive/`, `memory/`, etc.).
    workspace_root: PathBuf,
    /// Root of the currently active project (e.g. `{workspace}/projects/my-proj`).
    /// `None` when no project is active.
    active_project_root: Option<PathBuf>,
}

impl PathPolicy {
    /// Create a new path policy for the given workspace root.
    #[must_use]
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            active_project_root: None,
        }
    }

    /// Create a new shared path policy.
    #[must_use]
    pub fn new_shared(workspace_root: PathBuf) -> SharedPathPolicy {
        Arc::new(RwLock::new(Self::new(workspace_root)))
    }

    /// Set (or clear) the active project root.
    ///
    /// Called by `ProjectActivateTool` (with `Some`) and `ProjectDeactivateTool` (with `None`).
    pub fn set_active_project(&mut self, root: Option<PathBuf>) {
        self.active_project_root = root;
    }

    /// Check whether a write to `path` is allowed under the current policy.
    ///
    /// Returns `Ok(())` if allowed, or `Err(reason)` if rejected.
    ///
    /// # Errors
    /// Returns a descriptive error string if the write is rejected.
    pub fn check_write(&self, path: &Path) -> Result<(), String> {
        let canonical = canonicalize_for_check(path);

        let projects_dir = self.workspace_root.join("projects");
        let archive_dir = self.workspace_root.join("archive");

        // Rule 1: archive/ is always read-only
        if canonical.starts_with(&archive_dir) {
            return Err(
                "writes to archive/ are not allowed — archived projects are read-only".to_string(),
            );
        }

        // Rule 2: writes inside projects/ must target the active project
        if canonical.starts_with(&projects_dir) {
            return match &self.active_project_root {
                Some(active_root) => {
                    if canonical.starts_with(active_root) {
                        Ok(())
                    } else {
                        Err(format!(
                            "write rejected — path is in projects/ but outside the active project ({})",
                            active_root.display()
                        ))
                    }
                }
                None => Err(
                    "write rejected — path is in projects/ but no project is active".to_string(),
                ),
            };
        }

        // Rule 3: workspace-level writes are unrestricted
        Ok(())
    }
}

/// Canonicalize a path for policy checks.
///
/// For existing paths, uses `std::fs::canonicalize`. For new files (path doesn't
/// exist yet), canonicalizes the nearest existing ancestor and appends the
/// remaining segments.
fn canonicalize_for_check(path: &Path) -> PathBuf {
    // Try full canonicalization first
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }

    // Walk up to find the nearest existing ancestor
    let mut existing = path.to_path_buf();
    let mut remaining = Vec::new();

    while !existing.exists() {
        if let Some(file_name) = existing.file_name() {
            remaining.push(file_name.to_os_string());
        } else {
            // Can't walk further up; return the original path as-is
            return path.to_path_buf();
        }
        if !existing.pop() {
            return path.to_path_buf();
        }
    }

    // Canonicalize the existing ancestor and re-append the missing segments
    let mut canonical = std::fs::canonicalize(&existing).unwrap_or(existing);
    for segment in remaining.into_iter().rev() {
        canonical.push(segment);
    }
    canonical
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    fn make_workspace() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("workspace");
        std::fs::create_dir_all(ws.join("projects/project-a")).unwrap();
        std::fs::create_dir_all(ws.join("projects/project-b")).unwrap();
        std::fs::create_dir_all(ws.join("archive/old-project")).unwrap();
        std::fs::create_dir_all(ws.join("memory")).unwrap();
        let canonical_ws = std::fs::canonicalize(&ws).unwrap();
        (dir, canonical_ws)
    }

    #[test]
    fn workspace_level_writes_always_allowed() {
        let (_dir, ws) = make_workspace();
        let policy = PathPolicy::new(ws.clone());

        assert!(
            policy.check_write(&ws.join("memory/notes.md")).is_ok(),
            "memory writes should be allowed"
        );
        assert!(
            policy.check_write(&ws.join("MEMORY.md")).is_ok(),
            "MEMORY.md writes should be allowed"
        );
    }

    #[test]
    fn archive_always_rejected() {
        let (_dir, ws) = make_workspace();
        let policy = PathPolicy::new(ws.clone());

        let result = policy.check_write(&ws.join("archive/old-project/notes.md"));
        assert!(result.is_err(), "archive writes should be rejected");
        assert!(
            result.unwrap_err().contains("archive"),
            "error should mention archive"
        );
    }

    #[test]
    fn projects_rejected_without_active() {
        let (_dir, ws) = make_workspace();
        let policy = PathPolicy::new(ws.clone());

        let result = policy.check_write(&ws.join("projects/project-a/file.md"));
        assert!(result.is_err(), "project write without active should fail");
        assert!(
            result.unwrap_err().contains("no project is active"),
            "error should mention no active project"
        );
    }

    #[test]
    fn projects_allowed_for_active_project() {
        let (_dir, ws) = make_workspace();
        let mut policy = PathPolicy::new(ws.clone());
        policy.set_active_project(Some(ws.join("projects/project-a")));

        assert!(
            policy
                .check_write(&ws.join("projects/project-a/notes/file.md"))
                .is_ok(),
            "write inside active project should be allowed"
        );
    }

    #[test]
    fn projects_rejected_for_wrong_project() {
        let (_dir, ws) = make_workspace();
        let mut policy = PathPolicy::new(ws.clone());
        policy.set_active_project(Some(ws.join("projects/project-a")));

        let result = policy.check_write(&ws.join("projects/project-b/file.md"));
        assert!(
            result.is_err(),
            "write to inactive project should be rejected"
        );
        assert!(
            result.unwrap_err().contains("outside the active project"),
            "error should mention outside active project"
        );
    }

    #[test]
    fn set_active_project_clears() {
        let (_dir, ws) = make_workspace();
        let mut policy = PathPolicy::new(ws.clone());

        policy.set_active_project(Some(ws.join("projects/project-a")));
        assert!(
            policy
                .check_write(&ws.join("projects/project-a/file.md"))
                .is_ok(),
            "should be allowed while active"
        );

        policy.set_active_project(None);
        assert!(
            policy
                .check_write(&ws.join("projects/project-a/file.md"))
                .is_err(),
            "should be rejected after clearing"
        );
    }

    #[test]
    fn new_file_in_active_project_allowed() {
        let (_dir, ws) = make_workspace();
        let mut policy = PathPolicy::new(ws.clone());
        policy.set_active_project(Some(ws.join("projects/project-a")));

        // File doesn't exist yet but parent does
        assert!(
            policy
                .check_write(&ws.join("projects/project-a/new-file.md"))
                .is_ok(),
            "new file in active project should be allowed"
        );
    }

    #[test]
    fn new_file_in_new_subdir_of_active_project_allowed() {
        let (_dir, ws) = make_workspace();
        let mut policy = PathPolicy::new(ws.clone());
        policy.set_active_project(Some(ws.join("projects/project-a")));

        // Neither the file nor the subdir exist yet
        assert!(
            policy
                .check_write(&ws.join("projects/project-a/new-dir/new-file.md"))
                .is_ok(),
            "new file in new subdir of active project should be allowed"
        );
    }
}
