//! The workspace agent card (`{workspace}/config/agent-card.json`): what the
//! user/agent declares about this agent, merged at serve time with runtime
//! facts (interface URLs, capabilities, security) into the wire
//! [`a2a::AgentCard`]. See `docs/systems-usage/a2a.md`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::A2aVisibility;

/// One skill declared in the workspace agent card file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCardSkillFile {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
}

/// Serde model for the workspace agent card file. A skill `id` that also
/// names a workspace skill starts an inbound A2A session with that skill —
/// see `docs/systems-usage/a2a.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCardFile {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub skills: Vec<AgentCardSkillFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_input_modes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_output_modes: Option<Vec<String>>,
}

/// Errors loading, parsing, or validating the workspace agent card file.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum CardError {
    /// The file could not be read.
    #[error("failed to read agent card at {path}: {message}")]
    Read { path: String, message: String },
    /// The file's JSON did not match the expected shape.
    #[error("failed to parse agent card at {path}: {message}")]
    Parse { path: String, message: String },
    /// The file parsed but failed content validation.
    #[error("invalid agent card at {path}: {message}")]
    Invalid { path: String, message: String },
}

impl AgentCardFile {
    /// Load and validate the agent card file at `path`.
    ///
    /// # Errors
    /// Returns [`CardError::Read`] if the file can't be read,
    /// [`CardError::Parse`] if it isn't valid JSON in the expected shape,
    /// or [`CardError::Invalid`] if it fails content validation.
    pub fn load(path: &Path) -> Result<Self, CardError> {
        let raw = std::fs::read_to_string(path).map_err(|e| CardError::Read {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let file: Self = serde_json::from_str(&raw).map_err(|e| CardError::Parse {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        file.validate(path)?;
        Ok(file)
    }

    /// Check content invariants JSON schema alone can't express: non-empty
    /// name/description, and skill ids that are non-empty and unique (the
    /// A2A spec and the skill-mapping rule in `docs/systems-usage/a2a.md`
    /// both depend on ids being unique).
    ///
    /// # Errors
    /// Returns [`CardError::Invalid`] describing the first problem found.
    fn validate(&self, path: &Path) -> Result<(), CardError> {
        let invalid = |message: String| CardError::Invalid {
            path: path.display().to_string(),
            message,
        };
        if self.name.trim().is_empty() {
            return Err(invalid("name must not be empty".to_string()));
        }
        if self.description.trim().is_empty() {
            return Err(invalid("description must not be empty".to_string()));
        }
        let mut seen_ids = std::collections::HashSet::new();
        for skill in &self.skills {
            if skill.id.trim().is_empty() {
                return Err(invalid("every skill must have a non-empty id".to_string()));
            }
            if !seen_ids.insert(skill.id.as_str()) {
                return Err(invalid(format!("duplicate skill id '{}'", skill.id)));
            }
            if skill.name.trim().is_empty() {
                return Err(invalid(format!(
                    "skill '{}' must have a non-empty name",
                    skill.id
                )));
            }
            if skill.description.trim().is_empty() {
                return Err(invalid(format!(
                    "skill '{}' must have a non-empty description",
                    skill.id
                )));
            }
        }
        Ok(())
    }
}

/// Runtime facts the server fills into the card that the workspace file
/// doesn't (and shouldn't) know about.
#[derive(Debug, Clone)]
pub struct CardRuntime {
    /// Base URL other agents reach this instance's A2A interfaces at: JSON-RPC
    /// is served at this exact URL, HTTP+JSON (REST) at `{base}/rest`.
    ///
    /// This is `[a2a] public_url` when set, or a local fallback
    /// (`http://{bind}:{port}`) otherwise — good for same-host and
    /// same-network callers, not for callers over the public internet.
    pub interfaces_base_url: String,
    /// Who may reach this agent without a caller key. Not reflected in the
    /// card's content — a private agent's card is simply never served to an
    /// unauthenticated caller (see [`crate::a2a::auth`]) — but carried here
    /// so callers building the runtime don't need a second config lookup.
    pub visibility: A2aVisibility,
}

impl CardRuntime {
    /// Build runtime facts from the resolved `[a2a]` config and the
    /// gateway's bind address: `public_url` when set, otherwise a local
    /// fallback pointing at this instance's own A2A port.
    #[must_use]
    pub fn from_config(a2a: &crate::config::A2aConfig, gateway_bind: &str) -> Self {
        let interfaces_base_url = a2a
            .public_url
            .clone()
            .unwrap_or_else(|| format!("http://{gateway_bind}:{}", a2a.port));
        Self {
            interfaces_base_url,
            visibility: a2a.visibility,
        }
    }
}

/// Build the wire [`a2a::AgentCard`] from the workspace file and runtime facts.
#[must_use]
pub fn build_agent_card(file: &AgentCardFile, runtime: &CardRuntime) -> a2a::AgentCard {
    let base = runtime.interfaces_base_url.trim_end_matches('/');
    let jsonrpc = a2a::AgentInterface::new(base.to_string(), a2a::TRANSPORT_PROTOCOL_JSONRPC);
    let rest = a2a::AgentInterface::new(format!("{base}/rest"), a2a::TRANSPORT_PROTOCOL_HTTP_JSON);

    let mut security_schemes = HashMap::new();
    security_schemes.insert(
        "bearer".to_string(),
        a2a::SecurityScheme::HttpAuth(a2a::HttpAuthSecurityScheme {
            scheme: "Bearer".to_string(),
            description: Some("A caller key minted with `residuum a2a keys create`".to_string()),
            bearer_format: None,
        }),
    );
    let security_requirements = vec![
        [("bearer".to_string(), Vec::new())]
            .into_iter()
            .collect::<HashMap<_, _>>(),
    ];

    a2a::AgentCard {
        name: file.name.clone(),
        description: file.description.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        supported_interfaces: vec![jsonrpc, rest],
        capabilities: a2a::AgentCapabilities {
            streaming: Some(true),
            push_notifications: Some(false),
            extensions: None,
            extended_agent_card: None,
        },
        default_input_modes: file
            .default_input_modes
            .clone()
            .unwrap_or_else(|| vec!["text/plain".to_string()]),
        default_output_modes: file
            .default_output_modes
            .clone()
            .unwrap_or_else(|| vec!["text/plain".to_string()]),
        skills: file
            .skills
            .iter()
            .map(|skill| a2a::AgentSkill {
                id: skill.id.clone(),
                name: skill.name.clone(),
                description: skill.description.clone(),
                tags: skill.tags.clone(),
                examples: skill.examples.clone(),
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            })
            .collect(),
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: Some(security_schemes),
        security_requirements: Some(security_requirements),
        signatures: None,
    }
}

/// Live agent card state: the currently-served [`a2a::AgentCard`], hot-reloaded
/// from the workspace file by the workspace watcher. Implements
/// [`a2a_server::AgentCardProducer`] directly so it plugs straight into
/// `a2a_server::agent_card_router`.
pub struct CardState {
    current: RwLock<Arc<a2a::AgentCard>>,
}

/// Shared handle to the live agent card.
pub type SharedCardState = Arc<CardState>;

impl CardState {
    /// Build initial state from an already-built card.
    #[must_use]
    pub fn new(card: a2a::AgentCard) -> SharedCardState {
        Arc::new(Self {
            current: RwLock::new(Arc::new(card)),
        })
    }

    /// Load, build, and wrap the card at `path` under `runtime`.
    ///
    /// # Errors
    /// Returns whatever [`AgentCardFile::load`] returns — there is no "last
    /// good" card yet to fall back to at startup.
    pub fn load(path: &Path, runtime: &CardRuntime) -> Result<SharedCardState, CardError> {
        let file = AgentCardFile::load(path)?;
        Ok(Self::new(build_agent_card(&file, runtime)))
    }

    /// [`load`](Self::load), falling back to a minimal generic card and
    /// logging a warning if the workspace file is missing or invalid.
    /// Startup must never fail just because the agent card is broken.
    #[must_use]
    pub fn load_or_default(path: &Path, runtime: &CardRuntime) -> SharedCardState {
        Self::load(path, runtime).unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "failed to load agent card; serving a minimal fallback until it's fixed"
            );
            let fallback = AgentCardFile {
                name: "Residuum agent".to_string(),
                description: "A personal AI agent.".to_string(),
                skills: Vec::new(),
                default_input_modes: None,
                default_output_modes: None,
            };
            Self::new(build_agent_card(&fallback, runtime))
        })
    }

    /// The currently-served card.
    #[must_use]
    pub fn current(&self) -> Arc<a2a::AgentCard> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Reload from `path` under `runtime`, replacing the live card on
    /// success. On failure the previous card keeps being served; the caller
    /// decides what to log or notify.
    ///
    /// # Errors
    /// Returns whatever [`AgentCardFile::load`] returns.
    pub fn reload(&self, path: &Path, runtime: &CardRuntime) -> Result<(), CardError> {
        let file = AgentCardFile::load(path)?;
        let card = build_agent_card(&file, runtime);
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(card);
        Ok(())
    }
}

impl a2a_server::AgentCardProducer for CardState {
    fn card(&self) -> a2a::AgentCard {
        (*self.current()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> CardRuntime {
        CardRuntime {
            interfaces_base_url: "http://127.0.0.1:7702".to_string(),
            visibility: A2aVisibility::Public,
        }
    }

    fn write_card(dir: &std::path::Path, json: &str) -> std::path::PathBuf {
        let path = dir.join("agent-card.json");
        std::fs::write(&path, json).unwrap();
        path
    }

    #[test]
    fn loads_minimal_card_with_empty_skills() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_card(
            dir.path(),
            r#"{"name": "My Agent", "description": "does things", "skills": []}"#,
        );
        let file = AgentCardFile::load(&path).unwrap();
        assert_eq!(file.name, "My Agent");
        assert!(file.skills.is_empty());
    }

    #[test]
    fn rejects_empty_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_card(
            dir.path(),
            r#"{"name": "", "description": "d", "skills": []}"#,
        );
        assert!(matches!(
            AgentCardFile::load(&path),
            Err(CardError::Invalid { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_skill_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_card(
            dir.path(),
            r#"{"name": "A", "description": "d", "skills": [
                {"id": "x", "name": "X", "description": "d", "tags": []},
                {"id": "x", "name": "X2", "description": "d", "tags": []}
            ]}"#,
        );
        let err = AgentCardFile::load(&path).unwrap_err();
        assert!(err.to_string().contains("duplicate skill id"));
    }

    #[test]
    fn rejects_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_card(dir.path(), "not json");
        assert!(matches!(
            AgentCardFile::load(&path),
            Err(CardError::Parse { .. })
        ));
    }

    #[test]
    fn missing_file_is_a_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = AgentCardFile::load(&dir.path().join("nope.json")).unwrap_err();
        assert!(matches!(err, CardError::Read { .. }));
    }

    #[test]
    fn card_state_load_or_default_falls_back_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = CardState::load_or_default(&dir.path().join("nope.json"), &runtime());
        assert!(!state.current().name.trim().is_empty());
        assert!(state.current().skills.is_empty());
    }

    #[test]
    fn card_runtime_from_config_prefers_public_url() {
        let a2a = crate::config::A2aConfig {
            enabled: true,
            port: 7702,
            public_url: Some("https://example.com/a2a/laptop".to_string()),
            visibility: A2aVisibility::Private,
        };
        let runtime = CardRuntime::from_config(&a2a, "127.0.0.1");
        assert_eq!(
            runtime.interfaces_base_url,
            "https://example.com/a2a/laptop"
        );
        assert_eq!(runtime.visibility, A2aVisibility::Private);
    }

    #[test]
    fn card_runtime_from_config_falls_back_to_local_bind() {
        let a2a = crate::config::A2aConfig {
            enabled: true,
            port: 7702,
            public_url: None,
            visibility: A2aVisibility::Public,
        };
        let runtime = CardRuntime::from_config(&a2a, "127.0.0.1");
        assert_eq!(runtime.interfaces_base_url, "http://127.0.0.1:7702");
    }

    #[test]
    fn build_agent_card_fills_interfaces_and_bearer_security() {
        let file = AgentCardFile {
            name: "My Agent".to_string(),
            description: "does things".to_string(),
            skills: vec![],
            default_input_modes: None,
            default_output_modes: None,
        };
        let card = build_agent_card(&file, &runtime());

        assert_eq!(card.name, "My Agent");
        assert_eq!(card.capabilities.streaming, Some(true));
        assert_eq!(card.capabilities.push_notifications, Some(false));
        let [jsonrpc, rest] = card.supported_interfaces.as_slice() else {
            panic!(
                "expected exactly two interfaces, got {:?}",
                card.supported_interfaces
            );
        };
        assert_eq!(jsonrpc.url, "http://127.0.0.1:7702");
        assert_eq!(jsonrpc.protocol_binding, a2a::TRANSPORT_PROTOCOL_JSONRPC);
        assert_eq!(rest.url, "http://127.0.0.1:7702/rest");
        assert_eq!(rest.protocol_binding, a2a::TRANSPORT_PROTOCOL_HTTP_JSON);
        assert!(card.security_schemes.unwrap().contains_key("bearer"));
        assert_eq!(card.security_requirements.unwrap().len(), 1);
    }

    #[test]
    fn card_state_reload_keeps_last_good_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_card(
            dir.path(),
            r#"{"name": "Good", "description": "d", "skills": []}"#,
        );
        let state = CardState::load(&path, &runtime()).unwrap();
        assert_eq!(state.current().name, "Good");

        std::fs::write(&path, "not json").unwrap();
        let err = state.reload(&path, &runtime()).unwrap_err();
        assert!(matches!(err, CardError::Parse { .. }));
        assert_eq!(
            state.current().name,
            "Good",
            "a failed reload must keep serving the last good card"
        );
    }

    #[test]
    fn card_state_reload_picks_up_a_valid_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_card(
            dir.path(),
            r#"{"name": "First", "description": "d", "skills": []}"#,
        );
        let state = CardState::load(&path, &runtime()).unwrap();
        write_card(
            dir.path(),
            r#"{"name": "Second", "description": "d", "skills": []}"#,
        );
        state.reload(&path, &runtime()).unwrap();
        assert_eq!(state.current().name, "Second");
    }

    #[test]
    fn card_state_implements_agent_card_producer() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_card(
            dir.path(),
            r#"{"name": "Producer", "description": "d", "skills": []}"#,
        );
        let state = CardState::load(&path, &runtime()).unwrap();
        let producer: &dyn a2a_server::AgentCardProducer = &*state;
        assert_eq!(producer.card().name, "Producer");
    }
}
