use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentState {
    Starting,
    Running,
    Stopped,
    Failed,
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum A2aVisibility {
    Public,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentErrorKind {
    Config,
    PortConflict,
    Crash,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentLastError {
    pub message: String,
    pub kind: AgentErrorKind,
    pub reason: String,
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSummary {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    pub state: AgentState,
    pub last_error: Option<AgentLastError>,
    pub autostart: bool,
    pub role: Option<String>,
    pub a2a_visibility: A2aVisibility,
    #[serde(default)]
    pub teams_configured: bool,
}

impl AgentSummary {
    #[must_use]
    pub fn label(&self) -> &str {
        if self.display_name.is_empty() {
            &self.name
        } else {
            &self.display_name
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, TS)]
#[ts(export, type = "\"user\" | `agent:${string}`")]
pub enum Actor {
    User,
    Agent(String),
}

impl Actor {
    #[must_use]
    pub fn wire(&self) -> String {
        match self {
            Self::User => "user".to_string(),
            Self::Agent(name) => format!("agent:{name}"),
        }
    }
}

impl Serialize for Actor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.wire())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct CreateAgentRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub models_from: Option<String>,
    #[serde(default)]
    pub providers_toml: Option<String>,
    #[serde(default)]
    pub a2a_visibility: Option<A2aVisibility>,
    #[serde(skip)]
    #[ts(skip)]
    pub creator_hop: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct DeleteOutcome {
    pub deleted: bool,
    pub checkpoint_id: Option<String>,
}

/// Why a lifecycle or lookup call failed. The HTTP layer maps these to
/// status codes: `NotFound` and `NoDeletedAgent` 404, `InvalidName`/`InvalidRequest` 400,
/// `AlreadyExists`/`NotRunning` 409, `ShuttingDown` 503, `Failed` 500.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    #[error("no agent named '{0}'")]
    NotFound(String),
    #[error("{0}")]
    InvalidName(String),
    #[error("{0}")]
    InvalidRequest(String),
    #[error("an agent named '{0}' already exists")]
    AlreadyExists(String),
    #[error("there is no deleted agent named '{0}' to restore")]
    NoDeletedAgent(String),
    #[error("{name} is {state}")]
    NotRunning { name: String, state: AgentState },
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    ShuttingDown(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scout() -> AgentSummary {
        AgentSummary {
            name: "scout".to_string(),
            display_name: "scout".to_string(),
            state: AgentState::Running,
            last_error: None,
            autostart: true,
            role: Some("Research".to_string()),
            a2a_visibility: A2aVisibility::Private,
            teams_configured: false,
        }
    }

    #[test]
    fn summary_matches_the_contract_shape() {
        assert_eq!(
            serde_json::to_value(scout()).unwrap(),
            json!({
                "name": "scout",
                "display_name": "scout",
                "state": "running",
                "last_error": null,
                "autostart": true,
                "role": "Research",
                "a2a_visibility": "private",
                "teams_configured": false,
            })
        );
    }

    #[test]
    fn a_last_error_carries_its_kind_and_the_reason_without_the_wrapper() {
        let error = AgentLastError {
            message: "scout couldn't start: config error: bad model. Fix its settings.".to_string(),
            kind: AgentErrorKind::PortConflict,
            reason: "config error: bad model".to_string(),
            at: DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            json!({
                "message": "scout couldn't start: config error: bad model. Fix its settings.",
                "kind": "port_conflict",
                "reason": "config error: bad model",
                "at": "2026-09-30T12:00:00Z",
            })
        );
        for (kind, wire) in [
            (AgentErrorKind::Config, "config"),
            (AgentErrorKind::Crash, "crash"),
            (AgentErrorKind::Other, "other"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(wire));
        }
    }

    #[test]
    fn not_running_error_reads_plainly() {
        let err = LifecycleError::NotRunning {
            name: "scout".to_string(),
            state: AgentState::Stopped,
        };
        assert_eq!(err.to_string(), "scout is stopped");
    }
}
