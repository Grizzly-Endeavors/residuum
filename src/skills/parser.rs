use anyhow::Context;

use super::types::SkillFrontmatter;

/// Parse a `SKILL.md` file into frontmatter and body.
///
/// Expects YAML frontmatter delimited by `---` at the start of the file.
/// Validates the skill name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens.
///
/// # Errors
/// Returns an error if the frontmatter is missing, invalid YAML, or the
/// name fails validation.
pub(super) fn parse_skill_md(content: &str) -> anyhow::Result<(SkillFrontmatter, String)> {
    let trimmed = content.trim_start();

    let after_open = trimmed
        .strip_prefix("---")
        .ok_or_else(|| anyhow::anyhow!("SKILL.md missing frontmatter delimiter '---'"))?;

    let (yaml_str, after_close) = after_open
        .split_once("\n---")
        .ok_or_else(|| anyhow::anyhow!("SKILL.md missing closing frontmatter delimiter '---'"))?;

    let frontmatter: SkillFrontmatter =
        serde_yml::from_str(yaml_str).context("failed to parse SKILL.md frontmatter")?;

    validate_skill_name(&frontmatter.name)?;

    let body = after_close.trim().to_string();

    Ok((frontmatter, body))
}

/// Validate a skill name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens.
pub(super) fn validate_skill_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!(
            "skill name must be 1-64 characters, got {len}",
            len = name.len()
        );
    }

    if name.starts_with('-') || name.ends_with('-') {
        anyhow::bail!("skill name '{name}' must not start or end with a hyphen");
    }

    if name.contains("--") {
        anyhow::bail!("skill name '{name}' must not contain consecutive hyphens");
    }

    for ch in name.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            anyhow::bail!(
                "skill name '{name}' contains invalid character '{ch}' \
                 (only lowercase alphanumeric and hyphens allowed)"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::{parse_skill_md, validate_skill_name};

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
}
