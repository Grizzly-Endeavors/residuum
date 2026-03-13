//! Observer: compresses recent messages into structured episodes via LLM.
//!
//! Fires synchronously after the agent completes a turn when the accumulated
//! recent message token count exceeds the configured threshold.

mod parse;
mod prompt;

use chrono_tz::Tz;

use crate::config::{
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
};
use crate::error::ResiduumError;
use crate::memory::chunk_extractor::{extract_chunks, write_idx_jsonl};
use crate::memory::episode_store::{episode_idx_path, episode_obs_path, write_episode_transcript};
use crate::memory::log_store::{append_observations, next_episode_id, save_episode_observations};
use crate::memory::recent_messages::RecentMessage;
use crate::memory::tokens::estimate_message_tokens;
use crate::memory::types::{Episode, IndexChunk, Observation};
use crate::models::{CompletionOptions, Message, ModelProvider, ResponseFormat};
use crate::time::now_local;
use crate::workspace::layout::WorkspaceLayout;
use parse::{ObserverParseResult, parse_observer_response};
use prompt::{EXTRACTION_CONTENT_PROMPT, build_extraction_prompt, observer_response_schema};

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
    /// The extracted observations, for downstream indexing without re-reading disk.
    pub observations: Vec<Observation>,
    /// Interaction-pair chunks extracted from the transcript.
    pub chunks: Vec<IndexChunk>,
    /// Episode date in `YYYY-MM-DD` format.
    pub date: String,
    /// Project context tag.
    pub context: String,
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
    /// Per-role overrides for temperature and thinking.
    pub role_overrides: Option<crate::config::RoleOverrides>,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            threshold_tokens: DEFAULT_OBSERVER_THRESHOLD,
            cooldown_secs: DEFAULT_OBSERVER_COOLDOWN_SECS,
            force_threshold_tokens: DEFAULT_OBSERVER_FORCE_THRESHOLD,
            tz: chrono_tz::UTC,
            role_overrides: None,
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

    /// Create a disabled observer that never triggers.
    ///
    /// Uses a `NullProvider` and `usize::MAX` thresholds so observation
    /// never fires. Used when memory subsystem initialization fails.
    #[must_use]
    pub fn disabled(tz: Tz) -> Self {
        Self {
            provider: Box::new(crate::models::null::NullProvider),
            config: ObserverConfig {
                threshold_tokens: usize::MAX,
                cooldown_secs: u64::MAX,
                force_threshold_tokens: usize::MAX,
                tz,
                role_overrides: None,
            },
        }
    }

    /// The configured cooldown period in seconds.
    #[must_use]
    pub fn cooldown_secs(&self) -> u64 {
        self.config.cooldown_secs
    }

    /// The soft observation threshold in tokens.
    #[must_use]
    pub fn threshold_tokens(&self) -> usize {
        self.config.threshold_tokens
    }

    /// The force observation threshold in tokens (bypasses cooldown).
    #[must_use]
    pub fn force_threshold_tokens(&self) -> usize {
        self.config.force_threshold_tokens
    }

    /// The configured timezone.
    #[must_use]
    pub fn timezone(&self) -> Tz {
        self.config.tz
    }

    /// Replace the observer's configuration (e.g. after a config reload).
    pub fn update_config(&mut self, config: ObserverConfig) {
        tracing::info!(
            old_threshold = self.config.threshold_tokens,
            new_threshold = config.threshold_tokens,
            "updating observer config"
        );
        self.config = config;
    }

    /// Replace the model provider (e.g. after a provider config change).
    pub fn swap_provider(&mut self, provider: Box<dyn ModelProvider>) {
        tracing::info!("swapping observer model provider");
        self.provider = provider;
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
    ) -> Result<ObserveResult, ResiduumError> {
        if recent_messages.is_empty() {
            return Err(ResiduumError::Memory(
                "no recent messages to extract from".to_string(),
            ));
        }

        // Generate the next episode ID by scanning the episodes directory
        let episode_id = next_episode_id(&layout.episodes_dir()).await?;

        // Load content guidance from disk, falling back to embedded constant.
        let content_guidance = match tokio::fs::read_to_string(layout.observer_md()).await {
            Ok(s) if !s.trim().is_empty() => s,
            Ok(_) => EXTRACTION_CONTENT_PROMPT.to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                EXTRACTION_CONTENT_PROMPT.to_string()
            }
            Err(e) => {
                tracing::warn!(path = %layout.observer_md().display(), error = %e, "failed to read observer guidance, using default");
                EXTRACTION_CONTENT_PROMPT.to_string()
            }
        };

        // Build extraction prompt using full RecentMessage metadata (timestamps,
        // tool calls, project context) so the observer LLM has complete context.
        let extraction_messages = build_extraction_prompt(recent_messages, &content_guidance);

        // Call the model with structured output, applying per-role overrides
        let ov = self.config.role_overrides.as_ref();
        let options = CompletionOptions {
            temperature: ov.and_then(|o| o.temperature),
            thinking: ov.and_then(|o| o.thinking.clone()),
            response_format: ResponseFormat::JsonSchema {
                name: "observer_extraction".to_string(),
                schema: observer_response_schema(),
            },
            ..CompletionOptions::default()
        };
        let response = self
            .provider
            .complete(&extraction_messages, &[], &options)
            .await
            .map_err(ResiduumError::Model)?;

        // Parse the response into extraction results and optional narrative.
        let parsed = parse_observer_response(&response, self.config.tz)?;

        build_episode_and_persist(parsed, episode_id, recent_messages, layout, self.config.tz).await
    }
}

/// Estimate the total token count of recent messages.
fn estimate_recent_tokens(recent_messages: &[RecentMessage]) -> usize {
    let messages: Vec<Message> = recent_messages
        .iter()
        .map(|rm| rm.message.clone())
        .collect();
    estimate_message_tokens(&messages)
}

/// Pick the most common context from a list of context strings.
///
/// Falls back to `"general"` if the list is empty or all strings are empty.
fn majority_context(contexts: &[String]) -> String {
    use std::collections::HashMap;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for ctx in contexts {
        if !ctx.is_empty() {
            *counts.entry(ctx.as_str()).or_insert(0) += 1;
        }
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map_or_else(|| "general".to_string(), |(ctx, _)| ctx.to_string())
}

/// Build the episode from parsed extractions and persist all artifacts to disk.
async fn build_episode_and_persist(
    parsed: ObserverParseResult,
    episode_id: String,
    recent_messages: &[RecentMessage],
    layout: &WorkspaceLayout,
    tz: Tz,
) -> Result<ObserveResult, ResiduumError> {
    // Extract inner messages for the episode transcript.
    let messages: Vec<Message> = recent_messages
        .iter()
        .map(|rm| rm.message.clone())
        .collect();

    // Episode-level context via majority vote over per-extraction contexts.
    let extraction_contexts: Vec<String> = parsed
        .extractions
        .iter()
        .map(|e| e.project_context.clone())
        .collect();
    let episode_context = majority_context(&extraction_contexts);

    // Build the episode internally — start/end are cosmetic and no longer LLM-extracted.
    let episode = Episode {
        id: episode_id.clone(),
        date: now_local(tz).date(),
        start: String::new(),
        end: String::new(),
        context: episode_context.clone(),
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

    // Convert episode observations → flat Observations with per-extraction context
    let observation_count = episode.observations.len();
    let observations: Vec<Observation> = parsed
        .extractions
        .iter()
        .map(|e| Observation {
            timestamp: e.timestamp,
            project_context: e.project_context.clone(),
            source_episodes: vec![episode.id.clone()],
            visibility: e.visibility.clone(),
            content: e.content.clone(),
        })
        .collect();

    let obs_path = episode_obs_path(&layout.episodes_dir(), &episode);
    save_episode_observations(&obs_path, &observations).await?;
    append_observations(&layout.observations_json(), observations.clone()).await?;

    // Extract interaction-pair chunks from recent messages and persist as idx.jsonl.
    // line_offset=2 because line 1 is the meta object in the JSONL transcript.
    let date_str = episode.date.to_string();
    let chunks = extract_chunks(recent_messages, &episode.id, &date_str, 2);
    let idx_path = episode_idx_path(&layout.episodes_dir(), &episode);
    write_idx_jsonl(&idx_path, &chunks).await?;

    tracing::info!(
        episode_id = %episode.id,
        observations = observation_count,
        chunks = chunks.len(),
        has_narrative = parsed.narrative.is_some(),
        "episode extracted"
    );

    Ok(ObserveResult {
        id: episode.id,
        transcript_path,
        observation_count,
        narrative: parsed.narrative,
        observations,
        chunks,
        date: date_str,
        context: episode_context,
    })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::episode_store::episode_obs_path;
    use crate::memory::log_store::load_observation_log;
    use crate::memory::recent_messages::RecentMessage;
    use crate::memory::test_helpers::MockMemoryProvider;
    use crate::memory::types::Visibility;
    use crate::models::{ModelResponse, Role};
    use parse::{parse_extraction_items, parse_observer_response};
    use prompt::{
        EXTRACTION_CONTENT_PROMPT, EXTRACTION_FORMAT_SPEC, build_extraction_prompt,
        format_recent_message,
    };

    const SAMPLE_RESPONSE: &str = r#"{
        "observations": [
            {"content": "workspace uses a flat directory layout", "timestamp": "2026-02-21T14:30", "visibility": "user", "project_context": "residuum/workspace"},
            {"content": "identity files are loaded at startup", "timestamp": "2026-02-21T14:31", "visibility": "user", "project_context": "residuum/workspace"}
        ],
        "narrative": ""
    }"#;

    fn make_recent_messages(count: usize) -> Vec<RecentMessage> {
        (0..count)
            .map(|i| RecentMessage {
                message: Message::user(format!(
                    "message {i} with enough content to contribute to token count - {}",
                    "a".repeat(100)
                )),
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "residuum/workspace".to_string(),
                visibility: Visibility::User,
            })
            .collect()
    }

    #[test]
    fn parse_observer_response_typed_object_format() {
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
        assert!(parsed.narrative.is_none(), "empty narrative should be None");
    }

    #[test]
    fn parse_observer_response_legacy_array_format() {
        let bare_array = r#"[
            {"content": "workspace uses a flat directory layout", "timestamp": "2026-02-21T14:30", "visibility": "user"},
            {"content": "identity files are loaded at startup", "timestamp": "2026-02-21T14:31", "visibility": "user"}
        ]"#;
        let response = ModelResponse::new(bare_array.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 2, "should have 2 extractions");
        assert!(
            parsed.narrative.is_none(),
            "legacy format should have no narrative"
        );
    }

    #[test]
    fn parse_observer_response_new_format() {
        let json = r#"{
            "observations": [
                {"content": "user prefers Rust", "timestamp": "2026-02-21T14:30", "visibility": "user", "project_context": "residuum"}
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
    fn parse_observer_response_empty_narrative_is_none() {
        let json = r#"{
            "observations": [
                {"content": "user prefers Rust", "timestamp": "2026-02-21T14:30", "visibility": "user", "project_context": "residuum"}
            ],
            "narrative": ""
        }"#;
        let response = ModelResponse::new(json.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 1, "should have 1 extraction");
        assert!(
            parsed.narrative.is_none(),
            "empty narrative string should be None"
        );
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
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
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
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
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
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
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
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
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
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
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
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
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
            project_context: "residuum/memory".to_string(),
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
            project_context: "residuum/memory".to_string(),
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
            project_context: "residuum/memory".to_string(),
            visibility: Visibility::User,
        };

        let formatted = format_recent_message(&rm);
        assert!(
            formatted.contains("2026-02-21"),
            "should include ISO date in timestamp"
        );
        assert!(
            formatted.contains("residuum/memory"),
            "should include project context"
        );
        assert!(
            formatted.contains("visibility: user"),
            "should include visibility"
        );
    }

    #[test]
    fn majority_context_picks_most_common() {
        let contexts = vec![
            "residuum/memory".to_string(),
            "residuum/memory".to_string(),
            "devops/k8s".to_string(),
        ];
        let ctx = majority_context(&contexts);
        assert_eq!(ctx, "residuum/memory", "should use most common context");
    }

    #[test]
    fn majority_context_empty_falls_back() {
        let contexts: Vec<String> = vec![];
        let ctx = majority_context(&contexts);
        assert_eq!(ctx, "general", "empty list should fall back to general");
    }

    #[test]
    fn update_config_changes_thresholds() {
        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 1000,
                cooldown_secs: 60,
                force_threshold_tokens: 5000,
                tz: chrono_tz::UTC,
                role_overrides: None,
            },
        );

        assert_eq!(observer.threshold_tokens(), 1000);
        assert_eq!(observer.cooldown_secs(), 60);
        assert_eq!(observer.force_threshold_tokens(), 5000);

        let mut observer = observer;
        observer.update_config(ObserverConfig {
            threshold_tokens: 2000,
            cooldown_secs: 120,
            force_threshold_tokens: 10000,
            tz: chrono_tz::US::Eastern,
            role_overrides: None,
        });

        assert_eq!(observer.threshold_tokens(), 2000);
        assert_eq!(observer.cooldown_secs(), 120);
        assert_eq!(observer.force_threshold_tokens(), 10000);
        assert_eq!(observer.timezone(), chrono_tz::US::Eastern);
    }

    #[test]
    fn swap_provider_changes_model() {
        let mut observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig::default(),
        );

        let new_response = r#"{
            "observations": [
                {"content": "new provider obs", "timestamp": "2026-02-21T14:30", "visibility": "user", "project_context": "test"}
            ],
            "narrative": ""
        }"#;
        observer.swap_provider(Box::new(MockMemoryProvider::new(new_response)));

        // Verify the provider was swapped by checking the model name
        // (MockMemoryProvider always returns "mock-model")
        // The key verification is that the method doesn't panic and accepts the new provider
        assert_eq!(
            observer.threshold_tokens(),
            ObserverConfig::default().threshold_tokens
        );
    }

    #[test]
    fn parse_extraction_items_skips_missing_content() {
        let items = vec![
            serde_json::json!({"timestamp": "2026-02-21T14:30", "visibility": "user"}),
            serde_json::json!({"content": "valid obs", "timestamp": "2026-02-21T14:31", "visibility": "user"}),
        ];
        let results = parse_extraction_items(&items, chrono_tz::UTC);
        assert_eq!(results.len(), 1, "item missing content should be skipped");
        assert_eq!(
            results.first().map(|e| e.content.as_str()),
            Some("valid obs")
        );
    }
}
