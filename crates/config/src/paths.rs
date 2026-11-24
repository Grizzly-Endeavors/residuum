use std::path::{Path, PathBuf};

use residuum_core::util::FatalError;

pub const HUB_DIR_NAME: &str = "hub";

pub const TEAM_DIR_NAME: &str = "team";

const RESERVED_NAMES: &[&str] = &[HUB_DIR_NAME, TEAM_DIR_NAME, "agents"];

pub(crate) const MAX_AGENT_NAME_LEN: usize = 24;

pub fn residuum_root() -> Result<PathBuf, FatalError> {
    dirs::home_dir()
        .map(|h| h.join(".residuum"))
        .ok_or_else(|| FatalError::Config("could not determine home directory".to_string()))
}

pub fn default_hub_dir() -> Result<PathBuf, FatalError> {
    residuum_root().map(|root| hub_dir(&root))
}

#[must_use]
pub fn hub_dir(root: &Path) -> PathBuf {
    root.join(HUB_DIR_NAME)
}

#[must_use]
pub fn agent_dir(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

pub fn validate_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("agent name must not be empty".to_string());
    }
    if name.len() > MAX_AGENT_NAME_LEN {
        return Err(format!(
            "agent name '{name}' is too long: at most {MAX_AGENT_NAME_LEN} characters"
        ));
    }
    let well_formed = name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if !well_formed {
        return Err(format!(
            "agent name '{name}' must contain only lowercase letters, digits, and hyphens, \
             and must not start or end with a hyphen"
        ));
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(format!(
            "agent name '{name}' is reserved and cannot be used; reserved names: {}",
            RESERVED_NAMES.join(", ")
        ));
    }
    Ok(())
}

fn is_candidate_agent_dir_name(name: &str) -> bool {
    !name.starts_with('.') && !RESERVED_NAMES.contains(&name)
}

pub fn discover_agents(root: &Path) -> Result<Vec<String>, FatalError> {
    let unreadable = |err: &std::io::Error| {
        tracing::error!(root = %root.display(), error = %err, "failed to scan for agent directories");
        FatalError::Config(format!(
            "residuum couldn't read its data folder at {}: {err}. Check that the folder exists and that residuum has permission to read it.",
            root.display()
        ))
    };
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(unreadable(&err)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| unreadable(&err))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !is_candidate_agent_dir_name(&name) {
            continue;
        }
        let path = entry.path();
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => continue,
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "skipping unreadable entry while scanning for agents");
                continue;
            }
        }
        if path.join("config").join("config.toml").is_file() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

#[derive(Debug, Clone)]
pub struct HubPaths {
    root: PathBuf,
}

impl HubPaths {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn config_toml(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    #[must_use]
    pub fn config_last_known_good_toml(&self) -> PathBuf {
        self.root.join("config.last-known-good.toml")
    }

    #[must_use]
    pub fn agent_config_last_known_good_toml(&self, agent: &str) -> PathBuf {
        self.root
            .join(format!("{agent}.config.last-known-good.toml"))
    }

    #[must_use]
    pub fn agent_providers_last_known_good_toml(&self, agent: &str) -> PathBuf {
        self.root
            .join(format!("{agent}.providers.last-known-good.toml"))
    }

    #[must_use]
    pub fn secrets_enc(&self) -> PathBuf {
        self.root.join("secrets.toml.enc")
    }

    #[must_use]
    pub fn secrets_key(&self) -> PathBuf {
        self.root.join("secrets.key")
    }

    #[must_use]
    pub fn agent_keys_enc(&self) -> PathBuf {
        self.root.join("agent-keys.toml.enc")
    }

    #[must_use]
    pub fn agent_keys_key(&self) -> PathBuf {
        self.root.join("agent-keys.key")
    }

    #[must_use]
    pub fn agent_keys_lock(&self) -> PathBuf {
        self.root.join("agent-keys.lock")
    }

    #[must_use]
    pub fn a2a_keys_toml(&self) -> PathBuf {
        self.root.join("a2a-keys.toml")
    }

    #[must_use]
    pub fn a2a_keys_lock(&self) -> PathBuf {
        self.root.join("a2a-keys.lock")
    }

    #[must_use]
    pub fn remote_access_json(&self) -> PathBuf {
        self.root.join("remote-access.json")
    }

    #[must_use]
    pub fn remote_access_dir(&self) -> PathBuf {
        self.root.join("remote-access")
    }

    #[must_use]
    pub fn push_vapid_key(&self) -> PathBuf {
        self.root.join("push-vapid.key")
    }

    #[must_use]
    pub fn push_devices_json(&self) -> PathBuf {
        self.root.join("push-devices.json")
    }

    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    #[must_use]
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    #[must_use]
    pub fn checkpoints_dir(&self) -> PathBuf {
        self.root.join("checkpoints")
    }

    #[must_use]
    pub fn tool_results_dir(&self) -> PathBuf {
        self.root.join("tool-results")
    }

    #[must_use]
    pub fn pid_file(&self) -> PathBuf {
        self.root.join("residuum.pid")
    }

    #[must_use]
    pub fn ready_file(&self) -> PathBuf {
        self.root.join("residuum.ready")
    }

    #[must_use]
    pub fn startup_error_file(&self) -> PathBuf {
        self.root.join("residuum.startup-error")
    }

    #[must_use]
    pub fn crash_log(&self) -> PathBuf {
        self.root.join("crash.log")
    }
}

#[must_use]
pub fn team_dir(root: &Path) -> PathBuf {
    root.join(TEAM_DIR_NAME)
}

#[derive(Debug, Clone)]
pub struct TeamPaths {
    root: PathBuf,
}

impl TeamPaths {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn agents_md(&self) -> PathBuf {
        self.root.join("AGENTS.md")
    }

    #[must_use]
    pub fn user_md(&self) -> PathBuf {
        self.root.join("USER.md")
    }

    #[must_use]
    pub fn wiki_dir(&self) -> PathBuf {
        self.root.join("wiki")
    }

    #[must_use]
    pub fn wiki_index_md(&self) -> PathBuf {
        self.wiki_dir().join("index.md")
    }

    #[must_use]
    pub fn wiki_log_md(&self) -> PathBuf {
        self.wiki_dir().join("log.md")
    }

    #[must_use]
    pub fn wiki_agents_dir(&self) -> PathBuf {
        self.wiki_dir().join("agents")
    }

    #[must_use]
    pub fn wiki_agents_index_md(&self) -> PathBuf {
        self.wiki_agents_dir().join("index.md")
    }

    #[must_use]
    pub fn agent_role_page(&self, agent: &str) -> PathBuf {
        self.wiki_agents_dir().join(format!("{agent}.md"))
    }

    #[must_use]
    pub fn workbench_dir(&self) -> PathBuf {
        self.root.join("workbench")
    }

    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    #[must_use]
    pub fn search_index_dir(&self) -> PathBuf {
        self.root.join(".index")
    }

    #[must_use]
    pub fn vectors_db(&self) -> PathBuf {
        self.root.join("vectors.db")
    }

    #[must_use]
    pub fn required_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.wiki_dir(),
            self.wiki_agents_dir(),
            self.workbench_dir(),
            self.skills_dir(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_accepted() {
        for good in [
            "a",
            "assistant",
            "work-bot",
            "a1-b2",
            "x".repeat(24).as_str(),
        ] {
            assert!(
                validate_agent_name(good).is_ok(),
                "'{good}' should be accepted"
            );
        }
    }

    #[test]
    fn invalid_names_rejected() {
        for bad in [
            "",
            "-leading",
            "trailing-",
            "Upper",
            "has space",
            "has_underscore",
            "has.dot",
            ".hidden",
            &"x".repeat(25),
        ] {
            assert!(
                validate_agent_name(bad).is_err(),
                "'{bad}' should be rejected"
            );
        }
    }

    #[test]
    fn reserved_names_rejected() {
        for reserved in ["hub", "team", "agents"] {
            let err = validate_agent_name(reserved).unwrap_err();
            assert!(err.contains("reserved"), "{err}");
        }
    }

    #[test]
    fn hub_dir_and_agent_dir_paths() {
        let root = Path::new("/home/x/.residuum");
        assert_eq!(hub_dir(root), PathBuf::from("/home/x/.residuum/hub"));
        assert_eq!(
            agent_dir(root, "assistant"),
            PathBuf::from("/home/x/.residuum/assistant")
        );
    }

    #[test]
    fn discover_agents_finds_only_dirs_with_config_toml() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        std::fs::create_dir_all(root.join("assistant").join("config")).unwrap();
        std::fs::write(
            root.join("assistant").join("config").join("config.toml"),
            "",
        )
        .unwrap();

        std::fs::create_dir_all(root.join("hub").join("config")).unwrap();
        std::fs::write(root.join("hub").join("config").join("config.toml"), "").unwrap();

        std::fs::create_dir_all(root.join("incomplete")).unwrap();

        std::fs::create_dir_all(root.join(".git").join("config")).unwrap();
        std::fs::write(root.join(".git").join("config").join("config.toml"), "").unwrap();

        assert_eq!(
            discover_agents(root).unwrap(),
            vec!["assistant".to_string()]
        );
    }

    #[test]
    fn discover_agents_sorted_and_missing_root_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["zeta", "alpha", "mid"] {
            std::fs::create_dir_all(root.join(name).join("config")).unwrap();
            std::fs::write(root.join(name).join("config").join("config.toml"), "").unwrap();
        }
        assert_eq!(
            discover_agents(root).unwrap(),
            vec!["alpha".to_string(), "mid".to_string(), "zeta".to_string()]
        );

        let actual = &discover_agents(&root.join("does-not-exist")).unwrap();
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
    }

    #[cfg(unix)]
    #[test]
    fn discover_agents_follows_a_symlinked_agent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let elsewhere = dir.path().join("elsewhere").join("real-agent");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(elsewhere.join("config")).unwrap();
        std::fs::write(elsewhere.join("config").join("config.toml"), "").unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("linked")).unwrap();

        assert_eq!(discover_agents(&root).unwrap(), vec!["linked".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn discover_agents_skips_a_dangling_symlink() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("dangling")).unwrap();
        let actual_2 = &discover_agents(dir.path()).unwrap();
        assert!(actual_2.is_empty(), "expected {actual_2:?} to be empty");
    }

    #[test]
    fn discover_agents_reports_an_unreadable_root_instead_of_an_empty_install() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("root-file");
        std::fs::write(&not_a_dir, "").unwrap();

        let err = discover_agents(&not_a_dir).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("couldn't read"), "{message}");
        assert!(message.contains("root-file"), "{message}");
    }

    #[test]
    fn team_paths_live_under_the_team_root() {
        let root = Path::new("res");
        let team = TeamPaths::new(team_dir(root));
        assert_eq!(team.root(), root.join("team"));
        assert_eq!(team.agents_md(), root.join("team").join("AGENTS.md"));
        assert_eq!(team.user_md(), root.join("team").join("USER.md"));
        assert_eq!(
            team.wiki_index_md(),
            root.join("team").join("wiki").join("index.md")
        );
        assert_eq!(
            team.agent_role_page("scout"),
            root.join("team")
                .join("wiki")
                .join("agents")
                .join("scout.md")
        );
        assert_eq!(team.workbench_dir(), root.join("team").join("workbench"));
        assert_eq!(team.skills_dir(), root.join("team").join("skills"));
        assert_eq!(team.search_index_dir(), root.join("team").join(".index"));
    }

    #[test]
    fn team_required_dirs_list_parents_first() {
        let team = TeamPaths::new(Path::new("t"));
        let dirs = team.required_dirs();
        let wiki = dirs.iter().position(|d| *d == team.wiki_dir()).unwrap();
        let agents = dirs
            .iter()
            .position(|d| *d == team.wiki_agents_dir())
            .unwrap();
        assert_eq!(dirs.first().map(PathBuf::as_path), Some(team.root()));
        assert!(wiki < agents);
    }
}
