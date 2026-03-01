//! Google A2A (Agent-to-Agent) protocol types.
//!
//! Implements the core data structures from the A2A specification:
//! Agent Card, JSON-RPC 2.0 messages, Tasks, Messages, Parts, and Artifacts.
//! See <https://google.github.io/A2A/specification/> for the full specification.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Agent Card ──────────────────────────────────────────────────────────

/// Agent Card metadata served at `/.well-known/agent.json`.
///
/// Describes the agent's capabilities, skills, and how to communicate with it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// Human-readable agent name.
    pub name: String,
    /// Description of what this agent does.
    pub description: String,
    /// The A2A endpoint URL for this agent.
    pub url: String,
    /// Protocol version supported.
    pub version: String,
    /// Capabilities this agent supports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<AgentCapabilities>,
    /// Skills this agent can perform.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<AgentSkill>,
    /// Default input content types accepted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_input_modes: Vec<String>,
    /// Default output content types produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_output_modes: Vec<String>,
    /// Authentication requirements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<AgentAuthentication>,
}

/// Capabilities flags for an A2A agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// Whether the agent supports SSE streaming via `tasks/sendSubscribe`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub streaming: bool,
    /// Whether the agent supports push notification webhooks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub push_notifications: bool,
    /// Whether the agent tracks state transition history on tasks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub state_transition_history: bool,
}

/// A skill the agent can perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    /// Unique skill identifier.
    pub id: String,
    /// Human-readable skill name.
    pub name: String,
    /// Description of what the skill does.
    pub description: String,
    /// Tags for categorization and discovery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Example prompts that invoke this skill.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// Input content types this skill accepts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_modes: Vec<String>,
    /// Output content types this skill produces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_modes: Vec<String>,
}

/// Authentication requirements for the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAuthentication {
    /// Authentication schemes supported (aligned with OpenAPI security schemes).
    pub schemes: Vec<AuthScheme>,
}

/// A single authentication scheme.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthScheme {
    /// Scheme type: `"http"`, `"apiKey"`, `"oauth2"`, `"openIdConnect"`.
    #[serde(rename = "type")]
    pub scheme_type: String,
    /// Scheme name (e.g. `"bearer"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

// ── JSON-RPC 2.0 ────────────────────────────────────────────────────────

/// A JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version (must be `"2.0"`).
    pub jsonrpc: String,
    /// Request identifier.
    pub id: JsonRpcId,
    /// Method name (e.g. `"tasks/send"`).
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC request/response identifier (string or number).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// String identifier.
    String(String),
    /// Numeric identifier.
    Number(i64),
}

/// A JSON-RPC 2.0 success response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    /// Protocol version.
    pub jsonrpc: &'static str,
    /// Request identifier (echoed from request).
    pub id: JsonRpcId,
    /// Result payload.
    pub result: serde_json::Value,
}

impl JsonRpcResponse {
    /// Create a success response.
    #[must_use]
    pub fn success(id: JsonRpcId, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result,
        }
    }
}

/// A JSON-RPC 2.0 error response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcErrorResponse {
    /// Protocol version.
    pub jsonrpc: &'static str,
    /// Request identifier (echoed from request).
    pub id: JsonRpcId,
    /// Error details.
    pub error: JsonRpcError,
}

impl JsonRpcErrorResponse {
    /// Create an error response.
    #[must_use]
    pub fn new(id: JsonRpcId, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            error: JsonRpcError {
                code,
                message: message.into(),
                data: None,
            },
        }
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    /// Numeric error code.
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// Standard JSON-RPC error codes
/// Parse error: invalid JSON.
pub const PARSE_ERROR: i32 = -32_700;
/// Invalid request: not a valid JSON-RPC request.
pub const INVALID_REQUEST: i32 = -32_600;
/// Method not found.
pub const METHOD_NOT_FOUND: i32 = -32_601;
/// Invalid params.
pub const INVALID_PARAMS: i32 = -32_602;
/// Internal error.
pub const INTERNAL_ERROR: i32 = -32_603;

// A2A-specific error codes
/// Task not found.
pub const TASK_NOT_FOUND: i32 = -32_001;
/// Task cannot be cancelled (already completed or failed).
pub const TASK_NOT_CANCELABLE: i32 = -32_002;

// ── Task ────────────────────────────────────────────────────────────────

/// An A2A task — the fundamental unit of work.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Unique task identifier.
    pub id: String,
    /// Session identifier grouping related tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Current task status.
    pub status: TaskStatus,
    /// Conversation history for this task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<A2aMessage>,
    /// Output artifacts produced by the task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    /// Arbitrary metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Task status with state and optional message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    /// Current state of the task.
    pub state: TaskState,
    /// Optional status message from the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<A2aMessage>,
    /// Timestamp of the status change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// Possible states a task can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    /// Task has been submitted but not yet started.
    Submitted,
    /// Task is actively being processed.
    Working,
    /// Task needs additional input from the caller.
    InputRequired,
    /// Task completed successfully.
    Completed,
    /// Task failed.
    Failed,
    /// Task was cancelled.
    Canceled,
}

// ── Message ─────────────────────────────────────────────────────────────

/// An A2A message in the conversation between agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2aMessage {
    /// Role: `"user"` (requesting agent) or `"agent"` (responding agent).
    pub role: A2aRole,
    /// Content parts of the message.
    pub parts: Vec<Part>,
    /// Arbitrary metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Role of a message sender in A2A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum A2aRole {
    /// The requesting/client agent.
    User,
    /// The responding/server agent.
    Agent,
}

// ── Parts ───────────────────────────────────────────────────────────────

/// Content part — the atomic content unit in A2A messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Part {
    /// Plain text content.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },
    /// File content (inline or by reference).
    #[serde(rename = "file")]
    File {
        /// File metadata.
        file: FileContent,
    },
    /// Structured JSON data.
    #[serde(rename = "data")]
    Data {
        /// The structured data.
        data: serde_json::Value,
    },
}

/// File content — either inline bytes or a URI reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    /// MIME type of the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// File name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Base64-encoded file bytes (mutually exclusive with `uri`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<String>,
    /// URI reference to the file (mutually exclusive with `bytes`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

// ── Artifact ────────────────────────────────────────────────────────────

/// An output artifact produced by a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// Artifact name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Description of the artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Content parts.
    pub parts: Vec<Part>,
    /// Index for ordering multiple artifacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
}

// ── Request Params ──────────────────────────────────────────────────────

/// Parameters for `tasks/send` and `tasks/sendSubscribe`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSendParams {
    /// Task identifier (client-generated).
    pub id: String,
    /// Session identifier for grouping related tasks.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The message to send.
    pub message: A2aMessage,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Parameters for `tasks/get`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGetParams {
    /// Task identifier to retrieve.
    pub id: String,
    /// Number of history messages to include (None = all).
    #[serde(default)]
    pub history_length: Option<usize>,
}

/// Parameters for `tasks/cancel`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCancelParams {
    /// Task identifier to cancel.
    pub id: String,
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn agent_card_serializes_correctly() {
        let card = AgentCard {
            name: "Residuum".to_string(),
            description: "A personal AI agent".to_string(),
            url: "http://localhost:7700/a2a".to_string(),
            version: "0.2".to_string(),
            capabilities: Some(AgentCapabilities {
                streaming: false,
                push_notifications: false,
                state_transition_history: false,
            }),
            skills: vec![AgentSkill {
                id: "general".to_string(),
                name: "General Assistant".to_string(),
                description: "General-purpose AI assistant".to_string(),
                tags: vec!["general".to_string()],
                examples: vec!["Help me with a task".to_string()],
                input_modes: vec!["text/plain".to_string()],
                output_modes: vec!["text/plain".to_string()],
            }],
            default_input_modes: vec!["text/plain".to_string()],
            default_output_modes: vec!["text/plain".to_string()],
            authentication: None,
        };

        let json = serde_json::to_value(&card).unwrap();
        assert_eq!(
            json.get("name").and_then(|v| v.as_str()),
            Some("Residuum"),
            "agent name should serialize"
        );
        assert_eq!(
            json.get("version").and_then(|v| v.as_str()),
            Some("0.2"),
            "version should serialize"
        );
        assert_eq!(
            json.get("skills").and_then(|v| v.as_array()).map(Vec::len),
            Some(1),
            "skills should serialize"
        );
    }

    #[test]
    fn agent_card_with_auth_serializes() {
        let card = AgentCard {
            name: "Test Agent".to_string(),
            description: "test".to_string(),
            url: "http://localhost/a2a".to_string(),
            version: "0.2".to_string(),
            capabilities: None,
            skills: vec![],
            default_input_modes: vec![],
            default_output_modes: vec![],
            authentication: Some(AgentAuthentication {
                schemes: vec![AuthScheme {
                    scheme_type: "http".to_string(),
                    scheme: Some("bearer".to_string()),
                }],
            }),
        };

        let json = serde_json::to_string(&card).unwrap();
        assert!(
            json.contains("bearer"),
            "auth scheme should appear in serialized output"
        );
    }

    #[test]
    fn json_rpc_request_deserializes() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/send",
            "params": {
                "id": "task-1",
                "message": {
                    "role": "user",
                    "parts": [{"type": "text", "text": "hello"}]
                }
            }
        }"#;

        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tasks/send", "method should match");
        assert!(
            matches!(req.id, JsonRpcId::String(ref s) if s == "req-1"),
            "id should be string"
        );
    }

    #[test]
    fn json_rpc_request_with_numeric_id() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tasks/get",
            "params": {"id": "task-1"}
        }"#;

        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert!(
            matches!(req.id, JsonRpcId::Number(42)),
            "id should be numeric"
        );
    }

    #[test]
    fn json_rpc_response_serializes() {
        let resp = JsonRpcResponse::success(
            JsonRpcId::String("req-1".to_string()),
            serde_json::json!({"status": "ok"}),
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            json.get("jsonrpc").and_then(|v| v.as_str()),
            Some("2.0"),
            "jsonrpc should be 2.0"
        );
        assert_eq!(
            json.get("id").and_then(|v| v.as_str()),
            Some("req-1"),
            "id should echo request"
        );
    }

    #[test]
    fn json_rpc_error_response_serializes() {
        let resp = JsonRpcErrorResponse::new(
            JsonRpcId::Number(1),
            METHOD_NOT_FOUND,
            "method not found",
        );
        let json = serde_json::to_value(&resp).unwrap();
        let error = json.get("error").unwrap();
        assert_eq!(
            error.get("code").and_then(|v| v.as_i64()),
            Some(i64::from(METHOD_NOT_FOUND)),
            "error code should match"
        );
    }

    #[test]
    fn task_state_serializes_kebab_case() {
        let state = TaskState::InputRequired;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"input-required\"", "should use kebab-case");
    }

    #[test]
    fn task_state_roundtrips() {
        for state in [
            TaskState::Submitted,
            TaskState::Working,
            TaskState::InputRequired,
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Canceled,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: TaskState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state, "state should roundtrip through JSON");
        }
    }

    #[test]
    fn text_part_serializes_with_type_tag() {
        let part = Part::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(
            json.get("type").and_then(|v| v.as_str()),
            Some("text"),
            "type tag should be 'text'"
        );
        assert_eq!(
            json.get("text").and_then(|v| v.as_str()),
            Some("hello"),
            "text content should match"
        );
    }

    #[test]
    fn data_part_serializes_with_type_tag() {
        let part = Part::Data {
            data: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(
            json.get("type").and_then(|v| v.as_str()),
            Some("data"),
            "type tag should be 'data'"
        );
    }

    #[test]
    fn task_send_params_deserializes() {
        let json = r#"{
            "id": "task-abc",
            "sessionId": "session-1",
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "do something"}]
            }
        }"#;

        let params: TaskSendParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.id, "task-abc", "task id should match");
        assert_eq!(
            params.session_id.as_deref(),
            Some("session-1"),
            "session id should match"
        );
        assert_eq!(params.message.parts.len(), 1, "should have one part");
    }

    #[test]
    fn artifact_serializes() {
        let artifact = Artifact {
            name: Some("result".to_string()),
            description: Some("The agent's response".to_string()),
            parts: vec![Part::Text {
                text: "Hello from the agent".to_string(),
            }],
            index: Some(0),
        };

        let json = serde_json::to_value(&artifact).unwrap();
        assert_eq!(
            json.get("name").and_then(|v| v.as_str()),
            Some("result"),
            "artifact name should serialize"
        );
        assert_eq!(
            json.get("parts").and_then(|v| v.as_array()).map(Vec::len),
            Some(1),
            "parts should serialize"
        );
    }

    #[test]
    fn a2a_message_with_metadata() {
        let msg = A2aMessage {
            role: A2aRole::User,
            parts: vec![Part::Text {
                text: "test".to_string(),
            }],
            metadata: HashMap::from([(
                "source".to_string(),
                serde_json::json!("test-client"),
            )]),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert!(
            json.get("metadata").is_some(),
            "metadata should be present when non-empty"
        );
    }

    #[test]
    fn a2a_message_without_metadata_omits_field() {
        let msg = A2aMessage {
            role: A2aRole::Agent,
            parts: vec![],
            metadata: HashMap::new(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert!(
            json.get("metadata").is_none(),
            "metadata should be omitted when empty"
        );
    }
}
