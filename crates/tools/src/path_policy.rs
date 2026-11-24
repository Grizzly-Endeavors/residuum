use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;

use residuum_config::Config;
use residuum_config::WorkspaceLayout;
use residuum_workspace::team_files::TeamFiles;

#[must_use]
pub fn blocked_write_paths(
    cfg: &Config,
    layout: &WorkspaceLayout,
    hub_dir: &Path,
) -> HashSet<PathBuf> {
    let mut blocked = always_blocked_paths(&cfg.config_dir, hub_dir);
    if !cfg.agent.modify_mcp {
        blocked.insert(layout.mcp_json());
    }
    if !cfg.agent.modify_channels {
        blocked.insert(layout.channels_toml());
    }
    blocked
}

fn always_blocked_paths(config_dir: &Path, hub_dir: &Path) -> HashSet<PathBuf> {
    let hub = residuum_config::HubPaths::new(hub_dir);
    let credential_stores = [
        hub.secrets_enc(),
        hub.secrets_key(),
        hub.agent_keys_enc(),
        hub.agent_keys_key(),
        hub.agent_keys_lock(),
        hub.a2a_keys_toml(),
        hub.a2a_keys_lock(),
        hub.push_vapid_key(),
        hub.push_devices_json(),
        hub.remote_access_json(),
        hub.remote_access_dir(),
    ];
    let templates = ["config.example.toml", "providers.example.toml"]
        .into_iter()
        .map(|name| config_dir.join(name));
    credential_stores.into_iter().chain(templates).collect()
}

const REGENERATED_TEMPLATE_NAMES: [&str; 2] = ["config.example.toml", "providers.example.toml"];

pub type SharedPathPolicy = Arc<RwLock<PathPolicy>>;

pub struct PathPolicy {
    blocked_paths: HashSet<PathBuf>,
    team: Option<TeamFiles>,
}

impl PathPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self {
            blocked_paths: HashSet::new(),
            team: None,
        }
    }

    #[must_use]
    pub fn with_blocked_paths(blocked_paths: HashSet<PathBuf>) -> Self {
        let canonicalized: HashSet<PathBuf> = blocked_paths
            .into_iter()
            .map(|p| canonicalize_for_check(&p))
            .collect();
        Self {
            blocked_paths: canonicalized,
            team: None,
        }
    }

    #[must_use]
    pub fn with_team(mut self, team: TeamFiles) -> Self {
        self.team = Some(team);
        self
    }

    #[must_use]
    pub fn team(&self) -> Option<&TeamFiles> {
        self.team.as_ref()
    }

    #[must_use]
    pub fn new_shared() -> SharedPathPolicy {
        Arc::new(RwLock::new(Self::new()))
    }

    #[must_use]
    pub fn new_shared_with_team(
        blocked_paths: HashSet<PathBuf>,
        team: TeamFiles,
    ) -> SharedPathPolicy {
        Arc::new(RwLock::new(
            Self::with_blocked_paths(blocked_paths).with_team(team),
        ))
    }

    pub fn set_blocked_paths(&mut self, paths: HashSet<PathBuf>) {
        self.blocked_paths = paths
            .into_iter()
            .map(|p| canonicalize_for_check(&p))
            .collect();
    }

    pub fn check_write(&self, path: &Path) -> Result<(), String> {
        let canonical = canonicalize_for_check(path);

        if canonical
            .ancestors()
            .any(|ancestor| self.blocked_paths.contains(ancestor))
        {
            tracing::warn!(path = %path.display(), "write rejected: blocked path");
            let is_regenerated_template = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| REGENERATED_TEMPLATE_NAMES.contains(&name));
            if is_regenerated_template {
                return Err(format!(
                    "writes to {} are not allowed — Residuum regenerates this reference \
                     template from its own defaults on every startup, so an edit here would \
                     look like it worked and then be silently overwritten. Edit config.toml or \
                     providers.toml instead, which are writable",
                    path.display()
                ));
            }
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

fn canonicalize_for_check(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }

    let mut existing = path.to_path_buf();
    let mut remaining = Vec::new();

    while !existing.exists() {
        if let Some(file_name) = existing.file_name() {
            remaining.push(file_name.to_os_string());
        } else {
            return path.to_path_buf();
        }
        if !existing.pop() {
            return path.to_path_buf();
        }
    }

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
        std::fs::write(config_dir.join("config.toml"), "").unwrap();
        std::fs::write(config_dir.join("config.example.toml"), "").unwrap();
        let canonical_ws = std::fs::canonicalize(&ws).unwrap();
        let canonical_cfg = std::fs::canonicalize(&config_dir).unwrap();
        (dir, canonical_ws, canonical_cfg)
    }

    #[test]
    fn blocked_paths_rejected() {
        let (_dir, _ws, cfg_dir) = make_workspace_with_config();
        let blocked: HashSet<PathBuf> = [cfg_dir.join("config.example.toml")].into_iter().collect();
        let policy = PathPolicy::with_blocked_paths(blocked);

        assert!(
            policy
                .check_write(&cfg_dir.join("config.example.toml"))
                .is_err(),
            "config.example.toml should be blocked"
        );
    }

    #[test]
    fn config_toml_and_providers_toml_are_writable() {
        let (_dir, _ws, cfg_dir) = make_workspace_with_config();
        let policy = PathPolicy::with_blocked_paths(HashSet::new());

        assert!(
            policy.check_write(&cfg_dir.join("config.toml")).is_ok(),
            "config.toml should be writable so the agent can edit it on the user's behalf"
        );
        assert!(
            policy.check_write(&cfg_dir.join("providers.toml")).is_ok(),
            "providers.toml should be writable so the agent can edit it on the user's behalf"
        );
    }

    #[test]
    fn example_template_refusal_explains_regeneration() {
        let (_dir, _ws, cfg_dir) = make_workspace_with_config();
        let blocked: HashSet<PathBuf> = [cfg_dir.join("config.example.toml")].into_iter().collect();
        let policy = PathPolicy::with_blocked_paths(blocked);

        let err = policy
            .check_write(&cfg_dir.join("config.example.toml"))
            .unwrap_err();
        assert!(
            err.contains("regenerates") && err.contains("config.toml"),
            "refusal should explain the template is regenerated and point at the writable file: {err}"
        );
    }

    #[test]
    fn blocked_paths_allow_workspace_files() {
        let (_dir, ws, cfg_dir) = make_workspace_with_config();
        let blocked: HashSet<PathBuf> = [cfg_dir.join("config.example.toml")].into_iter().collect();
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

        assert!(policy.check_write(&cfg_dir.join("config.toml")).is_err());

        policy.set_blocked_paths(HashSet::new());
        assert!(
            policy.check_write(&cfg_dir.join("config.toml")).is_ok(),
            "config.toml should be writable after clearing blocked paths"
        );

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
    fn blocked_write_paths_cover_the_example_templates() {
        let config_dir = Path::new("/cfg");
        let blocked = always_blocked_paths(config_dir, Path::new("/hub"));
        for name in ["config.example.toml", "providers.example.toml"] {
            assert!(
                blocked.contains(&config_dir.join(name)),
                "{name} should be write-blocked"
            );
        }
    }

    #[test]
    fn writes_to_the_push_signing_key_and_device_list_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        let policy = PathPolicy::with_blocked_paths(always_blocked_paths(
            &dir.path().join("config"),
            &hub_dir,
        ));

        for name in ["push-vapid.key", "push-devices.json", "remote-access.json"] {
            let err = policy.check_write(&hub_dir.join(name)).unwrap_err();
            assert!(err.contains(name), "{err}");
            std::fs::write(hub_dir.join(name), "x").unwrap();
            assert!(policy.check_write(&hub_dir.join(name)).is_err(), "{name}");
        }
    }

    #[test]
    fn writes_under_the_remote_access_directory_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        let policy = PathPolicy::with_blocked_paths(always_blocked_paths(
            &dir.path().join("config"),
            &hub_dir,
        ));
        let file = hub_dir.join("remote-access").join("state.json");
        assert!(policy.check_write(&file).is_err());
        assert!(
            policy
                .check_write(&hub_dir.join("remote-access-notes.md"))
                .is_ok()
        );
    }

    #[test]
    fn blocked_write_paths_cover_every_hub_credential_store() {
        let hub_dir = Path::new("/hub");
        let blocked = always_blocked_paths(Path::new("/cfg"), hub_dir);
        for name in [
            "secrets.toml.enc",
            "secrets.key",
            "agent-keys.toml.enc",
            "agent-keys.key",
            "agent-keys.lock",
            "a2a-keys.toml",
            "a2a-keys.lock",
            "push-vapid.key",
            "push-devices.json",
            "remote-access.json",
            "remote-access",
        ] {
            assert!(
                blocked.contains(&hub_dir.join(name)),
                "{name} should be write-blocked"
            );
        }
    }

    #[test]
    fn config_and_providers_toml_are_not_in_the_always_blocked_set() {
        let config_dir = Path::new("/cfg");
        let hub_dir = Path::new("/hub");
        let blocked = always_blocked_paths(config_dir, hub_dir);
        assert!(
            !blocked.contains(&config_dir.join("config.toml")),
            "config.toml must be editable by the agent"
        );
        assert!(
            !blocked.contains(&config_dir.join("providers.toml")),
            "providers.toml must be editable by the agent"
        );
        assert!(
            !blocked.contains(&hub_dir.join("config.toml")),
            "the hub's config.toml must be editable by the agent"
        );
    }
}
