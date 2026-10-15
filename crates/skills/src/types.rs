use std::path::PathBuf;

use serde::Deserialize;

use residuum_config::SkillSource;

/// YAML frontmatter deserialized from a `SKILL.md` file.
#[derive(Debug, Deserialize)]
pub(super) struct SkillFrontmatter {
    /// Unique skill name (lowercase, alphanumeric + hyphens).
    pub(super) name: String,
    /// Brief description shown in the index.
    pub(super) description: String,
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
    /// Absolute path to the skill directory this body was read from.
    ///
    /// Names alone aren't stable identity: a rescan can make the same name
    /// resolve to a different physical skill (e.g. a team skill
    /// shadowed by an agent skill of the same name). This field lets
    /// `rescan` detect that the backing source changed even though the name
    /// still matches.
    pub skill_dir: PathBuf,
}
