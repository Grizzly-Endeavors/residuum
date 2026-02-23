use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

/// YAML frontmatter deserialized from a `SKILL.md` file.
#[derive(Debug, Deserialize)]
pub(super) struct SkillFrontmatter {
    /// Unique skill name (lowercase, alphanumeric + hyphens).
    pub(super) name: String,
    /// Brief description shown in the index.
    pub(super) description: String,
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
