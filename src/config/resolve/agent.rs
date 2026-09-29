//! Agent abilities, skills, tools, and retry settings.

use std::path::{Path, PathBuf};

use crate::inference::retry::RetryConfig;
use crate::skills::SkillDir;
use crate::util::FatalError;

use super::super::deserialize::{
    AgentAbilitiesConfigFile, AgentConfigFile, SkillsConfigFile, ToolsConfigFile,
};
use super::super::types::{AgentAbilitiesConfig, SkillsConfig, ToolsConfig};

/// Resolve skills configuration from TOML section.
///
/// Layers in priority order: the agent's own `skills/`, the shared
/// `team/skills/`, then the directories from `[skills].dirs` (expanded).
pub(super) fn resolve_skills_config(
    section: Option<&SkillsConfigFile>,
    workspace_dir: &Path,
) -> SkillsConfig {
    let layout = crate::workspace::layout::WorkspaceLayout::new(workspace_dir);
    let mut dirs = vec![
        SkillDir::agent(layout.skills_dir()),
        SkillDir::team(layout.team().skills_dir()),
    ];

    if let Some(extra) = section.and_then(|s| s.dirs.as_ref()) {
        for raw in extra {
            let expanded = shellexpand::tilde(raw);
            dirs.push(SkillDir::configured(PathBuf::from(expanded.as_ref())));
        }
    }

    SkillsConfig { dirs }
}

/// Resolve runtime tool PATH configuration from TOML section.
///
/// Directories are ordered highest precedence first: configured `[tools].path`
/// entries (expanded, in listed order) followed by the default persistent
/// `<hub_dir>/bin` (`~/.residuum/hub/bin`). All are prepended to the inherited
/// `PATH` of spawned children at spawn time.
pub(super) fn resolve_tools_config(
    section: Option<&ToolsConfigFile>,
    hub_dir: &Path,
) -> ToolsConfig {
    let mut dirs = Vec::new();

    if let Some(extra) = section.and_then(|s| s.path.as_ref()) {
        for raw in extra {
            let expanded = shellexpand::tilde(raw);
            dirs.push(PathBuf::from(expanded.as_ref()));
        }
    }

    // Default persistent dir, lowest precedence of the tool dirs.
    dirs.push(super::super::HubPaths::new(hub_dir).bin_dir());

    ToolsConfig { dirs }
}

/// Resolve retry configuration from TOML section with defaults.
pub(super) fn resolve_retry_config(file: Option<&AgentConfigFile>) -> RetryConfig {
    let r = file.and_then(|f| f.retry.as_ref());
    let mut cfg = RetryConfig::default();
    if let Some(v) = r.and_then(|r| r.max_retries) {
        cfg.max_retries = v;
    }
    if let Some(v) = r.and_then(|r| r.initial_delay_ms) {
        cfg.initial_delay = std::time::Duration::from_millis(v);
    }
    if let Some(v) = r.and_then(|r| r.max_delay_ms) {
        cfg.max_delay = std::time::Duration::from_millis(v);
    }
    if let Some(v) = r.and_then(|r| r.backoff_multiplier) {
        cfg.backoff_multiplier = v;
    }
    cfg
}

/// Resolve agent ability gates and turn limits from the TOML section.
///
/// # Errors
/// Returns `FatalError::Config` if `max_tool_iterations` is set to `0` — a
/// turn that stops before ever calling a tool isn't a usable limit, so this
/// is rejected rather than silently accepted.
pub(super) fn resolve_agent_config(
    section: Option<&AgentAbilitiesConfigFile>,
) -> Result<AgentAbilitiesConfig, FatalError> {
    let mut cfg = AgentAbilitiesConfig::default();
    if let Some(s) = section {
        if let Some(v) = s.modify_mcp {
            cfg.modify_mcp = v;
        }
        if let Some(v) = s.modify_channels {
            cfg.modify_channels = v;
        }
        if let Some(limit) = s.max_tool_iterations {
            if limit == 0 {
                return Err(FatalError::Config(
                    "agent.max_tool_iterations must be at least 1 (leave it unset for unlimited)"
                        .to_string(),
                ));
            }
            cfg.max_tool_iterations = Some(limit);
        }
        if let Some(v) = s.repeat_call_guard_enabled {
            cfg.repeat_call_guard.enabled = v;
        }
        if let Some(v) = s.repeat_call_steer_after {
            cfg.repeat_call_guard.steer_after = v;
        }
        if let Some(v) = s.repeat_call_stop_after {
            cfg.repeat_call_guard.stop_after = v;
        }
    }
    Ok(cfg)
}
