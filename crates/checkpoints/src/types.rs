use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RepoKind {
    Workspace,
    Team,
    AgentConfig,
    Hub,
}

impl RepoKind {
    #[must_use]
    pub(super) fn dir_name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace.git",
            Self::Team => "team.git",
            Self::AgentConfig => "agent-config.git",
            Self::Hub => "hub-config.git",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CheckpointTrigger {
    TurnStart,
    TurnEnd,
    PreAction,
    PreConfigWrite,
    Restore,
    Undo,
}

impl CheckpointTrigger {
    /// Stable string used in the commit trailer and the HTTP/tool-facing API.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TurnStart => "turn_start",
            Self::TurnEnd => "turn_end",
            Self::PreAction => "pre_action",
            Self::PreConfigWrite => "pre_config_write",
            Self::Restore => "restore",
            Self::Undo => "undo",
        }
    }

    #[must_use]
    pub(super) fn from_label(label: &str) -> Option<Self> {
        match label {
            "turn_start" => Some(Self::TurnStart),
            "turn_end" => Some(Self::TurnEnd),
            "pre_action" => Some(Self::PreAction),
            "pre_config_write" => Some(Self::PreConfigWrite),
            "restore" => Some(Self::Restore),
            "undo" => Some(Self::Undo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckpointContext {
    pub address: String,
    pub run_id: Option<String>,
    pub turn_id: Option<String>,
    pub trigger: CheckpointTrigger,
    pub summary: String,
}

impl CheckpointContext {
    #[must_use]
    pub fn system(trigger: CheckpointTrigger, summary: impl Into<String>) -> Self {
        Self {
            address: "system".to_string(),
            run_id: None,
            turn_id: None,
            trigger,
            summary: summary.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckpointSummary {
    pub id: String,
    #[ts(type = "string")]
    pub timestamp: DateTime<Utc>,
    pub address: String,
    pub run_id: Option<String>,
    pub turn_id: Option<String>,
    pub trigger: CheckpointTrigger,
    pub summary: String,
    pub changed_path_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChangedPath {
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckpointDetail {
    pub summary: CheckpointSummary,
    pub changed_paths: Vec<ChangedPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoStats {
    #[ts(type = "number")]
    pub on_disk_bytes: u64,
    #[ts(type = "number")]
    pub checkpoint_count: u64,
    #[ts(type = "string | null")]
    pub oldest: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RestoreOutcome {
    pub checkpoint_id: String,
    pub restored_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UndoOutcome {
    pub checkpoint_id: String,
    pub reverted_paths: Vec<String>,
    pub skipped_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckpointPage {
    pub items: Vec<CheckpointSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("no checkpoint found matching '{0}'")]
    NotFound(String),
    #[error("path '{0}' not found at checkpoint '{1}'")]
    PathNotFound(String, String),
    #[error("invalid page cursor")]
    InvalidCursor,
    #[error("the workspace repository is not a config repository")]
    NotAConfigRepo,
    #[error("checkpoint repository error: {0}")]
    Git(String),
    #[error("filesystem error: {0}")]
    Io(String),
}
