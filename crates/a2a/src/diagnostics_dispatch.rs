use std::path::{Path, PathBuf};

use residuum_core::diagnostics::Diagnostic;

#[derive(Debug, Clone)]
pub struct DiagnosticsPaths {
    pub config_dir: PathBuf,
    pub workspace_dir: PathBuf,
    pub hub_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    HubConfigToml,
    ConfigToml,
    ProvidersToml,
    ChannelsToml,
    McpJson,
    A2aJson,
    Heartbeat,
    SkillMd,
}

fn recognize(path: &Path, paths: &DiagnosticsPaths) -> Option<FileKind> {
    let file_name = path.file_name().and_then(|n| n.to_str())?;

    if paths_match(path, &paths.hub_dir.join("config.toml")) {
        return Some(FileKind::HubConfigToml);
    }
    if paths_match(path, &paths.config_dir.join("config.toml")) {
        return Some(FileKind::ConfigToml);
    }
    if paths_match(path, &paths.config_dir.join("providers.toml")) {
        return Some(FileKind::ProvidersToml);
    }

    let layout = residuum_config::WorkspaceLayout::new(&paths.workspace_dir);
    if paths_match(path, &layout.channels_toml()) {
        return Some(FileKind::ChannelsToml);
    }
    if paths_match(path, &layout.mcp_json()) {
        return Some(FileKind::McpJson);
    }
    if paths_match(path, &layout.a2a_agents_json()) {
        return Some(FileKind::A2aJson);
    }

    if file_name == "HEARTBEAT.yml" {
        return Some(FileKind::Heartbeat);
    }
    if file_name == "SKILL.md" {
        return Some(FileKind::SkillMd);
    }

    None
}

#[must_use]
pub fn is_recognized(path: &Path, paths: &DiagnosticsPaths) -> bool {
    recognize(path, paths).is_some()
}

#[must_use]
pub fn diagnose(path: &Path, content: &str, paths: &DiagnosticsPaths) -> Option<Vec<Diagnostic>> {
    let kind = recognize(path, paths)?;

    Some(match kind {
        FileKind::HubConfigToml => {
            residuum_config::HubConfig::diagnose_toml(content, &paths.hub_dir)
        }
        FileKind::ConfigToml | FileKind::ProvidersToml => {
            let agent_name = paths
                .workspace_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            match residuum_config::HubConfig::load_at(&paths.hub_dir) {
                Ok(hub) => {
                    if matches!(kind, FileKind::ConfigToml) {
                        residuum_config::Config::diagnose_agent_toml(
                            content,
                            &paths.workspace_dir,
                            agent_name,
                            &hub,
                        )
                    } else {
                        residuum_config::Config::diagnose_agent_providers_toml(
                            content,
                            &paths.workspace_dir,
                            agent_name,
                            &hub,
                        )
                    }
                }
                Err(e) => vec![Diagnostic::error(format!(
                    "hub config couldn't be loaded, so this can't be fully validated: {e}"
                ))],
            }
        }
        FileKind::ChannelsToml => residuum_workspace::config::diagnose_channels_toml(content),
        FileKind::McpJson => residuum_workspace::config::diagnose_mcp_json(content),
        FileKind::A2aJson => crate::client::config::diagnose_a2a_json(content),
        FileKind::Heartbeat => residuum_proactivity::pulse::types::diagnose_heartbeat(content),
        FileKind::SkillMd => residuum_skills::diagnose_skill_md(content),
    })
}

fn paths_match(path: &Path, target: &Path) -> bool {
    if path == target {
        return true;
    }
    match (path.canonicalize(), target.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_core::diagnostics::Severity;

    #[test]
    fn diagnose_returns_none_for_unrelated_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DiagnosticsPaths {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: dir.path().join("workspace"),
            hub_dir: dir.path().join("hub"),
        };
        assert!(diagnose(Path::new("notes.md"), "hello", &paths).is_none());
    }

    #[test]
    fn diagnose_validates_the_hub_config_against_the_hub_schema() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        let hub_config = hub_dir.join("config.toml");
        std::fs::write(&hub_config, "timezone = \"UTC\"\n").unwrap();
        let paths = DiagnosticsPaths {
            config_dir: dir.path().join("agent/config"),
            workspace_dir: dir.path().join("agent"),
            hub_dir,
        };

        let valid = diagnose(&hub_config, "timezone = \"UTC\"\n", &paths)
            .expect("the hub config.toml should be recognized");
        assert!(valid.is_empty(), "a valid hub config: {valid:?}");

        let wrong_schema = diagnose(
            &hub_config,
            "[memory]\nobserver_cooldown_secs = 1\n",
            &paths,
        )
        .expect("the hub config.toml should be recognized");
        assert!(
            wrong_schema
                .iter()
                .any(|d| d.severity == Severity::Warning || d.severity == Severity::Error),
            "an agent-only section in the hub config should be flagged: {wrong_schema:?}"
        );
    }

    #[test]
    fn diagnose_dispatches_config_toml_by_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("providers.toml"),
            "[models]\nmain = \"a/b\"\n",
        )
        .unwrap();
        let paths = DiagnosticsPaths {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: dir.path().join("workspace"),
            hub_dir: dir.path().join("hub"),
        };
        let diagnostics = diagnose(&dir.path().join("config.toml"), "not valid toml [", &paths)
            .expect("config.toml should be recognized");
        assert!(
            !diagnostics.is_empty(),
            "invalid TOML should produce a diagnostic"
        );
        assert_eq!(diagnostics.first().unwrap().severity, Severity::Error);
    }

    #[test]
    fn diagnose_dispatches_heartbeat_by_filename_regardless_of_directory() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DiagnosticsPaths {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: dir.path().join("workspace"),
            hub_dir: dir.path().join("hub"),
        };
        let diagnostics = diagnose(
            Path::new("some/nested/HEARTBEAT.yml"),
            "pulses:\n  - name: a\n    schedule: 30m\n",
            &paths,
        )
        .expect("HEARTBEAT.yml should be recognized regardless of directory");
        assert!(diagnostics.is_empty(), "valid heartbeat has no diagnostics");
    }
}
