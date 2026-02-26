//! Subagent preset data types: frontmatter and index entries.

use std::path::PathBuf;

use serde::Deserialize;

/// YAML frontmatter deserialized from a subagent preset `.md` file.
#[derive(Debug, Clone, Deserialize)]
pub struct SubagentPresetFrontmatter {
    /// Unique preset name (lowercase, alphanumeric + hyphens).
    pub name: String,
    /// Brief description shown in the index.
    pub description: String,
    /// Model tier to use (small/medium/large). Defaults to medium.
    pub model_tier: Option<String>,
    /// Tools removed from the subagent's available set.
    pub denied_tools: Option<Vec<String>>,
    /// If present, ONLY these tools are available. Mutually exclusive with `denied_tools`.
    pub allowed_tools: Option<Vec<String>>,
    /// Default result routing channels (overrideable at spawn time).
    pub channels: Option<Vec<String>>,
}

/// Lightweight index entry built from scanning a subagent preset's frontmatter.
#[derive(Debug, Clone)]
pub struct SubagentPresetEntry {
    /// Unique preset name.
    pub name: String,
    /// Brief description.
    pub description: String,
    /// Absolute path to the preset file (None for built-in presets).
    pub preset_path: Option<PathBuf>,
}
