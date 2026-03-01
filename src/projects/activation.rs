//! Project activation state management.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDateTime;

use crate::error::ResiduumError;
use crate::workspace::layout::WorkspaceLayout;

use super::manifest::{build_manifest, format_manifest};
use super::scanner::{ProjectIndex, parse_project_md};
use super::types::{ActiveProject, ProjectStatus};

/// Shared project state, following the `ActionStore` pattern.
pub type SharedProjectState = Arc<tokio::sync::Mutex<ProjectState>>;

/// Project state manager: holds the index and optional active project.
pub struct ProjectState {
    index: ProjectIndex,
    active: Option<ActiveProject>,
    layout: WorkspaceLayout,
}

impl ProjectState {
    /// Create a new project state with a pre-built index.
    #[must_use]
    pub fn new(index: ProjectIndex, layout: WorkspaceLayout) -> Self {
        Self {
            index,
            active: None,
            layout,
        }
    }

    /// Create a new shared project state.
    #[must_use]
    pub fn new_shared(index: ProjectIndex, layout: WorkspaceLayout) -> SharedProjectState {
        Arc::new(tokio::sync::Mutex::new(Self::new(index, layout)))
    }

    /// Activate a project by name.
    ///
    /// Looks up the project in the index, reads the full `PROJECT.md`, builds
    /// the manifest, and stores it as the active project.
    ///
    /// # Errors
    /// Returns `ResiduumError::Projects` if the project is not found, cannot
    /// be read, or is archived.
    pub async fn activate(&mut self, name: &str) -> Result<&ActiveProject, ResiduumError> {
        let entry = self
            .index
            .find_by_name(name)
            .ok_or_else(|| ResiduumError::Projects(format!("project '{name}' not found")))?;

        if entry.status == ProjectStatus::Archived {
            return Err(ResiduumError::Projects(format!(
                "project '{}' is archived and cannot be activated",
                entry.name
            )));
        }

        let dir_name = entry.dir_name.clone();
        let project_root = self.layout.projects_dir().join(&dir_name);

        let content = tokio::fs::read_to_string(project_root.join("PROJECT.md"))
            .await
            .map_err(|e| {
                ResiduumError::Projects(format!(
                    "failed to read PROJECT.md for '{}': {e}",
                    entry.name
                ))
            })?;

        let (frontmatter, body) = parse_project_md(&content)?;
        let manifest = build_manifest(&project_root).await?;

        let recent_log = read_recent_logs(&project_root).await;

        let active = ActiveProject {
            name: frontmatter.name.clone(),
            dir_name,
            frontmatter,
            body,
            recent_log,
            manifest,
            project_root,
        };

        self.active = Some(active);

        // Safe: we just set it to Some
        self.active.as_ref().ok_or_else(|| {
            ResiduumError::Projects("unexpected: active project not set after activation".into())
        })
    }

    /// Deactivate the current project, writing a log entry.
    ///
    /// Rejects empty log entries. Writes the log to `notes/log/YYYY-MM/log-DD.md`.
    ///
    /// # Errors
    /// Returns `ResiduumError::Projects` if no project is active, the log is empty,
    /// or the log file cannot be written.
    pub async fn deactivate(
        &mut self,
        log_entry: &str,
        now: NaiveDateTime,
    ) -> Result<String, ResiduumError> {
        let trimmed = log_entry.trim();
        if trimmed.is_empty() {
            return Err(ResiduumError::Projects(
                "deactivation requires a non-empty log entry".to_string(),
            ));
        }

        let active = self
            .active
            .as_ref()
            .ok_or_else(|| ResiduumError::Projects("no project is currently active".to_string()))?;

        let name = active.name.clone();
        write_deactivation_log(&active.project_root, trimmed, now).await?;

        self.active = None;

        Ok(name)
    }

    /// Rescan the project directories to rebuild the index.
    ///
    /// # Errors
    /// Returns `ResiduumError::Projects` if scanning fails.
    pub async fn rescan(&mut self) -> Result<(), ResiduumError> {
        self.index = ProjectIndex::scan(&self.layout).await?;
        Ok(())
    }

    /// Format the project index for the system prompt.
    #[must_use]
    pub fn format_index_for_prompt(&self) -> String {
        self.index.format_for_prompt()
    }

    /// Format the active project context for the system prompt.
    ///
    /// Returns `None` if no project is active.
    #[must_use]
    pub fn format_active_context_for_prompt(&self) -> Option<String> {
        let active = self.active.as_ref()?;
        let manifest_text = format_manifest(&active.manifest);

        let mut parts = Vec::new();
        parts.push(format!("**Project:** {}", active.name));

        if !active.body.is_empty() {
            parts.push(active.body.clone());
        }

        if let Some(log) = &active.recent_log {
            parts.push(format!("**Recent Session Log:**\n{log}"));
        }

        parts.push(format!("\n**Files:**\n{manifest_text}"));

        Some(parts.join("\n\n"))
    }

    /// Get the name of the active project, if any.
    #[must_use]
    pub fn active_project_name(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.name.as_str())
    }

    /// Get a reference to the current project index.
    #[must_use]
    pub fn index(&self) -> &ProjectIndex {
        &self.index
    }

    /// Get a reference to the active project, if any.
    #[must_use]
    pub fn active(&self) -> Option<&ActiveProject> {
        self.active.as_ref()
    }

    /// Get the project root path for a given `dir_name` (in projects/).
    #[must_use]
    pub fn project_root(&self, dir_name: &str) -> PathBuf {
        self.layout.projects_dir().join(dir_name)
    }

    /// Get a reference to the workspace layout.
    #[must_use]
    pub fn layout(&self) -> &WorkspaceLayout {
        &self.layout
    }
}

/// Write a deactivation log entry to the project's notes/log directory.
///
/// Format: `notes/log/YYYY-MM/log-DD.md`
/// Appends to existing file if it exists for that day.
async fn write_deactivation_log(
    project_root: &Path,
    log_text: &str,
    now: NaiveDateTime,
) -> Result<(), ResiduumError> {
    let date_dir = now.format("%Y-%m").to_string();
    let day_file = now.format("log-%d").to_string();
    let date_header = now.format("%Y-%m-%d").to_string();
    let time_str = now.format("%H:%M").to_string();

    let log_dir = project_root.join("notes/log").join(&date_dir);
    tokio::fs::create_dir_all(&log_dir).await.map_err(|e| {
        ResiduumError::Projects(format!(
            "failed to create log directory {}: {e}",
            log_dir.display()
        ))
    })?;

    let log_file = log_dir.join(format!("{day_file}.md"));
    let entry = format!("- **{time_str}** {log_text}\n");

    let content = match tokio::fs::read_to_string(&log_file).await {
        Ok(existing) if !existing.is_empty() => {
            // Append to existing file
            format!("{existing}{entry}")
        }
        _ => {
            // New file with date header
            format!("# {date_header}\n\n{entry}")
        }
    };

    tokio::fs::write(&log_file, &content).await.map_err(|e| {
        ResiduumError::Projects(format!(
            "failed to write log file {}: {e}",
            log_file.display()
        ))
    })?;

    Ok(())
}

/// Read the most recent project session logs, returning up to ~2000 tokens
/// (~8000 chars) of content with the most recent entries first.
///
/// Scans `notes/log/` for the most recent month directory, then reads the
/// most recent day files. Returns `None` if no logs exist or all reads fail.
async fn read_recent_logs(project_root: &Path) -> Option<String> {
    const MAX_CHARS: usize = 8000;

    let log_dir = project_root.join("notes/log");

    let Ok(mut months) = tokio::fs::read_dir(&log_dir).await else {
        return None;
    };

    // Collect month directories and sort descending
    let mut month_dirs: Vec<PathBuf> = Vec::new();
    loop {
        match months.next_entry().await {
            Ok(Some(entry)) => {
                if entry.file_type().await.is_ok_and(|ft| ft.is_dir()) {
                    month_dirs.push(entry.path());
                }
            }
            Ok(None) => break,
            Err(_) => {}
        }
    }
    month_dirs.sort_unstable();
    month_dirs.reverse();

    let mut collected = String::new();

    for month_path in &month_dirs {
        if collected.len() >= MAX_CHARS {
            break;
        }

        let Ok(mut day_rd) = tokio::fs::read_dir(month_path).await else {
            continue;
        };

        let mut day_files: Vec<PathBuf> = Vec::new();
        loop {
            match day_rd.next_entry().await {
                Ok(Some(entry)) => {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "md") {
                        day_files.push(path);
                    }
                }
                Ok(None) => break,
                Err(_) => {}
            }
        }
        day_files.sort_unstable();
        day_files.reverse();

        for day_file in &day_files {
            if collected.len() >= MAX_CHARS {
                break;
            }

            if let Ok(content) = tokio::fs::read_to_string(day_file).await {
                if !collected.is_empty() {
                    collected.push('\n');
                }
                let remaining = MAX_CHARS.saturating_sub(collected.len());
                if content.len() <= remaining {
                    collected.push_str(&content);
                } else {
                    // Truncate at a char boundary
                    let truncated: String = content.chars().take(remaining).collect();
                    collected.push_str(&truncated);
                }
            }
        }
    }

    if collected.is_empty() {
        None
    } else {
        Some(collected)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::workspace::bootstrap::ensure_workspace;

    async fn setup_workspace_with_project() -> (tempfile::TempDir, WorkspaceLayout) {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None, None).await.unwrap();

        let project_dir = layout.projects_dir().join("test-proj");
        tokio::fs::create_dir_all(project_dir.join("notes"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project_dir.join("references"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project_dir.join("workspace"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project_dir.join("skills"))
            .await
            .unwrap();
        tokio::fs::write(
            project_dir.join("PROJECT.md"),
            "---\nname: Test Project\ndescription: \"A test project\"\nstatus: active\ncreated: 2026-02-20\n---\n\nOverview body text.\n",
        )
        .await
        .unwrap();

        (dir, layout)
    }

    fn make_datetime(year: i32, month: u32, day: u32, hour: u32, min: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, min, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn activate_existing_project() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout);

        let active = state.activate("Test Project").await.unwrap();
        assert_eq!(active.name, "Test Project", "name should match");
        assert!(
            active.body.contains("Overview body text"),
            "body should be loaded"
        );
        assert!(
            state.active_project_name().is_some(),
            "should have active project"
        );
    }

    #[tokio::test]
    async fn activate_nonexistent_project() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout);

        let result = state.activate("Nonexistent").await;
        assert!(result.is_err(), "should error for nonexistent project");
    }

    #[tokio::test]
    async fn deactivate_with_log() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout.clone());

        state.activate("Test Project").await.unwrap();

        let now = make_datetime(2026, 2, 23, 14, 32);
        let name = state
            .deactivate("Session summary text here.", now)
            .await
            .unwrap();
        assert_eq!(name, "Test Project", "should return project name");
        assert!(
            state.active_project_name().is_none(),
            "should have no active project"
        );

        // Verify log file was written
        let log_file = layout
            .projects_dir()
            .join("test-proj/notes/log/2026-02/log-23.md");
        let content = tokio::fs::read_to_string(&log_file).await.unwrap();
        assert!(content.contains("# 2026-02-23"), "should have date header");
        assert!(content.contains("**14:32**"), "should have time prefix");
        assert!(
            content.contains("Session summary text here."),
            "should have log text"
        );
    }

    #[tokio::test]
    async fn deactivate_empty_log_rejected() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout);

        state.activate("Test Project").await.unwrap();

        let now = make_datetime(2026, 2, 23, 14, 32);
        let result = state.deactivate("", now).await;
        assert!(result.is_err(), "empty log should be rejected");
    }

    #[tokio::test]
    async fn deactivate_no_active_project() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout);

        let now = make_datetime(2026, 2, 23, 14, 32);
        let result = state.deactivate("some log", now).await;
        assert!(result.is_err(), "should error when no project is active");
    }

    #[tokio::test]
    async fn rescan_after_changes() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout.clone());

        assert_eq!(
            state.index().entries().len(),
            1,
            "should start with one project"
        );

        // Add another project
        let new_dir = layout.projects_dir().join("new-proj");
        tokio::fs::create_dir(&new_dir).await.unwrap();
        tokio::fs::write(
            new_dir.join("PROJECT.md"),
            "---\nname: New Project\ndescription: \"New\"\nstatus: active\ncreated: 2026-02-23\n---\n",
        )
        .await
        .unwrap();

        state.rescan().await.unwrap();
        assert_eq!(
            state.index().entries().len(),
            2,
            "should find two projects after rescan"
        );
    }

    #[tokio::test]
    async fn format_active_context() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout);

        assert!(
            state.format_active_context_for_prompt().is_none(),
            "should be None when no project is active"
        );

        state.activate("Test Project").await.unwrap();
        let ctx = state.format_active_context_for_prompt().unwrap();
        assert!(
            ctx.contains("Test Project"),
            "context should contain project name"
        );
        assert!(
            ctx.contains("Overview body text"),
            "context should contain body"
        );
    }

    #[tokio::test]
    async fn recent_log_loaded_on_activation() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout.clone());

        // First activate + deactivate to create a log file
        state.activate("Test Project").await.unwrap();
        let now = make_datetime(2026, 2, 25, 10, 0);
        state.deactivate("Set up CI pipeline.", now).await.unwrap();

        // Rescan and re-activate — recent_log should be populated
        state.rescan().await.unwrap();
        let active = state.activate("Test Project").await.unwrap();
        assert!(
            active.recent_log.is_some(),
            "recent_log should be populated after prior session"
        );
        let log = active.recent_log.as_ref().unwrap();
        assert!(
            log.contains("Set up CI pipeline"),
            "recent_log should contain the deactivation entry"
        );

        // Verify it appears in prompt context
        let ctx = state.format_active_context_for_prompt().unwrap();
        assert!(
            ctx.contains("Recent Session Log"),
            "prompt context should contain recent session log header"
        );
        assert!(
            ctx.contains("Set up CI pipeline"),
            "prompt context should contain log content"
        );
    }

    #[tokio::test]
    async fn recent_log_none_without_logs() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout);

        let active = state.activate("Test Project").await.unwrap();
        assert!(
            active.recent_log.is_none(),
            "recent_log should be None when no log files exist"
        );
    }

    #[tokio::test]
    async fn log_appends_to_existing() {
        let (_dir, layout) = setup_workspace_with_project().await;
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let mut state = ProjectState::new(index, layout.clone());

        // First activation + deactivation
        state.activate("Test Project").await.unwrap();
        let now1 = make_datetime(2026, 2, 23, 14, 0);
        state.deactivate("First session.", now1).await.unwrap();

        // Second activation + deactivation (same day)
        state.rescan().await.unwrap();
        state.activate("Test Project").await.unwrap();
        let now2 = make_datetime(2026, 2, 23, 16, 30);
        state.deactivate("Second session.", now2).await.unwrap();

        // Verify both entries in same file
        let log_file = layout
            .projects_dir()
            .join("test-proj/notes/log/2026-02/log-23.md");
        let content = tokio::fs::read_to_string(&log_file).await.unwrap();
        assert!(
            content.contains("**14:00** First session."),
            "should have first entry"
        );
        assert!(
            content.contains("**16:30** Second session."),
            "should have second entry"
        );
    }
}
