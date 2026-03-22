//! CLI handlers for `residuum agent` subcommands.
//!
//! - `residuum agent create <name>` — bootstrap a new named agent
//! - `residuum agent list` — show all agents and their status
//! - `residuum agent delete <name>` — remove a named agent
//! - `residuum agent info <name>` — show agent details

use tracing::{debug, trace, warn};

use crate::config::Config;
use crate::daemon;
use crate::util::FatalError;

use super::paths;
use super::registry::{AgentEntry, AgentRegistry};

/// Agent management subcommands.
#[derive(clap::Subcommand)]
pub enum AgentCommand {
    /// Create a new named agent
    Create {
        /// Name for the new agent (alphanumeric + hyphens, 1-32 chars)
        name: String,
    },
    /// List all agents and their status
    List,
    /// Remove a named agent
    Delete {
        /// Name of the agent to delete
        name: String,
    },
    /// Show agent details
    Info {
        /// Name of the agent to inspect
        name: String,
    },
}

/// Dispatch `residuum agent <subcommand>`.
///
/// # Errors
///
/// Returns `FatalError` if the subcommand fails.
pub fn run_agent_command(command: &AgentCommand) -> Result<(), FatalError> {
    match command {
        AgentCommand::Create { name } => run_agent_create(name),
        AgentCommand::List => run_agent_list(),
        AgentCommand::Delete { name } => run_agent_delete(name),
        AgentCommand::Info { name } => run_agent_info(name),
    }
}

/// Validate an agent name: alphanumeric + hyphens, 1-32 chars, not "default".
fn validate_name(name: &str) -> Result<(), FatalError> {
    if name.is_empty() {
        return Err(FatalError::Config("agent name cannot be empty".to_string()));
    }
    if name.len() > 32 {
        return Err(FatalError::Config(
            "agent name must be 32 characters or fewer".to_string(),
        ));
    }
    if name == "default" {
        return Err(FatalError::Config(
            "\"default\" is reserved for the unnamed agent".to_string(),
        ));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(FatalError::Config(
            "agent name must contain only alphanumeric characters and hyphens".to_string(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(FatalError::Config(
            "agent name must not start or end with a hyphen".to_string(),
        ));
    }
    Ok(())
}

fn write_file(
    path: &std::path::Path,
    content: impl AsRef<[u8]>,
    desc: &str,
) -> Result<(), FatalError> {
    std::fs::write(path, content)
        .map_err(|e| FatalError::Config(format!("failed to write {desc}: {e}")))
}

fn bootstrap_agent_workspace(name: &str, agent_dir: &std::path::Path) -> Result<(), FatalError> {
    let ws_config_dir = agent_dir.join("workspace").join("config");

    trace!(agent = name, path = %ws_config_dir.display(), "creating workspace/config directory");
    std::fs::create_dir_all(&ws_config_dir).map_err(|e| {
        FatalError::Config(format!(
            "failed to create workspace/config at {}: {e}",
            ws_config_dir.display()
        ))
    })?;

    // config.toml is always regenerated; workspace files are preserved if they already exist
    // so that user edits to mcp.json and channels.toml survive re-creation.
    if !ws_config_dir.join("mcp.json").exists() {
        trace!(agent = name, "writing mcp.json");
        write_file(
            &ws_config_dir.join("mcp.json"),
            "{ \"mcpServers\": {} }\n",
            "mcp.json",
        )?;
    }

    if !ws_config_dir.join("channels.toml").exists() {
        trace!(agent = name, "writing channels.toml");
        write_file(
            &ws_config_dir.join("channels.toml"),
            "# Notification channel configuration. See channels.example.toml for options.\n",
            "channels.toml",
        )?;
    }

    Ok(())
}

/// Create a new named agent with ready-to-run config.
fn run_agent_create(name: &str) -> Result<(), FatalError> {
    validate_name(name)?;

    let registry_dir = paths::registry_base_dir()?;
    let mut registry = AgentRegistry::load(&registry_dir)?;

    if registry.get(name).is_some() {
        return Err(FatalError::Config(format!("agent '{name}' already exists")));
    }

    // Read timezone from default agent's config
    let timezone = match Config::load() {
        Ok(cfg) => cfg.timezone.to_string(),
        Err(e) => {
            warn!(error = %e, "failed to load config for timezone, defaulting to UTC");
            "UTC".to_string()
        }
    };

    let port = registry.next_available_port();
    let agent_dir = paths::agent_config_dir(name)?;
    let workspace_dir = agent_dir.join("workspace");

    debug!(agent = name, port, "creating new agent");

    // Create workspace/config/ (also creates agent_dir and workspace/ as parents)
    bootstrap_agent_workspace(name, &agent_dir)?;

    // Write ready-to-run config.toml
    trace!(agent = name, "writing config.toml");
    let workspace = workspace_dir.display();
    let config_content = format!(
        "# Agent: {name}\n\
         # Created automatically. Edit as needed.\n\
         \n\
         timezone = \"{timezone}\"\n\
         workspace_dir = \"{workspace}\"\n\
         \n\
         [gateway]\n\
         port = {port}\n",
    );
    write_file(
        &agent_dir.join("config.toml"),
        config_content,
        "agent config.toml",
    )?;

    // Write example files for reference
    trace!(agent = name, "bootstrapping example files");
    Config::bootstrap_at_dir(&agent_dir)?;

    // Register in registry
    trace!(agent = name, port, "saving agent to registry");
    registry.add(AgentEntry {
        name: name.to_string(),
        port,
    });
    registry.save(&registry_dir)?;

    println!("agent '{name}' created (port {port})");
    println!("  config: {}", agent_dir.display());
    println!("  start:  residuum serve --agent {name}");

    Ok(())
}

/// List all agents and their status.
fn run_agent_list() -> Result<(), FatalError> {
    let registry_dir = paths::registry_base_dir()?;
    let registry = AgentRegistry::load(&registry_dir)?;

    println!("{:<16} {:<7} STATUS", "NAME", "PORT");

    // Default agent
    let default_status = check_agent_status(None)?;
    let default_port =
        Config::load().map_or_else(|_| "?".to_string(), |c| c.gateway.port.to_string());
    println!("{:<16} {:<7} {default_status}", "(default)", default_port);

    // Named agents
    for agent in registry.list() {
        let status = check_agent_status(Some(&agent.name))?;
        println!("{:<16} {:<7} {status}", agent.name, agent.port);
    }

    Ok(())
}

/// Delete a named agent.
fn run_agent_delete(name: &str) -> Result<(), FatalError> {
    let registry_dir = paths::registry_base_dir()?;
    let mut registry = AgentRegistry::load(&registry_dir)?;

    if registry.get(name).is_none() {
        return Err(FatalError::Config(format!(
            "agent '{name}' not found in registry"
        )));
    }

    // Check if running
    let pid_path = paths::resolve_pid_path(Some(name))?;
    if let Ok(pid) = daemon::read_pid_file(&pid_path)
        && daemon::is_process_running(pid)
    {
        return Err(FatalError::Config(format!(
            "agent '{name}' is still running (pid {pid}); stop it first with: residuum stop --agent {name}"
        )));
    }

    // Remove directory
    let agent_dir = paths::agent_config_dir(name)?;
    if agent_dir.exists() {
        std::fs::remove_dir_all(&agent_dir).map_err(|e| {
            FatalError::Config(format!(
                "failed to remove agent directory {}: {e}",
                agent_dir.display()
            ))
        })?;
    }

    // Remove from registry
    registry.remove(name);
    registry.save(&registry_dir)?;

    println!("agent '{name}' deleted");

    Ok(())
}

/// Show details for a named agent.
fn run_agent_info(name: &str) -> Result<(), FatalError> {
    let registry_dir = paths::registry_base_dir()?;
    let registry = AgentRegistry::load(&registry_dir)?;

    let Some(entry) = registry.get(name) else {
        return Err(FatalError::Config(format!(
            "agent '{name}' not found in registry"
        )));
    };

    let agent_dir = paths::agent_config_dir(name)?;
    let status = check_agent_status(Some(name))?;

    println!("agent: {name}");
    println!("  port:      {}", entry.port);
    println!("  status:    {status}");
    println!("  config:    {}", agent_dir.display());
    println!("  workspace: {}", agent_dir.join("workspace").display());
    println!("  logs:      {}", agent_dir.join("logs").display());

    // Check providers source
    let local_providers = agent_dir.join("providers.toml");
    if local_providers.exists() {
        println!("  providers: {} (local)", local_providers.display());
    } else {
        let global = Config::config_dir()?.join("providers.toml");
        println!("  providers: {} (inherited)", global.display());
    }

    Ok(())
}

/// Check whether an agent is running and return a status string.
fn check_agent_status(agent_name: Option<&str>) -> Result<String, FatalError> {
    let pid_path = paths::resolve_pid_path(agent_name)?;
    match daemon::read_pid_file(&pid_path) {
        Ok(pid) if daemon::is_process_running(pid) => Ok(format!("running (pid {pid})")),
        Ok(pid) => Ok(format!("stopped (stale pid {pid})")),
        Err(e) => {
            if pid_path.exists() {
                debug!(error = %e, path = %pid_path.display(), "could not read pid file");
            }
            Ok("stopped".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_valid_names() {
        assert!(validate_name("researcher").is_ok());
        assert!(validate_name("my-agent").is_ok());
        assert!(validate_name("agent1").is_ok());
        assert!(validate_name("a").is_ok());
    }

    #[test]
    fn validate_name_rejects_empty() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn validate_name_rejects_too_long() {
        let long = "a".repeat(33);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn validate_name_rejects_default() {
        assert!(validate_name("default").is_err());
    }

    #[test]
    fn validate_name_rejects_special_chars() {
        assert!(validate_name("my_agent").is_err());
        assert!(validate_name("my agent").is_err());
        assert!(validate_name("my.agent").is_err());
    }

    #[test]
    fn validate_name_rejects_leading_trailing_hyphen() {
        assert!(validate_name("-agent").is_err());
        assert!(validate_name("agent-").is_err());
    }
}
