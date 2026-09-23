//! Workspace access policy: which paths the workspace HTTP file API and the
//! change feed must never expose.
//!
//! One pure function owns the rule, matched by path segment rather than
//! substring, so every caller (listing, read, write, and — in later phases —
//! raw access, tree walks, batch reads, delete/mkdir/move, and the change
//! feed) agrees on what is hidden.

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
/// blocked database file or one of its sidecar files.
fn is_blocked_file_name(name: &str) -> bool {
    let base = name
        .strip_suffix("-wal")
        .or_else(|| name.strip_suffix("-shm"))
        .or_else(|| name.strip_suffix("-journal"))
        .unwrap_or(name);

    Path::new(base)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("db") || ext.eq_ignore_ascii_case("sqlite"))
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
