//! Observer: compresses recent messages into structured episodes via LLM.
//!
//! Fires synchronously after the agent completes a turn when the accumulated
//! recent message token count exceeds the configured threshold.

use chrono::Local;

use crate::error::IronclawError;
use crate::memory::episode_store::write_episode_transcript;
use crate::memory::log_store::{append_episode, load_observation_log, next_episode_id};
use crate::memory::tokens::estimate_message_tokens;
use crate::memory::types::Episode;
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse, Role};
use crate::workspace::layout::WorkspaceLayout;

/// Default token threshold before the observer fires.
const DEFAULT_THRESHOLD_TOKENS: usize = 30_000;

/// Observer configuration.
#[derive(Debug, Clone)]
pub struct ObserverConfig {
    /// Minimum estimated tokens in recent messages before observation triggers.
    pub threshold_tokens: usize,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            threshold_tokens: DEFAULT_THRESHOLD_TOKENS,
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
    pub fn should_observe(&self, recent_messages: &[Message]) -> bool {
        let tokens = estimate_message_tokens(recent_messages);
        tokens >= self.config.threshold_tokens
    }

    /// Extract an episode from recent messages and persist it.
    ///
    /// The caller is responsible for clearing the recent messages file
    /// after this succeeds.
    ///
    /// # Errors
    /// Returns an error if the LLM call fails or file persistence fails.
    pub async fn observe(
        &self,
        recent_messages: &[Message],
        layout: &WorkspaceLayout,
    ) -> Result<Episode, IronclawError> {
        if recent_messages.is_empty() {
            return Err(IronclawError::Memory(
                "no recent messages to extract from".to_string(),
            ));
        }

        // Load existing log for ID generation
        let log = load_observation_log(&layout.observations_json()).await?;
        let episode_id = next_episode_id(&log);

        // Build extraction prompt
        let extraction_messages = build_extraction_prompt(recent_messages);

        // Call the model
        let response = self
            .provider
            .complete(&extraction_messages, &[], &CompletionOptions::default())
            .await
            .map_err(IronclawError::Model)?;

        // Parse the response into an episode
        let episode = parse_episode_response(&response, &episode_id)?;

        // Persist transcript
        write_episode_transcript(&layout.episodes_dir(), &episode, recent_messages).await?;

        // Append to observation log
        append_episode(&layout.observations_json(), episode.clone()).await?;

        tracing::info!(
            episode_id = %episode.id,
            observations = episode.observations.len(),
            "episode extracted"
        );

        Ok(episode)
    }
}

/// Build the extraction prompt for the observer model.
fn build_extraction_prompt(messages: &[Message]) -> Vec<Message> {
    let transcript = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            format!("[{role}]: {}", m.content)
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let system = Message {
        role: Role::System,
        content: EXTRACTION_SYSTEM_PROMPT.to_string(),
        tool_calls: None,
        tool_call_id: None,
    };

    let user = Message {
        role: Role::User,
        content: format!("Extract observations from this conversation segment:\n\n{transcript}"),
        tool_calls: None,
        tool_call_id: None,
    };

    vec![system, user]
}

/// System prompt instructing the model to extract structured episode data.
const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are a memory extraction system. Given a conversation segment, extract key information into a structured JSON object.

Return ONLY a JSON object with these fields:
- "start": one-line summary of how the conversation segment started
- "end": one-line summary of how the segment ended
- "context": the project or topic being discussed (e.g. "ironclaw/memory", "devops/k8s", "general")
- "observations": array of concise single-sentence observations

For observations, extract:
- Key decisions made and their rationale
- Problems encountered and their solutions
- Corrections or mistakes that were fixed
- Important technical details or patterns discovered
- Action items or next steps identified

Each observation should be a complete, self-contained sentence that would be useful context in a future session. Be concise but specific.

Return ONLY valid JSON, no markdown fencing, no explanation."#;

/// Parse the model's JSON response into an `Episode`.
///
/// # Errors
/// Returns an error if the response cannot be parsed as the expected JSON structure.
fn parse_episode_response(
    response: &ModelResponse,
    episode_id: &str,
) -> Result<Episode, IronclawError> {
    let content = response.content.trim();

    // Strip markdown code fences if present
    let json_str = content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .map_or(content, str::trim);

    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse observer response as JSON: {e}\nresponse: {content}"
        ))
    })?;

    let start = value
        .get("start")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let end = value
        .get("end")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let topic = value
        .get("context")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("general")
        .to_string();

    let observations: Vec<String> = value
        .get("observations")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    Ok(Episode {
        id: episode_id.to_string(),
        date: Local::now().date_naive(),
        start,
        end,
        context: topic,
        observations,
        source_episodes: vec![],
    })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::models::{ModelError, ModelResponse, ToolDefinition};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    const SAMPLE_RESPONSE: &str = r#"{
        "start": "user asked about file structure",
        "end": "listed directory contents successfully",
        "context": "ironclaw/workspace",
        "observations": [
            "workspace uses a flat directory layout",
            "identity files are loaded at startup"
        ]
    }"#;

    fn make_messages(count: usize) -> Vec<Message> {
        (0..count)
            .map(|i| Message {
                role: Role::User,
                content: format!(
                    "message {i} with enough content to contribute to token count - {}",
                    "a".repeat(100)
                ),
                tool_calls: None,
                tool_call_id: None,
            })
            .collect()
    }

    #[test]
    fn parse_episode_from_json() {
        let response = ModelResponse::new(SAMPLE_RESPONSE.to_string(), vec![]);
        let episode = parse_episode_response(&response, "ep-001").unwrap();

        assert_eq!(episode.id, "ep-001", "ID should match");
        assert_eq!(
            episode.start, "user asked about file structure",
            "start should match"
        );
        assert_eq!(episode.observations.len(), 2, "should have 2 observations");
        assert_eq!(
            episode.context, "ironclaw/workspace",
            "context should match"
        );
    }

    #[test]
    fn parse_episode_with_code_fences() {
        let fenced = format!("```json\n{SAMPLE_RESPONSE}\n```");
        let response = ModelResponse::new(fenced, vec![]);
        let episode = parse_episode_response(&response, "ep-002").unwrap();

        assert_eq!(episode.id, "ep-002", "should parse despite fences");
        assert_eq!(episode.observations.len(), 2, "should extract observations");
    }

    #[test]
    fn parse_episode_missing_fields_uses_defaults() {
        let minimal = r#"{"observations": ["one thing"]}"#;
        let response = ModelResponse::new(minimal.to_string(), vec![]);
        let episode = parse_episode_response(&response, "ep-003").unwrap();

        assert_eq!(
            episode.start, "unknown",
            "missing start defaults to unknown"
        );
        assert_eq!(
            episode.context, "general",
            "missing context defaults to general"
        );
        assert_eq!(episode.observations.len(), 1, "should have one observation");
    }

    #[test]
    fn parse_episode_invalid_json_errors() {
        let response = ModelResponse::new("not json at all".to_string(), vec![]);
        let result = parse_episode_response(&response, "ep-004");
        assert!(result.is_err(), "invalid JSON should error");
    }

    #[test]
    fn should_observe_below_threshold() {
        let observer = Observer::new(
            Box::new(MockObserverProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 1000,
            },
        );
        let messages = make_messages(2);

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
        let messages = make_messages(5);

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

        let messages = make_messages(5);
        let episode = observer.observe(&messages, &layout).await.unwrap();

        assert_eq!(episode.id, "ep-001", "first episode should be ep-001");

        let transcript_path =
            crate::memory::episode_store::episode_path(&layout.episodes_dir(), &episode);
        assert!(transcript_path.exists(), "transcript file should exist");

        let log = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(log.len(), 1, "observation log should have one episode");
    }

    #[test]
    fn extraction_prompt_includes_messages() {
        let messages = vec![Message {
            role: Role::User,
            content: "test content".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];

        let prompt = build_extraction_prompt(&messages);
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
    }
}
