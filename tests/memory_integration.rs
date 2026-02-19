//! End-to-end integration test for the memory subsystem.
//!
//! Verifies the full flow: push messages → observer fires → episode file created →
//! observations.json updated → search indexes episode → reflector compresses.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod memory_integration {
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ironclaw::agent::session::Session;
    use ironclaw::memory::log_store::{load_observation_log, next_episode_id};
    use ironclaw::memory::observer::{Observer, ObserverConfig};
    use ironclaw::memory::reflector::{Reflector, ReflectorConfig};
    use ironclaw::memory::search::MemoryIndex;
    use ironclaw::models::{
        CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, Role, ToolDefinition,
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
            "start": "user asked about workspace structure",
            "end": "discussed file layout and bootstrapping",
            "context": "ironclaw/workspace",
            "observations": [
                "workspace uses a flat directory layout with identity files at root",
                "bootstrap creates 10 required directories on first run",
                "SOUL.md defines the agent personality and is loaded at startup"
            ]
        }"#
        .to_string()
    }

    fn reflector_response() -> String {
        r#"{
            "episodes": [
                {
                    "id": "ref-001",
                    "date": "2026-02-19",
                    "start": "workspace exploration",
                    "end": "file layout discussed",
                    "context": "ironclaw/workspace",
                    "observations": [
                        "workspace uses flat layout with identity files at root",
                        "bootstrap creates required directories on first run"
                    ],
                    "source_episodes": ["ep-001", "ep-002"]
                }
            ]
        }"#
        .to_string()
    }

    fn push_many_messages(session: &mut Session, count: usize) {
        for i in 0..count {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session.push(Message {
                role,
                content: format!(
                    "Message {i}: discussing workspace layout and file organization in detail. \
                     The workspace uses a flat structure with identity files like SOUL.md, \
                     AGENTS.md, USER.md, and MEMORY.md at the root level. {}",
                    "Additional context and detail to increase token count. ".repeat(20)
                ),
                tool_calls: None,
                tool_call_id: None,
            });
        }
    }

    #[tokio::test]
    async fn full_memory_flow() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        // Create required directories
        for d in layout.required_dirs() {
            tokio::fs::create_dir_all(&d).await.unwrap();
        }

        // Phase 1: Observer fires and creates episode
        let observer = Observer::new(
            Box::new(MockProvider::new(vec![observer_response()])),
            ObserverConfig {
                threshold_tokens: 100, // Very low threshold for testing
            },
        );

        let mut session = Session::new();
        push_many_messages(&mut session, 10);

        assert!(
            observer.should_observe(&session),
            "should trigger observation"
        );

        let episode = observer.observe(&mut session, &layout).await.unwrap();

        assert_eq!(episode.id, "ep-001", "first episode should be ep-001");
        assert_eq!(
            episode.context, "ironclaw/workspace",
            "context should match"
        );
        assert_eq!(episode.observations.len(), 3, "should have 3 observations");

        // Verify transcript file was created
        let transcript_path = layout.episodes_dir().join("ep-001.md");
        assert!(transcript_path.exists(), "transcript file should exist");

        let transcript = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        assert!(
            transcript.contains("---"),
            "transcript should have frontmatter"
        );
        assert!(
            transcript.contains("ep-001"),
            "transcript should reference episode ID"
        );

        // Verify observations.json was updated
        let log = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(log.len(), 1, "observation log should have one episode");

        // Verify session is marked observed
        assert_eq!(
            session.unobserved_count(),
            0,
            "all messages should be marked observed"
        );

        // Phase 2: Second observer run creates ep-002
        push_many_messages(&mut session, 10);

        let episode2 = observer.observe(&mut session, &layout).await.unwrap();
        assert_eq!(episode2.id, "ep-002", "second episode should be ep-002");

        let updated_log = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            updated_log.len(),
            2,
            "observation log should have two episodes"
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
            },
        );

        assert!(
            reflector.should_reflect(&updated_log),
            "should trigger reflection"
        );

        let compressed = reflector.reflect(&layout).await.unwrap();
        assert_eq!(
            compressed.len(),
            1,
            "compressed log should have one reflected episode"
        );

        let ref_episode = compressed.episodes.first().unwrap();
        assert_eq!(ref_episode.id, "ref-001", "should use ref- prefix");
        assert_eq!(
            ref_episode.source_episodes,
            vec!["ep-001", "ep-002"],
            "should track source episodes"
        );

        // Verify observation log was replaced
        let reloaded = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            reloaded.len(),
            1,
            "observation log should be compressed to one entry"
        );
    }

    #[test]
    fn episode_id_generation_is_sequential() {
        let mut log = ironclaw::memory::types::ObservationLog::new();

        assert_eq!(next_episode_id(&log), "ep-001", "first ID should be ep-001");

        log.push(ironclaw::memory::types::Episode {
            id: "ep-001".to_string(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            start: "s".to_string(),
            end: "e".to_string(),
            context: "test".to_string(),
            observations: vec![],
            source_episodes: vec![],
        });

        assert_eq!(
            next_episode_id(&log),
            "ep-002",
            "second ID should be ep-002"
        );
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
            },
        );

        let mut session = Session::new();
        session.push(Message {
            role: Role::User,
            content: "hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        });

        assert!(
            !observer.should_observe(&session),
            "should not fire below threshold"
        );
    }

    #[tokio::test]
    async fn daily_log_integration() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();

        ironclaw::memory::daily_log::append_daily_note(&memory_dir, "first observation")
            .await
            .unwrap();
        ironclaw::memory::daily_log::append_daily_note(&memory_dir, "second observation")
            .await
            .unwrap();

        // Verify the file exists and has content
        let path = ironclaw::memory::daily_log::daily_log_path(&memory_dir);
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(
            content.contains("first observation"),
            "should have first note"
        );
        assert!(
            content.contains("second observation"),
            "should have second note"
        );

        // Index the daily log
        let index_dir = dir.path().join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();
        index.rebuild(&memory_dir).unwrap();

        let results = index.search("observation", 5).unwrap();
        assert!(
            !results.is_empty(),
            "should find daily log content in search"
        );
    }
}
