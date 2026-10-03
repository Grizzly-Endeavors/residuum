//! Agent abilities, skills, tools, and retry settings.

use std::path::{Path, PathBuf};

use crate::inference::retry::RetryConfig;
use crate::skills::SkillDir;
use crate::util::FatalError;

use super::super::deserialize::{
    AgentAbilitiesConfigFile, AgentConfigFile, AutoModeConfigFile, SkillsConfigFile,
    ToolsConfigFile,
};
use super::super::types::{AgentAbilitiesConfig, AutoModeConfig, SkillsConfig, ToolsConfig};

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

/// Resolve the agent file's `[auto_mode]` section.
pub(super) fn resolve_auto_mode_config(
    file: Option<&AgentConfigFile>,
    notices: &mut Vec<String>,
) -> AutoModeConfig {
    resolve_auto_mode_section(file.and_then(|f| f.auto_mode.as_ref()), notices)
}

/// Resolve `[auto_mode]`. Blank rules are dropped; an out-of-range
/// threshold falls back to the default with a notice.
fn resolve_auto_mode_section(
    section: Option<&AutoModeConfigFile>,
    notices: &mut Vec<String>,
) -> AutoModeConfig {
    let mut cfg = AutoModeConfig::default();
    let Some(s) = section else {
        return cfg;
    };
    let clean = |rules: &Option<Vec<String>>| -> Vec<String> {
        rules
            .iter()
            .flatten()
            .map(|r| r.trim())
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .collect()
    };
    cfg.enabled = s.enabled.unwrap_or(false);
    cfg.deny = clean(&s.deny);
    cfg.allow = clean(&s.allow);
    if let Some(t) = s.threshold {
        if t > 0.0 && t <= 1.0 {
            cfg.threshold = t;
        } else {
            tracing::warn!(threshold = t, "[auto_mode] threshold out of range");
            notices.push(format!(
                "[auto_mode] threshold must be above 0 and at most 1, got {t} — using {}.",
                cfg.threshold
            ));
        }
    }
    if cfg.enabled && cfg.deny.is_empty() {
        notices.push(
            "[auto_mode] is on but has no deny rules, so no tool call is checked.".to_string(),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn auto_mode(toml_text: &str) -> (AutoModeConfig, Vec<String>) {
        let section: AutoModeConfigFile = toml::from_str(toml_text).unwrap();
        let mut notices = Vec::new();
        let cfg = resolve_auto_mode_section(Some(&section), &mut notices);
        (cfg, notices)
    }

    #[test]
    fn auto_mode_is_off_by_default() {
        let mut notices = Vec::new();
        let cfg = resolve_auto_mode_config(None, &mut notices);
        assert!(!cfg.enabled);
        assert!(!cfg.is_active());
        assert!(notices.is_empty());
    }

    #[test]
    fn auto_mode_trims_and_drops_blank_rules() {
        let (cfg, notices) = auto_mode(
            "enabled = true\ndeny = [\"  Push to main \", \"\"]\nallow = [\" \", \"Push to a branch\"]\nthreshold = 0.7\n",
        );
        assert_eq!(cfg.deny, ["Push to main"]);
        assert_eq!(cfg.allow, ["Push to a branch"]);
        assert!((cfg.threshold - 0.7).abs() < f64::EPSILON);
        assert!(cfg.is_active());
        assert!(notices.is_empty(), "{notices:?}");
    }

    #[test]
    fn out_of_range_threshold_falls_back_with_a_notice() {
        let (cfg, notices) = auto_mode("enabled = true\ndeny = [\"x\"]\nthreshold = 1.5\n");
        assert!(
            (cfg.threshold - crate::config::types::DEFAULT_AUTO_MODE_THRESHOLD).abs()
                < f64::EPSILON
        );
        assert!(
            notices.iter().any(|n| n.contains("threshold")),
            "{notices:?}"
        );
    }

    #[test]
    fn enabled_without_deny_rules_is_flagged() {
        let (cfg, notices) = auto_mode("enabled = true\n");
        assert!(!cfg.is_active(), "nothing to check");
        assert!(
            notices.iter().any(|n| n.contains("no deny rules")),
            "{notices:?}"
        );
    }
}
