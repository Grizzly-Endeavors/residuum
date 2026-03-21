//! Project manifest: recursive file listing for project subdirectories.

use std::path::Path;

use crate::error::FatalError;

use super::types::{ManifestEntry, ProjectManifest};

/// Build a manifest by listing files under the standard project subdirectories.
///
/// Non-existent subdirectories are treated as empty.
///
/// # Errors
/// Returns `FatalError::Projects` if a directory cannot be read.
pub async fn build_manifest(project_root: &Path) -> Result<ProjectManifest, FatalError> {
    let manifest = ProjectManifest {
        notes: list_files_recursive(&project_root.join("notes"), project_root).await?,
        references: list_files_recursive(&project_root.join("references"), project_root).await?,
        workspace: list_files_recursive(&project_root.join("workspace"), project_root).await?,
        skills: list_files_recursive(&project_root.join("skills"), project_root).await?,
    };
    let total = manifest.notes.len()
        + manifest.references.len()
        + manifest.workspace.len()
        + manifest.skills.len();
    tracing::debug!(total, "built project manifest");
    Ok(manifest)
}

/// Format a manifest as a human-readable grouped listing with sizes.
#[must_use]
pub fn format_manifest(manifest: &ProjectManifest) -> String {
    let mut sections = Vec::new();

    format_section("notes/", &manifest.notes, &mut sections);
    format_section("references/", &manifest.references, &mut sections);
    format_section("workspace/", &manifest.workspace, &mut sections);
    format_section("skills/", &manifest.skills, &mut sections);

    if sections.is_empty() {
        return "No files.".to_string();
    }

    sections.join("\n\n")
}

fn format_section(heading: &str, entries: &[ManifestEntry], sections: &mut Vec<String>) {
    if entries.is_empty() {
        return;
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

    sections.push(lines.join("\n"));
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
async fn list_files_recursive(
    dir: &Path,
    project_root: &Path,
) -> Result<Vec<ManifestEntry>, FatalError> {
    let mut entries = Vec::new();
    collect_files(dir, project_root, &mut entries).await?;
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(entries)
}

/// Recursive helper using `tokio::fs::read_dir`.
async fn collect_files(
    dir: &Path,
    project_root: &Path,
    entries: &mut Vec<ManifestEntry>,
) -> Result<(), FatalError> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(FatalError::Projects(format!(
                "failed to read directory {}: {e}",
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
                    "failed to read directory entry"
                );
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
                continue;
            }
        };

        if metadata.is_dir() {
            Box::pin(collect_files(&entry.path(), project_root, entries)).await?;
        } else {
            let rel = entry
                .path()
                .strip_prefix(project_root)
                .unwrap_or(&entry.path())
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
    }
}
