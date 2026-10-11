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

/// Which layer a skill was discovered in.
///
/// Layers are searched in the order agent, team, configured; the first skill
/// with a given name wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    /// From the agent's own `skills/` directory.
    Agent,
    /// From the shared `team/skills/` directory.
    Team,
    /// From an extra directory configured in `[skills].dirs`.
    Configured,
}

impl fmt::Display for SkillSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent => write!(f, "agent"),
            Self::Team => write!(f, "team"),
            Self::Configured => write!(f, "configured"),
        }
    }
}

/// A directory to scan for skills, tagged with the layer it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDir {
    /// Directory holding one subfolder per skill.
    pub path: PathBuf,
    /// Layer skills found here are attributed to.
    pub source: SkillSource,
}

impl SkillDir {
    /// A directory in the given layer.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, source: SkillSource) -> Self {
        Self {
            path: path.into(),
            source,
        }
    }

    /// The agent's own skills directory.
    #[must_use]
    pub fn agent(path: impl Into<PathBuf>) -> Self {
        Self::new(path, SkillSource::Agent)
    }

    /// The shared team skills directory.
    #[must_use]
    pub fn team(path: impl Into<PathBuf>) -> Self {
        Self::new(path, SkillSource::Team)
    }

    /// A directory from `[skills].dirs`.
    #[must_use]
    pub fn configured(path: impl Into<PathBuf>) -> Self {
        Self::new(path, SkillSource::Configured)
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
    /// Absolute path to the skill directory this body was read from.
    ///
    /// Names alone aren't stable identity: a rescan can make the same name
    /// resolve to a different physical skill (e.g. a team skill
    /// shadowed by an agent skill of the same name). This field lets
    /// `rescan` detect that the backing source changed even though the name
    /// still matches.
    pub skill_dir: PathBuf,
}
