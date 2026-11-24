use residuum_core::util::{parse_frontmatter_md, validate_kebab_name};

use super::types::SkillFrontmatter;

pub(super) const NOTICE_DESCRIPTION_LEN: usize = 1024;

const APPROX_CHARS_PER_TOKEN: usize = 4;

pub(super) fn parse_skill_md(content: &str) -> anyhow::Result<(SkillFrontmatter, String)> {
    let (frontmatter, body): (SkillFrontmatter, String) =
        parse_frontmatter_md(content, "SKILL.md")?;

    validate_skill_name(&frontmatter.name)?;
    validate_skill_description(&frontmatter.description)?;

    Ok((frontmatter, body))
}

#[must_use]
pub fn diagnose_skill_md(content: &str) -> Vec<residuum_core::diagnostics::Diagnostic> {
    use residuum_core::diagnostics::{Diagnostic, Location};

    match parse_skill_md(content) {
        Ok((frontmatter, _body)) => {
            match oversized_description_notice(&frontmatter.name, &frontmatter.description) {
                Some(notice) => vec![Diagnostic::warning(notice)],
                None => Vec::new(),
            }
        }
        Err(e) => {
            let location = e
                .chain()
                .find_map(|cause| cause.downcast_ref::<serde_yaml_ng::Error>())
                .and_then(serde_yaml_ng::Error::location)
                .map(|loc| Location::LineColumn {
                    line: u32::try_from(loc.line())
                        .unwrap_or(u32::MAX)
                        .saturating_add(1),
                    column: u32::try_from(loc.column()).unwrap_or(u32::MAX),
                });
            vec![match location {
                Some(loc) => Diagnostic::error_at(e.to_string(), loc),
                None => Diagnostic::error(e.to_string()),
            }]
        }
    }
}

pub(super) fn validate_skill_name(name: &str) -> anyhow::Result<()> {
    validate_kebab_name(name, "skill name")
}

pub(super) fn validate_skill_description(description: &str) -> anyhow::Result<()> {
    if description.trim().is_empty() {
        anyhow::bail!("skill description must not be empty");
    }

    Ok(())
}

pub(super) fn oversized_description_notice(skill_name: &str, description: &str) -> Option<String> {
    let len = description.chars().count();
    if len <= NOTICE_DESCRIPTION_LEN {
        return None;
    }

    let approx_tokens = len / APPROX_CHARS_PER_TOKEN;
    Some(format!(
        "skill '{skill_name}' has a {len}-character description (~{approx_tokens} tokens) that is sent on every turn — consider shortening it"
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        diagnose_skill_md, oversized_description_notice, parse_skill_md,
        validate_skill_description, validate_skill_name,
    };

    #[test]
    fn parse_valid_skill() {
        let content = "---\nname: pdf-processing\ndescription: \"Extracts text from PDFs\"\n---\n\nUse this skill to process PDF files.\n";
        let (fm, body) = parse_skill_md(content).unwrap();
        assert_eq!(fm.name, "pdf-processing", "name should match");
        assert_eq!(
            fm.description, "Extracts text from PDFs",
            "description should match"
        );
        assert_eq!(body, "Use this skill to process PDF files.");
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

    #[test]
    fn parse_skill_empty_description_rejected() {
        let content = "---\nname: my-skill\ndescription: \"\"\n---\n";
        assert!(
            parse_skill_md(content).is_err(),
            "empty description should error"
        );
    }

    #[test]
    fn parse_skill_description_too_long_accepted() {
        let long_description = "a".repeat(1025);
        let content = format!("---\nname: my-skill\ndescription: \"{long_description}\"\n---\n");
        assert!(
            parse_skill_md(&content).is_ok(),
            "description over 1024 chars should still parse and load; there is no cap"
        );
    }

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
    fn name_exactly_64_chars_accepted() {
        let name = "a".repeat(64);
        assert!(validate_skill_name(&name).is_ok());
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

    #[test]
    fn valid_description_accepted() {
        assert!(validate_skill_description("Extracts text from PDFs").is_ok());
    }

    #[test]
    fn description_empty_rejected() {
        assert!(
            validate_skill_description("").is_err(),
            "empty description should be rejected"
        );
    }

    #[test]
    fn description_whitespace_only_rejected() {
        assert!(
            validate_skill_description("   \n\t  ").is_err(),
            "whitespace-only description should be rejected"
        );
    }

    #[test]
    fn description_exactly_notice_len_accepted() {
        let description = "a".repeat(1024);
        assert!(validate_skill_description(&description).is_ok());
    }

    #[test]
    fn description_too_long_still_accepted() {
        let description = "a".repeat(1025);
        assert!(
            validate_skill_description(&description).is_ok(),
            "there is no length cap on skill descriptions"
        );
    }

    #[test]
    fn oversized_description_notice_none_under_threshold() {
        assert!(oversized_description_notice("my-skill", &"a".repeat(1024)).is_none());
    }

    #[test]
    fn oversized_description_notice_names_skill_and_length() {
        let description = "a".repeat(1025);
        let notice = oversized_description_notice("my-skill", &description)
            .expect("over-threshold description should produce a notice");
        assert!(notice.contains("my-skill"), "notice should name the skill");
        assert!(
            notice.contains("1025"),
            "notice should include the description length"
        );
        assert!(
            notice.contains("every turn"),
            "notice should mention the recurring per-turn cost"
        );
    }

    #[test]
    fn diagnose_valid_skill_has_no_diagnostics() {
        let content = "---\nname: pdf-processing\ndescription: \"Extracts text from PDFs\"\n---\n";
        let actual = &diagnose_skill_md(content);
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
    }

    #[test]
    fn diagnose_warns_on_oversized_description_without_failing() {
        let long_description = "a".repeat(1025);
        let content = format!("---\nname: my-skill\ndescription: \"{long_description}\"\n---\n");
        let diagnostics = diagnose_skill_md(&content);
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.first().unwrap();
        assert_eq!(
            diagnostic.severity,
            residuum_core::diagnostics::Severity::Warning
        );
        assert!(diagnostic.message.contains("1025"));
        assert!(diagnostic.message.contains("my-skill"));
    }

    #[test]
    fn diagnose_reports_nothing_for_a_normal_description() {
        let content = "---\nname: my-skill\ndescription: \"Extracts text from PDFs\"\n---\n";
        let actual_2 = &diagnose_skill_md(content);
        assert!(actual_2.is_empty(), "expected {actual_2:?} to be empty");
    }

    #[test]
    fn diagnose_reports_invalid_yaml_with_location() {
        use residuum_core::diagnostics::Location;

        let content = "---\n: invalid yaml [[\n---\n";
        let diagnostics = diagnose_skill_md(content);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            matches!(
                diagnostics.first().unwrap().location,
                Some(Location::LineColumn { .. })
            ),
            "invalid YAML should carry a line/column: {diagnostics:?}"
        );
    }

    #[test]
    fn all_bundled_skills_parse() {
        let bundled_root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/bundled-skills");

        let mut skill_dirs: Vec<PathBuf> = std::fs::read_dir(&bundled_root)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", bundled_root.display()))
            .map(|entry| entry.unwrap_or_else(|e| panic!("failed to read a dir entry: {e}")))
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        skill_dirs.sort();

        assert!(
            !skill_dirs.is_empty(),
            "no bundled skill directories found under {}",
            bundled_root.display()
        );

        let failures: Vec<String> = skill_dirs
            .into_iter()
            .filter_map(|dir| {
                let skill_md = dir.join("SKILL.md");
                let content = std::fs::read_to_string(&skill_md)
                    .unwrap_or_else(|e| panic!("failed to read {}: {e}", skill_md.display()));
                parse_skill_md(&content)
                    .err()
                    .map(|e| format!("{}: {e}", skill_md.display()))
            })
            .collect();

        assert!(
            failures.is_empty(),
            "bundled skill(s) failed to parse with the real skill parser:\n{}",
            failures.join("\n")
        );
    }
}
