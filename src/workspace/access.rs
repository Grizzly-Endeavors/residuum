//! Workspace access policy: which paths the workspace HTTP file API and the
//! change feed must never expose.
//!
//! One pure function owns the rule, matched by path segment or exact
//! workspace-relative path rather than substring, so every caller agrees on
//! what is hidden: the internal search index directory, Residuum's own
//! database files and their sidecars, and the temporary files atomic writes
//! create and rename away.
//!
//! The index and databases are Residuum's own data, open while it runs, so
//! bulk operations (recursive delete, directory moves) that would carry them
//! along are refused too: [`dir_holds_internal_data`] finds them.

use std::path::Path;

/// Residuum's own database files, as workspace-relative paths.
///
/// Matched exactly (plus each path's `-wal`/`-shm`/`-journal` sidecars), not
/// by extension: a user's own `*.db`/`*.sqlite` file elsewhere in the
/// workspace is their data, not Residuum's, and is not hidden.
const INTERNAL_DB_PATHS: &[&str] = &["memory/vectors.db"];

/// Returns true if `relative` names a path that must never be exposed
/// through the workspace file API or change feed.
///
/// Blocks:
/// - Any segment named exactly `.index` (the search index directory), at any
///   depth, including the directory itself.
/// - One of [`INTERNAL_DB_PATHS`], or one of its `-wal`/`-shm`/`-journal`
///   sidecar files.
/// - An atomic-write temp file (see [`crate::util::fs::is_atomic_write_temp`]).
///
/// A look-alike name that merely contains these as a substring — `index.md`,
/// `my.index.md`, or a user's own `notes.db` — is never blocked.
#[must_use]
pub fn is_blocked_path(relative: &str) -> bool {
    if is_internal_data_path(relative) {
        return true;
    }

    Path::new(relative)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(crate::util::fs::is_atomic_write_temp)
}

/// Returns true if `relative` names Residuum's own data: a `.index` segment
/// at any depth, or one of [`INTERNAL_DB_PATHS`] (or its sidecar files).
///
/// Distinct from an atomic-write temp file (see [`is_blocked_path`]): a bulk
/// delete/move carrying away an in-flight temp write is fine, only carrying
/// away Residuum's own persistent data is refused (see
/// [`dir_holds_internal_data`]).
fn is_internal_data_path(relative: &str) -> bool {
    if Path::new(relative)
        .components()
        .any(|c| c.as_os_str() == ".index")
    {
        return true;
    }

    let normalized = relative.trim_start_matches("./");
    INTERNAL_DB_PATHS.iter().any(|db| {
        normalized == *db
            || normalized == format!("{db}-wal")
            || normalized == format!("{db}-shm")
            || normalized == format!("{db}-journal")
    })
}

/// Whether the directory at `dir` — reached from the workspace root by
/// `relative` — holds Residuum's internal data (the search index or one of
/// [`INTERNAL_DB_PATHS`]) at any depth. Symlinks are not followed.
///
/// `relative` lets each entry's full workspace-relative path be checked
/// exactly, so a same-named file *outside* an internal path (e.g. a user's
/// own `vectors.db` sitting somewhere other than `memory/`) is not mistaken
/// for Residuum's own data. An in-flight atomic-write temp file is not
/// treated as internal data here — carrying one away in a bulk delete/move
/// is fine; only Residuum's own persistent data blocks the operation.
///
/// Blocking: call it from a blocking context.
///
/// # Errors
/// Returns an error if a directory under `dir` cannot be read.
pub fn dir_holds_internal_data(dir: &Path, relative: &str) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let entry_relative = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        if is_internal_data_path(&entry_relative) {
            return Ok(true);
        }
        if entry.file_type()?.is_dir() && dir_holds_internal_data(&entry.path(), &entry_relative)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_index_segment_at_any_depth() {
        assert!(is_blocked_path(".index"));
        assert!(is_blocked_path(".index/foo"));
        assert!(is_blocked_path("memory/.index"));
        assert!(is_blocked_path("memory/.index/segments/foo.bin"));
        assert!(is_blocked_path("deep/nested/path/.index/file"));
    }

    #[test]
    fn blocks_residuums_own_database_and_sidecars() {
        assert!(is_blocked_path("memory/vectors.db"));
        assert!(is_blocked_path("memory/vectors.db-wal"));
        assert!(is_blocked_path("memory/vectors.db-shm"));
        assert!(is_blocked_path("memory/vectors.db-journal"));
    }

    #[test]
    fn a_users_own_db_or_sqlite_file_is_not_blocked() {
        // Only Residuum's own database paths are hidden -- a same-named or
        // same-extension file elsewhere in the workspace is the user's data.
        assert!(!is_blocked_path("data.db"));
        assert!(!is_blocked_path("store.sqlite"));
        assert!(!is_blocked_path("notes/vectors.db"));
        assert!(!is_blocked_path("store.sqlite-wal"));
    }

    #[test]
    fn finds_internal_data_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("memory/.index")).unwrap();
        std::fs::create_dir_all(dir.path().join("notes/deep")).unwrap();
        std::fs::write(dir.path().join("notes/deep/page.md"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("memory")).unwrap();
        std::fs::write(dir.path().join("memory/vectors.db-wal"), "x").unwrap();

        assert!(dir_holds_internal_data(&dir.path().join("memory"), "memory").unwrap());
        assert!(!dir_holds_internal_data(&dir.path().join("notes"), "notes").unwrap());
    }

    #[test]
    fn a_users_own_database_elsewhere_is_not_internal_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        std::fs::write(dir.path().join("data/store.sqlite"), "x").unwrap();

        assert!(!dir_holds_internal_data(&dir.path().join("data"), "data").unwrap());
    }

    #[test]
    fn in_flight_writes_are_not_internal_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".page.md.0badf00d.residuum-tmp"), "x").unwrap();
        assert!(!dir_holds_internal_data(dir.path(), "").unwrap());
    }

    #[test]
    fn blocks_in_flight_atomic_write_temp_files() {
        assert!(is_blocked_path("wiki/.page.md.0badf00d.residuum-tmp"));
        assert!(!is_blocked_path("wiki/.page.md.tmp"));
    }

    #[test]
    fn does_not_block_look_alike_names() {
        assert!(!is_blocked_path("index.md"));
        assert!(!is_blocked_path("my.index.md"));
        assert!(!is_blocked_path("wiki/index.md"));
        assert!(!is_blocked_path("skills/index.md"));
        assert!(!is_blocked_path("SOUL.md"));
        assert!(!is_blocked_path("skills/research.md"));
        assert!(!is_blocked_path(""));
    }

    #[test]
    fn does_not_block_files_that_merely_contain_db_or_sqlite() {
        assert!(!is_blocked_path("database.md"));
        assert!(!is_blocked_path("sqlite-notes.md"));
    }
}
