//! Paths excluded from a workspace checkpoint snapshot: data Residuum
//! rebuilds itself, a `.git` directory the user keeps in the workspace, and
//! logs/locks/other runtime churn.
//!
//! One function, [`is_excluded`], is used both to decide whether to descend
//! into a directory while walking the workspace, and to decide whether to
//! include a single file — matched by path segment rather than substring,
//! same convention as [`crate::workspace::access`].

use std::path::Path;

/// The agent's own config file and providers file, tracked instead by the
/// `AgentConfig` checkpoint repository (see `crate::checkpoints::RepoKind`)
/// so plaintext provider keys never land in the workspace-style repo, which
/// is designed to allow a user-configured remote later.
const AGENT_CONFIG_TRACKED_PATHS: &[&str] = &["config/config.toml", "config/providers.toml"];

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
    if AGENT_CONFIG_TRACKED_PATHS.contains(&relative) {
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

/// Returns true if `relative` (team-relative, `/`-separated, no leading
/// `/`) must never be included in a team checkpoint snapshot: the team
/// wiki's search index (`.index`), its vector store (`vectors.db` and its
/// sidecars), atomic-write temp files, a user-kept `.git`, and lock/PID
/// files.
#[must_use]
pub(super) fn is_team_excluded(relative: &str) -> bool {
    const VECTOR_STORE_FILES: &[&str] = &[
        "vectors.db",
        "vectors.db-wal",
        "vectors.db-shm",
        "vectors.db-journal",
    ];

    let path = Path::new(relative);
    if path
        .components()
        .any(|c| c.as_os_str() == ".index" || c.as_os_str() == ".git")
    {
        return true;
    }

    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let is_root_vector_store = path.components().count() == 1 && VECTOR_STORE_FILES.contains(&name);
    is_root_vector_store
        || crate::util::fs::is_atomic_write_temp(name)
        || is_excluded_file_name(name)
}

/// Runtime churn matched by file name: lock files and PID files. Log files
/// live outside the workspace (`~/.residuum/hub/logs/`), so they never reach
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
    fn excludes_last_known_good_copies_and_atomic_write_temps_in_config() {
        assert!(is_excluded("config/config.last-known-good.toml"));
        assert!(is_excluded("config/providers.last-known-good.toml"));
        assert!(is_excluded("config/.providers.toml.0badf00d.residuum-tmp"));
        assert!(is_excluded("config/.config.toml.0badf00d.residuum-tmp"));
    }

    #[test]
    fn excludes_the_agents_own_config_and_providers_toml() {
        assert!(is_excluded("config/config.toml"));
        assert!(is_excluded("config/providers.toml"));
        // A look-alike path elsewhere in the tree is still tracked.
        assert!(!is_excluded("skills/config/config.toml"));
    }

    #[test]
    fn team_snapshot_excludes_the_search_index_vector_store_and_temps() {
        assert!(is_team_excluded(".index"));
        assert!(is_team_excluded(".index/segments/foo.bin"));
        assert!(is_team_excluded("vectors.db"));
        assert!(is_team_excluded("vectors.db-wal"));
        assert!(is_team_excluded("vectors.db-shm"));
        assert!(is_team_excluded("vectors.db-journal"));
        assert!(is_team_excluded("wiki/.page.md.0badf00d.residuum-tmp"));
        assert!(is_team_excluded("USER.md.lock"));
        assert!(is_team_excluded("notes/.git/config"));
    }

    #[test]
    fn team_snapshot_keeps_ordinary_team_files() {
        assert!(!is_team_excluded("AGENTS.md"));
        assert!(!is_team_excluded("USER.md"));
        assert!(!is_team_excluded("wiki/index.md"));
        assert!(!is_team_excluded("wiki/agents/scout.md"));
        assert!(!is_team_excluded("skills/wiki/SKILL.md"));
        assert!(!is_team_excluded("workbench/dash/index.html"));
        // A user's own vectors.db deeper in the tree is their data.
        assert!(!is_team_excluded("workbench/vectors.db"));
    }

    #[test]
    fn does_not_exclude_look_alike_names() {
        assert!(!is_excluded("gitignore.md"));
        assert!(!is_excluded("blocked.md"));
        assert!(!is_excluded("locksmith-notes.md"));
    }
}
