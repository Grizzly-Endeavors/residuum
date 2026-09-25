use crate::util::{parse_frontmatter_md, validate_kebab_name};

use super::types::SkillFrontmatter;

/// Description length above which loading the skill raises a user notice.
///
/// `skill-authoring` doctrine (see `docs/systems-usage/skills.md`) asks
/// authors to keep descriptions under ~60 characters so the skill index
/// stays scannable. There is no hard cap: a longer description still loads
/// and the skill still activates, since a skill someone actually wrote is
/// more valuable than one silently dropped over a wire-format concern. This
/// threshold only decides when to surface the recurring per-turn token cost
/// of a long description — every activated skill's description is
/// concatenated into the `<available_skills>` block sent on every turn (see
/// `SkillIndex::format_for_prompt`).
pub(super) const NOTICE_DESCRIPTION_LEN: usize = 280;

/// Rough characters-per-token estimate for English prose, used only to give
/// the oversized-description notice an approximate (not exact) token cost.
const APPROX_CHARS_PER_TOKEN: usize = 4;

/// Parse a `SKILL.md` file into frontmatter and body.
///
/// Expects YAML frontmatter delimited by `---` at the start of the file.
/// Validates the skill name: 1-64 chars, lowercase alphanumeric + hyphens,
/// no leading/trailing/consecutive hyphens. Validates the description is
/// non-empty; there is no length limit (see `oversized_description_notice`
/// for the non-blocking cost notice on a long one).
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
/// Reuses [`parse_skill_md`], so an error diagnostic can never disagree with
/// what loading rejects — a skill with an invalid name, an empty
/// description, or invalid YAML fails to parse here exactly as it would when
/// the skill scanner loads it. Extracts a line/column when the failure is a
/// YAML error; a name/description validation failure has no source position
/// once deserialization has already succeeded, so it's reported by message
/// alone.
///
/// A description long enough to trigger [`oversized_description_notice`]
/// doesn't fail parsing — the skill still loads — but is surfaced here as a
/// warning, so the live editor shows the same per-turn token-cost notice the
/// scanner would raise once the file is saved.
pub(crate) fn diagnose_skill_md(content: &str) -> Vec<crate::diagnostics::Diagnostic> {
    use crate::diagnostics::{Diagnostic, Location};

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

/// Validate a skill description: non-empty.
///
/// Length is not validated here: see `oversized_description_notice` for the
/// (non-blocking) per-turn cost notice raised for a long description.
pub(super) fn validate_skill_description(description: &str) -> anyhow::Result<()> {
    if description.trim().is_empty() {
        anyhow::bail!("skill description must not be empty");
    }

    Ok(())
}

/// Build a user-facing notice for a skill description long enough to have a
/// real, recurring per-turn token cost, or `None` if it's under
/// `NOTICE_DESCRIPTION_LEN`.
///
/// The skill loads and activates regardless of description length; this
/// only makes the cost visible instead of silent.
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
    fn parse_skill_description_too_long_accepted() {
        let long_description = "a".repeat(281);
        let content = format!("---\nname: my-skill\ndescription: \"{long_description}\"\n---\n");
        assert!(
            parse_skill_md(&content).is_ok(),
            "description over 280 chars should still parse and load; there is no cap"
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
    fn description_exactly_notice_len_accepted() {
        let description = "a".repeat(280);
        assert!(validate_skill_description(&description).is_ok());
    }

    #[test]
    fn description_too_long_still_accepted() {
        let description = "a".repeat(281);
        assert!(
            validate_skill_description(&description).is_ok(),
            "there is no length cap on skill descriptions"
        );
    }

    // ── oversized_description_notice ─────────────────────────────────────────

    #[test]
    fn oversized_description_notice_none_under_threshold() {
        assert!(oversized_description_notice("my-skill", &"a".repeat(280)).is_none());
    }

    #[test]
    fn oversized_description_notice_names_skill_and_length() {
        let description = "a".repeat(281);
        let notice = oversized_description_notice("my-skill", &description)
            .expect("over-threshold description should produce a notice");
        assert!(notice.contains("my-skill"), "notice should name the skill");
        assert!(
            notice.contains("281"),
            "notice should include the description length"
        );
        assert!(
            notice.contains("every turn"),
            "notice should mention the recurring per-turn cost"
        );
    }

    // ── diagnose_skill_md ────────────────────────────────────────────────────

    #[test]
    fn diagnose_valid_skill_has_no_diagnostics() {
        let content = "---\nname: pdf-processing\ndescription: \"Extracts text from PDFs\"\n---\n";
        assert!(diagnose_skill_md(content).is_empty());
    }

    #[test]
    fn diagnose_warns_on_oversized_description_without_failing() {
        // No length cap: the skill still loads (see
        // description_too_long_still_accepted below), so this is a warning
        // naming the per-turn cost, not an error.
        let long_description = "a".repeat(281);
        let content = format!("---\nname: my-skill\ndescription: \"{long_description}\"\n---\n");
        let diagnostics = diagnose_skill_md(&content);
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.first().unwrap();
        assert_eq!(diagnostic.severity, crate::diagnostics::Severity::Warning);
        assert!(diagnostic.message.contains("281"));
        assert!(diagnostic.message.contains("my-skill"));
    }

    #[test]
    fn diagnose_reports_nothing_for_a_normal_description() {
        let content = "---\nname: my-skill\ndescription: \"Extracts text from PDFs\"\n---\n";
        assert!(diagnose_skill_md(content).is_empty());
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
    /// workspace verbatim, so a frontmatter defect here — invalid YAML, a bad
    /// name, an empty description — ships broken to every user and is
    /// dropped by `SkillIndex::scan`'s skip handling, surfacing only as a
    /// runtime notice rather than a build-time failure. This test is the
    /// build-time signal: it fails naming the skill and the parser's exact
    /// complaint.
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
