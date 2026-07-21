//! Project manifest: recursive file listing for project subdirectories.

use std::fmt::Write as _;
use std::path::Path;

use super::types::{ManifestEntry, ProjectManifest};

/// Build a manifest by listing files under the standard project subdirectories.
///
/// Non-existent subdirectories are treated as empty.
///
/// # Errors
/// Returns an error if a directory cannot be read.
#[tracing::instrument(skip_all, fields(project_root = %project_root.display()))]
pub async fn build_manifest(project_root: &Path) -> anyhow::Result<ProjectManifest> {
    let (notes, notes_skipped) =
        list_files_recursive(&project_root.join("notes"), project_root).await?;
    let (references, references_skipped) =
        list_files_recursive(&project_root.join("references"), project_root).await?;
    let (workspace, workspace_skipped) =
        list_files_recursive(&project_root.join("workspace"), project_root).await?;
    let (skills, skills_skipped) =
        list_files_recursive(&project_root.join("skills"), project_root).await?;

    let skipped_count = notes_skipped + references_skipped + workspace_skipped + skills_skipped;

    let manifest = ProjectManifest {
        notes,
        references,
        workspace,
        skills,
        skipped_count,
    };
    tracing::debug!(
        notes = manifest.notes.len(),
        references = manifest.references.len(),
        workspace = manifest.workspace.len(),
        skills = manifest.skills.len(),
        skipped = manifest.skipped_count,
        "built project manifest"
    );
    if skipped_count > 0 {
        tracing::warn!(
            skipped = skipped_count,
            "project manifest is incomplete: some entries could not be listed"
        );
    }
    Ok(manifest)
}

/// Format a manifest as a human-readable grouped listing with sizes.
#[must_use]
pub fn format_manifest(manifest: &ProjectManifest) -> String {
    let sections: Vec<String> = [
        format_section("notes/", &manifest.notes),
        format_section("references/", &manifest.references),
        format_section("workspace/", &manifest.workspace),
        format_section("skills/", &manifest.skills),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut output = if sections.is_empty() {
        "No files.".to_string()
    } else {
        sections.join("\n\n")
    };

    if manifest.skipped_count > 0 {
        let plural = if manifest.skipped_count == 1 { "" } else { "s" };
        // write! to a String is infallible.
        _ = write!(
            output,
            "\n\n({} file{plural} could not be listed)",
            manifest.skipped_count
        );
    }

    output
}

fn format_section(heading: &str, entries: &[ManifestEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }

    let mut lines = Vec::with_capacity(entries.len() + 1);
    lines.push(format!("**{heading}**"));

    for entry in entries {
        lines.push(format!(
            "- {} ({})",
            entry.relative_path,
            format_size(entry.size_bytes)
        ));
    }

    Some(lines.join("\n"))
}

/// Format a byte count as a human-readable size string.
#[expect(
    clippy::cast_precision_loss,
    reason = "file sizes up to petabytes are representable in f64 with acceptable precision"
)]
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.1} KB");
    }
    let mb = kb / 1024.0;
    format!("{mb:.1} MB")
}

/// Recursively list files under a directory, returning paths relative to `project_root`.
///
/// Also returns the number of directory entries that could not be listed
/// (e.g. broken symlinks, permission-denied) and were therefore omitted.
async fn list_files_recursive(
    dir: &Path,
    project_root: &Path,
) -> anyhow::Result<(Vec<ManifestEntry>, usize)> {
    let mut entries = Vec::new();
    let mut skipped = 0_usize;
    collect_files(dir, project_root, &mut entries, &mut skipped).await?;
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok((entries, skipped))
}

/// Recursive helper using `tokio::fs::read_dir`.
///
/// `skipped` is incremented for every directory entry that can't be read or
/// stat'd, so callers can signal to the manifest's consumers that the
/// listing may be incomplete.
async fn collect_files(
    dir: &Path,
    project_root: &Path,
    entries: &mut Vec<ManifestEntry>,
    skipped: &mut usize,
) -> anyhow::Result<()> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("failed to read directory {}", dir.display())));
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
                    "failed to read directory entry"
                );
                *skipped += 1;
                continue;
            }
        };

        let metadata = match entry.metadata().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    path = %entry.path().display(),
                    error = %e,
                    "failed to read file metadata"
                );
                *skipped += 1;
                continue;
            }
        };

        if metadata.is_dir() {
            Box::pin(collect_files(&entry.path(), project_root, entries, skipped)).await?;
        } else {
            let path = entry.path();
            let rel = path
                .strip_prefix(project_root)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "entry {} is not under project root {}: {e}",
                        path.display(),
                        project_root.display()
                    )
                })?
                .to_string_lossy()
                .to_string();

            entries.push(ManifestEntry {
                relative_path: rel,
                size_bytes: metadata.len(),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_project() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_manifest(dir.path()).await.unwrap();

        assert!(manifest.notes.is_empty(), "notes should be empty");
        assert!(manifest.references.is_empty(), "references should be empty");
        assert!(manifest.workspace.is_empty(), "workspace should be empty");
        assert!(manifest.skills.is_empty(), "skills should be empty");
        assert_eq!(
            manifest.skipped_count, 0,
            "nothing should be reported as skipped"
        );

        let formatted = format_manifest(&manifest);
        assert_eq!(formatted, "No files.", "empty manifest should show message");
    }

    #[tokio::test]
    async fn files_in_all_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        for subfolder in &["notes", "references", "workspace", "skills"] {
            let sub = root.join(subfolder);
            tokio::fs::create_dir_all(&sub).await.unwrap();
            tokio::fs::write(sub.join("test.md"), "content")
                .await
                .unwrap();
        }

        let manifest = build_manifest(root).await.unwrap();
        assert_eq!(manifest.notes.len(), 1, "should find file in notes");
        assert_eq!(
            manifest.references.len(),
            1,
            "should find file in references"
        );
        assert_eq!(manifest.workspace.len(), 1, "should find file in workspace");
        assert_eq!(manifest.skills.len(), 1, "should find file in skills");
        assert!(
            manifest
                .notes
                .first()
                .unwrap()
                .relative_path
                .contains("notes/test.md"),
            "notes file path should be correct"
        );
        assert!(
            manifest.notes.first().unwrap().size_bytes > 0,
            "notes file size should be non-zero"
        );
        assert_eq!(
            manifest.skipped_count, 0,
            "nothing should be reported as skipped when every entry lists cleanly"
        );
    }

    #[tokio::test]
    async fn nested_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let nested = root.join("notes/sub/deep");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        tokio::fs::write(nested.join("deep-file.md"), "deep content")
            .await
            .unwrap();

        let manifest = build_manifest(root).await.unwrap();
        assert_eq!(manifest.notes.len(), 1, "should find nested file");
        assert!(
            manifest
                .notes
                .first()
                .unwrap()
                .relative_path
                .contains("sub/deep/deep-file.md"),
            "relative path should include nesting"
        );
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(500), "500 B", "small files show bytes");
    }

    #[test]
    fn format_size_zero() {
        assert_eq!(format_size(0), "0 B", "zero bytes");
    }

    #[test]
    fn format_size_kb_boundary() {
        assert_eq!(format_size(1024), "1.0 KB", "1024 bytes is exactly 1 KB");
    }

    #[test]
    fn format_size_mb_boundary() {
        assert_eq!(format_size(1024 * 1024), "1.0 MB", "1 MB boundary");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(2048), "2.0 KB", "2KB files show KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(1_500_000), "1.4 MB", "MB files show MB");
    }

    #[test]
    fn format_manifest_with_entries() {
        let manifest = ProjectManifest {
            notes: vec![ManifestEntry {
                relative_path: "notes/decisions.md".to_string(),
                size_bytes: 1024,
            }],
            references: vec![],
            workspace: vec![ManifestEntry {
                relative_path: "workspace/draft.md".to_string(),
                size_bytes: 512,
            }],
            skills: vec![],
            skipped_count: 0,
        };

        let output = format_manifest(&manifest);
        assert!(
            output.contains("**notes/**"),
            "should have notes section header"
        );
        assert!(
            output.contains("notes/decisions.md"),
            "should list notes file"
        );
        assert!(
            output.contains("**workspace/**"),
            "should have workspace section header"
        );
        assert!(
            !output.contains("**references/**"),
            "empty references should be omitted"
        );
        assert!(
            !output.contains("could not be listed"),
            "no skipped-entry note when nothing was skipped"
        );
    }

    #[test]
    fn format_manifest_notes_skipped_entries() {
        let manifest = ProjectManifest {
            notes: vec![ManifestEntry {
                relative_path: "notes/decisions.md".to_string(),
                size_bytes: 1024,
            }],
            references: vec![],
            workspace: vec![],
            skills: vec![],
            skipped_count: 2,
        };

        let output = format_manifest(&manifest);
        assert!(
            output.contains("(2 files could not be listed)"),
            "should note the number of skipped entries: {output}"
        );
    }

    #[test]
    fn format_manifest_notes_single_skipped_entry() {
        let manifest = ProjectManifest {
            notes: vec![],
            references: vec![],
            workspace: vec![],
            skills: vec![],
            skipped_count: 1,
        };

        let output = format_manifest(&manifest);
        assert!(
            output.contains("(1 file could not be listed)"),
            "should use singular phrasing for one skipped entry: {output}"
        );
    }

    #[tokio::test]
    async fn build_manifest_threads_skipped_count_through_to_rendered_prompt_text() {
        // `list_files_recursive`/`collect_files` count-and-skip on read errors
        // that are inherently racy to reproduce deterministically at the
        // filesystem level (e.g. a file vanishing between `read_dir` and
        // `metadata`). What we *can* pin down hermetically is that whatever
        // count `collect_files` produces survives the full round trip:
        // `build_manifest` sums it across all four subdirectories, and
        // `format_manifest` renders it into the text the agent actually sees.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let notes_dir = root.join("notes");
        let workspace_dir = root.join("workspace");
        tokio::fs::create_dir_all(&notes_dir).await.unwrap();
        tokio::fs::create_dir_all(&workspace_dir).await.unwrap();
        tokio::fs::write(notes_dir.join("real.md"), "content")
            .await
            .unwrap();

        let mut manifest = build_manifest(root).await.unwrap();
        assert_eq!(
            manifest.skipped_count, 0,
            "a clean listing should report nothing skipped"
        );

        // Simulate collect_files having hit read errors in two different
        // subdirectories, the way it would if e.g. metadata() failed for
        // one entry in notes/ and one in workspace/.
        manifest.skipped_count = 2;

        let formatted = format_manifest(&manifest);
        assert!(
            formatted.contains("notes/real.md"),
            "successfully listed files should still be shown: {formatted}"
        );
        assert!(
            formatted.contains("(2 files could not be listed)"),
            "the prompt text should flag that the manifest may be incomplete: {formatted}"
        );
    }
}
