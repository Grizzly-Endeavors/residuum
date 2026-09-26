//! Paths excluded from a workspace checkpoint snapshot: data Residuum
//! rebuilds itself, a `.git` directory the user keeps in the workspace, and
//! logs/locks/other runtime churn.
//!
//! One function, [`is_excluded`], is used both to decide whether to descend
//! into a directory while walking the workspace, and to decide whether to
//! include a single file — matched by path segment rather than substring,
//! same convention as [`crate::workspace::access`].

use std::path::Path;

/// Returns true if `relative` (workspace-relative, `/`-separated, no
/// leading `/`) must never be included in a workspace checkpoint snapshot.
///
/// Call this on a directory's own relative path before descending into it
/// (an excluded directory is pruned, never walked) and on a file's relative
/// path before snapshotting it.
#[must_use]
pub(super) fn is_excluded(relative: &str) -> bool {
    if crate::workspace::access::is_blocked_path(relative) {
        return true;
    }

    let path = Path::new(relative);
    if path.components().any(|c| c.as_os_str() == ".git") {
        // Never touch a `.git` directory the user keeps in the workspace —
        // not Residuum's data, and never read as blob content.
        return true;
    }

    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_excluded_file_name)
}

/// Runtime churn matched by file name: lock files and PID files. Log files
/// live outside the workspace (`~/.residuum/logs/`), so they never reach
/// this check, but a workspace-relative `logs/` directory is excluded too
/// in case one ever appears there.
fn is_excluded_file_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lock") || ext.eq_ignore_ascii_case("pid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_residuum_internal_data() {
        assert!(is_excluded("memory/.index"));
        assert!(is_excluded("memory/.index/segments/foo.bin"));
        assert!(is_excluded("memory/vectors.db"));
        assert!(is_excluded("memory/vectors.db-wal"));
    }

    #[test]
    fn excludes_a_git_directory_the_user_keeps_in_the_workspace() {
        assert!(is_excluded(".git"));
        assert!(is_excluded(".git/HEAD"));
        assert!(is_excluded(".git/objects/pack/pack-abc.idx"));
        assert!(is_excluded("notes/.git/config"));
    }

    #[test]
    fn excludes_lock_and_pid_files() {
        assert!(is_excluded("residuum.lock"));
        assert!(is_excluded("some/dir/session.lock"));
        assert!(is_excluded("residuum.pid"));
    }

    #[test]
    fn does_not_exclude_ordinary_workspace_files() {
        assert!(!is_excluded("SOUL.md"));
        assert!(!is_excluded("wiki/index.md"));
        assert!(!is_excluded("skills/research/SKILL.md"));
        assert!(!is_excluded("workbench/dashboard/index.html"));
        assert!(!is_excluded("config/mcp.json"));
    }

    #[test]
    fn does_not_exclude_look_alike_names() {
        assert!(!is_excluded("gitignore.md"));
        assert!(!is_excluded("blocked.md"));
        assert!(!is_excluded("locksmith-notes.md"));
    }
}
