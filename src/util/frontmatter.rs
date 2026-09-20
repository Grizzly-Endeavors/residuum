//! Shared parsing for `---`-delimited YAML frontmatter and kebab-case name validation.
//!
//! Used by skill (`SKILL.md`) files, which pair a frontmatter block with a
//! body and use kebab-case names.

use anyhow::Context;
use serde::de::DeserializeOwned;

/// Parse a file with `---`-delimited YAML frontmatter into the deserialized
/// frontmatter and the trimmed body that follows it.
///
/// `label` identifies the file kind in error messages (e.g. `"SKILL.md"`),
/// producing messages like `"{label} missing frontmatter
/// delimiter '---'"` and `"failed to parse {label} frontmatter"`.
///
/// # Errors
/// Returns an error if the opening or closing `---` delimiter is missing, or
/// if the frontmatter YAML fails to deserialize into `T`.
pub fn parse_frontmatter_md<T>(content: &str, label: &str) -> anyhow::Result<(T, String)>
where
    T: DeserializeOwned,
{
    let trimmed = content.trim_start();

    let after_open = trimmed
        .strip_prefix("---")
        .ok_or_else(|| anyhow::anyhow!("{label} missing frontmatter delimiter '---'"))?;

    let (yaml_str, after_close) = after_open
        .split_once("\n---")
        .ok_or_else(|| anyhow::anyhow!("{label} missing closing frontmatter delimiter '---'"))?;

    let frontmatter: T = serde_yaml_ng::from_str(yaml_str)
        .with_context(|| format!("failed to parse {label} frontmatter"))?;

    let body = after_close.trim().to_string();

    Ok((frontmatter, body))
}

/// Validate a kebab-case name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens.
///
/// `kind` identifies what's being validated in error messages (e.g.
/// `"skill name"`).
///
/// # Errors
/// Returns an error if the name violates any of the above rules.
pub fn validate_kebab_name(name: &str, kind: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!(
            "{kind} must be 1-64 characters, got {len}",
            len = name.len()
        );
    }

    if name.starts_with('-') || name.ends_with('-') {
        anyhow::bail!("{kind} '{name}' must not start or end with a hyphen");
    }

    if name.contains("--") {
        anyhow::bail!("{kind} '{name}' must not contain consecutive hyphens");
    }

    for ch in name.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            anyhow::bail!(
                "{kind} '{name}' contains invalid character '{ch}' \
                 (only lowercase alphanumeric and hyphens allowed)"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_frontmatter_md, validate_kebab_name};
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct TestFrontmatter {
        name: String,
    }

    #[test]
    fn parses_valid_frontmatter_and_body() {
        let content = "---\nname: pdf-processing\n---\n\nBody text.\n";
        let (fm, body) = parse_frontmatter_md::<TestFrontmatter>(content, "test file").unwrap();
        assert_eq!(fm.name, "pdf-processing");
        assert_eq!(body, "Body text.");
    }

    #[test]
    fn missing_open_delimiter_errors_with_label() {
        let content = "name: bad\n";
        let err = parse_frontmatter_md::<TestFrontmatter>(content, "test file").unwrap_err();
        assert!(
            err.to_string()
                .contains("test file missing frontmatter delimiter")
        );
    }

    #[test]
    fn missing_close_delimiter_errors_with_label() {
        let content = "---\nname: bad\n";
        let err = parse_frontmatter_md::<TestFrontmatter>(content, "test file").unwrap_err();
        assert!(
            err.to_string()
                .contains("test file missing closing frontmatter delimiter")
        );
    }

    #[test]
    fn invalid_yaml_errors_with_label() {
        let content = "---\n: invalid [[\n---\n";
        let err = parse_frontmatter_md::<TestFrontmatter>(content, "test file").unwrap_err();
        assert!(
            err.to_string()
                .contains("failed to parse test file frontmatter")
        );
    }

    #[test]
    fn valid_kebab_name_accepted() {
        assert!(validate_kebab_name("pdf-processing", "skill name").is_ok());
        assert!(validate_kebab_name("a", "skill name").is_ok());
        assert!(validate_kebab_name(&"a".repeat(64), "skill name").is_ok());
    }

    #[test]
    fn invalid_kebab_names_rejected() {
        assert!(validate_kebab_name("", "skill name").is_err());
        assert!(validate_kebab_name(&"a".repeat(65), "skill name").is_err());
        assert!(validate_kebab_name("-bad", "skill name").is_err());
        assert!(validate_kebab_name("bad-", "skill name").is_err());
        assert!(validate_kebab_name("bad--name", "skill name").is_err());
        assert!(validate_kebab_name("Bad-Name", "skill name").is_err());
        assert!(validate_kebab_name("bad_name", "skill name").is_err());
    }

    #[test]
    fn error_message_uses_kind_label() {
        let err = validate_kebab_name("Bad", "skill name").unwrap_err();
        assert!(err.to_string().contains("skill name"));
    }
}
