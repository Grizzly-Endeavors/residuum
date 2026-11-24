use std::path::{Path, PathBuf};

use residuum_config::{Config, HubConfig, HubPaths, paths};
use residuum_core::util::FatalError;

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

pub fn save(agent_config_dir: &Path) {
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

fn atomic_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dst.with_extension("tmp");
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dst)
}

#[must_use]
pub fn exists(agent_config_dir: &Path) -> bool {
    config_copy_path(agent_config_dir).is_some_and(|p| p.exists())
        && providers_copy_path(agent_config_dir).is_some_and(|p| p.exists())
}

pub fn load(agent_dir: &Path, hub: &HubConfig) -> Result<Config, FatalError> {
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

pub mod hub {
    use std::path::Path;

    use residuum_config::{HubConfig, HubPaths};
    use residuum_core::util::FatalError;

    use super::atomic_copy;

    fn config_copy_path(hub_dir: &Path) -> std::path::PathBuf {
        HubPaths::new(hub_dir).config_last_known_good_toml()
    }

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

    #[must_use]
    pub fn exists(hub_dir: &Path) -> bool {
        config_copy_path(hub_dir).exists()
    }

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
            gateway: residuum_config::GatewayConfig::default(),
            cloud: None,
            a2a: residuum_config::HubA2aConfig::default(),
            tracing: residuum_config::TracingConfig::default(),
            background: residuum_config::HubBackgroundConfig::default(),
            push: residuum_config::HubPushConfig::default(),
            system_one: None,
            pricing: std::collections::BTreeMap::new(),
            config_dir: base.join("hub"),
            load_notices: Vec::new(),
        }
    }

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
