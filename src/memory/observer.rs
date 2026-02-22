//! Observer: compresses recent messages into structured episodes via LLM.
//!
//! Fires synchronously after the agent completes a turn when the accumulated
//! recent message token count exceeds the configured threshold.

use chrono::{Local, SecondsFormat, Utc};

use crate::config::DEFAULT_OBSERVER_THRESHOLD;
use crate::error::IronclawError;
use crate::memory::episode_store::{episode_obs_path, write_episode_transcript};
use crate::memory::log_store::{append_observations, next_episode_id, save_episode_observations};
use crate::memory::recent_store::RecentMessage;
use crate::memory::tokens::estimate_message_tokens;
use crate::memory::types::{Episode, Observation, Visibility};
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse};
use crate::workspace::layout::WorkspaceLayout;

/// The result of a successful observation run.
pub struct ObserveResult {
    /// The episode identifier (e.g., `"ep-001"`).
    pub id: String,
    /// Path to the transcript file on disk.
    pub transcript_path: std::path::PathBuf,
    /// Number of observation strings extracted from the conversation.
    pub observation_count: usize,
}

/// Observer configuration.
#[derive(Debug, Clone)]
pub struct ObserverConfig {
    /// Minimum estimated tokens in recent messages before observation triggers.
    pub threshold_tokens: usize,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            threshold_tokens: DEFAULT_OBSERVER_THRESHOLD,
        }
    }
}

/// The observer extracts structured episodes from recent messages.
pub struct Observer {
    provider: Box<dyn ModelProvider>,
    config: ObserverConfig,
}

impl Observer {
    /// Create a new observer with the given provider and config.
    #[must_use]
    pub fn new(provider: Box<dyn ModelProvider>, config: ObserverConfig) -> Self {
        Self { provider, config }
    }

    /// Check whether the observer should fire based on recent message token count.
    #[must_use]
    pub fn should_observe(&self, recent_messages: &[RecentMessage]) -> bool {
        let messages: Vec<&Message> = recent_messages.iter().map(|rm| &rm.message).collect();
        let tokens =
            estimate_message_tokens(&messages.iter().map(|m| (*m).clone()).collect::<Vec<_>>());
        tokens >= self.config.threshold_tokens
    }

    /// Extract observations from recent messages and persist them.
    ///
    /// The caller is responsible for clearing the recent messages file
    /// after this succeeds.
    ///
    /// # Errors
    /// Returns an error if the LLM call fails or file persistence fails.
    pub async fn observe(
        &self,
        recent_messages: &[RecentMessage],
        layout: &WorkspaceLayout,
    ) -> Result<ObserveResult, IronclawError> {
        if recent_messages.is_empty() {
            return Err(IronclawError::Memory(
                "no recent messages to extract from".to_string(),
            ));
        }

        // Derive observation metadata from the batch of recent messages.
        let project_context = derive_project_context(recent_messages);
        let visibility = derive_visibility(recent_messages);

        // Generate the next episode ID by scanning the episodes directory
        let episode_id = next_episode_id(&layout.episodes_dir()).await?;

        // Load system prompt from disk, falling back to embedded constant.
        let system_prompt = tokio::fs::read_to_string(layout.observer_md())
            .await
            .ok()
            .and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
            .unwrap_or_else(|| EXTRACTION_SYSTEM_PROMPT.to_string());

        // Build extraction prompt using full RecentMessage metadata (timestamps,
        // tool calls, project context) so the observer LLM has complete context.
        let extraction_messages = build_extraction_prompt(recent_messages, &system_prompt);

        // Extract inner messages for the episode transcript written to disk.
        let messages: Vec<Message> = recent_messages
            .iter()
            .map(|rm| rm.message.clone())
            .collect();

        // Call the model
        let response = self
            .provider
            .complete(&extraction_messages, &[], &CompletionOptions::default())
            .await
            .map_err(IronclawError::Model)?;

        // Parse the bare string-array response into observation strings.
        let observation_strings = parse_observation_strings(&response)?;

        // Build the episode internally — start/end are cosmetic and no longer LLM-extracted.
        let episode = Episode {
            id: episode_id.clone(),
            date: Local::now().date_naive(),
            start: String::new(),
            end: String::new(),
            context: project_context.clone(),
            observations: observation_strings,
            source_episodes: vec![],
        };

        // Persist transcript
        let transcript_path =
            crate::memory::episode_store::episode_jsonl_path(&layout.episodes_dir(), &episode);
        write_episode_transcript(&layout.episodes_dir(), &episode, &messages).await?;

        // Convert episode observations → flat Observations and append
        let observation_count = episode.observations.len();
        let observations: Vec<Observation> = episode
            .observations
            .iter()
            .map(|content| Observation {
                timestamp: Utc::now(),
                project_context: project_context.clone(),
                source_episodes: vec![episode.id.clone()],
                visibility: visibility.clone(),
                content: content.clone(),
            })
            .collect();

        let obs_path = episode_obs_path(&layout.episodes_dir(), &episode);
        save_episode_observations(&obs_path, &observations).await?;
        append_observations(&layout.observations_json(), observations).await?;

        tracing::info!(
            episode_id = %episode.id,
            observations = observation_count,
            "episode extracted"
        );

        Ok(ObserveResult {
            id: episode.id,
            transcript_path,
            observation_count,
        })
    }
}

/// Derive the project context from a batch of recent messages.
///
/// Uses the most common `project_context` across the batch, falling back to
/// the first non-empty one, or `"general"` if all are empty.
fn derive_project_context(messages: &[RecentMessage]) -> String {
    use std::collections::HashMap;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for msg in messages {
        if !msg.project_context.is_empty() {
            *counts.entry(msg.project_context.as_str()).or_insert(0) += 1;
        }
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map_or_else(|| "general".to_string(), |(ctx, _)| ctx.to_string())
}

/// Derive visibility from a batch of recent messages.
///
/// If any message has `Visibility::User`, the batch is considered user-visible.
fn derive_visibility(messages: &[RecentMessage]) -> Visibility {
    if messages.iter().any(|m| m.visibility == Visibility::User) {
        Visibility::User
    } else {
        Visibility::Background
    }
}

/// Format a single `RecentMessage` for the extraction prompt transcript.
///
/// Includes timestamp, role, project context, visibility, content, and any
/// tool calls or tool call IDs, so the observer LLM has full context.
fn format_recent_message(rm: &RecentMessage) -> String {
    let role = rm.message.role.as_str();
    let timestamp = rm.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);
    let visibility = match rm.visibility {
        Visibility::User => "user",
        Visibility::Background => "background",
    };
    let tool_call_id_part = rm
        .message
        .tool_call_id
        .as_deref()
        .map_or_else(String::new, |id| format!(" (call: {id})"));

    let header = format!(
        "[{timestamp}] [{role}]{tool_call_id_part} (project: {}, visibility: {visibility}):",
        rm.project_context
    );

    let mut parts = vec![header];

    if !rm.message.content.is_empty() {
        parts.push(rm.message.content.clone());
    }

    if let Some(tool_calls) = &rm.message.tool_calls {
        let mut tc_lines = vec!["  tool_calls:".to_string()];
        for tc in tool_calls {
            tc_lines.push(format!(
                "    - {}({}) [id: {}]",
                tc.name, tc.arguments, tc.id
            ));
        }
        parts.push(tc_lines.join("\n"));
    }

    parts.join("\n")
}

/// Build the extraction prompt for the observer model.
fn build_extraction_prompt(recent_messages: &[RecentMessage], system_prompt: &str) -> Vec<Message> {
    let transcript = recent_messages
        .iter()
        .map(format_recent_message)
        .collect::<Vec<_>>()
        .join("\n\n");

    vec![
        Message::system(system_prompt),
        Message::user(format!(
            "Extract observations from this conversation segment:\n\n{transcript}"
        )),
    ]
}

/// Embedded fallback system prompt for the observer.
///
/// Used when `memory/OBSERVER.md` is absent. The workspace bootstrap writes
/// this same content to disk so users can customise it without recompiling.
const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are a memory extraction system. Given a conversation segment, extract key observations as a JSON array of strings.

Return ONLY a JSON array of concise, self-contained observation strings. Example:
["user prefers concise responses", "project uses Rust 2024 edition"]

For each observation, capture:
- Key decisions made and their rationale
- Problems encountered and their solutions
- Corrections or mistakes that were fixed
- Important technical details or patterns discovered
- Action items or next steps identified

Each string should be a complete sentence useful as context in a future session. Be specific and concise.

Return ONLY a valid JSON array of strings, no markdown fencing, no explanation."#;

/// Parse the model's JSON response into a list of observation strings.
///
/// Expects a bare JSON array: `["obs 1", "obs 2", ...]`
///
/// # Errors
/// Returns an error if the response cannot be parsed or the array is empty.
fn parse_observation_strings(response: &ModelResponse) -> Result<Vec<String>, IronclawError> {
    let content = response.content.trim();
    let json_str = crate::memory::strip_code_fences(content);

    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse observer response as JSON: {e}\nresponse: {content}"
        ))
    })?;

    let observations: Vec<String> = value
        .as_array()
        .ok_or_else(|| {
            IronclawError::Memory(format!(
                "observer response is not a JSON array\nresponse: {content}"
            ))
        })?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(String::from)
        .collect();

    if observations.is_empty() {
        return Err(IronclawError::Memory(
            "observer returned empty observations array".to_string(),
        ));
    }

    Ok(observations)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::episode_store::episode_obs_path;
    use crate::memory::log_store::load_observation_log;
    use crate::models::{ModelError, ModelResponse, Role, ToolDefinition};
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// See `MockProvider` in `agent::tests` for duplication rationale.
    struct MockObserverProvider {
        response_json: String,
        call_count: Arc<AtomicUsize>,
    }

    impl MockObserverProvider {
        fn new(response_json: &str) -> Self {
            Self {
                response_json: response_json.to_string(),
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockObserverProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ModelResponse::new(self.response_json.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-observer"
        }
    }

    const SAMPLE_RESPONSE: &str =
        r#"["workspace uses a flat directory layout", "identity files are loaded at startup"]"#;

    fn make_recent_messages(count: usize) -> Vec<RecentMessage> {
        (0..count)
            .map(|i| RecentMessage {
                message: Message::user(format!(
                    "message {i} with enough content to contribute to token count - {}",
                    "a".repeat(100)
                )),
                timestamp: Utc::now(),
                project_context: "ironclaw/workspace".to_string(),
                visibility: Visibility::User,
            })
            .collect()
    }

    #[test]
    fn parse_observation_strings_from_json_array() {
        let response = ModelResponse::new(SAMPLE_RESPONSE.to_string(), vec![]);
        let observations = parse_observation_strings(&response).unwrap();

        assert_eq!(observations.len(), 2, "should have 2 observations");
        assert_eq!(
            observations.first().map(String::as_str),
            Some("workspace uses a flat directory layout"),
            "first observation should match"
        );
        assert_eq!(
            observations.get(1).map(String::as_str),
            Some("identity files are loaded at startup"),
            "second observation should match"
        );
    }

    #[test]
    fn parse_observation_strings_with_code_fences() {
        let fenced = format!("```json\n{SAMPLE_RESPONSE}\n```");
        let response = ModelResponse::new(fenced, vec![]);
        let observations = parse_observation_strings(&response).unwrap();

        assert_eq!(observations.len(), 2, "should parse despite fences");
    }

    #[test]
    fn parse_observation_strings_invalid_json_errors() {
        let response = ModelResponse::new("not json at all".to_string(), vec![]);
        let result = parse_observation_strings(&response);
        assert!(result.is_err(), "invalid JSON should error");
    }

    #[test]
    fn parse_observation_strings_not_array_errors() {
        let response = ModelResponse::new(r#"{"observations": ["one thing"]}"#.to_string(), vec![]);
        let result = parse_observation_strings(&response);
        assert!(result.is_err(), "non-array JSON should error");
    }

    #[test]
    fn parse_observation_strings_empty_array_errors() {
        let response = ModelResponse::new("[]".to_string(), vec![]);
        let result = parse_observation_strings(&response);
        assert!(result.is_err(), "empty array should error");
    }

    #[test]
    fn should_observe_below_threshold() {
        let observer = Observer::new(
            Box::new(MockObserverProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 1000,
            },
        );
        let messages = make_recent_messages(2);

        assert!(
            !observer.should_observe(&messages),
            "should not observe below threshold"
        );
    }

    #[test]
    fn should_observe_above_threshold() {
        let observer = Observer::new(
            Box::new(MockObserverProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
            },
        );
        let messages = make_recent_messages(5);

        assert!(
            observer.should_observe(&messages),
            "should observe above threshold"
        );
    }

    #[tokio::test]
    async fn observe_creates_episode() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::create_dir_all(layout.episodes_dir())
            .await
            .unwrap();
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let observer = Observer::new(
            Box::new(MockObserverProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
            },
        );

        let messages = make_recent_messages(5);
        let result = observer.observe(&messages, &layout).await.unwrap();

        assert_eq!(result.id, "ep-001", "first episode should be ep-001");
        assert_eq!(
            result.observation_count, 2,
            "SAMPLE_RESPONSE has 2 observations"
        );
        assert!(
            result.transcript_path.exists(),
            "transcript file should exist"
        );

        let log = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        // SAMPLE_RESPONSE has 2 observation strings → 2 Observations in the log
        assert_eq!(
            log.len(),
            2,
            "observation log should have two observations (one per string)"
        );

        // Verify the per-episode obs archive was written alongside the transcript
        let episode = crate::memory::types::Episode {
            id: result.id.clone(),
            date: chrono::Local::now().date_naive(),
            start: String::new(),
            end: String::new(),
            context: String::new(),
            observations: vec![],
            source_episodes: vec![],
        };
        let obs_archive = episode_obs_path(&layout.episodes_dir(), &episode);
        assert!(
            obs_archive.exists(),
            "per-episode obs archive should exist alongside transcript"
        );
    }

    #[test]
    fn extraction_prompt_includes_messages() {
        let recent_messages = vec![RecentMessage {
            message: Message::user("test content"),
            timestamp: Utc::now(),
            project_context: "test/project".to_string(),
            visibility: Visibility::User,
        }];

        let prompt = build_extraction_prompt(&recent_messages, EXTRACTION_SYSTEM_PROMPT);
        assert_eq!(prompt.len(), 2, "should have system + user message");
        assert_eq!(
            prompt.first().map(|m| m.role),
            Some(Role::System),
            "first should be system"
        );

        let user_content = prompt.get(1).map_or("", |m| m.content.as_str());
        assert!(
            user_content.contains("test content"),
            "should include message content"
        );
        assert!(
            user_content.contains("test/project"),
            "should include project context"
        );
    }

    #[test]
    fn format_recent_message_includes_tool_calls() {
        use crate::models::ToolCall;

        let rm = RecentMessage {
            message: Message::assistant(
                String::new(),
                Some(vec![ToolCall {
                    id: "call_abc".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                }]),
            ),
            timestamp: Utc::now(),
            project_context: "ironclaw/memory".to_string(),
            visibility: Visibility::User,
        };

        let formatted = format_recent_message(&rm);
        assert!(formatted.contains("read_file"), "should include tool name");
        assert!(
            formatted.contains("call_abc"),
            "should include tool call id"
        );
        assert!(
            formatted.contains("src/main.rs"),
            "should include arguments"
        );
    }

    #[test]
    fn format_recent_message_includes_tool_call_id() {
        let rm = RecentMessage {
            message: Message::tool("file contents", "call_abc"),
            timestamp: Utc::now(),
            project_context: "ironclaw/memory".to_string(),
            visibility: Visibility::User,
        };

        let formatted = format_recent_message(&rm);
        assert!(
            formatted.contains("(call: call_abc)"),
            "should include tool call id in header"
        );
    }

    #[test]
    fn format_recent_message_includes_timestamp_and_context() {
        // 2026-02-21T00:00:00Z as unix timestamp
        let timestamp = chrono::DateTime::from_timestamp(1_771_632_000, 0).unwrap();
        let rm = RecentMessage {
            message: Message::user("hello"),
            timestamp,
            project_context: "ironclaw/memory".to_string(),
            visibility: Visibility::User,
        };

        let formatted = format_recent_message(&rm);
        assert!(
            formatted.contains("2026-02-21"),
            "should include ISO date in timestamp"
        );
        assert!(
            formatted.contains("ironclaw/memory"),
            "should include project context"
        );
        assert!(
            formatted.contains("visibility: user"),
            "should include visibility"
        );
    }

    #[test]
    fn derive_project_context_most_common() {
        let messages: Vec<RecentMessage> = vec![
            RecentMessage {
                message: Message::user("a"),
                timestamp: Utc::now(),
                project_context: "ironclaw/memory".to_string(),
                visibility: Visibility::User,
            },
            RecentMessage {
                message: Message::user("b"),
                timestamp: Utc::now(),
                project_context: "ironclaw/memory".to_string(),
                visibility: Visibility::User,
            },
            RecentMessage {
                message: Message::user("c"),
                timestamp: Utc::now(),
                project_context: "devops/k8s".to_string(),
                visibility: Visibility::User,
            },
        ];
        let ctx = derive_project_context(&messages);
        assert_eq!(ctx, "ironclaw/memory", "should use most common context");
    }

    #[test]
    fn derive_visibility_user_wins() {
        let messages = vec![
            RecentMessage {
                message: Message::user("a"),
                timestamp: Utc::now(),
                project_context: String::new(),
                visibility: Visibility::Background,
            },
            RecentMessage {
                message: Message::user("b"),
                timestamp: Utc::now(),
                project_context: String::new(),
                visibility: Visibility::User,
            },
        ];
        let vis = derive_visibility(&messages);
        assert_eq!(vis, Visibility::User, "User should win over Background");
    }

    #[test]
    fn derive_visibility_all_background() {
        let messages = vec![RecentMessage {
            message: Message::user("a"),
            timestamp: Utc::now(),
            project_context: String::new(),
            visibility: Visibility::Background,
        }];
        let vis = derive_visibility(&messages);
        assert_eq!(
            vis,
            Visibility::Background,
            "all-background batch should stay Background"
        );
    }
}
