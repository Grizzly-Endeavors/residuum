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
    use ironclaw::memory::recent_context::{
        RecentContext, load_recent_context, save_recent_context,
    };
    use ironclaw::memory::recent_messages::{
        append_recent_messages, clear_recent_messages, load_recent_messages,
    };
    use ironclaw::memory::reflector::{Reflector, ReflectorConfig};
    use ironclaw::memory::search::{MemoryIndex, SearchFilters};
    use ironclaw::memory::types::{IndexManifest, Visibility};
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
                {"content": "workspace uses a flat directory layout with identity files at root", "timestamp": "2026-02-21T14:30Z", "visibility": "user", "project_context": "ironclaw/workspace"},
                {"content": "bootstrap creates 10 required directories on first run", "timestamp": "2026-02-21T14:31Z", "visibility": "user", "project_context": "ironclaw/workspace"},
                {"content": "SOUL.md defines the agent personality and is loaded at startup", "timestamp": "2026-02-21T14:32Z", "visibility": "user", "project_context": "ironclaw/workspace"}
            ],
            "narrative": "We were discussing the workspace layout and how identity files are organized. The bootstrap process creates the required directory structure."
        }"#
        .to_string()
    }

    fn observer_response_legacy() -> String {
        r#"[
            {"content": "workspace uses a flat directory layout with identity files at root", "timestamp": "2026-02-21T14:30Z", "visibility": "user", "project_context": "ironclaw/workspace"},
            {"content": "bootstrap creates 10 required directories on first run", "timestamp": "2026-02-21T14:31Z", "visibility": "user", "project_context": "ironclaw/workspace"},
            {"content": "SOUL.md defines the agent personality and is loaded at startup", "timestamp": "2026-02-21T14:32Z", "visibility": "user", "project_context": "ironclaw/workspace"}
        ]"#
        .to_string()
    }

    fn reflector_response() -> String {
        r#"{
            "observations": [
                {"content": "workspace uses flat layout with identity files at root", "timestamp": "2026-02-21T14:32Z", "project_context": "ironclaw/workspace", "visibility": "user"},
                {"content": "bootstrap creates required directories on first run", "timestamp": "2026-02-21T14:31Z", "project_context": "ironclaw/workspace", "visibility": "user"}
            ]
        }"#
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

    fn no_filters() -> SearchFilters {
        SearchFilters::default()
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
            observations,
            chunks,
            date,
            context,
            ..
        } = observer.observe(&recent, &layout).await.unwrap();
        clear_recent_messages(&recent_path).await.unwrap();

        assert_eq!(episode_id, "ep-001", "first episode should be ep-001");
        // observer_response has 3 observation strings
        assert_eq!(observation_count, 3, "should have 3 observations");
        assert_eq!(
            observations.len(),
            3,
            "observations vec should have 3 items"
        );
        assert!(!date.is_empty(), "date should not be empty");
        assert_eq!(context, "ironclaw/workspace", "context should match");

        // Verify transcript file was created
        assert!(transcript_path.exists(), "transcript file should exist");

        let transcript = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let first_line = transcript.lines().next().unwrap();
        let meta: serde_json::Value = serde_json::from_str(first_line).unwrap();
        assert!(
            meta.get("type").is_some(),
            "transcript first line should be JSON with type field"
        );

        // Verify idx.jsonl was created alongside transcript
        let idx_path = transcript_path
            .parent()
            .unwrap()
            .join(format!("{episode_id}.idx.jsonl"));
        assert!(
            idx_path.exists(),
            "idx.jsonl should exist alongside transcript"
        );

        // Verify chunks were extracted (messages alternate user/assistant so should produce pairs)
        assert!(
            !chunks.is_empty(),
            "should have extracted at least one interaction pair"
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

        // Phase 3: Search indexes observations and chunks
        let index = MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap();
        let result = index.rebuild(&layout.memory_dir()).unwrap();
        assert!(
            result.obs_count >= 6,
            "should index at least 6 observations"
        );
        assert!(result.chunk_count >= 1, "should index at least 1 chunk");

        // Search should find observations
        let obs_results = index
            .search(
                "workspace layout",
                5,
                &SearchFilters {
                    source: Some("observation".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            !obs_results.is_empty(),
            "should find observation results for workspace layout"
        );
        assert!(
            obs_results.iter().all(|r| r.source_type == "observation"),
            "all should be observations"
        );

        // Search should also find chunks
        let chunk_results = index
            .search(
                "workspace layout",
                5,
                &SearchFilters {
                    source: Some("chunk".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            !chunk_results.is_empty(),
            "should find chunk results for workspace layout"
        );
        assert!(
            chunk_results.iter().all(|r| r.source_type == "chunk"),
            "all should be chunks"
        );

        // Unfiltered search should find both types
        let all_results = index.search("workspace layout", 10, &no_filters()).unwrap();
        assert!(
            all_results.len() >= 2,
            "should find both obs and chunk results"
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
    fn search_index_creation_and_observations() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();

        let obs = vec![ironclaw::memory::types::Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "ironclaw".to_string(),
            source_episodes: vec!["ep-001".to_string()],
            visibility: Visibility::User,
            content: "the agent uses SOUL.md for personality".to_string(),
        }];

        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let results = index.search("personality", 5, &no_filters()).unwrap();
        assert!(!results.is_empty(), "should find indexed content");
        assert!(
            results.first().unwrap().score > 0.0,
            "score should be positive"
        );
        assert_eq!(
            results.first().unwrap().source_type,
            "observation",
            "should be an observation"
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

        let messages = vec![ironclaw::memory::recent_messages::RecentMessage {
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
        let few_recent = vec![ironclaw::memory::recent_messages::RecentMessage {
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
        let many_recent: Vec<ironclaw::memory::recent_messages::RecentMessage> = many
            .into_iter()
            .map(|m| ironclaw::memory::recent_messages::RecentMessage {
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

    #[tokio::test]
    async fn incremental_sync_simulation() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        for d in layout.required_dirs() {
            tokio::fs::create_dir_all(&d).await.unwrap();
        }

        // Create first episode with observation + chunk files
        let day_dir = layout.episodes_dir().join("2026-02/19");
        tokio::fs::create_dir_all(&day_dir).await.unwrap();

        let obs1 = vec![ironclaw::memory::types::Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "ironclaw".to_string(),
            source_episodes: vec!["ep-001".to_string()],
            visibility: Visibility::User,
            content: "first observation about workspace".to_string(),
        }];
        tokio::fs::write(
            day_dir.join("ep-001.obs.json"),
            serde_json::to_string(&obs1).unwrap(),
        )
        .await
        .unwrap();

        let chunk1 = ironclaw::memory::types::IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "ironclaw".to_string(),
            line_start: 2,
            line_end: 3,
            content: "user: what about workspace?\nassistant: it uses flat layout".to_string(),
        };
        tokio::fs::write(
            day_dir.join("ep-001.idx.jsonl"),
            serde_json::to_string(&chunk1).unwrap() + "\n",
        )
        .await
        .unwrap();

        // Build index with full rebuild
        let index = MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap();
        let rebuild_result = index.rebuild(&layout.memory_dir()).unwrap();
        assert_eq!(rebuild_result.obs_count, 1);
        assert_eq!(rebuild_result.chunk_count, 1);

        // Create manifest from rebuild
        let mut manifest = IndexManifest::new();
        manifest.last_rebuild = "2026-02-19T14:00:00".to_string();
        for (path, entry) in rebuild_result.file_entries {
            manifest.files.insert(path, entry);
        }

        // Add a second episode
        let obs2 = vec![ironclaw::memory::types::Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "ironclaw".to_string(),
            source_episodes: vec!["ep-002".to_string()],
            visibility: Visibility::User,
            content: "second observation about testing".to_string(),
        }];
        tokio::fs::write(
            day_dir.join("ep-002.obs.json"),
            serde_json::to_string(&obs2).unwrap(),
        )
        .await
        .unwrap();

        // Incremental sync should only index the new file
        let (new_manifest, stats) = index
            .incremental_sync(&layout.memory_dir(), &manifest)
            .unwrap();
        assert_eq!(stats.added, 1, "should add 1 new file");
        assert_eq!(stats.unchanged, 2, "2 existing files unchanged");
        assert!(
            new_manifest.files.len() > manifest.files.len(),
            "manifest should grow"
        );

        // Should find both observations
        let results = index.search("observation", 10, &no_filters()).unwrap();
        assert!(
            results.len() >= 2,
            "should find observations from both episodes"
        );
    }

    #[test]
    fn filter_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        let day_dir_1 = memory_dir.join("episodes/2026-02/15");
        let day_dir_2 = memory_dir.join("episodes/2026-02/20");
        std::fs::create_dir_all(&day_dir_1).unwrap();
        std::fs::create_dir_all(&day_dir_2).unwrap();

        // Early ironclaw observation
        let obs1 = vec![ironclaw::memory::types::Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "ironclaw".to_string(),
            source_episodes: vec!["ep-001".to_string()],
            visibility: Visibility::User,
            content: "ironclaw uses tantivy for search".to_string(),
        }];
        std::fs::write(
            day_dir_1.join("ep-001.obs.json"),
            serde_json::to_string(&obs1).unwrap(),
        )
        .unwrap();

        // Later devops observation
        let obs2 = vec![ironclaw::memory::types::Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "devops".to_string(),
            source_episodes: vec!["ep-002".to_string()],
            visibility: Visibility::User,
            content: "devops uses kubernetes for search orchestration".to_string(),
        }];
        std::fs::write(
            day_dir_2.join("ep-002.obs.json"),
            serde_json::to_string(&obs2).unwrap(),
        )
        .unwrap();

        // Chunk from ep-001
        let chunk = ironclaw::memory::types::IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-15".to_string(),
            context: "ironclaw".to_string(),
            line_start: 2,
            line_end: 3,
            content: "user: how does search work?\nassistant: we use tantivy BM25".to_string(),
        };
        std::fs::write(
            day_dir_1.join("ep-001.idx.jsonl"),
            serde_json::to_string(&chunk).unwrap() + "\n",
        )
        .unwrap();

        let index_dir = memory_dir.join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();
        index.rebuild(&memory_dir).unwrap();

        // Filter: observations only
        let obs_only = index
            .search(
                "search",
                10,
                &SearchFilters {
                    source: Some("observation".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            obs_only.iter().all(|r| r.source_type == "observation"),
            "should only return observations"
        );

        // Filter: date range (only 2026-02-18+)
        let date_filtered = index
            .search(
                "search",
                10,
                &SearchFilters {
                    date_from: Some("2026-02-18".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            date_filtered
                .iter()
                .all(|r| r.date.as_str() >= "2026-02-18"),
            "should only return results from 2026-02-18 onwards"
        );

        // Filter: project context
        let ctx_filtered = index
            .search(
                "search",
                10,
                &SearchFilters {
                    project_context: Some("ironclaw".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            ctx_filtered.iter().all(|r| r.context == "ironclaw"),
            "should only return ironclaw results"
        );
    }
}
