//! End-to-end integration tests for the skills subsystem (Phase 6).
//!
//! Tests the full lifecycle: scan → verify index → activate → verify active →
//! deactivate → verify removed. Also tests project-scoped skills and malformed
//! SKILL.md handling.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod skills_integration {
    use std::sync::Arc;

    use ironclaw::skills::{SkillIndex, SkillSource, SkillState};
    use ironclaw::tools::Tool;
    use ironclaw::tools::skills::{SkillActivateTool, SkillDeactivateTool};
    use ironclaw::workspace::bootstrap::ensure_workspace;
    use ironclaw::workspace::layout::WorkspaceLayout;

    /// Set up a workspace with skills directory and optional skill files.
    async fn setup_workspace() -> (tempfile::TempDir, WorkspaceLayout) {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None).await.unwrap();
        (dir, layout)
    }

    /// Create a valid skill directory with SKILL.md.
    async fn create_skill(
        base_dir: &std::path::Path,
        dir_name: &str,
        name: &str,
        description: &str,
        body: &str,
    ) {
        let skill_dir = base_dir.join(dir_name);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        let content = format!("---\nname: {name}\ndescription: \"{description}\"\n---\n\n{body}\n");
        tokio::fs::write(skill_dir.join("SKILL.md"), content)
            .await
            .unwrap();
    }

    // ── Scanning ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn scan_empty_workspace() {
        let (_dir, layout) = setup_workspace().await;
        let index = SkillIndex::scan(&[layout.skills_dir()], None)
            .await
            .unwrap();
        assert!(
            index.entries().is_empty(),
            "empty skills dir should have no skills"
        );
    }

    #[tokio::test]
    async fn scan_discovers_workspace_skills() {
        let (_dir, layout) = setup_workspace().await;
        create_skill(
            &layout.skills_dir(),
            "code-review",
            "code-review",
            "Reviews code for quality",
            "Check for bugs, style, and correctness.",
        )
        .await;

        let index = SkillIndex::scan(&[layout.skills_dir()], None)
            .await
            .unwrap();
        assert_eq!(index.entries().len(), 1);
        assert_eq!(index.entries().first().unwrap().name, "code-review");
        assert_eq!(
            index.entries().first().unwrap().source,
            SkillSource::Workspace
        );
    }

    #[tokio::test]
    async fn scan_skips_malformed_skill() {
        let (_dir, layout) = setup_workspace().await;

        // Valid skill
        create_skill(
            &layout.skills_dir(),
            "good-skill",
            "good-skill",
            "Valid skill",
            "Instructions.",
        )
        .await;

        // Malformed skill (invalid YAML)
        let bad_dir = layout.skills_dir().join("bad-skill");
        tokio::fs::create_dir_all(&bad_dir).await.unwrap();
        tokio::fs::write(bad_dir.join("SKILL.md"), "---\n: invalid [[\n---\n")
            .await
            .unwrap();

        // Skill with invalid name
        let bad_name_dir = layout.skills_dir().join("Bad-Name");
        tokio::fs::create_dir_all(&bad_name_dir).await.unwrap();
        tokio::fs::write(
            bad_name_dir.join("SKILL.md"),
            "---\nname: Bad-Name\ndescription: \"Has uppercase\"\n---\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[layout.skills_dir()], None)
            .await
            .unwrap();
        assert_eq!(index.entries().len(), 1, "should only find the valid skill");
        assert_eq!(index.entries().first().unwrap().name, "good-skill");
    }

    #[tokio::test]
    async fn index_format_for_prompt() {
        let (_dir, layout) = setup_workspace().await;
        create_skill(
            &layout.skills_dir(),
            "pdf-processing",
            "pdf-processing",
            "Extracts text from PDFs",
            "Use this for PDF work.",
        )
        .await;

        let index = SkillIndex::scan(&[layout.skills_dir()], None)
            .await
            .unwrap();
        let output = index.format_for_prompt();
        assert!(output.contains("<available_skills>"));
        assert!(output.contains("</available_skills>"));
        assert!(output.contains("<name>pdf-processing</name>"));
        assert!(output.contains("<description>Extracts text from PDFs</description>"));
    }

    // ── Full lifecycle ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn full_lifecycle_scan_activate_deactivate() {
        let (_dir, layout) = setup_workspace().await;
        create_skill(
            &layout.skills_dir(),
            "test-skill",
            "test-skill",
            "A test skill",
            "Follow these instructions carefully.",
        )
        .await;

        let dirs = vec![layout.skills_dir()];
        let index = SkillIndex::scan(&dirs, None).await.unwrap();
        let state = SkillState::new_shared(index, dirs);

        // Verify index
        {
            let s = state.lock().await;
            assert_eq!(s.index().entries().len(), 1);
            assert!(s.active_skill_names().is_empty());
        }

        // Activate via tool
        let activate = SkillActivateTool::new(Arc::clone(&state));
        let result = activate
            .execute(serde_json::json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(
            !result.is_error,
            "activate should succeed: {}",
            result.output
        );

        // Verify active
        {
            let s = state.lock().await;
            assert_eq!(s.active_skill_names(), vec!["test-skill"]);
            let active_text = s.format_active_for_prompt().unwrap();
            assert!(active_text.contains("<active_skill name=\"test-skill\">"));
            assert!(active_text.contains("Follow these instructions carefully."));
        }

        // Deactivate via tool
        let deactivate = SkillDeactivateTool::new(Arc::clone(&state));
        let deactivate_result = deactivate
            .execute(serde_json::json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(
            !deactivate_result.is_error,
            "deactivate should succeed: {}",
            deactivate_result.output
        );

        // Verify removed
        {
            let s = state.lock().await;
            assert!(s.active_skill_names().is_empty());
            assert!(s.format_active_for_prompt().is_none());
        }
    }

    #[tokio::test]
    async fn activate_already_active_errors() {
        let (_dir, layout) = setup_workspace().await;
        create_skill(
            &layout.skills_dir(),
            "test-skill",
            "test-skill",
            "A test skill",
            "Body.",
        )
        .await;

        let dirs = vec![layout.skills_dir()];
        let index = SkillIndex::scan(&dirs, None).await.unwrap();
        let state = SkillState::new_shared(index, dirs);

        let activate = SkillActivateTool::new(Arc::clone(&state));
        activate
            .execute(serde_json::json!({"name": "test-skill"}))
            .await
            .unwrap();

        // Second activation should error
        let result = activate
            .execute(serde_json::json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(result.is_error, "should error for already active skill");
        assert!(
            result.output.contains("already active"),
            "error should mention already active: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn multiple_skills_can_be_active() {
        let (_dir, layout) = setup_workspace().await;
        create_skill(
            &layout.skills_dir(),
            "skill-a",
            "skill-a",
            "Skill A",
            "Body A.",
        )
        .await;
        create_skill(
            &layout.skills_dir(),
            "skill-b",
            "skill-b",
            "Skill B",
            "Body B.",
        )
        .await;

        let dirs = vec![layout.skills_dir()];
        let index = SkillIndex::scan(&dirs, None).await.unwrap();
        let state = SkillState::new_shared(index, dirs);

        let activate = SkillActivateTool::new(Arc::clone(&state));
        activate
            .execute(serde_json::json!({"name": "skill-a"}))
            .await
            .unwrap();
        activate
            .execute(serde_json::json!({"name": "skill-b"}))
            .await
            .unwrap();

        let s = state.lock().await;
        assert_eq!(s.active_skill_names().len(), 2);
        let active_text = s.format_active_for_prompt().unwrap();
        assert!(active_text.contains("Body A."));
        assert!(active_text.contains("Body B."));
    }

    // ── Project-scoped skills ────────────────────────────────────────────────

    #[tokio::test]
    async fn project_scoped_skills_appear_on_rescan() {
        let (_dir, layout) = setup_workspace().await;

        // Create a workspace skill
        create_skill(
            &layout.skills_dir(),
            "ws-skill",
            "ws-skill",
            "Workspace skill",
            "WS body.",
        )
        .await;

        // Create a project-like skills directory
        let project_skills_dir = layout.projects_dir().join("my-project/skills");
        tokio::fs::create_dir_all(&project_skills_dir)
            .await
            .unwrap();
        create_skill(
            &project_skills_dir,
            "proj-skill",
            "proj-skill",
            "Project skill",
            "Project body.",
        )
        .await;

        let dirs = vec![layout.skills_dir()];
        let index = SkillIndex::scan(&dirs, None).await.unwrap();
        let mut state = SkillState::new(index, dirs);

        // Before rescan: only workspace skill
        assert_eq!(state.index().entries().len(), 1);

        // Rescan with project skills dir
        state.rescan(Some(&project_skills_dir)).await.unwrap();

        // After rescan: both skills
        assert_eq!(state.index().entries().len(), 2);
        assert!(state.index().find_by_name("proj-skill").is_some());
        assert_eq!(
            state.index().find_by_name("proj-skill").unwrap().source,
            SkillSource::Project
        );
    }

    #[tokio::test]
    async fn project_skills_removed_on_rescan_without_project() {
        let (_dir, layout) = setup_workspace().await;

        create_skill(
            &layout.skills_dir(),
            "ws-skill",
            "ws-skill",
            "Workspace skill",
            "WS body.",
        )
        .await;

        let project_skills_dir = layout.projects_dir().join("my-project/skills");
        tokio::fs::create_dir_all(&project_skills_dir)
            .await
            .unwrap();
        create_skill(
            &project_skills_dir,
            "proj-skill",
            "proj-skill",
            "Project skill",
            "Project body.",
        )
        .await;

        let dirs = vec![layout.skills_dir()];
        let index = SkillIndex::scan(&dirs, Some(&project_skills_dir))
            .await
            .unwrap();
        let mut state = SkillState::new(index, dirs);

        // Activate project skill
        state.activate("proj-skill").await.unwrap();
        assert_eq!(state.active_skill_names(), vec!["proj-skill"]);

        // Rescan without project dir (simulates project deactivation)
        state.rescan(None).await.unwrap();

        // Project skill should be gone from index and active list
        assert!(state.index().find_by_name("proj-skill").is_none());
        assert!(
            state.active_skill_names().is_empty(),
            "stale active skill should be removed"
        );
    }
}
