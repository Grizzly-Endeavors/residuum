//! Last-known-good config: after startup or a reload succeeds, `config.toml`
//! and `providers.toml` are copied here. If a later startup hits a fatal
//! config problem — the live files fail to load, or load but can't produce
//! a working gateway (no usable main provider, etc.) — the gateway falls
//! back to running on these copies instead of refusing to start, without
//! touching the user's live files. A copy is only ever saved after the
//! config has proven itself (a successful start or reload), so a bad reload
//! can never overwrite a good copy with itself.
//!
//! The hub and each agent keep independent last-known-good copies, since
//! the two configs are loaded, validated, and reloaded independently — see
//! [`hub`] for the hub's counterpart to the agent functions below.
//!
//! Every copy lives in `hub/`, never in the agent's workspace: providers
//! may hold plaintext API keys, and the agent's directory is a checkpointed,
//! web-browsable workspace. An agent's copies are
//! `hub/<agent>.config.last-known-good.toml` and
//! `hub/<agent>.providers.last-known-good.toml`, next to the hub's own
//! `hub/config.last-known-good.toml`.

use std::path::{Path, PathBuf};

use crate::config::{Config, HubConfig, HubPaths, paths};
use crate::util::FatalError;

/// The `hub/` directory and agent name for an agent whose own `config/`
/// directory is `agent_config_dir`. Agent directories sit beside `hub/`
/// under the residuum root, so the hub directory is that sibling.
fn hub_and_agent_name(agent_config_dir: &Path) -> Option<(PathBuf, String)> {
    let agent_dir = agent_config_dir.parent()?;
    let name = agent_dir.file_name()?.to_str()?.to_string();
    let hub_dir = paths::hub_dir(agent_dir.parent()?);
    Some((hub_dir, name))
}

fn config_copy_path(agent_config_dir: &Path) -> Option<PathBuf> {
    let (hub_dir, name) = hub_and_agent_name(agent_config_dir)?;
    Some(HubPaths::new(hub_dir).agent_config_last_known_good_toml(&name))
}

fn providers_copy_path(agent_config_dir: &Path) -> Option<PathBuf> {
    let (hub_dir, name) = hub_and_agent_name(agent_config_dir)?;
    Some(HubPaths::new(hub_dir).agent_providers_last_known_good_toml(&name))
}

/// Save an agent's `config.toml` and `providers.toml` as its last-known-good
/// copies.
///
/// Call only once the gateway has actually started successfully on them,
/// or a reload has applied them successfully — that's what "known good"
/// means here. Each copy is written atomically (write to a temp file, then
/// rename over the old copy) so a crash mid-write can never leave a
/// half-written copy for a later fallback to load. Best-effort: a copy
/// failure is logged, not fatal, and leaves the previous last-known-good
/// copy (if any) in place.
pub(crate) fn save(agent_config_dir: &Path) {
    for (src_name, dst_path) in [
        ("config.toml", config_copy_path(agent_config_dir)),
        ("providers.toml", providers_copy_path(agent_config_dir)),
    ] {
        let Some(dst_path) = dst_path else {
            tracing::warn!(
                config_dir = %agent_config_dir.display(),
                "failed to save last-known-good config copy: config directory is not inside an agent directory"
            );
            return;
        };
        let src = agent_config_dir.join(src_name);
        if !src.exists() {
            continue;
        }
        if let Err(err) = atomic_copy(&src, &dst_path) {
            tracing::warn!(
                file = src_name,
                error = %err,
                "failed to save last-known-good config copy"
            );
        }
    }
}

/// Copy `src` to `dst` via a temp file in the same directory plus a rename,
/// so the copy is atomic from any other reader's point of view.
fn atomic_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dst.with_extension("tmp");
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dst)
}

/// Whether a last-known-good pair is saved for the agent whose own
/// `config/` directory is `agent_config_dir`.
#[must_use]
pub fn exists(agent_config_dir: &Path) -> bool {
    config_copy_path(agent_config_dir).is_some_and(|p| p.exists())
        && providers_copy_path(agent_config_dir).is_some_and(|p| p.exists())
}

/// Load an agent's `Config` from its last-known-good copies, without
/// touching its live `config.toml`/`providers.toml`.
///
/// # Errors
/// Returns `FatalError::Config` if no last-known-good pair is saved, or the
/// saved copies themselves fail to load — the latter shouldn't happen,
/// since they were only ever saved right after a successful start.
pub(crate) fn load(agent_dir: &Path, hub: &HubConfig) -> Result<Config, FatalError> {
    let agent_config_dir = agent_dir.join("config");
    if !exists(&agent_config_dir) {
        return Err(FatalError::Config(
            "no last-known-good configuration is saved yet".to_string(),
        ));
    }
    let agent_name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            FatalError::Config(format!(
                "agent directory {} has no valid name",
                agent_dir.display()
            ))
        })?;
    let (Some(config_copy), Some(providers_copy)) = (
        config_copy_path(&agent_config_dir),
        providers_copy_path(&agent_config_dir),
    ) else {
        return Err(FatalError::Config(format!(
            "agent directory {} has no valid location for last-known-good copies",
            agent_dir.display()
        )));
    };
    Config::load_agent_from_paths(agent_dir, &config_copy, &providers_copy, agent_name, hub)
}

/// The hub's own last-known-good config, independent of any agent's.
pub(crate) mod hub {
    use std::path::Path;

    use crate::config::{HubConfig, HubPaths};
    use crate::util::FatalError;

    use super::atomic_copy;

    fn config_copy_path(hub_dir: &Path) -> std::path::PathBuf {
        HubPaths::new(hub_dir).config_last_known_good_toml()
    }

    /// Save `hub/config.toml` as its last-known-good copy. See
    /// [`super::save`] for the semantics.
    pub(crate) fn save(hub_dir: &Path) {
        let src = hub_dir.join("config.toml");
        if !src.exists() {
            return;
        }
        if let Err(err) = atomic_copy(&src, &config_copy_path(hub_dir)) {
            tracing::warn!(
                file = "hub/config.toml",
                error = %err,
                "failed to save last-known-good hub config copy"
            );
        }
    }

    /// Whether a last-known-good copy is saved for `hub_dir`.
    #[must_use]
    pub fn exists(hub_dir: &Path) -> bool {
        config_copy_path(hub_dir).exists()
    }

    /// Load `HubConfig` from the last-known-good copy, without touching the
    /// live `hub/config.toml`.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if no last-known-good copy is saved, or
    /// the saved copy itself fails to load.
    pub fn load(hub_dir: &Path) -> Result<HubConfig, FatalError> {
        if !exists(hub_dir) {
            return Err(FatalError::Config(
                "no last-known-good hub configuration is saved yet".to_string(),
            ));
        }
        HubConfig::load_from_path(&config_copy_path(hub_dir), hub_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_PROVIDERS: &str = "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";

    fn test_hub(base: &Path) -> HubConfig {
        HubConfig {
            timezone: chrono_tz::UTC,
            gateway: crate::config::GatewayConfig::default(),
            cloud: None,
            a2a: crate::config::HubA2aConfig::default(),
            tracing: crate::config::TracingConfig::default(),
            background: crate::config::HubBackgroundConfig::default(),
            config_dir: base.join("hub"),
            load_notices: Vec::new(),
        }
    }

    /// Set up `base/myagent/config/` for tests below, returning `(agent_dir,
    /// agent_config_dir)`.
    fn agent_dirs(base: &Path) -> (PathBuf, PathBuf) {
        let agent_dir = base.join("myagent");
        let agent_config_dir = agent_dir.join("config");
        std::fs::create_dir_all(&agent_config_dir).unwrap();
        (agent_dir, agent_config_dir)
    }

    #[test]
    fn no_copy_saved_reports_not_existing_and_fails_to_load() {
        let dir = tempfile::tempdir().unwrap();
        let (agent_dir, agent_config_dir) = agent_dirs(dir.path());
        assert!(!exists(&agent_config_dir));
        assert!(load(&agent_dir, &test_hub(dir.path())).is_err());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let (agent_dir, agent_config_dir) = agent_dirs(dir.path());
        std::fs::write(agent_config_dir.join("config.toml"), "").unwrap();
        std::fs::write(agent_config_dir.join("providers.toml"), VALID_PROVIDERS).unwrap();

        save(&agent_config_dir);
        assert!(exists(&agent_config_dir));

        let cfg = load(&agent_dir, &test_hub(dir.path()))
            .expect("saved last-known-good copy should load");
        assert_eq!(
            cfg.main.first().map(|p| p.model.model.as_str()),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn copies_live_in_hub_never_in_the_agent_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let (_agent_dir, agent_config_dir) = agent_dirs(dir.path());
        std::fs::write(agent_config_dir.join("config.toml"), "").unwrap();
        std::fs::write(
            agent_config_dir.join("providers.toml"),
            "[providers.x]\napi_key = \"sk-plaintext\"\n",
        )
        .unwrap();
        save(&agent_config_dir);

        let hub_dir = dir.path().join("hub");
        assert!(
            hub_dir
                .join("myagent.config.last-known-good.toml")
                .is_file()
        );
        assert!(
            hub_dir
                .join("myagent.providers.last-known-good.toml")
                .is_file()
        );
        let leaked: Vec<_> = std::fs::read_dir(&agent_config_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("last-known-good"))
            .collect();
        assert!(
            leaked.is_empty(),
            "copies leaked into the workspace: {leaked:?}"
        );
    }

    #[test]
    fn save_never_touches_the_live_files() {
        let dir = tempfile::tempdir().unwrap();
        let (agent_dir, agent_config_dir) = agent_dirs(dir.path());
        std::fs::write(agent_config_dir.join("config.toml"), "").unwrap();
        std::fs::write(agent_config_dir.join("providers.toml"), VALID_PROVIDERS).unwrap();
        save(&agent_config_dir);

        // Now break the live config; the saved copy must stay untouched.
        std::fs::write(agent_config_dir.join("config.toml"), "not valid toml [[[").unwrap();
        assert_eq!(
            std::fs::read_to_string(agent_config_dir.join("config.toml")).unwrap(),
            "not valid toml [[["
        );
        let cfg = load(&agent_dir, &test_hub(dir.path()))
            .expect("last-known-good copy should still load");
        assert_eq!(
            cfg.main.first().map(|p| p.model.model.as_str()),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn save_is_a_noop_when_nothing_has_loaded_yet() {
        let dir = tempfile::tempdir().unwrap();
        let (_agent_dir, agent_config_dir) = agent_dirs(dir.path());
        save(&agent_config_dir);
        assert!(
            !exists(&agent_config_dir),
            "no source files, nothing to save"
        );
    }

    #[test]
    fn later_save_overwrites_the_previous_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (agent_dir, agent_config_dir) = agent_dirs(dir.path());
        std::fs::write(agent_config_dir.join("config.toml"), "").unwrap();
        std::fs::write(agent_config_dir.join("providers.toml"), VALID_PROVIDERS).unwrap();
        save(&agent_config_dir);

        let updated_providers = "[models]\nmain = \"anthropic/claude-opus-4-6\"\n".to_string();
        std::fs::write(agent_config_dir.join("providers.toml"), &updated_providers).unwrap();
        save(&agent_config_dir);

        let cfg = load(&agent_dir, &test_hub(dir.path())).unwrap();
        assert_eq!(
            cfg.main.first().map(|p| p.model.model.as_str()),
            Some("claude-opus-4-6"),
            "the newer save should replace the old last-known-good copy"
        );
    }

    mod hub_tests {
        use super::super::hub;

        const VALID_HUB_CONFIG: &str = "timezone = \"UTC\"\n";

        #[test]
        fn no_copy_saved_reports_not_existing_and_fails_to_load() {
            let dir = tempfile::tempdir().unwrap();
            assert!(!hub::exists(dir.path()));
            assert!(hub::load(dir.path()).is_err());
        }

        #[test]
        fn save_then_load_roundtrips() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.toml"), VALID_HUB_CONFIG).unwrap();

            hub::save(dir.path());
            assert!(hub::exists(dir.path()));

            let cfg = hub::load(dir.path()).expect("saved last-known-good copy should load");
            assert_eq!(cfg.timezone, chrono_tz::UTC);
        }

        #[test]
        fn save_never_touches_the_live_file() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.toml"), VALID_HUB_CONFIG).unwrap();
            hub::save(dir.path());

            std::fs::write(dir.path().join("config.toml"), "not valid toml [[[").unwrap();
            let cfg = hub::load(dir.path()).expect("last-known-good copy should still load");
            assert_eq!(cfg.timezone, chrono_tz::UTC);
        }
    }
}
