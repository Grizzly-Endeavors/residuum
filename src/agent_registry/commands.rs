//! CLI handlers for `residuum agent` subcommands.
//!
//! - `residuum agent create <name>` — bootstrap a new named agent
//! - `residuum agent list` — show all agents and their status
//! - `residuum agent delete <name>` — remove a named agent
//! - `residuum agent info <name>` — show agent details

use crate::config::Config;
use crate::daemon;
use crate::error::ResiduumError;

use super::paths;
use super::registry::AgentRegistry;

/// Dispatch `residuum agent <subcommand>` from CLI args.
///
/// # Errors
///
/// Returns `ResiduumError` if the subcommand fails.
pub fn run_agent_command(args: &[String]) -> Result<(), ResiduumError> {
    let sub = args.get(2).map(String::as_str);
    match sub {
        Some("create") => {
            let Some(name) = args.get(3) else {
                eprintln!("usage: residuum agent create <name>");
                return Ok(());
            };
            run_agent_create(name)
        }
        Some("list") => run_agent_list(),
        Some("delete") => {
            let Some(name) = args.get(3) else {
                eprintln!("usage: residuum agent delete <name>");
                return Ok(());
            };
            run_agent_delete(name)
        }
        Some("info") => {
            let Some(name) = args.get(3) else {
                eprintln!("usage: residuum agent info <name>");
                return Ok(());
            };
            run_agent_info(name)
        }
        _ => {
            eprintln!("usage: residuum agent <create|list|delete|info>");
            eprintln!();
            eprintln!("  create <name>   create a new named agent");
            eprintln!("  list            list all agents and their status");
            eprintln!("  delete <name>   remove a named agent");
            eprintln!("  info <name>     show agent details");
            Ok(())
        }
    }
}

/// Validate an agent name: alphanumeric + hyphens, 1-32 chars, not "default".
fn validate_name(name: &str) -> Result<(), ResiduumError> {
    if name.is_empty() || name.len() > 32 {
        return Err(ResiduumError::Config(
            "agent name must be 1-32 characters".to_string(),
        ));
    }
    if name == "default" {
        return Err(ResiduumError::Config(
            "\"default\" is reserved for the unnamed agent".to_string(),
        ));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ResiduumError::Config(
            "agent name must contain only alphanumeric characters and hyphens".to_string(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ResiduumError::Config(
            "agent name must not start or end with a hyphen".to_string(),
        ));
    }
    Ok(())
}

/// Create a new named agent with ready-to-run config.
fn run_agent_create(name: &str) -> Result<(), ResiduumError> {
    validate_name(name)?;

    let registry_dir = paths::registry_base_dir()?;
    let mut registry = AgentRegistry::load(&registry_dir)?;

    if registry.get(name).is_some() {
        return Err(ResiduumError::Config(format!(
            "agent '{name}' already exists"
        )));
    }

    // Read timezone from default agent's config
    let timezone = match Config::load() {
        Ok(cfg) => cfg.timezone.to_string(),
        Err(_) => "UTC".to_string(),
    };

    let port = registry.next_available_port();
    let agent_dir = paths::agent_config_dir(name)?;
    let workspace_dir = agent_dir.join("workspace");

    // Create agent directory
    std::fs::create_dir_all(&agent_dir).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to create agent directory {}: {e}",
            agent_dir.display()
        ))
    })?;

    // Write ready-to-run config.toml
    let config_content = format!(
        "# Agent: {name}\n\
         # Created automatically. Edit as needed.\n\
         \n\
         timezone = \"{timezone}\"\n\
         workspace_dir = \"{workspace}\"\n\
         \n\
         [gateway]\n\
         port = {port}\n",
        workspace = workspace_dir.display(),
    );
    std::fs::write(agent_dir.join("config.toml"), config_content)
        .map_err(|e| ResiduumError::Config(format!("failed to write agent config.toml: {e}")))?;

    // Write example files for reference
    Config::bootstrap_at_dir(&agent_dir)?;

    // Create workspace/config/ with starter files
    let ws_config_dir = workspace_dir.join("config");
    std::fs::create_dir_all(&ws_config_dir).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to create workspace/config at {}: {e}",
            ws_config_dir.display()
        ))
    })?;

    if !ws_config_dir.join("mcp.json").exists() {
        std::fs::write(ws_config_dir.join("mcp.json"), "{ \"mcpServers\": {} }\n")
            .map_err(|e| ResiduumError::Config(format!("failed to write mcp.json: {e}")))?;
    }

    if !ws_config_dir.join("channels.toml").exists() {
        std::fs::write(
            ws_config_dir.join("channels.toml"),
            "# Notification channel configuration. See channels.example.toml for options.\n",
        )
        .map_err(|e| ResiduumError::Config(format!("failed to write channels.toml: {e}")))?;
    }

    // Register in registry
    registry.add(name.to_string(), port);
    registry.save(&registry_dir)?;

    eprintln!("agent '{name}' created (port {port})");
    eprintln!("  config: {}", agent_dir.display());
    eprintln!("  start:  residuum serve --agent {name}");

    Ok(())
}

/// List all agents and their status.
fn run_agent_list() -> Result<(), ResiduumError> {
    let registry_dir = paths::registry_base_dir()?;
    let registry = AgentRegistry::load(&registry_dir)?;

    eprintln!("{:<16} {:<7} STATUS", "NAME", "PORT");

    // Default agent
    let default_status = check_agent_status(None)?;
    eprintln!("{:<16} {:<7} {default_status}", "(default)", "7700");

    // Named agents
    for agent in registry.list() {
        let status = check_agent_status(Some(&agent.name))?;
        eprintln!("{:<16} {:<7} {status}", agent.name, agent.port);
    }

    Ok(())
}

/// Delete a named agent.
fn run_agent_delete(name: &str) -> Result<(), ResiduumError> {
    let registry_dir = paths::registry_base_dir()?;
    let mut registry = AgentRegistry::load(&registry_dir)?;

    if registry.get(name).is_none() {
        return Err(ResiduumError::Config(format!(
            "agent '{name}' not found in registry"
        )));
    }

    // Check if running
    let pid_path = paths::resolve_pid_path(Some(name))?;
    if let Ok(pid) = daemon::read_pid_file(&pid_path)
        && daemon::is_process_running(pid)
    {
        return Err(ResiduumError::Config(format!(
            "agent '{name}' is still running (pid {pid}); stop it first with: residuum stop --agent {name}"
        )));
    }

    // Remove directory
    let agent_dir = paths::agent_config_dir(name)?;
    if agent_dir.exists() {
        std::fs::remove_dir_all(&agent_dir).map_err(|e| {
            ResiduumError::Config(format!(
                "failed to remove agent directory {}: {e}",
                agent_dir.display()
            ))
        })?;
    }

    // Remove from registry
    registry.remove(name);
    registry.save(&registry_dir)?;

    eprintln!("agent '{name}' deleted");

    Ok(())
}

/// Show details for a named agent.
fn run_agent_info(name: &str) -> Result<(), ResiduumError> {
    let registry_dir = paths::registry_base_dir()?;
    let registry = AgentRegistry::load(&registry_dir)?;

    let Some(entry) = registry.get(name) else {
        return Err(ResiduumError::Config(format!(
            "agent '{name}' not found in registry"
        )));
    };

    let agent_dir = paths::agent_config_dir(name)?;
    let status = check_agent_status(Some(name))?;

    eprintln!("agent: {name}");
    eprintln!("  port:      {}", entry.port);
    eprintln!("  status:    {status}");
    eprintln!("  config:    {}", agent_dir.display());
    eprintln!("  workspace: {}", agent_dir.join("workspace").display());
    eprintln!("  logs:      {}", agent_dir.join("logs").display());

    // Check providers source
    let local_providers = agent_dir.join("providers.toml");
    if local_providers.exists() {
        eprintln!("  providers: {} (local)", local_providers.display());
    } else {
        let global = Config::config_dir()?.join("providers.toml");
        eprintln!("  providers: {} (inherited)", global.display());
    }

    Ok(())
}

/// Check whether an agent is running and return a status string.
fn check_agent_status(agent_name: Option<&str>) -> Result<String, ResiduumError> {
    let pid_path = paths::resolve_pid_path(agent_name)?;
    match daemon::read_pid_file(&pid_path) {
        Ok(pid) if daemon::is_process_running(pid) => Ok(format!("running (pid {pid})")),
        Ok(pid) => Ok(format!("stopped (stale pid {pid})")),
        Err(_) => Ok("stopped".to_string()),
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
