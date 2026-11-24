use std::path::PathBuf;

use serde::Deserialize;

use residuum_config::SkillSource;

#[derive(Debug, Deserialize)]
pub(super) struct SkillFrontmatter {
    pub(super) name: String,
    pub(super) description: String,
}

#[derive(Debug, Clone)]
pub struct SkillIndexEntry {
    pub name: String,
    pub description: String,
    pub(crate) skill_dir: PathBuf,
    pub source: SkillSource,
}

#[derive(Debug, Clone)]
pub struct ActiveSkill {
    pub name: String,
    pub body: String,
    pub(crate) skill_dir: PathBuf,
}
