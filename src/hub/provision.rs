//! Writing and removing agent directories: the filesystem half of agent
//! creation and deletion. The agent host calls these in the order the
//! design gives (validate, write the directory, role page, start; and for
//! deletion: stop, checkpoint, remove directory, remove role page).
//!
//! Nothing here starts or stops an agent, publishes notices, or touches hub
//! state, so each function can be tested against a temp directory.

use std::path::{Path, PathBuf};

use crate::checkpoints::{
    CheckpointContext, CheckpointEngine, CheckpointError, CheckpointTrigger, RepoKind,
};
use crate::config::paths::{TeamPaths, agent_dir, hub_dir, validate_agent_name};
use crate::config::{Config, HubConfig};
use crate::workspace::bootstrap::blank_agent_template;
use crate::workspace::layout::WorkspaceLayout;
use crate::workspace::team::{
    ensure_agent_role_page_as, remove_agent_role_page, restore_agent_role_page,
};
use crate::workspace::team_files::{TeamWriteCoordinator, TeamWriter};

use super::types::{A2aVisibility, LifecycleError};

/// Marks a workspace whose first-run interview is over. Written to every
/// created agent so `BOOTSTRAP.md` is never recreated.
const BOOTSTRAPPED_MARKER: &str = ".bootstrapped";

/// What a new agent is created from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// The agent's folder name.
    pub name: String,
    /// The name people see and address.
    pub display_name: String,
    /// Full `providers.toml` content. Validated before anything is written.
    pub providers_toml: String,
    /// The agent's A2A visibility, written to its `config.toml`.
    pub a2a_visibility: A2aVisibility,
    /// One-line role for the agent's role page; a placeholder when absent.
    pub description: Option<String>,
}

/// One file of a new agent, at its final path under the staging directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedFile {
    path: PathBuf,
    content: String,
}

/// Create the agent's directory from the blank template and its role page.
///
/// The directory is assembled under a hidden staging directory beside the
/// final one, with `config/config.toml` (the file that makes a directory an
/// agent) written last, and moved into place only when complete. A failure
/// or crash therefore never leaves a discoverable half-agent, and a failure
/// removes what was written. Returns the agent's directory.
///
/// The caller serializes creation of one name; two concurrent calls for the
/// same name share a staging directory. Creating different names at the same
/// time is safe: the role page, roster index, and wiki log are written under
/// `coordinator`'s path locks (it must guard `team`), with `actor`, the
/// creator, recorded as their writer.
///
/// # Errors
/// - [`LifecycleError::InvalidName`] when the name breaks the agent-name
///   rules.
/// - [`LifecycleError::AlreadyExists`] when a directory with that name
///   exists.
/// - [`LifecycleError::InvalidRequest`] when `providers_toml` is not a valid
///   model configuration.
/// - [`LifecycleError::Failed`] when the directory or role page cannot be
///   written.
#[tracing::instrument(skip_all, fields(agent = %spec.name))]
pub async fn provision_agent(
    root: &Path,
    team: &TeamPaths,
    coordinator: &TeamWriteCoordinator,
    actor: &TeamWriter,
    spec: &AgentSpec,
) -> Result<PathBuf, LifecycleError> {
    validate_agent_name(&spec.name).map_err(LifecycleError::InvalidName)?;
    crate::config::canonicalize_display_name(&spec.display_name)
        .map_err(LifecycleError::InvalidName)?;
    let final_dir = agent_dir(root, &spec.name);
    if path_exists(&final_dir).await? {
        return Err(LifecycleError::AlreadyExists(spec.name.clone()));
    }
    validate_providers(root, spec).await?;

    let staging = root.join(format!(".provision-{}", spec.name));
    let layout = WorkspaceLayout::new(&staging);
    let files = plan_files(&layout, spec);
    install_agent_dir(
        &staging,
        &final_dir,
        &spec.display_name,
        &layout.required_dirs(),
        &layout.config_dir(),
        &files,
    )
    .await?;

    if let Err(e) = ensure_agent_role_page_as(
        team,
        coordinator,
        actor,
        &spec.name,
        &spec.display_name,
        spec.description.as_deref(),
    )
    .await
    {
        tracing::error!(error = %e, agent = %spec.name, "failed to write the agent's role page");
        // The agent must not exist without its role page, so undo the directory.
        remove_dir_logged(&final_dir).await;
        if let Err(cleanup) = remove_agent_role_page(team, coordinator, actor, &spec.name).await {
            tracing::warn!(error = %cleanup, agent = %spec.name, "failed to clean up a partial role page");
        }
        return Err(LifecycleError::Failed(format!(
            "Couldn't create the agent '{}': its role page in the team wiki couldn't be written ({e}). Nothing was created.",
            spec.display_name
        )));
    }

    tracing::info!(agent = %spec.name, dir = %final_dir.display(), "agent provisioned");
    Ok(final_dir)
}

/// The template files of a new agent in write order: the blank template,
/// the bootstrapped marker, `providers.toml`, and `config/config.toml`
/// last.
fn plan_files(layout: &WorkspaceLayout, spec: &AgentSpec) -> Vec<PlannedFile> {
    let mut files: Vec<PlannedFile> = blank_agent_template(layout, &spec.name)
        .into_iter()
        .map(|(path, content)| PlannedFile { path, content })
        .collect();
    files.push(PlannedFile {
        path: layout.root().join(BOOTSTRAPPED_MARKER),
        content: String::new(),
    });
    files.push(PlannedFile {
        path: layout.config_dir().join("providers.toml"),
        content: spec.providers_toml.clone(),
    });
    files.push(PlannedFile {
        path: layout.config_dir().join("config.toml"),
        content: agent_config_toml(&spec.display_name, spec.a2a_visibility),
    });
    files
}

/// The created agent's `config.toml`: the name people see, `autostart`, and
/// its A2A visibility. Every other value is the default.
fn agent_config_toml(display_name: &str, visibility: A2aVisibility) -> String {
    let visibility = match visibility {
        A2aVisibility::Public => "public",
        A2aVisibility::Private => "private",
    };
    format!(
        "display_name = {display}\n\nautostart = true\n\n[a2a]\nvisibility = \"{visibility}\"\n",
        display = toml_edit::Value::from(display_name),
    )
}

/// Check `spec.providers_toml` against the hub's config the way the agent's
/// own settings save does, so a bad model configuration is refused before
/// anything is written.
async fn validate_providers(root: &Path, spec: &AgentSpec) -> Result<(), LifecycleError> {
    let hub_dir = hub_dir(root);
    let agent_dir = agent_dir(root, &spec.name);
    let name = spec.name.clone();
    let contents = spec.providers_toml.clone();
    let outcome = crate::util::spawn_blocking_in_span(move || {
        let hub = HubConfig::load_at(&hub_dir).map_err(|e| {
            tracing::error!(error = %e, hub_dir = %hub_dir.display(), "failed to load the hub config to validate a new agent");
            LifecycleError::Failed(
                "Couldn't read the hub's configuration, so the new agent's model settings can't be checked. See the logs for details."
                    .to_string(),
            )
        })?;
        Config::validate_agent_providers_toml(&contents, &agent_dir, &name, &hub).map_err(|e| {
            LifecycleError::InvalidRequest(format!(
                "The model configuration isn't valid: {e}"
            ))
        })
    })
    .await;
    outcome.unwrap_or_else(|e| {
        tracing::error!(error = %e, "providers validation task failed");
        Err(LifecycleError::Failed(
            "Couldn't check the new agent's model settings. See the logs for details.".to_string(),
        ))
    })
}

/// Write `dirs`, the config reference files into `config_dir`, and `files`
/// (in order) under `staging`, then move `staging`
/// to `final_dir`. On any failure `staging` is removed and nothing exists
/// at `final_dir`.
async fn install_agent_dir(
    staging: &Path,
    final_dir: &Path,
    label: &str,
    dirs: &[PathBuf],
    config_dir: &Path,
    files: &[PlannedFile],
) -> Result<(), LifecycleError> {
    let name = final_dir
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let fail = |what: &str, detail: &dyn std::fmt::Display| {
        tracing::error!(error = %detail, agent = %name, "failed to {what}");
        LifecycleError::Failed(format!(
            "Couldn't create the agent '{label}': failed to {what} ({detail}). Nothing was created."
        ))
    };

    remove_dir_logged(staging).await;
    for dir in dirs {
        if let Err(e) = tokio::fs::create_dir_all(dir).await {
            let err = fail(&format!("create {}", dir.display()), &e);
            remove_dir_logged(staging).await;
            return Err(err);
        }
    }
    let config_dir = config_dir.to_path_buf();
    let templates = crate::util::spawn_blocking_in_span(move || {
        Config::bootstrap_agent_config_dir(&config_dir)
    })
    .await;
    let failure = match templates {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e.to_string()),
        Err(e) => Some(e.to_string()),
    };
    if let Some(detail) = failure {
        let err = fail("write the config reference files", &detail);
        remove_dir_logged(staging).await;
        return Err(err);
    }
    for file in files {
        if let Err(e) = crate::util::fs::atomic_write(&file.path, &file.content).await {
            let err = fail(&format!("write {}", file.path.display()), &format!("{e:#}"));
            remove_dir_logged(staging).await;
            return Err(err);
        }
    }

    if path_exists(final_dir).await? {
        remove_dir_logged(staging).await;
        return Err(LifecycleError::AlreadyExists(name));
    }
    if let Err(e) = tokio::fs::rename(staging, final_dir).await {
        let err = fail("move the new agent into place", &e);
        remove_dir_logged(staging).await;
        return Err(err);
    }
    Ok(())
}

/// The text of `providers.toml` from an existing agent, to give a new agent
/// the same model configuration.
///
/// # Errors
/// [`LifecycleError::NotFound`] when the agent has no `providers.toml`
/// (no such agent, or one that was never configured);
/// [`LifecycleError::InvalidName`] for a malformed name;
/// [`LifecycleError::Failed`] when the file cannot be read.
pub fn copy_providers_from(root: &Path, agent: &str) -> Result<String, LifecycleError> {
    validate_agent_name(agent).map_err(LifecycleError::InvalidName)?;
    let path = agent_dir(root, agent).join("config").join("providers.toml");
    std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            LifecycleError::NotFound(agent.to_string())
        } else {
            tracing::error!(error = %e, path = %path.display(), "failed to read providers.toml to copy");
            LifecycleError::Failed(format!(
                "Couldn't read the model configuration of '{agent}'. See the logs for details."
            ))
        }
    })
}

/// The first message a new agent receives when it was created with a
/// description: it asks the agent to turn the description into its own
/// notes and its role page.
#[must_use]
pub fn first_message(description: &str) -> String {
    format!(
        "You were just created. This is the description of your role:\n\
         \n\
         <description>\n\
         {}\n\
         </description>\n\
         \n\
         Do both of these in this turn, then reply with a short summary of what you wrote.\n\
         \n\
         1. Edit SOUL.md. Turn the description into your own notes: what you are for, how you work, and the tone that fits. Write in your own words. Keep the existing sections that still apply and edit the ones the description changes.\n\
         2. Fill in your role page, team/wiki/agents/<your-name>.md (your name is in SOUL.md). The page exists with a placeholder. Set its frontmatter `description` to one line stating your role, and write the body: your role, your responsibilities, and what teammates should hand you. Activate the wiki skill first and follow its page format, then update your line in team/wiki/agents/index.md and add an entry to team/wiki/log.md.\n\
         \n\
         Where the description leaves gaps, make reasonable choices and record them in SOUL.md.",
        description.trim()
    )
}

/// Remove an agent's directory, role page, and roster entry, after
/// checkpointing the directory so the agent can be restored.
///
/// Precondition: the caller has already stopped the agent, so nothing is
/// writing to the directory. The role page, roster entry, and log line are
/// removed under `coordinator`'s path locks (it must guard `team`), with
/// `actor`, whoever deletes the agent, recorded as their writer. The caller also passes the agent's own
/// checkpoint engine: its workspace root is the agent's directory and its
/// config root is the agent's `config/` directory (see [`restore_agent`]).
///
/// Returns the id of the workspace checkpoint holding the directory as it
/// was, or `None` when it could not be recorded (logged at warn; the
/// deletion still proceeds, as for every checkpoint taken before an
/// action). The agent's `config.toml` and `providers.toml` are recorded in
/// the agent-config repository at the same time.
///
/// # Errors
/// - [`LifecycleError::InvalidName`] for a malformed name.
/// - [`LifecycleError::NotFound`] when no agent has this name.
/// - [`LifecycleError::Failed`] when the directory or role page cannot be
///   removed. The message names the checkpoint when one was taken.
#[tracing::instrument(skip_all, fields(agent = %name))]
pub async fn deprovision_agent(
    root: &Path,
    team: &TeamPaths,
    coordinator: &TeamWriteCoordinator,
    actor: &TeamWriter,
    name: &str,
    checkpoints: &CheckpointEngine,
) -> Result<Option<String>, LifecycleError> {
    validate_agent_name(name).map_err(LifecycleError::InvalidName)?;
    let dir = agent_dir(root, name);
    if !path_exists(&dir.join("config").join("config.toml")).await? {
        return Err(LifecycleError::NotFound(name.to_string()));
    }

    let ctx =
        CheckpointContext::system(CheckpointTrigger::PreAction, format!("delete agent {name}"));
    let checkpoint_id = checkpoints
        .checkpoint_workspace_id_before_action(ctx.clone())
        .await;
    if checkpoint_id.is_none() {
        tracing::warn!(agent = %name, "no workspace checkpoint was recorded before deleting the agent; it cannot be restored");
    }
    let config_id = checkpoints
        .checkpoint_config_kind_id_before_write(RepoKind::AgentConfig, ctx)
        .await;
    if config_id.is_none() {
        tracing::warn!(agent = %name, "no config checkpoint was recorded before deleting the agent; its settings cannot be restored");
    }

    // Renaming first takes the agent out of discovery in one step, so a
    // slow or failing recursive removal never leaves a half-deleted agent.
    let trash = root.join(format!(".deleting-{name}"));
    remove_dir_logged(&trash).await;
    if let Err(e) = crate::util::fs::rename_dir_when_released(&dir, &trash).await {
        tracing::error!(error = %e, dir = %dir.display(), "failed to remove the agent directory");
        let advice = if crate::util::fs::is_held_open(&e) {
            " Another program has its files open. Close it, then delete the agent again."
        } else {
            ""
        };
        return Err(LifecycleError::Failed(format!(
            "Couldn't delete the agent '{name}': its directory couldn't be removed ({e}).{advice}{}",
            checkpoint_note(checkpoint_id.as_deref())
        )));
    }
    remove_dir_logged(&trash).await;

    if let Err(e) = remove_agent_role_page(team, coordinator, actor, name).await {
        tracing::error!(error = %e, agent = %name, "failed to remove the agent's role page");
        return Err(LifecycleError::Failed(format!(
            "The agent '{name}' was deleted, but its role page in the team wiki couldn't be removed ({e}).{}",
            checkpoint_note(checkpoint_id.as_deref())
        )));
    }

    tracing::info!(agent = %name, checkpoint = ?checkpoint_id, "agent deprovisioned");
    Ok(checkpoint_id)
}

fn checkpoint_note(checkpoint_id: Option<&str>) -> String {
    checkpoint_id.map_or_else(String::new, |id| format!(" Its checkpoint is {id}."))
}

/// What a deleted agent is restored from.
pub struct RestoreSource<'a> {
    /// The agent's own checkpoint engine, built the way it was when the agent
    /// ran (workspace root = the agent's directory, config root = its
    /// `config/` directory, over the agent's checkpoint repositories).
    pub checkpoints: &'a CheckpointEngine,
    /// The workspace checkpoint to restore the directory from, such as the id
    /// [`deprovision_agent`] returned.
    pub workspace_checkpoint: &'a str,
    /// The text of the agent's role page when it was deleted. Without it the
    /// page comes back with the placeholder role.
    pub role_page: Option<&'a str>,
}

/// Bring a deleted agent back from the checkpoint [`deprovision_agent`]
/// returned.
///
/// The directory comes back from the workspace repository at
/// `source.workspace_checkpoint`, then `providers.toml` and `config.toml`
/// from the tip of the agent-config repository, `config.toml` last, so the
/// agent is discoverable only once it is complete. The role page is
/// recreated from `source.role_page`, or with the placeholder role, which
/// the agent replaces on its next turn. The restored agent is stopped; the
/// caller starts it. The role page is written under `coordinator`'s path
/// locks (it must guard `team`) with `actor` recorded as its writer; the
/// agent's own workspace and config repositories are private to it and need
/// none.
///
/// # Errors
/// - [`LifecycleError::InvalidName`] for a malformed name.
/// - [`LifecycleError::AlreadyExists`] when the agent exists again.
/// - [`LifecycleError::Failed`] when the checkpoint or the agent's saved
///   settings cannot be restored. A retry is safe: restoring is repeatable
///   and the agent stays undiscoverable until `config.toml` is back.
#[tracing::instrument(skip_all, fields(agent = %name))]
pub async fn restore_agent(
    root: &Path,
    team: &TeamPaths,
    coordinator: &TeamWriteCoordinator,
    actor: &TeamWriter,
    name: &str,
    source: &RestoreSource<'_>,
) -> Result<(), LifecycleError> {
    validate_agent_name(name).map_err(LifecycleError::InvalidName)?;
    let RestoreSource {
        checkpoints,
        workspace_checkpoint,
        role_page,
    } = *source;
    let dir = agent_dir(root, name);
    if path_exists(&dir.join("config").join("config.toml")).await? {
        return Err(LifecycleError::AlreadyExists(name.to_string()));
    }

    let fail = |what: &str, e: &CheckpointError| {
        tracing::error!(error = %e, agent = %name, "failed to {what}");
        LifecycleError::Failed(format!(
            "Couldn't restore the agent '{name}': failed to {what} ({e})."
        ))
    };
    let ctx =
        || CheckpointContext::system(CheckpointTrigger::Restore, format!("restore agent {name}"));

    checkpoints
        .restore_tree(
            RepoKind::Workspace,
            workspace_checkpoint.to_string(),
            ctx(),
            actor,
        )
        .await
        .map_err(|e| fail("restore the agent's files", &e))?;

    let config_tip = checkpoints
        .list_checkpoints(RepoKind::AgentConfig, None, None, None, Some(1))
        .await
        .map_err(|e| fail("look up the agent's saved settings", &e))?
        .items
        .into_iter()
        .next()
        .ok_or_else(|| {
            LifecycleError::Failed(format!(
                "Couldn't restore the agent '{name}': its saved settings were never recorded."
            ))
        })?;
    match checkpoints
        .restore_path(
            RepoKind::AgentConfig,
            config_tip.id.clone(),
            "providers.toml".to_string(),
            ctx(),
            actor,
        )
        .await
    {
        Ok(_) | Err(CheckpointError::PathNotFound(..)) => {}
        Err(e) => return Err(fail("restore the agent's model settings", &e)),
    }
    checkpoints
        .restore_path(
            RepoKind::AgentConfig,
            config_tip.id,
            "config.toml".to_string(),
            ctx(),
            actor,
        )
        .await
        .map_err(|e| fail("restore the agent's settings", &e))?;

    let role_page_result = match role_page {
        Some(text) => restore_agent_role_page(team, coordinator, actor, name, text).await,
        None => ensure_agent_role_page_as(team, coordinator, actor, name, name, None).await,
    };
    role_page_result.map_err(|e| {
        tracing::error!(error = %e, agent = %name, "failed to recreate the restored agent's role page");
        LifecycleError::Failed(format!(
            "The agent '{name}' was restored, but its role page in the team wiki couldn't be recreated ({e})."
        ))
    })?;

    tracing::info!(agent = %name, checkpoint = %workspace_checkpoint, "agent restored");
    Ok(())
}

async fn path_exists(path: &Path) -> Result<bool, LifecycleError> {
    tokio::fs::try_exists(path).await.map_err(|e| {
        tracing::error!(error = %e, path = %path.display(), "failed to check a path");
        LifecycleError::Failed(format!(
            "Couldn't check {}: {e}. See the logs for details.",
            path.display()
        ))
    })
}

/// Remove `dir` and everything under it; an absent directory is fine. A
/// failure is logged and leaves hidden leftovers that no discovery scan
/// sees.
async fn remove_dir_logged(dir: &Path) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "failed to remove a directory");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::paths::discover_agents;
    use crate::pulse::types::HeartbeatConfig;
    use crate::workspace::team::ensure_team;

    const PROVIDERS: &str = "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";

    struct Fixture {
        dir: tempfile::TempDir,
        team: TeamPaths,
        coordinator: TeamWriteCoordinator,
        actor: TeamWriter,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let hub = dir.path().join("hub");
            std::fs::create_dir_all(&hub).unwrap();
            std::fs::write(hub.join("config.toml"), "timezone = \"UTC\"\n").unwrap();
            let team = TeamPaths::new(dir.path().join("team"));
            ensure_team(&team, None, None).await.unwrap();
            let coordinator = TeamWriteCoordinator::new(&team);
            Self {
                dir,
                team,
                coordinator,
                actor: TeamWriter::Agent("creator".to_string()),
            }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn engine_for(&self, name: &str) -> CheckpointEngine {
            let agent = agent_dir(self.root(), name);
            CheckpointEngine::new(
                name,
                agent.clone(),
                &self.team,
                agent.join("config"),
                hub_dir(self.root()),
                &hub_dir(self.root()).join("checkpoints"),
                None,
            )
            .unwrap()
        }
    }

    fn spec(name: &str, description: Option<&str>) -> AgentSpec {
        AgentSpec {
            name: name.to_string(),
            display_name: name.to_string(),
            providers_toml: PROVIDERS.to_string(),
            a2a_visibility: A2aVisibility::Private,
            description: description.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn provision_writes_an_agent_the_config_loader_accepts() {
        let fx = Fixture::new().await;
        let dir = provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", None),
        )
        .await
        .unwrap();

        assert_eq!(dir, fx.root().join("scout"));
        let hub = HubConfig::load_at(&hub_dir(fx.root())).unwrap();
        let cfg = Config::load_agent_at(&dir, &hub).unwrap();
        assert_eq!(cfg.agent_name, "scout");
        assert!(cfg.autostart);
        assert_eq!(cfg.a2a.visibility, crate::config::A2aVisibility::Private);
        assert_eq!(discover_agents(fx.root()).unwrap(), vec!["scout"]);
        for required in WorkspaceLayout::new(&dir).required_dirs() {
            assert!(required.is_dir(), "{} should exist", required.display());
        }
        let providers = std::fs::read_to_string(dir.join("config").join("providers.toml")).unwrap();
        assert_eq!(providers, PROVIDERS);
        let soul = std::fs::read_to_string(dir.join("SOUL.md")).unwrap();
        assert!(soul.contains("**Name**: scout"));
        assert!(dir.join("SUBCONSCIOUS.md").is_file());
        let leftovers: Vec<_> = std::fs::read_dir(fx.root())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".provision-"))
            .collect();
        assert!(leftovers.is_empty(), "no staging directory remains");
    }

    #[tokio::test]
    async fn provision_writes_the_visibility_the_spec_names() {
        let fx = Fixture::new().await;
        let mut public = spec("herald", None);
        public.a2a_visibility = A2aVisibility::Public;
        let dir = provision_agent(fx.root(), &fx.team, &fx.coordinator, &fx.actor, &public)
            .await
            .unwrap();

        let hub = HubConfig::load_at(&hub_dir(fx.root())).unwrap();
        let cfg = Config::load_agent_at(&dir, &hub).unwrap();
        assert_eq!(cfg.a2a.visibility, crate::config::A2aVisibility::Public);
    }

    #[tokio::test]
    async fn provision_skips_the_interview_and_drops_wiki_lint() {
        let fx = Fixture::new().await;
        let dir = provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", None),
        )
        .await
        .unwrap();

        assert!(!dir.join("BOOTSTRAP.md").exists());
        assert!(dir.join(BOOTSTRAPPED_MARKER).is_file());
        let heartbeat = std::fs::read_to_string(dir.join("HEARTBEAT.yml")).unwrap();
        assert_eq!(heartbeat, crate::workspace::team::created_agent_heartbeat());
        let pulses: HeartbeatConfig = serde_yaml_ng::from_str(&heartbeat).unwrap();
        assert!(pulses.pulses.iter().all(|p| p.name != "wiki_lint"));
        assert!(pulses.pulses.iter().any(|p| p.name == "memory_tending"));
    }

    #[tokio::test]
    async fn starting_a_provisioned_agent_never_recreates_bootstrap() {
        let fx = Fixture::new().await;
        let dir = provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", None),
        )
        .await
        .unwrap();

        let layout = WorkspaceLayout::new(&dir);
        let coordinator = TeamWriteCoordinator::new(layout.team());
        crate::workspace::bootstrap::ensure_workspace(&layout, &coordinator, None, None)
            .await
            .unwrap();

        assert!(!dir.join("BOOTSTRAP.md").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("HEARTBEAT.yml")).unwrap(),
            crate::workspace::team::created_agent_heartbeat(),
            "the workspace bootstrap keeps the created-agent heartbeat"
        );
    }

    #[tokio::test]
    async fn provision_writes_the_role_page_with_the_description() {
        let fx = Fixture::new().await;
        provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", Some("Watches the feeds")),
        )
        .await
        .unwrap();
        provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("nova", None),
        )
        .await
        .unwrap();

        let page = std::fs::read_to_string(fx.team.agent_role_page("scout")).unwrap();
        assert!(page.contains("description: \"Watches the feeds\""));
        let index = std::fs::read_to_string(fx.team.wiki_agents_index_md()).unwrap();
        assert!(index.contains("- [scout](/agents/scout.md) — Watches the feeds"));
        let nova = std::fs::read_to_string(fx.team.agent_role_page("nova")).unwrap();
        assert!(nova.contains("Role not described yet"));
    }

    #[tokio::test]
    async fn provision_refuses_bad_names_and_existing_directories() {
        let fx = Fixture::new().await;
        for bad in ["", "Scout", "-scout", "hub", "team", "agents", ".hidden"] {
            let err = provision_agent(
                fx.root(),
                &fx.team,
                &fx.coordinator,
                &fx.actor,
                &spec(bad, None),
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, LifecycleError::InvalidName(_)),
                "{bad:?} gave {err:?}"
            );
        }

        std::fs::create_dir_all(fx.root().join("taken")).unwrap();
        std::fs::write(fx.root().join("taken").join("keep.txt"), "mine").unwrap();
        let err = provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("taken", None),
        )
        .await
        .unwrap_err();
        assert_eq!(err, LifecycleError::AlreadyExists("taken".to_string()));
        assert_eq!(
            std::fs::read_to_string(fx.root().join("taken").join("keep.txt")).unwrap(),
            "mine"
        );
    }

    #[tokio::test]
    async fn provision_rejects_a_bad_providers_file_before_writing_anything() {
        let fx = Fixture::new().await;
        let mut bad = spec("scout", Some("role"));
        bad.providers_toml = "[models\nmain = ".to_string();

        let err = provision_agent(fx.root(), &fx.team, &fx.coordinator, &fx.actor, &bad)
            .await
            .unwrap_err();

        assert!(matches!(err, LifecycleError::InvalidRequest(_)), "{err:?}");
        assert!(!fx.root().join("scout").exists());
        assert!(!fx.team.agent_role_page("scout").exists());

        bad.providers_toml = "[models]\nmain = \"nonsense\"\n".to_string();
        let unresolvable = provision_agent(fx.root(), &fx.team, &fx.coordinator, &fx.actor, &bad)
            .await
            .unwrap_err();
        assert!(
            matches!(unresolvable, LifecycleError::InvalidRequest(_)),
            "{unresolvable:?}"
        );
    }

    #[test]
    fn the_discovery_marker_is_written_last() {
        let layout = WorkspaceLayout::new(Path::new("res").join(".provision-scout"));
        let files = plan_files(&layout, &spec("scout", None));

        assert_eq!(
            files.last().map(|f| f.path.clone()),
            Some(layout.config_dir().join("config.toml"))
        );
        assert_eq!(
            files
                .iter()
                .filter(|f| f.path.ends_with(Path::new("config").join("config.toml")))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn a_failure_midway_leaves_no_agent_and_no_partial_directory() {
        let fx = Fixture::new().await;
        let staging = fx.root().join(".provision-scout");
        let final_dir = fx.root().join("scout");
        let layout = WorkspaceLayout::new(&staging);
        let mut files = plan_files(&layout, &spec("scout", None));
        // SOUL.md is a file, so writing beneath it fails after earlier files are written.
        let config_at = files.len() - 1;
        files.insert(
            config_at,
            PlannedFile {
                path: layout.soul_md().join("impossible"),
                content: "x".to_string(),
            },
        );

        let err = install_agent_dir(
            &staging,
            &final_dir,
            "scout",
            &layout.required_dirs(),
            &layout.config_dir(),
            &files,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, LifecycleError::Failed(_)), "{err:?}");
        assert!(err.to_string().contains("Nothing was created"));
        assert!(!final_dir.exists());
        assert!(!staging.exists(), "the partial directory is removed");
        assert!(discover_agents(fx.root()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failed_role_page_removes_the_new_directory() {
        let fx = Fixture::new().await;
        // A file where the roster directory belongs makes the role page unwritable.
        std::fs::remove_dir_all(fx.team.wiki_agents_dir()).unwrap();
        std::fs::write(fx.team.wiki_agents_dir(), "not a directory").unwrap();

        let err = provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", None),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, LifecycleError::Failed(_)), "{err:?}");
        assert!(!fx.root().join("scout").exists());
        assert!(discover_agents(fx.root()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_stale_staging_directory_does_not_block_creation() {
        let fx = Fixture::new().await;
        let stale = fx.root().join(".provision-scout");
        std::fs::create_dir_all(stale.join("memory")).unwrap();
        std::fs::write(stale.join("junk.txt"), "left by a crash").unwrap();

        let dir = provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", None),
        )
        .await
        .unwrap();

        assert!(!dir.join("junk.txt").exists());
        assert!(!stale.exists());
    }

    #[tokio::test]
    async fn copy_providers_reads_an_existing_agents_file() {
        let fx = Fixture::new().await;
        provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", None),
        )
        .await
        .unwrap();

        assert_eq!(copy_providers_from(fx.root(), "scout").unwrap(), PROVIDERS);
        assert_eq!(
            copy_providers_from(fx.root(), "ghost").unwrap_err(),
            LifecycleError::NotFound("ghost".to_string())
        );
        assert!(matches!(
            copy_providers_from(fx.root(), "../etc").unwrap_err(),
            LifecycleError::InvalidName(_)
        ));
    }

    #[test]
    fn first_message_carries_the_description_and_both_asks() {
        let message = first_message("  Triage incoming mail.\n");

        assert!(message.contains("<description>\nTriage incoming mail.\n</description>"));
        assert!(message.contains("SOUL.md"));
        assert!(message.contains("team/wiki/agents/<your-name>.md"));
        assert!(message.contains("`description`"));
        assert!(message.contains("what teammates should hand you"));
    }

    #[tokio::test]
    async fn deprovision_removes_the_agent_and_returns_a_checkpoint_that_restores_it() {
        let fx = Fixture::new().await;
        let dir = provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", Some("Watches the feeds")),
        )
        .await
        .unwrap();
        provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("nova", None),
        )
        .await
        .unwrap();
        std::fs::write(dir.join("memory").join("notes.md"), "remember this").unwrap();
        std::fs::write(dir.join("SOUL.md"), "scout's own soul").unwrap();
        let engine = fx.engine_for("scout");

        let checkpoint = deprovision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            "scout",
            &engine,
        )
        .await
        .unwrap()
        .expect("a checkpoint is recorded");

        assert!(!dir.exists());
        assert_eq!(discover_agents(fx.root()).unwrap(), vec!["nova"]);
        assert!(!fx.team.agent_role_page("scout").exists());
        assert!(fx.team.agent_role_page("nova").exists());
        let index = std::fs::read_to_string(fx.team.wiki_agents_index_md()).unwrap();
        assert!(!index.contains("scout"));
        assert!(index.contains("(/agents/nova.md)"));
        let log = std::fs::read_to_string(fx.team.wiki_log_md()).unwrap();
        assert!(log.contains("removed role page agents/scout.md for deleted agent scout"));
        assert!(
            !fx.root().join(".deleting-scout").exists(),
            "no trash directory remains"
        );

        restore_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            "scout",
            &RestoreSource {
                checkpoints: &engine,
                workspace_checkpoint: &checkpoint,
                role_page: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(discover_agents(fx.root()).unwrap(), vec!["nova", "scout"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("memory").join("notes.md")).unwrap(),
            "remember this"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("SOUL.md")).unwrap(),
            "scout's own soul"
        );
        assert!(dir.join(BOOTSTRAPPED_MARKER).is_file());
        assert!(!dir.join("BOOTSTRAP.md").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("config").join("providers.toml")).unwrap(),
            PROVIDERS
        );
        let hub = HubConfig::load_at(&hub_dir(fx.root())).unwrap();
        assert!(Config::load_agent_at(&dir, &hub).is_ok());
        assert!(fx.team.agent_role_page("scout").exists());
        let restored_index = std::fs::read_to_string(fx.team.wiki_agents_index_md()).unwrap();
        assert!(restored_index.contains("(/agents/scout.md)"));
    }

    #[tokio::test]
    async fn deprovision_of_an_unknown_agent_is_not_found() {
        let fx = Fixture::new().await;
        let engine = fx.engine_for("ghost");

        let err = deprovision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            "ghost",
            &engine,
        )
        .await
        .unwrap_err();

        assert_eq!(err, LifecycleError::NotFound("ghost".to_string()));
    }

    #[tokio::test]
    async fn restore_refuses_when_the_agent_exists_again() {
        let fx = Fixture::new().await;
        provision_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            &spec("scout", None),
        )
        .await
        .unwrap();
        let engine = fx.engine_for("scout");

        let err = restore_agent(
            fx.root(),
            &fx.team,
            &fx.coordinator,
            &fx.actor,
            "scout",
            &RestoreSource {
                checkpoints: &engine,
                workspace_checkpoint: "abc123",
                role_page: None,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err, LifecycleError::AlreadyExists("scout".to_string()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_provisions_of_different_names_lose_no_roster_entry() {
        let fx = Fixture::new().await;
        let names = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];
        let mut tasks = Vec::new();
        for name in names {
            let root = fx.root().to_path_buf();
            let team = fx.team.clone();
            let coordinator = fx.coordinator.clone();
            let actor = fx.actor.clone();
            tasks.push(crate::util::spawn_in_span(async move {
                provision_agent(
                    &root,
                    &team,
                    &coordinator,
                    &actor,
                    &spec(name, Some("a role")),
                )
                .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        let index = std::fs::read_to_string(fx.team.wiki_agents_index_md()).unwrap();
        let log = std::fs::read_to_string(fx.team.wiki_log_md()).unwrap();
        for name in names {
            assert!(
                index.contains(&format!("(/agents/{name}.md)")),
                "roster index lost {name}: {index}"
            );
            assert!(fx.team.agent_role_page(name).is_file());
            assert!(log.contains(&format!("added role page agents/{name}.md")));
        }
    }
}
