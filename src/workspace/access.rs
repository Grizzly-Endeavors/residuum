//! Workspace access policy: which paths the workspace HTTP file API and the
//! change feed must never expose.
//!
//! One pure function owns the rule, matched by path segment rather than
//! substring, so every caller agrees on what is hidden: internal index
//! directories, database files and their sidecars, and the temporary files
//! atomic writes create and rename away.
//!
//! The index and databases are Residuum's own data, open while it runs, so
//! bulk operations (recursive delete, directory moves) that would carry them
//! along are refused too: [`dir_holds_internal_data`] finds them.

use std::path::Path;

/// Returns true if `relative` names a path that must never be exposed
/// through the workspace file API or change feed.
///
/// Blocks two things, matched by path segment rather than substring:
/// - Any segment named exactly `.index` (the search index directory), at any
///   depth, including the directory itself.
/// - A file whose name ends in `.db` or `.sqlite`, or one of their
///   `-wal`/`-shm`/`-journal` sidecar files.
///
/// A look-alike name that merely contains these as a substring — `index.md`,
/// `my.index.md` — is never blocked.
#[must_use]
pub fn is_blocked_path(relative: &str) -> bool {
    let path = Path::new(relative);

    if path.components().any(|c| c.as_os_str() == ".index") {
        return true;
    }

    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    is_blocked_file_name(name)
}

/// Returns true if `name` (a bare file name, no directory components) is a
/// blocked database file, one of its sidecar files, or an atomic-write temp.
fn is_blocked_file_name(name: &str) -> bool {
    crate::util::fs::is_atomic_write_temp(name) || is_internal_data_name(name)
}

/// Returns true if `name` (a bare entry name) is Residuum's own data: the
/// search index directory, or a database file or its sidecar.
fn is_internal_data_name(name: &str) -> bool {
    if name == ".index" {
        return true;
    }

    let base = name
        .strip_suffix("-wal")
        .or_else(|| name.strip_suffix("-shm"))
        .or_else(|| name.strip_suffix("-journal"))
        .unwrap_or(name);

    Path::new(base)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("db") || ext.eq_ignore_ascii_case("sqlite"))
}

/// Whether the directory at `dir` holds Residuum's internal data (the search
/// index or a database file) at any depth. Symlinks are not followed.
///
/// Blocking: call it from a blocking context.
///
/// # Errors
/// Returns an error if a directory under `dir` cannot be read.
pub fn dir_holds_internal_data(dir: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(is_internal_data_name)
        {
            return Ok(true);
        }
        if entry.file_type()?.is_dir() && dir_holds_internal_data(&entry.path())? {
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
    fn blocks_database_files_and_sidecars() {
        assert!(is_blocked_path("data.db"));
        assert!(is_blocked_path("store.sqlite"));
        assert!(is_blocked_path("memory/vectors.db"));
        assert!(is_blocked_path("vectors.db-wal"));
        assert!(is_blocked_path("vectors.db-shm"));
        assert!(is_blocked_path("vectors.db-journal"));
        assert!(is_blocked_path("store.sqlite-wal"));
        assert!(is_blocked_path("store.sqlite-shm"));
        assert!(is_blocked_path("store.sqlite-journal"));
    }

    #[test]
    fn finds_internal_data_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("memory/.index")).unwrap();
        std::fs::create_dir_all(dir.path().join("notes/deep")).unwrap();
        std::fs::write(dir.path().join("notes/deep/page.md"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        std::fs::write(dir.path().join("data/store.sqlite-wal"), "x").unwrap();

        assert!(dir_holds_internal_data(&dir.path().join("memory")).unwrap());
        assert!(dir_holds_internal_data(&dir.path().join("data")).unwrap());
        assert!(!dir_holds_internal_data(&dir.path().join("notes")).unwrap());
    }

    #[test]
    fn in_flight_writes_are_not_internal_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".page.md.0badf00d.residuum-tmp"), "x").unwrap();
        assert!(!dir_holds_internal_data(dir.path()).unwrap());
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
