use crate::util::{parse_frontmatter_md, validate_kebab_name};

use super::types::SkillFrontmatter;

/// Hard ceiling on `description` length.
///
/// `skill-authoring` doctrine (see `docs/systems-usage/skills.md`) asks
/// authors to keep descriptions under ~60 characters so the skill index
/// stays scannable. That's a style guideline, not a wire limit, so the
/// enforced cap here is generous headroom above it — just enough to catch
/// a runaway multi-paragraph description before it gets concatenated into
/// every `<available_skills>` block sent on every turn.
pub(super) const MAX_DESCRIPTION_LEN: usize = 280;

/// Parse a `SKILL.md` file into frontmatter and body.
///
/// Expects YAML frontmatter delimited by `---` at the start of the file.
/// Validates the skill name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens. Validates the description is
/// non-empty and no longer than `MAX_DESCRIPTION_LEN` chars.
///
/// # Errors
/// Returns an error if the frontmatter is missing, invalid YAML, or the
/// name or description fails validation.
pub(super) fn parse_skill_md(content: &str) -> anyhow::Result<(SkillFrontmatter, String)> {
    let (frontmatter, body): (SkillFrontmatter, String) =
        parse_frontmatter_md(content, "SKILL.md")?;

    validate_skill_name(&frontmatter.name)?;
    validate_skill_description(&frontmatter.description)?;

    Ok((frontmatter, body))
}

/// Diagnostics for `content` as a skill's `SKILL.md` frontmatter.
///
/// Reuses [`parse_skill_md`], so a diagnostic can never disagree with what
/// loading rejects — a skill with an invalid name, an over-long or empty
/// description, or invalid YAML fails to parse here exactly as it would when
/// the skill scanner loads it. Extracts a line/column when the failure is a
/// YAML error; a name/description validation failure has no source position
/// once deserialization has already succeeded, so it's reported by message
/// alone.
pub(crate) fn diagnose_skill_md(content: &str) -> Vec<crate::diagnostics::Diagnostic> {
    use crate::diagnostics::{Diagnostic, Location};

    match parse_skill_md(content) {
        Ok(_) => Vec::new(),
        Err(e) => {
            let location = e
                .chain()
                .find_map(|cause| cause.downcast_ref::<serde_yaml_ng::Error>())
                .and_then(serde_yaml_ng::Error::location)
                .map(|loc| Location::LineColumn {
                    // +1: `parse_frontmatter_md` parses the YAML block after
                    // stripping the opening "---" line, so a position inside
                    // it is one line short of the position in the full file.
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

/// Validate a skill name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens.
pub(super) fn validate_skill_name(name: &str) -> anyhow::Result<()> {
    validate_kebab_name(name, "skill name")
}

/// Validate a skill description: non-empty and no longer than
/// `MAX_DESCRIPTION_LEN` chars.
///
/// Every activated skill's description is concatenated into the
/// `<available_skills>` block sent on every turn, so an unbounded
/// description silently bloats every subsequent prompt.
pub(super) fn validate_skill_description(description: &str) -> anyhow::Result<()> {
    if description.trim().is_empty() {
        anyhow::bail!("skill description must not be empty");
    }

    let len = description.chars().count();
    if len > MAX_DESCRIPTION_LEN {
        anyhow::bail!(
            "skill description must be at most {MAX_DESCRIPTION_LEN} characters, got {len}"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        diagnose_skill_md, parse_skill_md, validate_skill_description, validate_skill_name,
    };

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
    fn parse_skill_description_too_long_rejected() {
        let long_description = "a".repeat(281);
        let content = format!("---\nname: my-skill\ndescription: \"{long_description}\"\n---\n");
        assert!(
            parse_skill_md(&content).is_err(),
            "description over 280 chars should error"
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

    // ── validate_skill_description ──────────────────────────────────────────

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
    fn description_exactly_max_len_accepted() {
        let description = "a".repeat(280);
        assert!(validate_skill_description(&description).is_ok());
    }

    #[test]
    fn description_too_long_rejected() {
        let description = "a".repeat(281);
        assert!(
            validate_skill_description(&description).is_err(),
            "description over 280 chars should be rejected"
        );
    }

    // ── diagnose_skill_md ────────────────────────────────────────────────────

    #[test]
    fn diagnose_valid_skill_has_no_diagnostics() {
        let content = "---\nname: pdf-processing\ndescription: \"Extracts text from PDFs\"\n---\n";
        assert!(diagnose_skill_md(content).is_empty());
    }

    #[test]
    fn diagnose_reports_description_too_long() {
        let long_description = "a".repeat(281);
        let content = format!("---\nname: my-skill\ndescription: \"{long_description}\"\n---\n");
        let diagnostics = diagnose_skill_md(&content);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics
                .first()
                .unwrap()
                .message
                .contains("280 characters")
        );
    }

    #[test]
    fn diagnose_reports_invalid_yaml_with_location() {
        use crate::diagnostics::Location;

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

    // ── Bundled skills ───────────────────────────────────────────────────────

    /// Every bundled `SKILL.md` under `assets/bundled-skills/` must parse with
    /// the same `parse_skill_md` the workspace skill scanner uses.
    ///
    /// A bundled skill is embedded into the binary at compile time
    /// (`include_str!` in `workspace::bootstrap`) and written into every new
    /// workspace verbatim, so a frontmatter defect here — an over-long
    /// `description`, invalid YAML, a bad name — ships broken to every user
    /// and is silently dropped by `SkillIndex::scan`'s warn-and-skip handling,
    /// with no build-time signal. This test is the build-time signal: it
    /// fails naming the skill and the parser's exact complaint, instead of
    /// only surfacing as a `skipping skill with invalid frontmatter` warning
    /// discovered at runtime.
    #[test]
    fn all_bundled_skills_parse() {
        let bundled_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/bundled-skills");

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
