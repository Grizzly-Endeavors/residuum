//! Project lifecycle: creation and archiving.

use std::path::PathBuf;

use chrono::NaiveDate;

use crate::error::ResiduumError;
use crate::workspace::layout::WorkspaceLayout;

use super::scanner::write_project_md_content;
use super::types::{ProjectFrontmatter, ProjectStatus};

/// Create a new project with the standard directory structure.
///
/// Returns the path to the newly created project directory.
///
/// # Errors
/// Returns `ResiduumError::Projects` if the name is invalid, a project with
/// the same dir name already exists, or filesystem operations fail.
pub async fn create_project(
    layout: &WorkspaceLayout,
    name: &str,
    description: &str,
    tools: Vec<String>,
    today: NaiveDate,
) -> Result<PathBuf, ResiduumError> {
    let dir_name = sanitize_dir_name(name);

    if dir_name.is_empty() {
        return Err(ResiduumError::Projects(format!(
            "project name '{name}' produces an empty directory name"
        )));
    }

    let project_dir = layout.projects_dir().join(&dir_name);

    if project_dir.exists() {
        return Err(ResiduumError::Projects(format!(
            "project directory '{dir_name}' already exists"
        )));
    }

    // Create directory structure
    for subdir in &["notes/log", "references", "workspace", "skills"] {
        tokio::fs::create_dir_all(project_dir.join(subdir))
            .await
            .map_err(|e| {
                ResiduumError::Projects(format!(
                    "failed to create directory {}: {e}",
                    project_dir.join(subdir).display()
                ))
            })?;
    }

    // Write PROJECT.md
    let frontmatter = ProjectFrontmatter {
        name: name.to_string(),
        description: description.to_string(),
        status: ProjectStatus::Active,
        created: today,
        tools,
        mcp_servers: vec![],
        archived: None,
    };

    let content = write_project_md_content(&frontmatter, "")?;
    tokio::fs::write(project_dir.join("PROJECT.md"), &content)
        .await
        .map_err(|e| {
            ResiduumError::Projects(format!(
                "failed to write PROJECT.md at {}: {e}",
                project_dir.display()
            ))
        })?;

    tracing::info!(project = name, dir = %dir_name, "created project");

    Ok(project_dir)
}

/// Archive a project: update its frontmatter and move from projects/ to archive/.
///
/// # Errors
/// Returns `ResiduumError::Projects` if the project doesn't exist, can't be
/// read, or the move fails.
pub async fn archive_project(
    layout: &WorkspaceLayout,
    dir_name: &str,
    today: NaiveDate,
) -> Result<(), ResiduumError> {
    let source = layout.projects_dir().join(dir_name);

    if !source.exists() {
        return Err(ResiduumError::Projects(format!(
            "project directory '{dir_name}' not found in projects/"
        )));
    }

    // Read and update PROJECT.md
    let project_md = source.join("PROJECT.md");
    let content = tokio::fs::read_to_string(&project_md).await.map_err(|e| {
        ResiduumError::Projects(format!(
            "failed to read PROJECT.md at {}: {e}",
            project_md.display()
        ))
    })?;

    let (mut frontmatter, body) = super::scanner::parse_project_md(&content)?;
    frontmatter.status = ProjectStatus::Archived;
    frontmatter.archived = Some(today);

    let updated = write_project_md_content(&frontmatter, &body)?;
    tokio::fs::write(&project_md, &updated).await.map_err(|e| {
        ResiduumError::Projects(format!(
            "failed to update PROJECT.md at {}: {e}",
            project_md.display()
        ))
    })?;

    // Move to archive
    let dest = layout.archive_dir().join(dir_name);
    tokio::fs::rename(&source, &dest).await.map_err(|e| {
        ResiduumError::Projects(format!(
            "failed to move project from {} to {}: {e}",
            source.display(),
            dest.display()
        ))
    })?;

    tracing::info!(project = dir_name, "archived project");

    Ok(())
}

/// Sanitize a project name to a valid directory name.
///
/// Lowercases, replaces spaces and special characters with hyphens,
/// and collapses multiple hyphens.
fn sanitize_dir_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    // Collapse multiple hyphens and trim leading/trailing hyphens
    let mut result = String::with_capacity(sanitized.len());
    let mut last_was_hyphen = true; // true to trim leading hyphens

    for c in sanitized.chars() {
        if c == '-' {
            if !last_was_hyphen {
                result.push(c);
            }
            last_was_hyphen = true;
        } else {
            result.push(c);
            last_was_hyphen = false;
        }
    }

    // Trim trailing hyphen
    if result.ends_with('-') {
        result.pop();
    }

    result
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::projects::scanner::parse_project_md;
    use crate::workspace::bootstrap::ensure_workspace;

    #[test]
    fn sanitize_simple_name() {
        assert_eq!(
            sanitize_dir_name("My Project"),
            "my-project",
            "spaces become hyphens"
        );
    }

    #[test]
    fn sanitize_special_chars() {
        assert_eq!(
            sanitize_dir_name("project@v2.0!"),
            "project-v2-0",
            "special chars become hyphens"
        );
    }

    #[test]
    fn sanitize_collapse_hyphens() {
        assert_eq!(
            sanitize_dir_name("a---b"),
            "a-b",
            "multiple hyphens collapse"
        );
    }

    #[test]
    fn sanitize_trim_hyphens() {
        assert_eq!(
            sanitize_dir_name("--project--"),
            "project",
            "leading/trailing hyphens trimmed"
        );
    }

    #[test]
    fn sanitize_preserves_underscores() {
        assert_eq!(
            sanitize_dir_name("my_project"),
            "my_project",
            "underscores preserved"
        );
    }

    #[tokio::test]
    async fn create_produces_correct_structure() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None, None).await.unwrap();

        let today = NaiveDate::from_ymd_opt(2026, 2, 23).unwrap();
        let path = create_project(&layout, "Test Project", "A test", vec![], today)
            .await
            .unwrap();

        assert!(path.exists(), "project dir should exist");
        assert!(path.join("PROJECT.md").exists(), "PROJECT.md should exist");
        assert!(
            path.join("notes/log").exists(),
            "notes/log dir should exist"
        );
        assert!(
            path.join("references").exists(),
            "references dir should exist"
        );
        assert!(
            path.join("workspace").exists(),
            "workspace dir should exist"
        );
        assert!(path.join("skills").exists(), "skills dir should exist");

        // Verify frontmatter
        let content = tokio::fs::read_to_string(path.join("PROJECT.md"))
            .await
            .unwrap();
        let (fm, _body) = parse_project_md(&content).unwrap();
        assert_eq!(fm.name, "Test Project", "name should match");
        assert_eq!(fm.status, ProjectStatus::Active, "should be active");
    }

    #[tokio::test]
    async fn create_duplicate_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None, None).await.unwrap();

        let today = NaiveDate::from_ymd_opt(2026, 2, 23).unwrap();
        create_project(&layout, "Test", "First", vec![], today)
            .await
            .unwrap();

        let result = create_project(&layout, "Test", "Second", vec![], today).await;
        assert!(result.is_err(), "duplicate should be rejected");
    }

    #[tokio::test]
    async fn archive_moves_and_updates_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None, None).await.unwrap();

        let created = NaiveDate::from_ymd_opt(2026, 2, 20).unwrap();
        create_project(&layout, "Archive Me", "To archive", vec![], created)
            .await
            .unwrap();

        let archive_date = NaiveDate::from_ymd_opt(2026, 2, 23).unwrap();
        archive_project(&layout, "archive-me", archive_date)
            .await
            .unwrap();

        // Verify moved
        assert!(
            !layout.projects_dir().join("archive-me").exists(),
            "should be gone from projects/"
        );
        assert!(
            layout.archive_dir().join("archive-me").exists(),
            "should exist in archive/"
        );

        // Verify updated frontmatter
        let content = tokio::fs::read_to_string(layout.archive_dir().join("archive-me/PROJECT.md"))
            .await
            .unwrap();
        let (fm, _body) = parse_project_md(&content).unwrap();
        assert_eq!(fm.status, ProjectStatus::Archived, "should be archived");
        assert_eq!(fm.archived, Some(archive_date), "should have archived date");
    }
}
