//! Write-scoping policy for file tools.
//!
//! Blocks writes to unconditionally protected paths (config files and
//! credential stores). All other workspace writes are unrestricted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::config::Config;
use crate::workspace::layout::WorkspaceLayout;

/// Paths the file tools may never write: user-managed config, both
/// credential stores, and `mcp.json`/`channels.toml` unless the matching
/// agent ability (`agent.modify_mcp`/`agent.modify_channels`) is on.
///
/// The one definition of the blocked set, used at startup and on every
/// config reload.
#[must_use]
pub fn blocked_write_paths(cfg: &Config, layout: &WorkspaceLayout) -> HashSet<PathBuf> {
    let mut blocked = always_blocked_paths(&cfg.config_dir);
    if !cfg.agent.modify_mcp {
        blocked.insert(layout.mcp_json());
    }
    if !cfg.agent.modify_channels {
        blocked.insert(layout.channels_toml());
    }
    blocked
}

/// Config and credential-store files in `config_dir` that are blocked
/// regardless of agent abilities.
fn always_blocked_paths(config_dir: &Path) -> HashSet<PathBuf> {
    [
        "config.toml",
        "config.example.toml",
        "providers.toml",
        "providers.example.toml",
        crate::config::secrets::ENCRYPTED_FILE,
        crate::config::secrets::KEY_FILE,
        crate::agent_keys::ENCRYPTED_FILE,
        crate::agent_keys::KEY_FILE,
        crate::agent_keys::LOCK_FILE,
        crate::a2a::KEYS_FILE,
        crate::a2a::LOCK_FILE,
    ]
    .into_iter()
    .map(|name| config_dir.join(name))
    .collect()
}

/// Shared path policy, checked by `WriteTool` and `EditTool` before every write.
pub type SharedPathPolicy = Arc<RwLock<PathPolicy>>;

/// Write-scoping policy based on a set of unconditionally blocked paths.
pub struct PathPolicy {
    /// Paths that are unconditionally blocked from writes (e.g. config files).
    blocked_paths: HashSet<PathBuf>,
}

impl PathPolicy {
    /// Create a new path policy with no blocked paths.
    #[must_use]
    pub fn new() -> Self {
        Self {
            blocked_paths: HashSet::new(),
        }
    }

    /// Create a new path policy with blocked paths (e.g. config files).
    #[must_use]
    pub fn with_blocked_paths(blocked_paths: HashSet<PathBuf>) -> Self {
        let canonicalized: HashSet<PathBuf> = blocked_paths
            .into_iter()
            .map(|p| canonicalize_for_check(&p))
            .collect();
        Self {
            blocked_paths: canonicalized,
        }
    }

    /// Create a new shared path policy.
    #[must_use]
    pub fn new_shared() -> SharedPathPolicy {
        Arc::new(RwLock::new(Self::new()))
    }

    /// Create a new shared path policy with blocked paths.
    #[must_use]
    pub fn new_shared_with_blocked(blocked_paths: HashSet<PathBuf>) -> SharedPathPolicy {
        Arc::new(RwLock::new(Self::with_blocked_paths(blocked_paths)))
    }

    /// Replace the set of unconditionally blocked paths.
    ///
    /// Called during config hot-reload when agent ability gates change.
    pub fn set_blocked_paths(&mut self, paths: HashSet<PathBuf>) {
        self.blocked_paths = paths
            .into_iter()
            .map(|p| canonicalize_for_check(&p))
            .collect();
    }

    /// Check whether a write to `path` is allowed under the current policy.
    ///
    /// Returns `Ok(())` if allowed, or `Err(reason)` if rejected.
    ///
    /// # Errors
    /// Returns a descriptive error string if the write is rejected.
    pub fn check_write(&self, path: &Path) -> Result<(), String> {
        let canonical = canonicalize_for_check(path);

        if self.blocked_paths.contains(&canonical) {
            tracing::warn!(path = %path.display(), "write rejected: blocked path");
            return Err(format!(
                "writes to {} are not allowed — it is user-managed configuration or credential \
                 storage",
                path.display()
            ));
        }

        Ok(())
    }
}

impl Default for PathPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonicalize a path for policy checks.
///
/// For existing paths, uses `std::fs::canonicalize`. For new files (path doesn't
/// exist yet), canonicalizes the nearest existing ancestor and appends the
/// remaining segments.
fn canonicalize_for_check(path: &Path) -> PathBuf {
    // Try full canonicalization first
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }

    // Walk up to find the nearest existing ancestor
    let mut existing = path.to_path_buf();
    let mut remaining = Vec::new();

    while !existing.exists() {
        if let Some(file_name) = existing.file_name() {
            remaining.push(file_name.to_os_string());
        } else {
            // Can't walk further up; return the original path as-is
            return path.to_path_buf();
        }
        if !existing.pop() {
            return path.to_path_buf();
        }
    }

    // Canonicalize the existing ancestor and re-append the missing segments
    let mut canonical = std::fs::canonicalize(&existing).unwrap_or(existing);
    for segment in remaining.into_iter().rev() {
        canonical.push(segment);
    }
    canonical
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_workspace() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("workspace");
        std::fs::create_dir_all(ws.join("memory")).unwrap();
        let canonical_ws = std::fs::canonicalize(&ws).unwrap();
        (dir, canonical_ws)
    }

    #[test]
    fn workspace_level_writes_always_allowed() {
        let (_dir, ws) = make_workspace();
        let policy = PathPolicy::new();

        assert!(
            policy.check_write(&ws.join("memory/notes.md")).is_ok(),
            "memory writes should be allowed"
        );
        assert!(
            policy.check_write(&ws.join("wiki/index.md")).is_ok(),
            "wiki writes should be allowed"
        );
    }

    fn make_workspace_with_config() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("workspace");
        std::fs::create_dir_all(ws.join("memory")).unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        // Create the config files so they can be canonicalized
        std::fs::write(config_dir.join("config.toml"), "").unwrap();
        std::fs::write(config_dir.join("config.example.toml"), "").unwrap();
        let canonical_ws = std::fs::canonicalize(&ws).unwrap();
        let canonical_cfg = std::fs::canonicalize(&config_dir).unwrap();
        (dir, canonical_ws, canonical_cfg)
    }

    #[test]
    fn blocked_paths_rejected() {
        let (_dir, _ws, cfg_dir) = make_workspace_with_config();
        let blocked: HashSet<PathBuf> = [
            cfg_dir.join("config.toml"),
            cfg_dir.join("config.example.toml"),
        ]
        .into_iter()
        .collect();
        let policy = PathPolicy::with_blocked_paths(blocked);

        assert!(
            policy.check_write(&cfg_dir.join("config.toml")).is_err(),
            "config.toml should be blocked"
        );
        assert!(
            policy
                .check_write(&cfg_dir.join("config.example.toml"))
                .is_err(),
            "config.example.toml should be blocked"
        );
    }

    #[test]
    fn blocked_paths_allow_workspace_files() {
        let (_dir, ws, cfg_dir) = make_workspace_with_config();
        let blocked: HashSet<PathBuf> = [
            cfg_dir.join("config.toml"),
            cfg_dir.join("config.example.toml"),
        ]
        .into_iter()
        .collect();
        let policy = PathPolicy::with_blocked_paths(blocked);

        assert!(
            policy.check_write(&ws.join("wiki/index.md")).is_ok(),
            "wiki pages should be writable"
        );
        assert!(
            policy.check_write(&ws.join("memory/notes.md")).is_ok(),
            "memory files should be writable"
        );
    }

    #[test]
    fn set_blocked_paths_updates_policy() {
        let (_dir, _ws, cfg_dir) = make_workspace_with_config();
        let blocked: HashSet<PathBuf> = [cfg_dir.join("config.toml")].into_iter().collect();
        let mut policy = PathPolicy::with_blocked_paths(blocked);

        // config.toml is blocked initially
        assert!(policy.check_write(&cfg_dir.join("config.toml")).is_err());

        // Replace blocked set with empty — config.toml should now be writable
        policy.set_blocked_paths(HashSet::new());
        assert!(
            policy.check_write(&cfg_dir.join("config.toml")).is_ok(),
            "config.toml should be writable after clearing blocked paths"
        );

        // Re-block config.toml
        let new_blocked: HashSet<PathBuf> = [cfg_dir.join("config.toml")].into_iter().collect();
        policy.set_blocked_paths(new_blocked);
        assert!(
            policy.check_write(&cfg_dir.join("config.toml")).is_err(),
            "config.toml should be blocked again"
        );
    }

    #[test]
    fn blocked_paths_error_message() {
        let (_dir, _ws, cfg_dir) = make_workspace_with_config();
        let blocked: HashSet<PathBuf> = [cfg_dir.join("config.toml")].into_iter().collect();
        let policy = PathPolicy::with_blocked_paths(blocked);

        let err = policy
            .check_write(&cfg_dir.join("config.toml"))
            .unwrap_err();
        assert!(
            err.contains("config.toml") && err.contains("user-managed"),
            "error should name the path and say it is user-managed: {err}"
        );
    }

    #[test]
    fn blocked_write_paths_cover_both_credential_stores() {
        let config_dir = Path::new("/cfg");
        let blocked = always_blocked_paths(config_dir);
        for name in [
            "config.toml",
            "secrets.toml.enc",
            "secrets.key",
            "agent-keys.toml.enc",
            "agent-keys.key",
            "agent-keys.lock",
            "a2a-keys.toml",
            "a2a-keys.lock",
        ] {
            assert!(
                blocked.contains(&config_dir.join(name)),
                "{name} should be write-blocked"
            );
        }
    }
}
