//! Subagent preset index: scanning, lookup, and prompt formatting.

use std::path::Path;

use super::parser::parse_preset_md;
use super::types::{SubagentPresetEntry, SubagentPresetFrontmatter};
use crate::error::ResiduumError;

/// Built-in general-purpose preset name.
const GENERAL_PURPOSE_NAME: &str = "general-purpose";
const GENERAL_PURPOSE_DESCRIPTION: &str = "General-purpose subagent for self-contained tasks";
const GENERAL_PURPOSE_BODY: &str = "\
You are a general-purpose background worker. Complete the task described \
in your prompt. Use the available tools as needed. If you activate a \
project, deactivate it with a session log before finishing.";

/// In-memory index of discovered subagent presets.
#[derive(Debug, Clone, Default)]
pub struct SubagentPresetIndex {
    entries: Vec<SubagentPresetEntry>,
    /// Bodies for built-in presets (keyed by name).
    builtin_bodies: Vec<(String, SubagentPresetFrontmatter, String)>,
}

impl SubagentPresetIndex {
    /// Scan a directory for `*.md` preset files and combine with built-in presets.
    ///
    /// User-defined presets with the same name as a built-in override the built-in.
    /// Invalid or unparseable files are warned and skipped.
    ///
    /// # Errors
    /// Returns `ResiduumError::Subagents` if the directory cannot be read
    /// (except `NotFound`, which is silently skipped).
    pub async fn scan(dir: &Path) -> Result<Self, ResiduumError> {
        let mut entries = Vec::new();
        let mut seen_names: Vec<String> = Vec::new();
        let mut builtin_bodies = Vec::new();

        // Seed with built-in presets
        let (builtin_entry, builtin_fm, builtin_body) = builtin_general_purpose();
        entries.push(builtin_entry);
        seen_names.push(GENERAL_PURPOSE_NAME.to_string());
        builtin_bodies.push((GENERAL_PURPOSE_NAME.to_string(), builtin_fm, builtin_body));

        // Scan user-defined presets from disk
        tracing::debug!(dir = %dir.display(), "scanning subagent presets");
        scan_preset_directory(dir, &mut entries, &mut seen_names).await?;

        Ok(Self {
            entries,
            builtin_bodies,
        })
    }

    /// Look up a preset by name (case-insensitive).
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<&SubagentPresetEntry> {
        let lower = name.to_lowercase();
        self.entries.iter().find(|e| e.name.to_lowercase() == lower)
    }

    /// Load a preset's full frontmatter and body from disk (or from built-in).
    ///
    /// # Errors
    /// Returns `ResiduumError::Subagents` if the preset file cannot be read or parsed,
    /// or if the name is not found in the index.
    pub async fn load_preset(
        &self,
        name: &str,
    ) -> Result<(SubagentPresetFrontmatter, String), ResiduumError> {
        let lower = name.to_lowercase();

        let entry = self.find_by_name(name).ok_or_else(|| {
            let available: Vec<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
            ResiduumError::Subagents(format!(
                "unknown preset '{name}'. Available: {}",
                available.join(", ")
            ))
        })?;

        // If it has a path, load from disk
        if let Some(path) = &entry.preset_path {
            let content = tokio::fs::read_to_string(path).await.map_err(|e| {
                ResiduumError::Subagents(format!(
                    "failed to read preset file {}: {e}",
                    path.display()
                ))
            })?;
            let result = parse_preset_md(&content)?;
            tracing::debug!(name = %name, path = %path.display(), "loaded preset from disk");
            return Ok(result);
        }

        // Otherwise, check built-in presets
        for (builtin_name, fm, body) in &self.builtin_bodies {
            if builtin_name.to_lowercase() == lower {
                return Ok((fm.clone(), body.clone()));
            }
        }

        tracing::error!(name = %name, "preset found in index but has no path and is not a built-in — this is a bug in scan()");
        Err(ResiduumError::Subagents(format!(
            "preset '{name}' found in index but has no path and is not a built-in"
        )))
    }

    /// Format the index as XML for the system prompt.
    #[must_use]
    pub fn format_for_prompt(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }

        let mut parts = Vec::with_capacity(self.entries.len() + 2);
        parts.push("<available_subagents>".to_string());

        for entry in &self.entries {
            parts.push(format!(
                "  <subagent>\n    <name>{}</name>\n    <description>{}</description>\n  </subagent>",
                entry.name, entry.description,
            ));
        }

        parts.push("</available_subagents>".to_string());
        parts.join("\n")
    }

    /// Get all index entries.
    #[must_use]
    pub fn entries(&self) -> &[SubagentPresetEntry] {
        &self.entries
    }

    /// List all available preset names (for error messages).
    #[must_use]
    pub fn available_names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }
}

/// Construct the built-in general-purpose preset.
fn builtin_general_purpose() -> (SubagentPresetEntry, SubagentPresetFrontmatter, String) {
    let entry = SubagentPresetEntry {
        name: GENERAL_PURPOSE_NAME.to_string(),
        description: GENERAL_PURPOSE_DESCRIPTION.to_string(),
        preset_path: None,
    };
    let fm = SubagentPresetFrontmatter {
        name: GENERAL_PURPOSE_NAME.to_string(),
        description: GENERAL_PURPOSE_DESCRIPTION.to_string(),
        model_tier: None,
        denied_tools: None,
        allowed_tools: None,
    };
    (entry, fm, GENERAL_PURPOSE_BODY.to_string())
}

/// Scan a single directory for `*.md` preset files.
async fn scan_preset_directory(
    dir: &Path,
    entries: &mut Vec<SubagentPresetEntry>,
    seen_names: &mut Vec<String>,
) -> Result<(), ResiduumError> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(dir = %dir.display(), "subagents directory not found, skipping");
            return Ok(());
        }
        Err(e) => {
            return Err(ResiduumError::Subagents(format!(
                "failed to read subagents directory {}: {e}",
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
                    "failed to read subagents directory entry"
                );
                continue;
            }
        };

        let path = entry.path();

        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            continue;
        }

        let file_type = match entry.file_type().await {
            Ok(ft) => ft,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to get file type for subagent preset"
                );
                continue;
            }
        };

        if file_type.is_dir() {
            continue;
        }

        let file_content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to read subagent preset file"
                );
                continue;
            }
        };

        match parse_preset_md(&file_content) {
            Ok((fm, _body)) => register_preset(fm, path, entries, seen_names),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping preset with invalid frontmatter"
                );
            }
        }
    }

    tracing::debug!(
        dir = %dir.display(),
        loaded = entries.len(),
        "finished scanning subagents directory"
    );

    Ok(())
}

fn register_preset(
    fm: SubagentPresetFrontmatter,
    path: std::path::PathBuf,
    entries: &mut Vec<SubagentPresetEntry>,
    seen_names: &mut Vec<String>,
) {
    let lower = fm.name.to_lowercase();

    if let Some(pos) = seen_names.iter().position(|n| *n == lower) {
        if entries.get(pos).is_some_and(|e| e.preset_path.is_none()) {
            tracing::info!(
                name = %fm.name,
                path = %path.display(),
                "user preset overrides built-in"
            );
            if let Some(slot) = entries.get_mut(pos) {
                *slot = SubagentPresetEntry {
                    name: fm.name,
                    description: fm.description,
                    preset_path: Some(path),
                };
            }
            return;
        }

        let kept_path = entries
            .get(pos)
            .and_then(|e| e.preset_path.as_ref())
            .map_or_else(|| "built-in".to_string(), |p| p.display().to_string());
        tracing::warn!(
            name = %fm.name,
            rejected = %path.display(),
            kept = %kept_path,
            "duplicate preset name, keeping first found"
        );
        return;
    }

    seen_names.push(lower);
    entries.push(SubagentPresetEntry {
        name: fm.name,
        description: fm.description,
        preset_path: Some(path),
    });
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes known-length slices"
)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[tokio::test]
    async fn scan_empty_dir_has_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();
        assert_eq!(
            index.entries().len(),
            1,
            "should have general-purpose built-in"
        );
        assert_eq!(index.entries()[0].name, "general-purpose");
    }

    #[tokio::test]
    async fn scan_nonexistent_dir_has_builtin() {
        let index = SubagentPresetIndex::scan(&PathBuf::from("/tmp/nonexistent-subagents-dir"))
            .await
            .unwrap();
        assert_eq!(
            index.entries().len(),
            1,
            "should still have general-purpose built-in"
        );
    }

    #[tokio::test]
    async fn scan_with_valid_preset() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("researcher.md"),
            "---\nname: researcher\ndescription: \"Research agent\"\n---\n\nDo research.\n",
        )
        .await
        .unwrap();

        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();
        assert_eq!(
            index.entries().len(),
            2,
            "should have built-in + user preset"
        );
        let researcher = index.find_by_name("researcher").unwrap();
        assert_eq!(researcher.description, "Research agent");
        assert!(researcher.preset_path.is_some());
    }

    #[tokio::test]
    async fn user_preset_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("general-purpose.md"),
            "---\nname: general-purpose\ndescription: \"Custom general-purpose\"\n---\n\nCustom instructions.\n",
        )
        .await
        .unwrap();

        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();
        assert_eq!(
            index.entries().len(),
            1,
            "should have one entry (override, not duplicate)"
        );
        let gp = index.find_by_name("general-purpose").unwrap();
        assert_eq!(gp.description, "Custom general-purpose");
        assert!(
            gp.preset_path.is_some(),
            "overridden entry should have a file path"
        );
    }

    #[tokio::test]
    async fn scan_skips_invalid_frontmatter() {
        let dir = tempfile::tempdir().unwrap();

        tokio::fs::write(dir.path().join("bad.md"), "---\n: invalid [[\n---\n")
            .await
            .unwrap();

        tokio::fs::write(
            dir.path().join("good.md"),
            "---\nname: good\ndescription: \"Valid preset\"\n---\n",
        )
        .await
        .unwrap();

        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();
        assert_eq!(
            index.entries().len(),
            2,
            "should have built-in + valid preset"
        );
        assert!(
            index.find_by_name("good").is_some(),
            "good preset should be in index"
        );
    }

    #[tokio::test]
    async fn scan_skips_non_md_files() {
        let dir = tempfile::tempdir().unwrap();

        tokio::fs::write(dir.path().join("notes.txt"), "just a text file")
            .await
            .unwrap();

        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();
        assert_eq!(
            index.entries().len(),
            1,
            "should only have built-in (txt file skipped)"
        );
    }

    #[tokio::test]
    async fn duplicate_names_keeps_first() {
        let dir = tempfile::tempdir().unwrap();

        // Create two files with the same preset name.
        // Filesystem ordering isn't guaranteed, so we check that exactly one is kept.
        tokio::fs::write(
            dir.path().join("first.md"),
            "---\nname: duplicate\ndescription: \"First\"\n---\n",
        )
        .await
        .unwrap();

        tokio::fs::write(
            dir.path().join("second.md"),
            "---\nname: duplicate\ndescription: \"Second\"\n---\n",
        )
        .await
        .unwrap();

        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();
        let dup_count = index
            .entries()
            .iter()
            .filter(|e| e.name == "duplicate")
            .count();
        assert_eq!(dup_count, 1, "should deduplicate by name");
    }

    #[test]
    fn find_by_name_case_insensitive() {
        let index = SubagentPresetIndex {
            entries: vec![SubagentPresetEntry {
                name: "researcher".to_string(),
                description: "Research agent".to_string(),
                preset_path: Some(PathBuf::from("/tmp/researcher.md")),
            }],
            builtin_bodies: Vec::new(),
        };

        assert!(
            index.find_by_name("RESEARCHER").is_some(),
            "should find case-insensitive"
        );
        assert!(
            index.find_by_name("nonexistent").is_none(),
            "should not find missing"
        );
    }

    #[test]
    fn format_for_prompt_empty() {
        let index = SubagentPresetIndex {
            entries: Vec::new(),
            builtin_bodies: Vec::new(),
        };
        assert!(
            index.format_for_prompt().is_empty(),
            "empty index should produce empty string"
        );
    }

    #[test]
    fn format_for_prompt_with_entries() {
        let index = SubagentPresetIndex {
            entries: vec![
                SubagentPresetEntry {
                    name: "general-purpose".to_string(),
                    description: "General-purpose subagent".to_string(),
                    preset_path: None,
                },
                SubagentPresetEntry {
                    name: "researcher".to_string(),
                    description: "Research specialist".to_string(),
                    preset_path: Some(PathBuf::from("/tmp/researcher.md")),
                },
            ],
            builtin_bodies: Vec::new(),
        };

        let output = index.format_for_prompt();
        assert!(
            output.contains("<available_subagents>"),
            "should have opening tag"
        );
        assert!(
            output.contains("</available_subagents>"),
            "should have closing tag"
        );
        assert!(
            output.contains("<name>general-purpose</name>"),
            "should contain built-in name"
        );
        assert!(
            output.contains("<name>researcher</name>"),
            "should contain user preset name"
        );
        assert!(
            output.contains("<description>Research specialist</description>"),
            "should contain description"
        );
    }

    #[tokio::test]
    async fn load_preset_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();

        let (fm, body) = index.load_preset("general-purpose").await.unwrap();
        assert_eq!(fm.name, "general-purpose");
        assert!(!body.is_empty(), "built-in should have a body");
        assert!(
            body.contains("general-purpose background worker"),
            "body should contain built-in instructions"
        );
    }

    #[tokio::test]
    async fn load_preset_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("coder.md"),
            "---\nname: coder\ndescription: \"Code writer\"\nmodel_tier: large\n---\n\nWrite clean code.\n",
        )
        .await
        .unwrap();

        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();
        let (fm, body) = index.load_preset("coder").await.unwrap();
        assert_eq!(fm.name, "coder");
        assert_eq!(fm.model_tier.as_deref(), Some("large"));
        assert!(body.contains("clean code"));
    }

    #[tokio::test]
    async fn load_preset_unknown_name() {
        let dir = tempfile::tempdir().unwrap();
        let index = SubagentPresetIndex::scan(dir.path()).await.unwrap();

        let result = index.load_preset("nonexistent").await;
        assert!(result.is_err(), "unknown name should error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unknown preset"),
            "error should say unknown"
        );
        assert!(
            err_msg.contains("general-purpose"),
            "error should list available presets"
        );
    }
}
