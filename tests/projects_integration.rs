//! End-to-end integration tests for the projects context subsystem (Phase 5).
//!
//! Tests the full lifecycle: bootstrap → empty index → create → activate →
//! deactivate with log → archive → context assembly includes project data.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod projects_integration {
    use std::sync::Arc;

    use ironclaw::agent::context::ProjectsContext;
    use ironclaw::projects::activation::{ProjectState, SharedProjectState};
    use ironclaw::projects::lifecycle;
    use ironclaw::projects::scanner::ProjectIndex;
    use ironclaw::tools::Tool;
    use ironclaw::tools::projects::{
        ProjectActivateTool, ProjectArchiveTool, ProjectCreateTool, ProjectDeactivateTool,
        ProjectListTool,
    };
    use ironclaw::workspace::bootstrap::ensure_workspace;
    use ironclaw::workspace::layout::WorkspaceLayout;

    /// Set up a workspace with no projects.
    async fn setup() -> (tempfile::TempDir, WorkspaceLayout, SharedProjectState) {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout).await.unwrap();
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(ProjectState::new(
            index,
            layout.clone(),
        )));
        (dir, layout, state)
    }

    // ── Empty bootstrap ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn empty_workspace_has_no_projects() {
        let (_dir, _layout, state) = setup().await;

        let s = state.lock().await;
        assert!(
            s.index().entries().is_empty(),
            "freshly bootstrapped workspace should have no projects"
        );
        assert!(
            s.active_project_name().is_none(),
            "no project should be active"
        );
    }

    #[tokio::test]
    async fn empty_index_format_is_empty() {
        let (_dir, _layout, state) = setup().await;

        let s = state.lock().await;
        let prompt_text = s.format_index_for_prompt();
        assert!(
            prompt_text.contains("No projects"),
            "empty index should indicate no projects: {prompt_text}"
        );
    }

    // ── Create project via lifecycle ────────────────────────────────────────

    #[tokio::test]
    async fn create_project_produces_directory_structure() {
        let (_dir, layout, state) = setup().await;

        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 23).unwrap();
        let path =
            lifecycle::create_project(&layout, "My Project", "A test project", vec![], today)
                .await
                .unwrap();

        assert!(path.exists(), "project directory should exist");
        assert!(path.join("PROJECT.md").exists(), "PROJECT.md should exist");
        assert!(path.join("notes/log").exists(), "notes/log should exist");
        assert!(path.join("references").exists(), "references should exist");
        assert!(path.join("workspace").exists(), "workspace should exist");
        assert!(path.join("skills").exists(), "skills should exist");

        // Rescan should pick it up
        let mut s = state.lock().await;
        s.rescan().await.unwrap();
        assert_eq!(
            s.index().entries().len(),
            1,
            "should find one project after create + rescan"
        );
        assert_eq!(
            s.index().entries().first().unwrap().name,
            "My Project",
            "project name should match"
        );
    }

    // ── Full lifecycle: create → activate → deactivate → archive ───────────

    #[tokio::test]
    async fn full_project_lifecycle() {
        let (_dir, layout, state) = setup().await;

        // 1. Create
        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 23).unwrap();
        lifecycle::create_project(
            &layout,
            "Lifecycle Test",
            "Testing lifecycle",
            vec![],
            today,
        )
        .await
        .unwrap();

        {
            let mut s = state.lock().await;
            s.rescan().await.unwrap();
            assert_eq!(s.index().entries().len(), 1, "one project after create");
        }

        // 2. Activate
        {
            let mut s = state.lock().await;
            let active = s.activate("Lifecycle Test").await.unwrap();
            assert_eq!(active.name, "Lifecycle Test", "active project name");
            assert!(
                s.active_project_name().is_some(),
                "should have active project"
            );
        }

        // 3. Deactivate with log
        {
            let now = chrono::NaiveDate::from_ymd_opt(2026, 2, 23)
                .unwrap()
                .and_hms_opt(14, 32, 0)
                .unwrap();
            let mut s = state.lock().await;
            let name = s
                .deactivate("Completed lifecycle testing.", now)
                .await
                .unwrap();
            assert_eq!(name, "Lifecycle Test", "deactivation returns project name");
            assert!(
                s.active_project_name().is_none(),
                "no active project after deactivate"
            );
        }

        // 4. Verify log file
        let log_file = layout
            .projects_dir()
            .join("lifecycle-test/notes/log/2026-02/log-23.md");
        let log_content = std::fs::read_to_string(&log_file).unwrap();
        assert!(
            log_content.contains("# 2026-02-23"),
            "log should have date header"
        );
        assert!(log_content.contains("**14:32**"), "log should have time");
        assert!(
            log_content.contains("Completed lifecycle testing."),
            "log should have entry text"
        );

        // 5. Archive
        lifecycle::archive_project(&layout, "lifecycle-test", today)
            .await
            .unwrap();

        assert!(
            !layout.projects_dir().join("lifecycle-test").exists(),
            "should be removed from projects/"
        );
        assert!(
            layout.archive_dir().join("lifecycle-test").exists(),
            "should exist in archive/"
        );

        // 6. Rescan shows archived
        {
            let mut s = state.lock().await;
            s.rescan().await.unwrap();

            let archived_entry = s.index().find_by_name("Lifecycle Test");
            assert!(
                archived_entry.is_some(),
                "archived project should still be in index"
            );
            assert!(
                archived_entry.unwrap().is_archived,
                "should be marked as archived"
            );
        }

        // 7. Cannot activate archived project
        {
            let mut s = state.lock().await;
            let result = s.activate("Lifecycle Test").await;
            assert!(result.is_err(), "should not activate archived project");
        }
    }

    // ── Tool-level lifecycle ────────────────────────────────────────────────

    #[tokio::test]
    async fn tool_level_lifecycle() {
        let (_dir, _layout, state) = setup().await;
        let tz = chrono_tz::UTC;

        // List: empty
        let list_tool = ProjectListTool::new(Arc::clone(&state));
        let empty_list = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            empty_list.output.contains("No projects"),
            "empty list: {}",
            empty_list.output
        );

        // Create
        let create_tool = ProjectCreateTool::new(Arc::clone(&state), tz);
        let create_result = create_tool
            .execute(serde_json::json!({
                "name": "Tool Test",
                "description": "Created via tool"
            }))
            .await
            .unwrap();
        assert!(
            !create_result.is_error,
            "create should succeed: {}",
            create_result.output
        );

        // List: one project
        let after_create = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            after_create.output.contains("Tool Test"),
            "list should show project: {}",
            after_create.output
        );
        assert!(
            after_create.output.contains("1 project"),
            "should count one project: {}",
            after_create.output
        );

        // Activate
        let activate_tool = ProjectActivateTool::new(Arc::clone(&state));
        let activate_result = activate_tool
            .execute(serde_json::json!({"name": "Tool Test"}))
            .await
            .unwrap();
        assert!(
            !activate_result.is_error,
            "activate should succeed: {}",
            activate_result.output
        );

        // List shows [ACTIVE]
        let after_activate = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            after_activate.output.contains("[ACTIVE]"),
            "list should show active marker: {}",
            after_activate.output
        );

        // Deactivate
        let deactivate_tool = ProjectDeactivateTool::new(Arc::clone(&state), tz);
        let deactivate_result = deactivate_tool
            .execute(serde_json::json!({"log": "Tool-level test session."}))
            .await
            .unwrap();
        assert!(
            !deactivate_result.is_error,
            "deactivate should succeed: {}",
            deactivate_result.output
        );

        // Archive
        let archive_tool = ProjectArchiveTool::new(Arc::clone(&state), tz);
        let archive_result = archive_tool
            .execute(serde_json::json!({"name": "Tool Test"}))
            .await
            .unwrap();
        assert!(
            !archive_result.is_error,
            "archive should succeed: {}",
            archive_result.output
        );

        // List: no active projects (archived is hidden by default)
        let after_archive = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            after_archive.output.contains("No projects"),
            "list should show no projects after archive: {}",
            after_archive.output
        );

        // List with include_archived
        let with_archived = list_tool
            .execute(serde_json::json!({"include_archived": true}))
            .await
            .unwrap();
        assert!(
            with_archived.output.contains("Tool Test"),
            "archived project should appear with include_archived: {}",
            with_archived.output
        );
    }

    // ── Context assembly ────────────────────────────────────────────────────

    #[tokio::test]
    async fn context_includes_project_index() {
        let (_dir, layout, state) = setup().await;

        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 23).unwrap();
        lifecycle::create_project(
            &layout,
            "Context Test",
            "For context testing",
            vec![],
            today,
        )
        .await
        .unwrap();

        {
            let mut s = state.lock().await;
            s.rescan().await.unwrap();
        }

        let s = state.lock().await;
        let index_text = s.format_index_for_prompt();

        let projects_ctx = ProjectsContext {
            index: Some(&index_text),
            active_context: None,
        };

        assert!(projects_ctx.index.is_some(), "index should be present");
        assert!(
            index_text.contains("Context Test"),
            "index should contain project name: {index_text}"
        );
    }

    #[tokio::test]
    async fn context_includes_active_project() {
        let (_dir, layout, state) = setup().await;

        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 23).unwrap();
        lifecycle::create_project(
            &layout,
            "Active Context Test",
            "For active context testing",
            vec![],
            today,
        )
        .await
        .unwrap();

        // Write some body content to PROJECT.md
        let project_md = layout.projects_dir().join("active-context-test/PROJECT.md");
        let content = std::fs::read_to_string(&project_md).unwrap();
        let new_content = format!("{content}\nThis is the project overview.\n");
        std::fs::write(&project_md, new_content).unwrap();

        {
            let mut s = state.lock().await;
            s.rescan().await.unwrap();
            s.activate("Active Context Test").await.unwrap();
        }

        let s = state.lock().await;
        let index_text = s.format_index_for_prompt();
        let active_text = s.format_active_context_for_prompt();

        let projects_ctx = ProjectsContext {
            index: Some(&index_text),
            active_context: active_text.as_deref(),
        };

        assert!(
            projects_ctx.active_context.is_some(),
            "active context should be present"
        );

        let active_str = projects_ctx.active_context.unwrap();
        assert!(
            active_str.contains("Active Context Test"),
            "active context should contain project name: {active_str}"
        );
        assert!(
            active_str.contains("project overview"),
            "active context should contain body: {active_str}"
        );
    }

    // ── Error cases ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cannot_archive_active_project_via_tool() {
        let (_dir, _layout, state) = setup().await;
        let tz = chrono_tz::UTC;

        // Create and activate
        let create_tool = ProjectCreateTool::new(Arc::clone(&state), tz);
        create_tool
            .execute(serde_json::json!({
                "name": "Active Proj",
                "description": "Will try to archive while active"
            }))
            .await
            .unwrap();

        let activate_tool = ProjectActivateTool::new(Arc::clone(&state));
        activate_tool
            .execute(serde_json::json!({"name": "Active Proj"}))
            .await
            .unwrap();

        // Try to archive — should fail
        let archive_tool = ProjectArchiveTool::new(Arc::clone(&state), tz);
        let result = archive_tool
            .execute(serde_json::json!({"name": "Active Proj"}))
            .await
            .unwrap();
        assert!(
            result.is_error,
            "archiving active project should fail: {}",
            result.output
        );
        assert!(
            result.output.contains("deactivate"),
            "error should mention deactivation: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn duplicate_create_rejected() {
        let (_dir, _layout, state) = setup().await;
        let tz = chrono_tz::UTC;

        let create_tool = ProjectCreateTool::new(Arc::clone(&state), tz);
        let first = create_tool
            .execute(serde_json::json!({
                "name": "Duplicate",
                "description": "First"
            }))
            .await
            .unwrap();
        assert!(!first.is_error, "first create should succeed");

        let second = create_tool
            .execute(serde_json::json!({
                "name": "Duplicate",
                "description": "Second"
            }))
            .await
            .unwrap();
        assert!(
            second.is_error,
            "duplicate create should fail: {}",
            second.output
        );
    }

    #[tokio::test]
    async fn deactivate_empty_log_rejected_via_tool() {
        let (_dir, _layout, state) = setup().await;
        let tz = chrono_tz::UTC;

        // Create and activate
        let create_tool = ProjectCreateTool::new(Arc::clone(&state), tz);
        create_tool
            .execute(serde_json::json!({
                "name": "Empty Log",
                "description": "Test empty log rejection"
            }))
            .await
            .unwrap();

        let activate_tool = ProjectActivateTool::new(Arc::clone(&state));
        activate_tool
            .execute(serde_json::json!({"name": "Empty Log"}))
            .await
            .unwrap();

        let deactivate_tool = ProjectDeactivateTool::new(Arc::clone(&state), tz);
        let result = deactivate_tool
            .execute(serde_json::json!({"log": ""}))
            .await
            .unwrap();
        assert!(
            result.is_error,
            "empty log should be rejected: {}",
            result.output
        );
    }

    // ── ProjectsContext::none ────────────────────────────────────────────────

    #[test]
    fn projects_context_none() {
        let ctx = ProjectsContext::none();
        assert!(ctx.index.is_none(), "none() should have no index");
        assert!(
            ctx.active_context.is_none(),
            "none() should have no active context"
        );
    }
}
