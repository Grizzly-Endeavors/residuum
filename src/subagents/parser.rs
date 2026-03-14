//! Subagent preset parsing: frontmatter extraction and name validation.

use super::types::SubagentPresetFrontmatter;
use crate::error::ResiduumError;

/// Parse a subagent preset `.md` file into frontmatter and body.
///
/// Expects YAML frontmatter delimited by `---` at the start of the file.
/// Validates the preset name and checks that `denied_tools` and `allowed_tools`
/// are not both set.
///
/// # Errors
/// Returns `ResiduumError::Subagents` if the frontmatter is missing, invalid
/// YAML, the name fails validation, or both tool restriction fields are set.
pub fn parse_preset_md(
    content: &str,
) -> Result<(SubagentPresetFrontmatter, String), ResiduumError> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Err(ResiduumError::Subagents(
            "preset file missing frontmatter delimiter '---'".to_string(),
        ));
    }

    let after_open = trimmed
        .get(3..)
        .ok_or_else(|| ResiduumError::Subagents("preset file is too short".to_string()))?;

    let close_pos = after_open.find("\n---").ok_or_else(|| {
        ResiduumError::Subagents(
            "preset file missing closing frontmatter delimiter '---'".to_string(),
        )
    })?;

    let yaml_str = after_open
        .get(..close_pos)
        .ok_or_else(|| ResiduumError::Subagents("failed to extract YAML content".to_string()))?;

    let frontmatter: SubagentPresetFrontmatter = serde_yml::from_str(yaml_str).map_err(|e| {
        ResiduumError::Subagents(format!("failed to parse preset frontmatter: {e}"))
    })?;

    validate_preset_name(&frontmatter.name)?;

    // Reject if both denied_tools and allowed_tools are set
    if frontmatter.denied_tools.is_some() && frontmatter.allowed_tools.is_some() {
        return Err(ResiduumError::Subagents(format!(
            "preset '{}' has both denied_tools and allowed_tools — only one is allowed",
            frontmatter.name
        )));
    }

    let body_start = 3 + close_pos + 4; // "---" prefix + yaml + "\n---"
    let body = trimmed.get(body_start..).unwrap_or("").trim().to_string();

    Ok((frontmatter, body))
}

/// Validate a preset name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens.
///
/// Uses the same rules as skill names.
///
/// # Errors
/// Returns `ResiduumError::Subagents` if the name is invalid.
pub fn validate_preset_name(name: &str) -> Result<(), ResiduumError> {
    if name.is_empty() || name.len() > 64 {
        return Err(ResiduumError::Subagents(format!(
            "preset name must be 1-64 characters, got {len}",
            len = name.len()
        )));
    }

    if name.starts_with('-') || name.ends_with('-') {
        return Err(ResiduumError::Subagents(format!(
            "preset name '{name}' must not start or end with a hyphen"
        )));
    }

    if name.contains("--") {
        return Err(ResiduumError::Subagents(format!(
            "preset name '{name}' must not contain consecutive hyphens"
        )));
    }

    for ch in name.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(ResiduumError::Subagents(format!(
                "preset name '{name}' contains invalid character '{ch}' \
                 (only lowercase alphanumeric and hyphens allowed)"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::{parse_preset_md, validate_preset_name};

    // ── parse_preset_md ────────────────────────────────────────────────────

    #[test]
    fn parse_valid_preset_all_fields() {
        let content = r#"---
name: researcher
description: "Research specialist for gathering information"
model_tier: small
denied_tools:
  - exec
  - write_file
channels:
  - inbox
---

You are a research specialist. Focus on gathering information.
"#;
        let (fm, body) = parse_preset_md(content).unwrap();
        assert_eq!(fm.name, "researcher");
        assert_eq!(
            fm.description,
            "Research specialist for gathering information"
        );
        assert_eq!(fm.model_tier.as_deref(), Some("small"));
        assert_eq!(
            fm.denied_tools.as_deref(),
            Some(vec!["exec".to_string(), "write_file".to_string()].as_slice())
        );
        assert!(fm.allowed_tools.is_none());
        assert_eq!(
            fm.channels.as_deref(),
            Some(vec!["inbox".to_string()].as_slice())
        );
        assert!(body.contains("research specialist"));
    }

    #[test]
    fn parse_minimal_preset() {
        let content = "---\nname: simple\ndescription: \"A simple preset\"\n---\n";
        let (fm, body) = parse_preset_md(content).unwrap();
        assert_eq!(fm.name, "simple");
        assert_eq!(fm.description, "A simple preset");
        assert!(fm.model_tier.is_none());
        assert!(fm.denied_tools.is_none());
        assert!(fm.allowed_tools.is_none());
        assert!(fm.channels.is_none());
        assert!(body.is_empty());
    }

    #[test]
    fn parse_missing_frontmatter() {
        let content = "name: bad\ndescription: \"No delimiters\"\n";
        assert!(
            parse_preset_md(content).is_err(),
            "missing delimiter should error"
        );
    }

    #[test]
    fn parse_invalid_yaml() {
        let content = "---\n: invalid [[\n---\n";
        assert!(
            parse_preset_md(content).is_err(),
            "invalid YAML should error"
        );
    }

    #[test]
    fn parse_both_denied_and_allowed() {
        let content = r#"---
name: conflicting
description: "Has both tool restrictions"
denied_tools:
  - exec
allowed_tools:
  - read_file
---
"#;
        let result = parse_preset_md(content);
        assert!(result.is_err(), "both denied and allowed should error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("both denied_tools and allowed_tools"),
            "error should mention the conflict"
        );
    }

    #[test]
    fn parse_preset_with_allowed_tools() {
        let content = r#"---
name: read-only
description: "Can only read files"
allowed_tools:
  - read_file
  - memory_search
---

You can only read files and search memory.
"#;
        let (fm, body) = parse_preset_md(content).unwrap();
        assert_eq!(fm.name, "read-only");
        assert!(fm.denied_tools.is_none());
        assert_eq!(
            fm.allowed_tools.as_deref(),
            Some(vec!["read_file".to_string(), "memory_search".to_string()].as_slice())
        );
        assert!(body.contains("only read files"));
    }

    // ── validate_preset_name ───────────────────────────────────────────────

    #[test]
    fn valid_names() {
        assert!(validate_preset_name("general-purpose").is_ok());
        assert!(validate_preset_name("a").is_ok());
        assert!(validate_preset_name("researcher").is_ok());
        assert!(validate_preset_name("my-cool-agent").is_ok());
    }

    #[test]
    fn name_uppercase_rejected() {
        assert!(
            validate_preset_name("Researcher").is_err(),
            "uppercase should be rejected"
        );
    }

    #[test]
    fn name_leading_hyphen_rejected() {
        assert!(
            validate_preset_name("-bad").is_err(),
            "leading hyphen should be rejected"
        );
    }

    #[test]
    fn name_trailing_hyphen_rejected() {
        assert!(
            validate_preset_name("bad-").is_err(),
            "trailing hyphen should be rejected"
        );
    }

    #[test]
    fn name_consecutive_hyphens_rejected() {
        assert!(
            validate_preset_name("bad--name").is_err(),
            "consecutive hyphens should be rejected"
        );
    }

    #[test]
    fn name_empty_rejected() {
        assert!(
            validate_preset_name("").is_err(),
            "empty name should be rejected"
        );
    }

    #[test]
    fn name_too_long_rejected() {
        let long_name = "a".repeat(65);
        assert!(
            validate_preset_name(&long_name).is_err(),
            "name over 64 chars should be rejected"
        );
    }

    #[test]
    fn name_special_chars_rejected() {
        assert!(
            validate_preset_name("bad_name").is_err(),
            "underscore should be rejected"
        );
        assert!(
            validate_preset_name("bad.name").is_err(),
            "period should be rejected"
        );
        assert!(
            validate_preset_name("bad name").is_err(),
            "space should be rejected"
        );
    }
}
