//! Project directory scanning and index building.

use std::path::Path;

use crate::error::FatalError;
use crate::workspace::layout::WorkspaceLayout;

use super::types::{ProjectFrontmatter, ProjectIndexEntry};

/// In-memory index of discovered projects.
#[derive(Debug, Clone, Default)]
pub struct ProjectIndex {
    entries: Vec<ProjectIndexEntry>,
}

impl ProjectIndex {
    /// Scan the projects and archive directories to build the index.
    ///
    /// Reads each subfolder's `PROJECT.md` frontmatter (not body). Invalid or
    /// missing frontmatter is logged as a warning and skipped.
    ///
    /// # Errors
    /// Returns `FatalError::Projects` if the directories cannot be read.
    pub async fn scan(layout: &WorkspaceLayout) -> Result<Self, FatalError> {
        let mut entries = Vec::new();

        scan_directory(&layout.projects_dir(), false, &mut entries).await?;
        scan_directory(&layout.archive_dir(), true, &mut entries).await?;

        tracing::debug!(total = entries.len(), "project index scan complete");

        Ok(Self { entries })
    }

    /// Look up a project by name (case-insensitive).
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<&ProjectIndexEntry> {
        let lower = name.to_lowercase();
        self.entries.iter().find(|e| e.name.to_lowercase() == lower)
    }

    /// Look up a project by directory name (case-insensitive).
    #[must_use]
    pub fn find_by_dir_name(&self, dir_name: &str) -> Option<&ProjectIndexEntry> {
        let lower = dir_name.to_lowercase();
        self.entries
            .iter()
            .find(|e| e.dir_name.to_lowercase() == lower)
    }

    /// Format the index as a markdown table for the system prompt.
    #[must_use]
    pub fn format_for_prompt(&self) -> String {
        if self.entries.is_empty() {
            return "No projects found.".to_string();
        }

        let mut lines = Vec::with_capacity(self.entries.len() + 2);
        lines.push("| Name | Status | Description |".to_string());
        lines.push("|------|--------|-------------|".to_string());

        for entry in &self.entries {
            lines.push(format!(
                "| {} | {} | {} |",
                entry.name, entry.status, entry.description
            ));
        }

        lines.join("\n")
    }

    /// Get all index entries.
    #[must_use]
    pub fn entries(&self) -> &[ProjectIndexEntry] {
        &self.entries
    }
}

/// Scan a single directory (projects/ or archive/) for project subfolders.
async fn scan_directory(
    dir: &Path,
    is_archive: bool,
    entries: &mut Vec<ProjectIndexEntry>,
) -> Result<(), FatalError> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(FatalError::Projects(format!(
                "failed to read directory {}: {e}",
                dir.display()
            )));
        }
    };

    loop {
        let entry = match read_dir.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(
                    dir = %dir.display(),
                    error = %e,
                    "failed to read directory entry"
                );
                continue;
            }
        };

        let file_type = match entry.file_type().await {
            Ok(ft) => ft,
            Err(e) => {
                tracing::warn!(
                    path = %entry.path().display(),
                    error = %e,
                    "failed to get file type"
                );
                continue;
            }
        };

        if !file_type.is_dir() {
            continue;
        }

        let project_md = entry.path().join("PROJECT.md");
        let content = match tokio::fs::read_to_string(&project_md).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    path = %entry.path().display(),
                    "skipping directory without PROJECT.md"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    path = %project_md.display(),
                    error = %e,
                    "failed to read PROJECT.md"
                );
                continue;
            }
        };

        match parse_project_md(&content) {
            Ok((fm, _body)) => {
                let dir_name = if let Some(s) = entry.file_name().to_str() {
                    s.to_string()
                } else {
                    tracing::warn!(
                        path = %entry.path().display(),
                        "skipping project directory with non-UTF-8 name"
                    );
                    continue;
                };

                entries.push(ProjectIndexEntry {
                    name: fm.name.clone(),
                    description: fm.description.clone(),
                    status: fm.status,
                    dir_name,
                    is_archived: is_archive,
                });
            }
            Err(e) => {
                tracing::warn!(
                    path = %project_md.display(),
                    scan_root = %dir.display(),
                    error = %e,
                    "skipping project with invalid frontmatter"
                );
            }
        }
    }

    Ok(())
}

/// Parse a `PROJECT.md` file into frontmatter and body.
///
/// Expects YAML frontmatter delimited by `---` at the start of the file.
///
/// # Errors
/// Returns `FatalError::Projects` if the frontmatter is missing or invalid YAML.
pub fn parse_project_md(content: &str) -> Result<(ProjectFrontmatter, String), FatalError> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Err(FatalError::Projects(
            "PROJECT.md missing frontmatter delimiter '---'".to_string(),
        ));
    }

    // Skip the opening "---" and find the closing "---"
    let after_open = trimmed
        .get(3..)
        .ok_or_else(|| FatalError::Projects("PROJECT.md is too short".to_string()))?;

    let close_pos = after_open.find("\n---").ok_or_else(|| {
        FatalError::Projects("PROJECT.md missing closing frontmatter delimiter '---'".to_string())
    })?;

    let yaml_str = after_open
        .get(..close_pos)
        .ok_or_else(|| FatalError::Projects("failed to extract YAML content".to_string()))?;

    let frontmatter: ProjectFrontmatter = serde_yml::from_str(yaml_str).map_err(|e| {
        FatalError::Projects(format!("failed to parse PROJECT.md frontmatter: {e}"))
    })?;

    // Body is everything after the closing "---" and its newline
    let body_start = 3 + close_pos + 4; // "---" prefix + yaml + "\n---"
    let body = trimmed.get(body_start..).unwrap_or("").trim().to_string();

    Ok((frontmatter, body))
}

/// Reconstruct a `PROJECT.md` file from frontmatter and body.
///
/// # Errors
/// Returns `FatalError::Projects` if YAML serialization fails.
pub fn write_project_md_content(
    frontmatter: &ProjectFrontmatter,
    body: &str,
) -> Result<String, FatalError> {
    let yaml = serde_yml::to_string(frontmatter)
        .map_err(|e| FatalError::Projects(format!("failed to serialize frontmatter: {e}")))?;

    let mut output = format!("---\n{yaml}---\n");

    if !body.is_empty() {
        output.push('\n');
        output.push_str(body);
        output.push('\n');
    }

    Ok(output)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::projects::types::ProjectStatus;
    use crate::workspace::bootstrap::ensure_workspace;

    #[test]
    fn parse_project_md_valid() {
        let content = r#"---
name: test-project
description: "A test project"
status: active
created: 2026-02-10
---

Some overview body text here.
"#;
        let (fm, body) = parse_project_md(content).unwrap();
        assert_eq!(fm.name, "test-project", "name should match");
        assert_eq!(fm.status, ProjectStatus::Active, "status should be active");
        assert!(
            body.contains("overview body text"),
            "body should contain overview text"
        );
    }

    #[test]
    fn parse_project_md_no_body() {
        let content = "---\nname: minimal\ndescription: \"Minimal\"\nstatus: active\ncreated: 2026-02-20\n---\n";
        let (fm, body) = parse_project_md(content).unwrap();
        assert_eq!(fm.name, "minimal", "name should match");
        assert!(body.is_empty(), "body should be empty");
    }

    #[test]
    fn parse_project_md_missing_delimiter() {
        let content = "name: bad\ndescription: \"No delimiters\"\n";
        assert!(
            parse_project_md(content).is_err(),
            "missing delimiter should error"
        );
    }

    #[test]
    fn parse_project_md_invalid_yaml() {
        let content = "---\n: invalid yaml [[\n---\n";
        assert!(
            parse_project_md(content).is_err(),
            "invalid YAML should error"
        );
    }

    #[test]
    fn write_and_reparse() {
        let fm = ProjectFrontmatter {
            name: "round-trip".to_string(),
            description: "Round-trip test".to_string(),
            status: ProjectStatus::Active,
            created: chrono::NaiveDate::from_ymd_opt(2026, 2, 20).unwrap(),
            tools: vec!["exec".to_string()],
            mcp_servers: vec![],
            archived: None,
        };
        let body = "Some body content.";

        let content = write_project_md_content(&fm, body).unwrap();
        let (parsed_fm, parsed_body) = parse_project_md(&content).unwrap();

        assert_eq!(parsed_fm.name, "round-trip", "name should round-trip");
        assert_eq!(parsed_fm.tools.len(), 1, "tools should round-trip");
        assert!(
            parsed_body.contains("Some body content"),
            "body should round-trip"
        );
    }

    #[tokio::test]
    async fn scan_empty_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None, None).await.unwrap();

        let index = ProjectIndex::scan(&layout).await.unwrap();
        assert!(
            index.entries().is_empty(),
            "empty workspace should have no projects"
        );
    }

    #[tokio::test]
    async fn scan_with_projects() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None, None).await.unwrap();

        // Create a project
        let project_dir = layout.projects_dir().join("test-proj");
        tokio::fs::create_dir(&project_dir).await.unwrap();
        tokio::fs::write(
            project_dir.join("PROJECT.md"),
            "---\nname: Test Project\ndescription: \"A test\"\nstatus: active\ncreated: 2026-02-20\n---\n",
        )
        .await
        .unwrap();

        let index = ProjectIndex::scan(&layout).await.unwrap();
        assert_eq!(index.entries().len(), 1, "should find one project");
        assert_eq!(
            index.entries().first().unwrap().name,
            "Test Project",
            "name should match"
        );
        assert!(
            !index.entries().first().unwrap().is_archived,
            "should not be archived"
        );
    }

    #[tokio::test]
    async fn scan_with_archived_project() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None, None).await.unwrap();

        let archive_dir = layout.archive_dir().join("old-proj");
        tokio::fs::create_dir(&archive_dir).await.unwrap();
        tokio::fs::write(
            archive_dir.join("PROJECT.md"),
            "---\nname: Old Project\ndescription: \"Archived\"\nstatus: archived\ncreated: 2025-01-01\narchived: 2026-01-01\n---\n",
        )
        .await
        .unwrap();

        let index = ProjectIndex::scan(&layout).await.unwrap();
        assert_eq!(index.entries().len(), 1, "should find one archived project");
        assert!(
            index.entries().first().unwrap().is_archived,
            "should be archived"
        );
    }

    #[tokio::test]
    async fn scan_skips_invalid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout, None, None).await.unwrap();

        // Create project with invalid YAML
        let bad_dir = layout.projects_dir().join("bad-proj");
        tokio::fs::create_dir(&bad_dir).await.unwrap();
        tokio::fs::write(bad_dir.join("PROJECT.md"), "---\n: invalid [[\n---\n")
            .await
            .unwrap();

        // Create valid project
        let good_dir = layout.projects_dir().join("good-proj");
        tokio::fs::create_dir(&good_dir).await.unwrap();
        tokio::fs::write(
            good_dir.join("PROJECT.md"),
            "---\nname: Good\ndescription: \"Valid\"\nstatus: active\ncreated: 2026-02-20\n---\n",
        )
        .await
        .unwrap();

        let index = ProjectIndex::scan(&layout).await.unwrap();
        assert_eq!(
            index.entries().len(),
            1,
            "should only find valid project, skip invalid"
        );
    }

    #[test]
    fn find_by_name_case_insensitive() {
        let index = ProjectIndex {
            entries: vec![ProjectIndexEntry {
                name: "My Project".to_string(),
                description: "A project".to_string(),
                status: ProjectStatus::Active,
                dir_name: "my-project".to_string(),
                is_archived: false,
            }],
        };

        assert!(
            index.find_by_name("my project").is_some(),
            "should find case-insensitive match"
        );
        assert!(
            index.find_by_name("MY PROJECT").is_some(),
            "should find uppercase match"
        );
        assert!(
            index.find_by_name("nonexistent").is_none(),
            "should not find nonexistent"
        );
    }

    #[test]
    fn format_for_prompt_empty() {
        let index = ProjectIndex::default();
        assert_eq!(
            index.format_for_prompt(),
            "No projects found.",
            "empty index should show no projects message"
        );
    }

    #[test]
    fn format_for_prompt_with_entries() {
        let index = ProjectIndex {
            entries: vec![
                ProjectIndexEntry {
                    name: "Project A".to_string(),
                    description: "First project".to_string(),
                    status: ProjectStatus::Active,
                    dir_name: "project-a".to_string(),
                    is_archived: false,
                },
                ProjectIndexEntry {
                    name: "Project B".to_string(),
                    description: "Second project".to_string(),
                    status: ProjectStatus::Archived,
                    dir_name: "project-b".to_string(),
                    is_archived: true,
                },
            ],
        };

        let output = index.format_for_prompt();
        assert!(output.contains("| Name |"), "should have table header");
        assert!(
            output.contains("| Project A |"),
            "should contain first project"
        );
        assert!(
            output.contains("| Project B |"),
            "should contain second project"
        );
    }
}
