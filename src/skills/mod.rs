//! Agent skills: discovery, activation, and lifecycle management.
//!
//! Skills are opt-in capability modules discovered from `SKILL.md` files.
//! The agent sees a lightweight index in the system prompt and can activate
//! or deactivate skills via tools to load their full instructions.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use crate::error::IronclawError;

// ── Types ────────────────────────────────────────────────────────────────────

/// YAML frontmatter deserialized from a `SKILL.md` file.
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    /// Unique skill name (lowercase, alphanumeric + hyphens).
    name: String,
    /// Brief description shown in the index.
    description: String,
}

/// Where a skill was discovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// From the workspace `skills/` directory.
    Workspace,
    /// From an extra directory configured in `[skills].dirs`.
    UserGlobal,
    /// From an active project's `skills/` subdirectory.
    Project,
}

impl fmt::Display for SkillSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace => write!(f, "workspace"),
            Self::UserGlobal => write!(f, "user-global"),
            Self::Project => write!(f, "project"),
        }
    }
}

/// Lightweight index entry built from scanning a `SKILL.md` frontmatter.
#[derive(Debug, Clone)]
pub struct SkillIndexEntry {
    /// Unique skill name.
    pub name: String,
    /// Brief description.
    pub description: String,
    /// Absolute path to the skill's directory.
    pub skill_dir: PathBuf,
    /// Where this skill was found.
    pub source: SkillSource,
}

/// Fully loaded skill with its body content (after activation).
#[derive(Debug, Clone)]
pub struct ActiveSkill {
    /// Skill name (matches index entry).
    pub name: String,
    /// Markdown body from `SKILL.md` (everything after frontmatter).
    pub body: String,
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/// Parse a `SKILL.md` file into frontmatter and body.
///
/// Expects YAML frontmatter delimited by `---` at the start of the file.
/// Validates the skill name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens.
///
/// # Errors
/// Returns `IronclawError::Skills` if the frontmatter is missing, invalid
/// YAML, or the name fails validation.
fn parse_skill_md(content: &str) -> Result<(SkillFrontmatter, String), IronclawError> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Err(IronclawError::Skills(
            "SKILL.md missing frontmatter delimiter '---'".to_string(),
        ));
    }

    let after_open = trimmed
        .get(3..)
        .ok_or_else(|| IronclawError::Skills("SKILL.md is too short".to_string()))?;

    let close_pos = after_open.find("\n---").ok_or_else(|| {
        IronclawError::Skills("SKILL.md missing closing frontmatter delimiter '---'".to_string())
    })?;

    let yaml_str = after_open
        .get(..close_pos)
        .ok_or_else(|| IronclawError::Skills("failed to extract YAML content".to_string()))?;

    let frontmatter: SkillFrontmatter = serde_yml::from_str(yaml_str)
        .map_err(|e| IronclawError::Skills(format!("failed to parse SKILL.md frontmatter: {e}")))?;

    validate_skill_name(&frontmatter.name)?;

    let body_start = 3 + close_pos + 4; // "---" prefix + yaml + "\n---"
    let body = trimmed.get(body_start..).unwrap_or("").trim().to_string();

    Ok((frontmatter, body))
}

/// Validate a skill name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens.
fn validate_skill_name(name: &str) -> Result<(), IronclawError> {
    if name.is_empty() || name.len() > 64 {
        return Err(IronclawError::Skills(format!(
            "skill name must be 1-64 characters, got {len}",
            len = name.len()
        )));
    }

    if name.starts_with('-') || name.ends_with('-') {
        return Err(IronclawError::Skills(format!(
            "skill name '{name}' must not start or end with a hyphen"
        )));
    }

    if name.contains("--") {
        return Err(IronclawError::Skills(format!(
            "skill name '{name}' must not contain consecutive hyphens"
        )));
    }

    for ch in name.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(IronclawError::Skills(format!(
                "skill name '{name}' contains invalid character '{ch}' \
                 (only lowercase alphanumeric and hyphens allowed)"
            )));
        }
    }

    Ok(())
}

// ── Index ────────────────────────────────────────────────────────────────────

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

        for dir in dirs {
            let source = if dirs.first().is_some_and(|first| first == dir) {
                SkillSource::Workspace
            } else {
                SkillSource::UserGlobal
            };
            scan_skill_directory(dir, source, &mut entries, &mut seen_names).await?;
        }

        if let Some(project_dir) = project_skills_dir {
            scan_skill_directory(
                project_dir,
                SkillSource::Project,
                &mut entries,
                &mut seen_names,
            )
            .await?;
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

// ── State ────────────────────────────────────────────────────────────────────

/// Shared skill state, following the `SharedProjectState` pattern.
pub type SharedSkillState = Arc<tokio::sync::Mutex<SkillState>>;

/// Skill state manager: holds the index and active skills.
pub struct SkillState {
    index: SkillIndex,
    active: Vec<ActiveSkill>,
    dirs: Vec<PathBuf>,
}

impl SkillState {
    /// Create a new skill state with a pre-built index.
    #[must_use]
    pub fn new(index: SkillIndex, dirs: Vec<PathBuf>) -> Self {
        Self {
            index,
            active: Vec::new(),
            dirs,
        }
    }

    /// Create a new shared skill state.
    #[must_use]
    pub fn new_shared(index: SkillIndex, dirs: Vec<PathBuf>) -> SharedSkillState {
        Arc::new(tokio::sync::Mutex::new(Self::new(index, dirs)))
    }

    /// Activate a skill by name.
    ///
    /// Reads the full `SKILL.md`, parses the body, and adds it to the active list.
    ///
    /// # Errors
    /// Returns `IronclawError::Skills` if the skill is not found, already active,
    /// or the file cannot be read.
    pub async fn activate(&mut self, name: &str) -> Result<&ActiveSkill, IronclawError> {
        if self
            .active
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case(name))
        {
            return Err(IronclawError::Skills(format!(
                "skill '{name}' is already active"
            )));
        }

        let entry = self
            .index
            .find_by_name(name)
            .ok_or_else(|| IronclawError::Skills(format!("skill '{name}' not found")))?;

        let skill_md_path = entry.skill_dir.join("SKILL.md");
        let file_content = tokio::fs::read_to_string(&skill_md_path)
            .await
            .map_err(|e| {
                IronclawError::Skills(format!("failed to read SKILL.md for '{}': {e}", entry.name))
            })?;

        let (_fm, body) = parse_skill_md(&file_content)?;

        let skill_name = entry.name.clone();
        self.active.push(ActiveSkill {
            name: skill_name,
            body,
        });

        // Safe: we just pushed
        self.active.last().ok_or_else(|| {
            IronclawError::Skills("unexpected: active skill not set after activation".into())
        })
    }

    /// Deactivate a skill by name.
    ///
    /// # Errors
    /// Returns `IronclawError::Skills` if the skill is not currently active.
    pub fn deactivate(&mut self, name: &str) -> Result<(), IronclawError> {
        let pos = self
            .active
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                IronclawError::Skills(format!("skill '{name}' is not currently active"))
            })?;

        self.active.remove(pos);
        Ok(())
    }

    /// Rescan skill directories to rebuild the index.
    ///
    /// Removes any active skills whose names no longer appear in the new index.
    ///
    /// # Errors
    /// Returns `IronclawError::Skills` if scanning fails.
    pub async fn rescan(&mut self, project_skills_dir: Option<&Path>) -> Result<(), IronclawError> {
        self.index = SkillIndex::scan(&self.dirs, project_skills_dir).await?;

        // Remove active skills that no longer exist in the index
        self.active
            .retain(|a| self.index.find_by_name(&a.name).is_some());

        Ok(())
    }

    /// Format the skill index for the system prompt.
    #[must_use]
    pub fn format_index_for_prompt(&self) -> String {
        self.index.format_for_prompt()
    }

    /// Format active skill instructions for the system prompt.
    ///
    /// Returns `None` if no skills are active.
    #[must_use]
    pub fn format_active_for_prompt(&self) -> Option<String> {
        if self.active.is_empty() {
            return None;
        }

        let parts: Vec<String> = self
            .active
            .iter()
            .map(|skill| {
                format!(
                    "<active_skill name=\"{}\">\n{}\n</active_skill>",
                    skill.name, skill.body
                )
            })
            .collect();

        Some(parts.join("\n\n"))
    }

    /// Get the names of all active skills.
    #[must_use]
    pub fn active_skill_names(&self) -> Vec<&str> {
        self.active.iter().map(|a| a.name.as_str()).collect()
    }

    /// Get a reference to the current skill index.
    #[must_use]
    pub fn index(&self) -> &SkillIndex {
        &self.index
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    // ── parse_skill_md ───────────────────────────────────────────────────────

    #[test]
    fn parse_valid_skill() {
        let content = "---\nname: pdf-processing\ndescription: \"Extracts text from PDFs\"\n---\n\nUse this skill to process PDF files.\n";
        let (fm, body) = parse_skill_md(content).unwrap();
        assert_eq!(fm.name, "pdf-processing", "name should match");
        assert_eq!(
            fm.description, "Extracts text from PDFs",
            "description should match"
        );
        assert!(
            body.contains("Use this skill"),
            "body should contain instructions"
        );
    }

    #[test]
    fn parse_skill_no_body() {
        let content = "---\nname: minimal\ndescription: \"Minimal skill\"\n---\n";
        let (fm, body) = parse_skill_md(content).unwrap();
        assert_eq!(fm.name, "minimal", "name should match");
        assert!(body.is_empty(), "body should be empty");
    }

    #[test]
    fn parse_skill_missing_frontmatter() {
        let content = "name: bad\ndescription: \"No delimiters\"\n";
        assert!(
            parse_skill_md(content).is_err(),
            "missing delimiter should error"
        );
    }

    #[test]
    fn parse_skill_missing_name() {
        let content = "---\ndescription: \"No name field\"\n---\n";
        assert!(
            parse_skill_md(content).is_err(),
            "missing name should error"
        );
    }

    #[test]
    fn parse_skill_invalid_yaml() {
        let content = "---\n: invalid yaml [[\n---\n";
        assert!(
            parse_skill_md(content).is_err(),
            "invalid YAML should error"
        );
    }

    // ── validate_skill_name ──────────────────────────────────────────────────

    #[test]
    fn valid_names() {
        assert!(validate_skill_name("pdf-processing").is_ok());
        assert!(validate_skill_name("a").is_ok());
        assert!(validate_skill_name("skill123").is_ok());
        assert!(validate_skill_name("my-cool-skill").is_ok());
    }

    #[test]
    fn name_uppercase_rejected() {
        assert!(
            validate_skill_name("PDF-Processing").is_err(),
            "uppercase should be rejected"
        );
    }

    #[test]
    fn name_leading_hyphen_rejected() {
        assert!(
            validate_skill_name("-bad").is_err(),
            "leading hyphen should be rejected"
        );
    }

    #[test]
    fn name_trailing_hyphen_rejected() {
        assert!(
            validate_skill_name("bad-").is_err(),
            "trailing hyphen should be rejected"
        );
    }

    #[test]
    fn name_consecutive_hyphens_rejected() {
        assert!(
            validate_skill_name("bad--name").is_err(),
            "consecutive hyphens should be rejected"
        );
    }

    #[test]
    fn name_empty_rejected() {
        assert!(
            validate_skill_name("").is_err(),
            "empty name should be rejected"
        );
    }

    #[test]
    fn name_too_long_rejected() {
        let long_name = "a".repeat(65);
        assert!(
            validate_skill_name(&long_name).is_err(),
            "name over 64 chars should be rejected"
        );
    }

    #[test]
    fn name_special_chars_rejected() {
        assert!(
            validate_skill_name("bad_name").is_err(),
            "underscore should be rejected"
        );
        assert!(
            validate_skill_name("bad.name").is_err(),
            "period should be rejected"
        );
        assert!(
            validate_skill_name("bad name").is_err(),
            "space should be rejected"
        );
    }

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

    // ── SkillState ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn activate_and_deactivate() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n\nSkill body here.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        assert!(
            state.active_skill_names().is_empty(),
            "should start with no active skills"
        );

        let active = state.activate("test-skill").await.unwrap();
        assert_eq!(active.name, "test-skill");
        assert!(active.body.contains("Skill body here"));
        assert_eq!(state.active_skill_names(), vec!["test-skill"]);

        state.deactivate("test-skill").unwrap();
        assert!(
            state.active_skill_names().is_empty(),
            "should have no active skills after deactivation"
        );
    }

    #[tokio::test]
    async fn activate_nonexistent() {
        let index = SkillIndex::default();
        let mut state = SkillState::new(index, vec![]);

        let result = state.activate("nonexistent").await;
        assert!(result.is_err(), "should error for nonexistent skill");
    }

    #[tokio::test]
    async fn activate_already_active() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("test-skill").await.unwrap();
        let result = state.activate("test-skill").await;
        assert!(result.is_err(), "should error for already active skill");
    }

    #[test]
    fn deactivate_not_active() {
        let index = SkillIndex::default();
        let mut state = SkillState::new(index, vec![]);

        let result = state.deactivate("nonexistent");
        assert!(result.is_err(), "should error for inactive skill");
    }

    #[tokio::test]
    async fn rescan_removes_stale_active() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n\nBody.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("test-skill").await.unwrap();
        assert_eq!(state.active_skill_names().len(), 1);

        // Remove the skill directory
        tokio::fs::remove_dir_all(&skill_dir).await.unwrap();

        state.rescan(None).await.unwrap();
        assert!(
            state.active_skill_names().is_empty(),
            "stale active skill should be removed after rescan"
        );
    }

    #[test]
    fn format_active_none_when_empty() {
        let state = SkillState::new(SkillIndex::default(), vec![]);
        assert!(
            state.format_active_for_prompt().is_none(),
            "should return None when no skills active"
        );
    }

    #[tokio::test]
    async fn format_active_with_skills() {
        let dir = tempfile::tempdir().unwrap();

        let skill1 = dir.path().join("skill-a");
        tokio::fs::create_dir(&skill1).await.unwrap();
        tokio::fs::write(
            skill1.join("SKILL.md"),
            "---\nname: skill-a\ndescription: \"Skill A\"\n---\n\nBody A.\n",
        )
        .await
        .unwrap();

        let skill2 = dir.path().join("skill-b");
        tokio::fs::create_dir(&skill2).await.unwrap();
        tokio::fs::write(
            skill2.join("SKILL.md"),
            "---\nname: skill-b\ndescription: \"Skill B\"\n---\n\nBody B.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("skill-a").await.unwrap();
        state.activate("skill-b").await.unwrap();

        let output = state.format_active_for_prompt().unwrap();
        assert!(
            output.contains("<active_skill name=\"skill-a\">"),
            "should contain skill-a tag"
        );
        assert!(output.contains("Body A."), "should contain skill-a body");
        assert!(
            output.contains("<active_skill name=\"skill-b\">"),
            "should contain skill-b tag"
        );
        assert!(output.contains("Body B."), "should contain skill-b body");
    }
}
