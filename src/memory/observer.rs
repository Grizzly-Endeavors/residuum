//! Observer: compresses recent messages into structured episodes via LLM.
//!
//! Fires synchronously after the agent completes a turn when the accumulated
//! recent message token count exceeds the configured threshold.

use chrono::NaiveDateTime;
use chrono_tz::Tz;

use crate::config::{
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
};
use crate::error::IronclawError;
use crate::memory::episode_store::{episode_obs_path, write_episode_transcript};
use crate::memory::log_store::{append_observations, next_episode_id, save_episode_observations};
use crate::memory::recent_store::RecentMessage;
use crate::memory::tokens::estimate_message_tokens;
use crate::memory::types::{Episode, Observation, Visibility};
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse};
use crate::time::now_local;
use crate::workspace::layout::WorkspaceLayout;

/// The result of a successful observation run.
pub struct ObserveResult {
    /// The episode identifier (e.g., `"ep-001"`).
    pub id: String,
    /// Path to the transcript file on disk.
    pub transcript_path: std::path::PathBuf,
    /// Number of observation strings extracted from the conversation.
    pub observation_count: usize,
    /// Narrative summary of the conversation at the time of observation.
    pub narrative: Option<String>,
}

/// What the observer thinks should happen after checking token thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserveAction {
    /// Token count is below the soft threshold — do nothing.
    None,
    /// Token count is at or above the soft threshold — start or reset the cooldown timer.
    StartCooldown,
    /// Token count is at or above the force threshold — observe immediately.
    ForceNow,
}

/// Observer configuration.
#[derive(Debug, Clone)]
pub struct ObserverConfig {
    /// Minimum estimated tokens in recent messages before observation triggers.
    pub threshold_tokens: usize,
    /// Cooldown period in seconds after the soft threshold is crossed.
    pub cooldown_secs: u64,
    /// Token threshold that forces immediate observation (bypasses cooldown).
    pub force_threshold_tokens: usize,
    /// Timezone used for timestamps in observations.
    pub tz: Tz,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            threshold_tokens: DEFAULT_OBSERVER_THRESHOLD,
            cooldown_secs: DEFAULT_OBSERVER_COOLDOWN_SECS,
            force_threshold_tokens: DEFAULT_OBSERVER_FORCE_THRESHOLD,
            tz: chrono_tz::UTC,
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

    /// The configured cooldown period in seconds.
    #[must_use]
    pub fn cooldown_secs(&self) -> u64 {
        self.config.cooldown_secs
    }

    /// The configured timezone.
    #[must_use]
    pub fn timezone(&self) -> Tz {
        self.config.tz
    }

    /// Check whether the observer should fire based on recent message token count.
    #[must_use]
    pub fn should_observe(&self, recent_messages: &[RecentMessage]) -> bool {
        let tokens = estimate_recent_tokens(recent_messages);
        tokens >= self.config.threshold_tokens
    }

    /// Check token thresholds and return the appropriate action.
    ///
    /// Returns `ForceNow` if tokens >= force threshold, `StartCooldown` if
    /// tokens >= soft threshold, or `None` if below both.
    #[must_use]
    pub fn check_thresholds(&self, recent_messages: &[RecentMessage]) -> ObserveAction {
        let tokens = estimate_recent_tokens(recent_messages);
        if tokens >= self.config.force_threshold_tokens {
            ObserveAction::ForceNow
        } else if tokens >= self.config.threshold_tokens {
            ObserveAction::StartCooldown
        } else {
            ObserveAction::None
        }
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

        // Derive project context from the batch of recent messages.
        let project_context = derive_project_context(recent_messages);

        // Generate the next episode ID by scanning the episodes directory
        let episode_id = next_episode_id(&layout.episodes_dir()).await?;

        // Load content guidance from disk, falling back to embedded constant.
        let content_guidance = tokio::fs::read_to_string(layout.observer_md())
            .await
            .ok()
            .and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
            .unwrap_or_else(|| EXTRACTION_CONTENT_PROMPT.to_string());

        // Build extraction prompt using full RecentMessage metadata (timestamps,
        // tool calls, project context) so the observer LLM has complete context.
        let extraction_messages = build_extraction_prompt(recent_messages, &content_guidance);

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

        // Parse the response into extraction results and optional narrative.
        let parsed = parse_observer_response(&response, self.config.tz)?;

        // Build the episode internally — start/end are cosmetic and no longer LLM-extracted.
        let episode = Episode {
            id: episode_id.clone(),
            date: now_local(self.config.tz).date(),
            start: String::new(),
            end: String::new(),
            context: project_context.clone(),
            observations: parsed
                .extractions
                .iter()
                .map(|e| e.content.clone())
                .collect(),
            source_episodes: vec![],
        };

        // Persist transcript
        let transcript_path =
            crate::memory::episode_store::episode_jsonl_path(&layout.episodes_dir(), &episode);
        write_episode_transcript(&layout.episodes_dir(), &episode, &messages).await?;

        // Convert episode observations → flat Observations and append
        let observation_count = episode.observations.len();
        let observations: Vec<Observation> = parsed
            .extractions
            .iter()
            .map(|e| Observation {
                timestamp: e.timestamp,
                project_context: project_context.clone(),
                source_episodes: vec![episode.id.clone()],
                visibility: e.visibility.clone(),
                content: e.content.clone(),
            })
            .collect();

        let obs_path = episode_obs_path(&layout.episodes_dir(), &episode);
        save_episode_observations(&obs_path, &observations).await?;
        append_observations(&layout.observations_json(), observations).await?;

        tracing::info!(
            episode_id = %episode.id,
            observations = observation_count,
            has_narrative = parsed.narrative.is_some(),
            "episode extracted"
        );

        Ok(ObserveResult {
            id: episode.id,
            transcript_path,
            observation_count,
            narrative: parsed.narrative,
        })
    }
}

/// Intermediate extraction result from the observer LLM response.
struct ObserverExtraction {
    content: String,
    timestamp: NaiveDateTime,
    visibility: Visibility,
}

/// Combined parse result: extractions plus optional narrative.
struct ObserverParseResult {
    extractions: Vec<ObserverExtraction>,
    narrative: Option<String>,
}

/// Estimate the total token count of recent messages.
fn estimate_recent_tokens(recent_messages: &[RecentMessage]) -> usize {
    let messages: Vec<Message> = recent_messages
        .iter()
        .map(|rm| rm.message.clone())
        .collect();
    estimate_message_tokens(&messages)
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

/// Format a single `RecentMessage` for the extraction prompt transcript.
///
/// Includes timestamp, role, project context, visibility, content, and any
/// tool calls or tool call IDs, so the observer LLM has full context.
fn format_recent_message(rm: &RecentMessage) -> String {
    let role = rm.message.role.as_str();
    let timestamp = rm.timestamp.format("%Y-%m-%dT%H:%M:%S").to_string();
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
///
/// Injects the format spec alongside user-customizable content guidance so the
/// format requirement cannot be lost by editing the disk file.
fn build_extraction_prompt(
    recent_messages: &[RecentMessage],
    content_guidance: &str,
) -> Vec<Message> {
    let system = format!("{content_guidance}\n\n{EXTRACTION_FORMAT_SPEC}");
    let transcript = recent_messages
        .iter()
        .map(format_recent_message)
        .collect::<Vec<_>>()
        .join("\n\n");

    vec![
        Message::system(system),
        Message::user(format!(
            "Extract observations from this conversation segment:\n\n{transcript}"
        )),
    ]
}

/// User-customizable content guidance — default when `memory/OBSERVER.md` is absent.
///
/// The workspace bootstrap writes this same content to disk so users can customise
/// it without recompiling. The format spec is always appended by code.
const EXTRACTION_CONTENT_PROMPT: &str =
    "You are a memory extraction system. Given a conversation segment, extract key observations.

For each observation, capture:
- Key decisions made and their rationale
- Problems encountered and their solutions
- Corrections or mistakes that were fixed
- Important technical details or patterns discovered
- Action items or next steps identified

Each observation should be a complete sentence useful as future context. Be specific and concise.";

/// Output format spec — always appended by code, never stored in editable files.
///
/// This is injected unconditionally so editing `OBSERVER.md` cannot break JSON parsing.
const EXTRACTION_FORMAT_SPEC: &str = r#"Return ONLY a JSON object with two fields:

1. "observations": an array of objects, each with:
   - "content": a complete, self-contained observation sentence
   - "timestamp": timestamp at minute precision (YYYY-MM-DDTHH:MM) matching the most relevant message
   - "visibility": "user" if the observation involves a user-visible turn, "background" if from a system/background turn

2. "narrative": a 2-4 sentence summary of what was being discussed and where things left off,
   written as if briefing someone who needs to continue the conversation. Include the current
   topic, any open questions, and the overall direction of the conversation.

Example:
{
  "observations": [
    {"content": "user prefers concise responses", "timestamp": "2026-02-21T14:30", "visibility": "user"},
    {"content": "cron job executed daily backup successfully", "timestamp": "2026-02-21T03:00", "visibility": "background"}
  ],
  "narrative": "We were implementing a new caching layer for the API. The user chose Redis over in-memory caching. The basic connection setup is done but we haven't started on cache invalidation yet."
}

Return ONLY a valid JSON object, no markdown fencing, no explanation."#;

/// Parse the model's JSON response into extractions and an optional narrative.
///
/// Accepts two formats:
/// - **New (object)**: `{"observations": [...], "narrative": "..."}`
/// - **Legacy (bare array)**: `[{"content": ..., ...}, ...]`
///
/// # Errors
/// Returns an error if the response cannot be parsed or the observations are empty.
fn parse_observer_response(
    response: &ModelResponse,
    tz: Tz,
) -> Result<ObserverParseResult, IronclawError> {
    let content = response.content.trim();
    let json_str = crate::memory::strip_code_fences(content);

    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse observer response as JSON: {e}\nresponse: {content}"
        ))
    })?;

    // Determine the items array and optional narrative
    let (items, narrative) = if let Some(arr) = value.as_array() {
        // Legacy bare-array format
        (arr.clone(), None)
    } else if let Some(obj) = value.as_object() {
        let obs_array = obj
            .get("observations")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                IronclawError::Memory(format!(
                    "observer response object missing 'observations' array\nresponse: {content}"
                ))
            })?
            .clone();

        let narr = obj
            .get("narrative")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);

        (obs_array, narr)
    } else {
        return Err(IronclawError::Memory(format!(
            "observer response is not a JSON array or object\nresponse: {content}"
        )));
    };

    let extractions = parse_extraction_items(&items, tz);

    if extractions.is_empty() {
        return Err(IronclawError::Memory(
            "observer returned empty observations array".to_string(),
        ));
    }

    Ok(ObserverParseResult {
        extractions,
        narrative,
    })
}

/// Parse individual observation items from a JSON array.
fn parse_extraction_items(items: &[serde_json::Value], tz: Tz) -> Vec<ObserverExtraction> {
    let mut extractions = Vec::new();

    for item in items {
        let Some(obs_content) = item.get("content").and_then(serde_json::Value::as_str) else {
            tracing::warn!("observer response item missing 'content' field, skipping");
            continue;
        };

        if obs_content.is_empty() {
            continue;
        }

        let timestamp = item
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || {
                    tracing::warn!(
                        "observer response item missing 'timestamp', using current time"
                    );
                    now_local(tz)
                },
                |ts| crate::memory::parse_minute_timestamp(ts, tz),
            );

        let visibility = item
            .get("visibility")
            .and_then(serde_json::Value::as_str)
            .map_or(Visibility::User, |v| {
                if v == "background" {
                    Visibility::Background
                } else {
                    Visibility::User
                }
            });

        extractions.push(ObserverExtraction {
            content: obs_content.to_string(),
            timestamp,
            visibility,
        });
    }

    extractions
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::episode_store::episode_obs_path;
    use crate::memory::log_store::load_observation_log;
    use crate::models::{ModelError, ModelResponse, Role, ToolDefinition};
    use async_trait::async_trait;
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

    const SAMPLE_RESPONSE: &str = r#"[
        {"content": "workspace uses a flat directory layout", "timestamp": "2026-02-21T14:30", "visibility": "user"},
        {"content": "identity files are loaded at startup", "timestamp": "2026-02-21T14:31", "visibility": "user"}
    ]"#;

    fn make_recent_messages(count: usize) -> Vec<RecentMessage> {
        (0..count)
            .map(|i| RecentMessage {
                message: Message::user(format!(
                    "message {i} with enough content to contribute to token count - {}",
                    "a".repeat(100)
                )),
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "ironclaw/workspace".to_string(),
                visibility: Visibility::User,
            })
            .collect()
    }

    #[test]
    fn parse_observer_response_legacy_array_format() {
        let response = ModelResponse::new(SAMPLE_RESPONSE.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 2, "should have 2 extractions");
        assert_eq!(
            parsed.extractions.first().map(|e| e.content.as_str()),
            Some("workspace uses a flat directory layout"),
            "first extraction content should match"
        );
        assert_eq!(
            parsed.extractions.get(1).map(|e| e.content.as_str()),
            Some("identity files are loaded at startup"),
            "second extraction content should match"
        );
        assert!(
            parsed.narrative.is_none(),
            "legacy format should have no narrative"
        );
    }

    #[test]
    fn parse_observer_response_new_format() {
        let json = r#"{
            "observations": [
                {"content": "user prefers Rust", "timestamp": "2026-02-21T14:30", "visibility": "user"}
            ],
            "narrative": "We were discussing language preferences."
        }"#;
        let response = ModelResponse::new(json.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 1, "should have 1 extraction");
        assert_eq!(
            parsed.narrative.as_deref(),
            Some("We were discussing language preferences."),
            "narrative should be extracted"
        );
    }

    #[test]
    fn parse_observer_response_narrative_missing() {
        let json = r#"{
            "observations": [
                {"content": "user prefers Rust", "timestamp": "2026-02-21T14:30", "visibility": "user"}
            ]
        }"#;
        let response = ModelResponse::new(json.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 1, "should have 1 extraction");
        assert!(
            parsed.narrative.is_none(),
            "missing narrative should be None"
        );
    }

    #[test]
    fn parse_observer_response_with_code_fences() {
        let fenced = format!("```json\n{SAMPLE_RESPONSE}\n```");
        let response = ModelResponse::new(fenced, vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 2, "should parse despite fences");
    }

    #[test]
    fn parse_observer_response_invalid_json_errors() {
        let response = ModelResponse::new("not json at all".to_string(), vec![]);
        let result = parse_observer_response(&response, chrono_tz::UTC);
        assert!(result.is_err(), "invalid JSON should error");
    }

    #[test]
    fn parse_observer_response_empty_array_errors() {
        let response = ModelResponse::new("[]".to_string(), vec![]);
        let result = parse_observer_response(&response, chrono_tz::UTC);
        assert!(result.is_err(), "empty array should error");
    }

    #[test]
    fn parse_observer_response_timestamp_minute_precision() {
        let response = ModelResponse::new(
            r#"[{"content": "test obs", "timestamp": "2026-02-21T14:30", "visibility": "user"}]"#
                .to_string(),
            vec![],
        );
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();
        let ts = parsed.extractions.first().unwrap().timestamp;
        assert_eq!(ts.format("%Y-%m-%dT%H:%M").to_string(), "2026-02-21T14:30");
    }

    #[test]
    fn parse_observer_response_background_visibility() {
        let response = ModelResponse::new(
            r#"[{"content": "cron job ran", "timestamp": "2026-02-21T03:00", "visibility": "background"}]"#
                .to_string(),
            vec![],
        );
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();
        assert_eq!(
            parsed.extractions.first().map(|e| &e.visibility),
            Some(&Visibility::Background),
            "background visibility should be parsed"
        );
    }

    #[test]
    fn should_observe_below_threshold() {
        let observer = Observer::new(
            Box::new(MockObserverProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 1000,
                ..ObserverConfig::default()
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
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(5);

        assert!(
            observer.should_observe(&messages),
            "should observe above threshold"
        );
    }

    #[test]
    fn check_thresholds_below_soft() {
        let observer = Observer::new(
            Box::new(MockObserverProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 100_000,
                force_threshold_tokens: 200_000,
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(2);
        assert_eq!(
            observer.check_thresholds(&messages),
            ObserveAction::None,
            "below soft threshold should return None"
        );
    }

    #[test]
    fn check_thresholds_between_soft_and_force() {
        let observer = Observer::new(
            Box::new(MockObserverProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
                force_threshold_tokens: 100_000,
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(5);
        assert_eq!(
            observer.check_thresholds(&messages),
            ObserveAction::StartCooldown,
            "between soft and force should return StartCooldown"
        );
    }

    #[test]
    fn check_thresholds_above_force() {
        let observer = Observer::new(
            Box::new(MockObserverProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
                force_threshold_tokens: 10,
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(5);
        assert_eq!(
            observer.check_thresholds(&messages),
            ObserveAction::ForceNow,
            "above force threshold should return ForceNow"
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
                ..ObserverConfig::default()
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
        // SAMPLE_RESPONSE has 2 observation objects → 2 Observations in the log
        assert_eq!(
            log.len(),
            2,
            "observation log should have two observations (one per object)"
        );

        // Verify the per-episode obs archive was written alongside the transcript
        let episode = crate::memory::types::Episode {
            id: result.id.clone(),
            date: chrono::Utc::now().naive_utc().date(),
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
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "test/project".to_string(),
            visibility: Visibility::User,
        }];

        let prompt = build_extraction_prompt(&recent_messages, EXTRACTION_CONTENT_PROMPT);
        assert_eq!(prompt.len(), 2, "should have system + user message");
        assert_eq!(
            prompt.first().map(|m| m.role),
            Some(Role::System),
            "first should be system"
        );

        let system_content = prompt.first().map_or("", |m| m.content.as_str());
        assert!(
            system_content.contains(EXTRACTION_FORMAT_SPEC),
            "system prompt should always include format spec"
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
            timestamp: chrono::Utc::now().naive_utc(),
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
            timestamp: chrono::Utc::now().naive_utc(),
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
        let timestamp = chrono::NaiveDate::from_ymd_opt(2026, 2, 21)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
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
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "ironclaw/memory".to_string(),
                visibility: Visibility::User,
            },
            RecentMessage {
                message: Message::user("b"),
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "ironclaw/memory".to_string(),
                visibility: Visibility::User,
            },
            RecentMessage {
                message: Message::user("c"),
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "devops/k8s".to_string(),
                visibility: Visibility::User,
            },
        ];
        let ctx = derive_project_context(&messages);
        assert_eq!(ctx, "ironclaw/memory", "should use most common context");
    }
}
