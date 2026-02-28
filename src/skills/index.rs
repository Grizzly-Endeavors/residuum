use std::path::{Path, PathBuf};

use super::{
    parser::parse_skill_md,
    types::{SkillIndexEntry, SkillSource},
};
use crate::error::IronclawError;

/// In-memory index of discovered skills.
#[derive(Debug, Clone, Default)]
pub struct SkillIndex {
    entries: Vec<SkillIndexEntry>,
}

impl SkillIndex {
    /// Scan configured directories and an optional project skills directory.
    ///
    /// For each subdirectory containing a `SKILL.md`, parses the frontmatter
    /// and builds an index entry. Invalid or missing files are warned and
    /// skipped. Duplicate names keep the first found.
    ///
    /// # Errors
    /// Returns `IronclawError::Skills` if a directory cannot be read (except
    /// `NotFound`, which is silently skipped).
    pub async fn scan(
        dirs: &[PathBuf],
        project_skills_dir: Option<&Path>,
    ) -> Result<Self, IronclawError> {
        let mut entries = Vec::new();
        let mut seen_names: Vec<String> = Vec::new();

        // Project skills have highest priority — scan first
        if let Some(project_dir) = project_skills_dir {
            scan_skill_directory(
                project_dir,
                SkillSource::Project,
                &mut entries,
                &mut seen_names,
            )
            .await?;
        }

        // Then workspace and user-global (workspace is dirs[0], user-global are the rest)
        for dir in dirs {
            let source = if dirs.first().is_some_and(|first| first == dir) {
                SkillSource::Workspace
            } else {
                SkillSource::UserGlobal
            };
            scan_skill_directory(dir, source, &mut entries, &mut seen_names).await?;
        }

        Ok(Self { entries })
    }

    /// Look up a skill by name (case-insensitive).
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<&SkillIndexEntry> {
        let lower = name.to_lowercase();
        self.entries.iter().find(|e| e.name.to_lowercase() == lower)
    }

    /// Format the index as XML for the system prompt.
    #[must_use]
    pub fn format_for_prompt(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }

        let mut parts = Vec::with_capacity(self.entries.len() + 2);
        parts.push("<available_skills>".to_string());

        for entry in &self.entries {
            let skill_md = entry.skill_dir.join("SKILL.md");
            parts.push(format!(
                "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>",
                entry.name,
                entry.description,
                skill_md.display(),
            ));
        }

        parts.push("</available_skills>".to_string());
        parts.join("\n")
    }

    /// Get all index entries.
    #[must_use]
    pub fn entries(&self) -> &[SkillIndexEntry] {
        &self.entries
    }
}

/// Scan a single directory for skill subfolders.
async fn scan_skill_directory(
    dir: &Path,
    source: SkillSource,
    entries: &mut Vec<SkillIndexEntry>,
    seen_names: &mut Vec<String>,
) -> Result<(), IronclawError> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(IronclawError::Skills(format!(
                "failed to read skills directory {}: {e}",
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
                    "failed to read skills directory entry"
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
                    "failed to get file type for skill entry"
                );
                continue;
            }
        };

        if !file_type.is_dir() {
            continue;
        }

        let skill_md = entry.path().join("SKILL.md");
        let file_content = match tokio::fs::read_to_string(&skill_md).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    path = %entry.path().display(),
                    "skipping directory without SKILL.md"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    path = %skill_md.display(),
                    error = %e,
                    "failed to read SKILL.md"
                );
                continue;
            }
        };

        match parse_skill_md(&file_content) {
            Ok((fm, _body)) => {
                let lower = fm.name.to_lowercase();
                if seen_names.contains(&lower) {
                    tracing::warn!(
                        name = %fm.name,
                        path = %skill_md.display(),
                        "duplicate skill name, keeping first found"
                    );
                    continue;
                }
                seen_names.push(lower);

                entries.push(SkillIndexEntry {
                    name: fm.name,
                    description: fm.description,
                    skill_dir: entry.path(),
                    source: source.clone(),
                });
            }
            Err(e) => {
                tracing::warn!(
                    path = %skill_md.display(),
                    error = %e,
                    "skipping skill with invalid frontmatter"
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::path::PathBuf;

    use super::super::types::{SkillIndexEntry, SkillSource};
    use super::SkillIndex;

    // ── SkillIndex ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn scan_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        assert!(
            index.entries().is_empty(),
            "empty dir should have no skills"
        );
    }

    #[tokio::test]
    async fn scan_nonexistent_dir() {
        let index = SkillIndex::scan(&[PathBuf::from("/tmp/nonexistent-skills-dir")], None)
            .await
            .unwrap();
        assert!(
            index.entries().is_empty(),
            "nonexistent dir should be silently skipped"
        );
    }

    #[tokio::test]
    async fn scan_with_valid_skill() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: \"A test skill\"\n---\n\nInstructions here.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        assert_eq!(index.entries().len(), 1, "should find one skill");
        assert_eq!(index.entries().first().unwrap().name, "my-skill");
        assert_eq!(
            index.entries().first().unwrap().source,
            SkillSource::Workspace
        );
    }

    #[tokio::test]
    async fn scan_skips_invalid_frontmatter() {
        let dir = tempfile::tempdir().unwrap();

        // Invalid skill
        let bad_dir = dir.path().join("bad-skill");
        tokio::fs::create_dir(&bad_dir).await.unwrap();
        tokio::fs::write(bad_dir.join("SKILL.md"), "---\n: invalid [[\n---\n")
            .await
            .unwrap();

        // Valid skill
        let good_dir = dir.path().join("good-skill");
        tokio::fs::create_dir(&good_dir).await.unwrap();
        tokio::fs::write(
            good_dir.join("SKILL.md"),
            "---\nname: good-skill\ndescription: \"Valid\"\n---\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        assert_eq!(index.entries().len(), 1, "should only find valid skill");
    }

    #[tokio::test]
    async fn scan_with_project_skills() {
        let ws_dir = tempfile::tempdir().unwrap();
        let proj_dir = tempfile::tempdir().unwrap();

        // Workspace skill
        let ws_skill = ws_dir.path().join("ws-skill");
        tokio::fs::create_dir(&ws_skill).await.unwrap();
        tokio::fs::write(
            ws_skill.join("SKILL.md"),
            "---\nname: ws-skill\ndescription: \"Workspace skill\"\n---\n",
        )
        .await
        .unwrap();

        // Project skill
        let proj_skill = proj_dir.path().join("proj-skill");
        tokio::fs::create_dir(&proj_skill).await.unwrap();
        tokio::fs::write(
            proj_skill.join("SKILL.md"),
            "---\nname: proj-skill\ndescription: \"Project skill\"\n---\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[ws_dir.path().to_path_buf()], Some(proj_dir.path()))
            .await
            .unwrap();
        assert_eq!(index.entries().len(), 2, "should find both skills");

        let proj_entry = index.find_by_name("proj-skill").unwrap();
        assert_eq!(proj_entry.source, SkillSource::Project);
    }

    #[tokio::test]
    async fn project_skill_shadows_workspace_on_name_collision() {
        let ws_dir = tempfile::tempdir().unwrap();
        let proj_dir = tempfile::tempdir().unwrap();

        // Workspace skill named "shared"
        let ws_skill = ws_dir.path().join("shared");
        tokio::fs::create_dir(&ws_skill).await.unwrap();
        tokio::fs::write(
            ws_skill.join("SKILL.md"),
            "---\nname: shared\ndescription: \"Workspace version\"\n---\n",
        )
        .await
        .unwrap();

        // Project skill with same name "shared"
        let proj_skill = proj_dir.path().join("shared");
        tokio::fs::create_dir(&proj_skill).await.unwrap();
        tokio::fs::write(
            proj_skill.join("SKILL.md"),
            "---\nname: shared\ndescription: \"Project version\"\n---\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[ws_dir.path().to_path_buf()], Some(proj_dir.path()))
            .await
            .unwrap();
        assert_eq!(index.entries().len(), 1, "should deduplicate by name");

        let entry = index.find_by_name("shared").unwrap();
        assert_eq!(
            entry.description, "Project version",
            "project skill should shadow workspace skill"
        );
        assert_eq!(entry.source, SkillSource::Project);
    }

    #[tokio::test]
    async fn scan_duplicate_names_keeps_first() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        // Same name in both dirs
        let skill1 = dir1.path().join("dupe");
        tokio::fs::create_dir(&skill1).await.unwrap();
        tokio::fs::write(
            skill1.join("SKILL.md"),
            "---\nname: dupe\ndescription: \"First\"\n---\n",
        )
        .await
        .unwrap();

        let skill2 = dir2.path().join("dupe");
        tokio::fs::create_dir(&skill2).await.unwrap();
        tokio::fs::write(
            skill2.join("SKILL.md"),
            "---\nname: dupe\ndescription: \"Second\"\n---\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(
            &[dir1.path().to_path_buf(), dir2.path().to_path_buf()],
            None,
        )
        .await
        .unwrap();
        assert_eq!(index.entries().len(), 1, "should deduplicate");
        assert_eq!(
            index.entries().first().unwrap().description,
            "First",
            "should keep first found"
        );
    }

    #[test]
    fn find_by_name_case_insensitive() {
        let index = SkillIndex {
            entries: vec![SkillIndexEntry {
                name: "my-skill".to_string(),
                description: "A skill".to_string(),
                skill_dir: PathBuf::from("/tmp/my-skill"),
                source: SkillSource::Workspace,
            }],
        };

        assert!(
            index.find_by_name("MY-SKILL").is_some(),
            "should find case-insensitive"
        );
        assert!(
            index.find_by_name("nonexistent").is_none(),
            "should not find missing"
        );
    }

    #[test]
    fn format_for_prompt_empty() {
        let index = SkillIndex::default();
        assert!(
            index.format_for_prompt().is_empty(),
            "empty index should produce empty string"
        );
    }

    #[test]
    fn format_for_prompt_with_entries() {
        let index = SkillIndex {
            entries: vec![SkillIndexEntry {
                name: "pdf-processing".to_string(),
                description: "Extracts text from PDFs".to_string(),
                skill_dir: PathBuf::from("/tmp/skills/pdf-processing"),
                source: SkillSource::Workspace,
            }],
        };

        let output = index.format_for_prompt();
        assert!(
            output.contains("<available_skills>"),
            "should have opening tag"
        );
        assert!(
            output.contains("</available_skills>"),
            "should have closing tag"
        );
        assert!(
            output.contains("<name>pdf-processing</name>"),
            "should contain skill name"
        );
        assert!(
            output.contains("<description>Extracts text from PDFs</description>"),
            "should contain description"
        );
        assert!(output.contains("<location>"), "should contain location");
    }
}
