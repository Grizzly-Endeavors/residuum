//! End-to-end integration test for the memory subsystem.
//!
//! Verifies the full flow: accumulate messages → observer fires → observations created →
//! observations.json updated → recent messages cleared → search indexes episode →
//! reflector compresses.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod memory_integration {
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ironclaw::memory::log_store::{load_observation_log, next_episode_id};
    use ironclaw::memory::observer::{ObserveAction, ObserveResult, Observer, ObserverConfig};
    use ironclaw::memory::recent_store::{RecentContext, load_recent_context, save_recent_context};
    use ironclaw::memory::recent_store::{
        append_recent_messages, clear_recent_messages, load_recent_messages,
    };
    use ironclaw::memory::reflector::{Reflector, ReflectorConfig};
    use ironclaw::memory::search::MemoryIndex;
    use ironclaw::memory::types::Visibility;
    use ironclaw::models::{
        CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, ToolDefinition,
    };
    use ironclaw::workspace::layout::WorkspaceLayout;

    /// Mock provider that returns configurable JSON responses.
    struct MockProvider {
        responses: Vec<String>,
        call_idx: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses,
                call_idx: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            let idx = self.call_idx.fetch_add(1, Ordering::SeqCst);
            let content = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| self.responses.last().cloned().unwrap_or_default());
            Ok(ModelResponse::new(content, vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-integration"
        }
    }

    fn observer_response() -> String {
        r#"{
            "observations": [
                {"content": "workspace uses a flat directory layout with identity files at root", "timestamp": "2026-02-21T14:30Z", "visibility": "user"},
                {"content": "bootstrap creates 10 required directories on first run", "timestamp": "2026-02-21T14:31Z", "visibility": "user"},
                {"content": "SOUL.md defines the agent personality and is loaded at startup", "timestamp": "2026-02-21T14:32Z", "visibility": "user"}
            ],
            "narrative": "We were discussing the workspace layout and how identity files are organized. The bootstrap process creates the required directory structure."
        }"#
        .to_string()
    }

    fn observer_response_legacy() -> String {
        r#"[
            {"content": "workspace uses a flat directory layout with identity files at root", "timestamp": "2026-02-21T14:30Z", "visibility": "user"},
            {"content": "bootstrap creates 10 required directories on first run", "timestamp": "2026-02-21T14:31Z", "visibility": "user"},
            {"content": "SOUL.md defines the agent personality and is loaded at startup", "timestamp": "2026-02-21T14:32Z", "visibility": "user"}
        ]"#
        .to_string()
    }

    fn reflector_response() -> String {
        r#"[
            {"content": "workspace uses flat layout with identity files at root", "timestamp": "2026-02-21T14:32Z", "project_context": "ironclaw/workspace", "visibility": "user"},
            {"content": "bootstrap creates required directories on first run", "timestamp": "2026-02-21T14:31Z", "project_context": "ironclaw/workspace", "visibility": "user"}
        ]"#
        .to_string()
    }

    fn make_messages(count: usize) -> Vec<Message> {
        (0..count)
            .map(|i| {
                let content = format!(
                    "Message {i}: discussing workspace layout and file organization in detail. \
                     The workspace uses a flat structure with identity files like SOUL.md, \
                     AGENTS.md, USER.md, and MEMORY.md at the root level. {}",
                    "Additional context and detail to increase token count. ".repeat(20)
                );
                if i % 2 == 0 {
                    Message::user(content)
                } else {
                    Message::assistant(content, None)
                }
            })
            .collect()
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "integration test exercises the full multi-phase memory pipeline"
    )]
    async fn full_memory_flow() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        // Create required directories
        for d in layout.required_dirs() {
            tokio::fs::create_dir_all(&d).await.unwrap();
        }

        let recent_path = layout.recent_messages_json();

        // Phase 1: Accumulate messages and observer fires
        let observer = Observer::new(
            Box::new(MockProvider::new(vec![observer_response()])),
            ObserverConfig {
                threshold_tokens: 100, // Very low threshold for testing
                ..ObserverConfig::default()
            },
        );

        let messages = make_messages(10);
        append_recent_messages(
            &recent_path,
            &messages,
            "ironclaw/workspace",
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let recent = load_recent_messages(&recent_path).await.unwrap();
        assert!(
            observer.should_observe(&recent),
            "should trigger observation"
        );

        let ObserveResult {
            id: episode_id,
            transcript_path,
            observation_count,
            ..
        } = observer.observe(&recent, &layout).await.unwrap();
        clear_recent_messages(&recent_path).await.unwrap();

        assert_eq!(episode_id, "ep-001", "first episode should be ep-001");
        // observer_response has 3 observation strings
        assert_eq!(observation_count, 3, "should have 3 observations");

        // Verify transcript file was created
        assert!(transcript_path.exists(), "transcript file should exist");

        let transcript = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let first_line = transcript.lines().next().unwrap();
        let meta: serde_json::Value = serde_json::from_str(first_line).unwrap();
        assert!(
            meta.get("type").is_some(),
            "transcript first line should be JSON with type field"
        );

        // Verify observations.json was updated — 3 observation strings → 3 Observations
        let log = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            log.len(),
            3,
            "observation log should have three observations (one per string)"
        );
        assert_eq!(
            log.observations.first().map(|o| o.project_context.as_str()),
            Some("ironclaw/workspace"),
            "project_context should be preserved"
        );
        assert_eq!(
            log.observations.first().map(|o| &o.visibility),
            Some(&Visibility::User),
            "visibility should be User"
        );
        assert_eq!(
            log.observations
                .first()
                .and_then(|o| o.source_episodes.first())
                .map(String::as_str),
            Some("ep-001"),
            "source_episodes should reference ep-001"
        );

        // Verify recent messages were cleared
        let cleared = load_recent_messages(&recent_path).await.unwrap();
        assert!(
            cleared.is_empty(),
            "recent messages should be cleared after episode creation"
        );

        // Phase 2: More messages accumulate, second episode created
        let more_messages = make_messages(10);
        append_recent_messages(
            &recent_path,
            &more_messages,
            "ironclaw/workspace",
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let recent2 = load_recent_messages(&recent_path).await.unwrap();
        let second_ep = observer.observe(&recent2, &layout).await.unwrap();
        clear_recent_messages(&recent_path).await.unwrap();

        assert_eq!(second_ep.id, "ep-002", "second episode should be ep-002");

        let updated_log = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        // 3 from ep-001 + 3 from ep-002 = 6 observations
        assert_eq!(
            updated_log.len(),
            6,
            "observation log should have six observations after two episodes"
        );

        // Phase 3: Search indexes episodes
        let index = MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap();
        let count = index.rebuild(&layout.memory_dir()).unwrap();
        assert!(count >= 2, "should index at least 2 episode files");

        let results = index.search("workspace layout", 5).unwrap();
        assert!(
            !results.is_empty(),
            "should find results for workspace layout"
        );

        // Phase 4: Reflector compresses when threshold hit
        let reflector = Reflector::new(
            Box::new(MockProvider::new(vec![reflector_response()])),
            ReflectorConfig {
                threshold_tokens: 10, // Very low threshold for testing
                tz: chrono_tz::UTC,
            },
        );

        assert!(
            reflector.should_reflect(&updated_log),
            "should trigger reflection"
        );

        let compressed = reflector.reflect(&layout).await.unwrap();
        // reflector_response has 2 observation strings → 2 observations
        assert_eq!(
            compressed.len(),
            2,
            "compressed log should have two observations"
        );

        // Reflector observations have empty source_episodes
        assert!(
            compressed
                .observations
                .first()
                .is_some_and(|o| o.source_episodes.is_empty()),
            "reflected observations should have empty source_episodes"
        );
    }

    #[tokio::test]
    async fn messages_persist_across_simulated_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let recent_path = layout.recent_messages_json();

        // "Run 1" — add some messages, exit without hitting threshold
        let run1_msgs = make_messages(3);
        append_recent_messages(
            &recent_path,
            &run1_msgs,
            "ironclaw",
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        // "Run 2" — load and verify messages survived
        let loaded = load_recent_messages(&recent_path).await.unwrap();
        assert_eq!(loaded.len(), 3, "messages from previous run should persist");

        // Add more messages in run 2
        let run2_msgs = make_messages(3);
        append_recent_messages(
            &recent_path,
            &run2_msgs,
            "ironclaw",
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let all = load_recent_messages(&recent_path).await.unwrap();
        assert_eq!(all.len(), 6, "should have messages from both runs");
    }

    #[tokio::test]
    async fn episode_id_generation_is_sequential() {
        let dir = tempfile::tempdir().unwrap();
        let episodes_dir = dir.path().join("episodes");
        tokio::fs::create_dir_all(&episodes_dir).await.unwrap();

        // Empty dir → ep-001
        let id1 = next_episode_id(&episodes_dir).await.unwrap();
        assert_eq!(id1, "ep-001", "first ID should be ep-001");

        // Write ep-001.jsonl → next should be ep-002
        let month_dir = episodes_dir.join("2026-02/19");
        tokio::fs::create_dir_all(&month_dir).await.unwrap();
        tokio::fs::write(month_dir.join("ep-001.jsonl"), "")
            .await
            .unwrap();

        let id2 = next_episode_id(&episodes_dir).await.unwrap();
        assert_eq!(id2, "ep-002", "second ID should be ep-002");
    }

    #[test]
    fn search_index_creation_and_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();

        index
            .index_file(
                "episodes/ep-001.md",
                "the agent uses SOUL.md for personality",
            )
            .unwrap();

        let results = index.search("personality", 5).unwrap();
        assert!(!results.is_empty(), "should find indexed content");
        assert!(
            results.first().unwrap().score > 0.0,
            "score should be positive"
        );
    }

    #[test]
    fn observer_does_not_fire_below_threshold() {
        let observer = Observer::new(
            Box::new(MockProvider::new(vec![observer_response()])),
            ObserverConfig {
                threshold_tokens: 1_000_000,
                ..ObserverConfig::default()
            },
        );

        let messages = vec![ironclaw::memory::recent_store::RecentMessage {
            message: Message::user("hello"),
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "test".to_string(),
            visibility: Visibility::User,
        }];

        assert!(
            !observer.should_observe(&messages),
            "should not fire below threshold"
        );
    }

    #[test]
    fn check_thresholds_returns_correct_actions() {
        let observer = Observer::new(
            Box::new(MockProvider::new(vec![observer_response()])),
            ObserverConfig {
                threshold_tokens: 500,
                force_threshold_tokens: 100_000,
                ..ObserverConfig::default()
            },
        );

        // Below soft threshold — single short message is ~2 tokens
        let few_recent = vec![ironclaw::memory::recent_store::RecentMessage {
            message: Message::user("hello"),
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "test".to_string(),
            visibility: Visibility::User,
        }];

        assert_eq!(
            observer.check_thresholds(&few_recent),
            ObserveAction::None,
            "single short message should be below soft threshold"
        );

        // Above soft threshold but below force — make_messages(10) produces ~1250+ tokens
        let many = make_messages(10);
        let many_recent: Vec<ironclaw::memory::recent_store::RecentMessage> = many
            .into_iter()
            .map(|m| ironclaw::memory::recent_store::RecentMessage {
                message: m,
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "test".to_string(),
                visibility: Visibility::User,
            })
            .collect();

        assert_eq!(
            observer.check_thresholds(&many_recent),
            ObserveAction::StartCooldown,
            "many messages should start cooldown"
        );
    }

    #[tokio::test]
    async fn narrative_summary_in_observe_result() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        for d in layout.required_dirs() {
            tokio::fs::create_dir_all(&d).await.unwrap();
        }

        let observer = Observer::new(
            Box::new(MockProvider::new(vec![observer_response()])),
            ObserverConfig {
                threshold_tokens: 10,
                ..ObserverConfig::default()
            },
        );

        let messages = make_messages(5);
        let recent_path = layout.recent_messages_json();
        append_recent_messages(
            &recent_path,
            &messages,
            "test",
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let recent = load_recent_messages(&recent_path).await.unwrap();
        let result = observer.observe(&recent, &layout).await.unwrap();

        assert!(
            result.narrative.is_some(),
            "observe result should include narrative"
        );
        assert!(
            result
                .narrative
                .as_ref()
                .is_some_and(|n| n.contains("workspace layout")),
            "narrative should contain conversation summary"
        );
    }

    #[tokio::test]
    async fn backward_compat_legacy_array_format() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        for d in layout.required_dirs() {
            tokio::fs::create_dir_all(&d).await.unwrap();
        }

        // Use legacy bare-array format
        let observer = Observer::new(
            Box::new(MockProvider::new(vec![observer_response_legacy()])),
            ObserverConfig {
                threshold_tokens: 10,
                ..ObserverConfig::default()
            },
        );

        let messages = make_messages(5);
        let recent_path = layout.recent_messages_json();
        append_recent_messages(
            &recent_path,
            &messages,
            "test",
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let recent = load_recent_messages(&recent_path).await.unwrap();
        let result = observer.observe(&recent, &layout).await.unwrap();

        assert_eq!(
            result.observation_count, 3,
            "legacy format should still extract 3 observations"
        );
        assert!(
            result.narrative.is_none(),
            "legacy format should have no narrative"
        );
    }

    #[tokio::test]
    async fn recent_context_persistence_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_context.json");

        let ctx = RecentContext {
            narrative: "We were discussing workspace layout.".to_string(),
            created_at: chrono::Utc::now().naive_utc(),
            episode_id: "ep-001".to_string(),
        };

        save_recent_context(&path, &ctx).await.unwrap();
        let loaded = load_recent_context(&path).await.unwrap();

        assert!(loaded.is_some(), "should load persisted context");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.narrative, ctx.narrative);
        assert_eq!(loaded.episode_id, ctx.episode_id);
    }
}
