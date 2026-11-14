//! Local agent integration for Meeting Rooms (`#860`).
//!
//! [`GatewayRoomParticipant`] implements [`LocalRoomParticipant`] for one
//! running local agent, executing each room turn inside an isolated
//! `external-room-<room_id>` background session on [`ModelSelector::MAIN`] so
//! the agent's 1:1 `recent_messages.json` conversation is never touched.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

use residuum_background::SpawnContext;
use residuum_background::messaging::{
    AgentMessenger, ConversationDeliveryOutcome, ConversationSpawn,
};
use residuum_background::registry::SessionRegistry;
use residuum_bus::origin::{ConversationContext, ConversationKind, MessageOrigin, ROOM_ENDPOINT};
use residuum_bus::{
    AgentResultStatus, BusHandle, MessageEvent, Publisher, SessionAddress, SessionEvent,
    SessionEventKind, Subscriber, UserInboxAddedEvent, topics,
};
use residuum_config::ModelSelector;
use residuum_inference::{
    CacheHints, CompletionOptions, ImageData, Message, ResponseFormat, UsageComponent,
    UsageRecorder, build_provider_chain,
};
use residuum_rooms::{LocalRoomParticipant, LocalRoomTurnRequest, RoomAttachment, RoomTurnOutput};
use residuum_workspace::inbox::attachment::{
    MAX_IMAGE_INLINE_SIZE, encode_image_from_file, is_supported_image,
};

/// Deterministic isolated session address for one room on a local agent.
#[must_use]
pub(crate) fn room_session_address(room_id: &str) -> SessionAddress {
    SessionAddress::from(format!("external-room-{room_id}"))
}

/// Bridges a running local agent's session runtime and main provider chain to
/// [`residuum_rooms::RoomService`].
pub(crate) struct GatewayRoomParticipant {
    pub(crate) agent_messenger: Arc<AgentMessenger>,
    pub(crate) session_registry: Arc<SessionRegistry>,
    pub(crate) bus_handle: BusHandle,
    pub(crate) publisher: Publisher,
    pub(crate) spawn_context_rx: watch::Receiver<Arc<SpawnContext>>,
    pub(crate) rooms_root: PathBuf,
    pub(crate) tz: chrono_tz::Tz,
}

impl GatewayRoomParticipant {
    async fn prepare_attachments(
        &self,
        room_id: &str,
        attachments: &[RoomAttachment],
    ) -> (String, Vec<ImageData>) {
        let mut extra = String::new();
        let mut images = Vec::new();
        for att in attachments {
            let path = self
                .rooms_root
                .join(room_id)
                .join("attachments")
                .join(&att.id)
                .join(&att.filename);
            if !path.is_file() {
                continue;
            }
            if is_supported_image(&att.mime_type)
                && att.size <= u64::from(MAX_IMAGE_INLINE_SIZE)
                && let Ok(img) = encode_image_from_file(&path, &att.mime_type).await
            {
                images.push(img);
            }
            _ = write!(
                extra,
                "\n[Attachment saved at {} ({}, {} bytes)]",
                path.display(),
                att.mime_type,
                att.size
            );
        }
        (extra, images)
    }
}

struct TurnTracker {
    await_new_run: bool,
    own_turn_id: Option<String>,
    final_text: Option<String>,
    error_msg: Option<String>,
}

impl TurnTracker {
    fn new(await_new_run: bool) -> Self {
        Self {
            await_new_run,
            own_turn_id: None,
            final_text: None,
            error_msg: None,
        }
    }

    fn on_event(
        &mut self,
        kind: SessionEventKind,
        tool_status_tx: &mpsc::UnboundedSender<Option<String>>,
    ) -> Option<Result<RoomTurnOutput, String>> {
        match kind {
            SessionEventKind::Started(_) => {
                self.await_new_run = false;
                None
            }
            SessionEventKind::TurnStarted { turn_id } => {
                if self.own_turn_id.is_none() {
                    self.await_new_run = false;
                    self.own_turn_id = Some(turn_id);
                }
                None
            }
            SessionEventKind::ToolCall(call) => {
                if self.own_turn_id.is_some() {
                    tool_status_tx.send(Some(call.name)).ok();
                }
                None
            }
            SessionEventKind::ToolResult(_) => {
                if self.own_turn_id.is_some() {
                    tool_status_tx.send(None).ok();
                }
                None
            }
            SessionEventKind::Response { turn_id, content } => {
                if self.own_turn_id.as_deref() == Some(&turn_id) {
                    self.final_text = Some(content);
                }
                None
            }
            SessionEventKind::Error { message, .. } => {
                if self.own_turn_id.is_some() {
                    self.error_msg = Some(message);
                }
                None
            }
            SessionEventKind::TurnEnded { turn_id } => {
                if self.own_turn_id.as_deref() != Some(&turn_id) {
                    return None;
                }
                tool_status_tx.send(None).ok();
                if let Some(err) = self.error_msg.take() {
                    return Some(Err(err));
                }
                match self.final_text.take() {
                    Some(content) => Some(Ok(RoomTurnOutput {
                        content,
                        attachments: Vec::new(),
                    })),
                    None => Some(Err("turn ended without a reply".to_owned())),
                }
            }
            SessionEventKind::Completed { status, .. } => {
                if self.await_new_run {
                    return None;
                }
                match status {
                    AgentResultStatus::Cancelled => Some(Err("turn canceled".to_owned())),
                    AgentResultStatus::Failed { error, .. } => Some(Err(error)),
                    AgentResultStatus::Completed => self.own_turn_id.is_some().then(|| match self
                        .final_text
                        .take()
                    {
                        Some(content) => Ok(RoomTurnOutput {
                            content,
                            attachments: Vec::new(),
                        }),
                        None => Err("session completed without a reply".to_owned()),
                    }),
                }
            }
            SessionEventKind::StateChanged(_)
            | SessionEventKind::MessageRead { .. }
            | SessionEventKind::Intermediate { .. }
            | SessionEventKind::TurnUsage { .. } => None,
        }
    }
}

#[async_trait]
impl LocalRoomParticipant for GatewayRoomParticipant {
    async fn run_room_turn(&self, req: LocalRoomTurnRequest) -> Result<RoomTurnOutput, String> {
        let address = room_session_address(&req.room_id);
        let mut sub: Subscriber<SessionEvent> =
            self.bus_handle
                .subscribe(topics::Sessions)
                .await
                .map_err(|e| format!("failed to subscribe to session events: {e}"))?;

        let (att_note, images) = self
            .prepare_attachments(&req.room_id, &req.attachments)
            .await;
        let mut content = req.prompt;
        if !att_note.is_empty() {
            content.push_str(&att_note);
        }

        let inbound = MessageEvent {
            id: format!("room-{}-{}", req.room_id, uuid::Uuid::new_v4()),
            content,
            origin: MessageOrigin {
                endpoint: ROOM_ENDPOINT.to_owned(),
                sender: None,
                conversation: Some(ConversationContext {
                    id: req.room_id.clone(),
                    kind: ConversationKind::GroupChat,
                    is_owner: true,
                }),
                agent_sender: None,
            },
            timestamp: residuum_core::time::now_local(self.tz),
            images,
            attachments: Vec::new(),
            context: None,
        };
        let spawn = ConversationSpawn {
            source_label: format!("room:{}", req.room_name),
            model: ModelSelector::MAIN,
            skill: None,
        };

        let outcome = self
            .agent_messenger
            .deliver_conversation(&address, inbound, spawn)
            .await
            .map_err(|e| format!("failed to deliver room turn: {e}"))?;

        let mut tracker =
            TurnTracker::new(matches!(outcome, ConversationDeliveryOutcome::Queued(_)));
        while let Ok(Some(ev)) = sub.recv().await {
            if ev.address != address {
                continue;
            }
            if let Some(res) = tracker.on_event(ev.kind, &req.tool_status_tx) {
                return res;
            }
        }
        Err("session event stream closed before turn finished".to_owned())
    }

    async fn cancel_room_turn(&self, room_id: &str) -> Result<(), String> {
        let address = room_session_address(room_id);
        self.session_registry.stop_if_running(&address);
        Ok(())
    }

    async fn close_room_session(&self, room_id: &str) -> Result<(), String> {
        let address = room_session_address(room_id);
        self.session_registry.stop(&address);
        Ok(())
    }

    async fn generate_minutes(&self, prompt: String) -> Result<String, String> {
        let ctx = Arc::clone(&*self.spawn_context_rx.borrow());
        let (provider, _dropped) = build_provider_chain(
            &ctx.main_provider_specs,
            ctx.max_tokens,
            ctx.http_client.clone(),
            ctx.retry_config.clone(),
        )
        .map_err(|e| format!("failed to build minutes provider: {e}"))?;

        let ov = ctx.role_overrides.get("main");
        let options = CompletionOptions {
            max_tokens: Some(ctx.max_tokens),
            temperature: ov.and_then(|o| o.temperature).or(ctx.options.temperature),
            thinking: ov
                .and_then(|o| o.thinking.clone())
                .or_else(|| ctx.options.thinking.clone()),
            response_format: ResponseFormat::Text,
            web_search: None,
            cache: CacheHints::default(),
        };

        let messages = [
            Message::system(
                "You write concise, factual markdown meeting minutes strictly grounded in the provided transcript.",
            ),
            Message::user(prompt),
        ];
        let result = provider.complete(&messages, &[], &options).await;
        UsageRecorder::new(&ctx.layout)
            .record_call(
                &UsageComponent::Session {
                    kind: "room_minutes".to_owned(),
                },
                Some("room_minutes"),
                &*provider,
                &result,
            )
            .await;
        let resp = result.map_err(|e| format!("minutes completion failed: {e}"))?;
        if resp.content.trim().is_empty() {
            return Err("model returned empty meeting minutes".to_owned());
        }
        Ok(resp.content)
    }

    async fn announce_user_inbox(&self, item_id: &str) {
        self.publisher
            .publish(
                topics::UserInbox,
                UserInboxAddedEvent {
                    item_id: item_id.to_owned(),
                },
            )
            .await
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use serde_json::json;
    use tokio::sync::{Semaphore, mpsc, watch};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use residuum_background::messaging::AgentMessenger;
    use residuum_background::registry::SessionRegistry;
    use residuum_background::store::SessionStore;
    use residuum_background::{
        BackgroundTierActiveIndex, SessionRuntime, SessionRuntimeHandles, spawn_listener,
    };
    use residuum_config::{BackgroundConfig, ModelCatalog, WorkspaceLayout};
    use residuum_core::testing::wait;
    use residuum_inference::{
        CompletionOptions, HttpClientConfig, SharedHttpClient, UsageRecorder,
    };
    use residuum_memory::merge_writer::MemoryMergeWriter;
    use residuum_memory::observer::Observer;
    use residuum_memory::reflector::Reflector;
    use residuum_memory::search::{HybridSearcher, MemoryIndex};
    use residuum_skills::{SkillIndex, SkillState};

    use super::*;

    fn mock_provider_spec(url: &str) -> residuum_config::ProviderSpec {
        residuum_config::ProviderSpec {
            name: "mock".to_owned(),
            model: residuum_config::ModelSpec {
                kind: residuum_config::ProviderKind::OpenAi,
                model: "main-test-model".to_owned(),
            },
            provider_url: url.to_owned(),
            api_key: Some("test-key".to_owned()),
            keep_alive: None,
            session_affinity: None,
            image_input: residuum_config::ImageInput::Undeclared,
        }
    }

    struct RoomTestHarness {
        _dir: tempfile::TempDir,
        layout: WorkspaceLayout,
        participant: GatewayRoomParticipant,
        listener_handle: tokio::task::JoinHandle<()>,
    }

    struct TestInfra {
        hub_root: PathBuf,
        layout: WorkspaceLayout,
        publisher: Publisher,
        registry: Arc<SessionRegistry>,
        messenger: Arc<AgentMessenger>,
        searcher: Arc<HybridSearcher>,
        tracing_service: Arc<residuum_tracing_service::TracingService>,
        client_context: Arc<residuum_tracing_service::ClientContext>,
    }

    async fn build_test_tool_factory(infra: &TestInfra) -> Arc<residuum_tools::SessionTools> {
        let a2a_hub = residuum_a2a::A2aClientHub::new_shared();
        Arc::new(residuum_tools::SessionTools {
            path_policy: residuum_tools::PathPolicy::new_shared(),
            tools_path: Arc::new(tokio::sync::RwLock::new(None)),
            agent_keys: residuum_config::agent_keys::AgentKeys::new_shared(&infra.hub_root),
            checkpoints: residuum_checkpoints::testing::test_engine(),
            publisher: infra.publisher.clone(),
            endpoint_registry: residuum_bus::EndpointRegistry::default(),
            session_registry: Arc::clone(&infra.registry),
            action_store: Arc::new(tokio::sync::Mutex::new(
                residuum_proactivity::actions::store::ActionStore::new_empty(
                    infra.layout.scheduled_actions_json(),
                ),
            )),
            action_notify: Arc::new(tokio::sync::Notify::new()),
            hybrid_searcher: Arc::clone(&infra.searcher),
            tracing_service: Arc::clone(&infra.tracing_service),
            tracing_client_context: Arc::clone(&infra.client_context),
            build: residuum_core::testing::HARNESS_BUILD,
            web_search_backend: None,
            model_catalog: Arc::new(tokio::sync::RwLock::new(ModelCatalog::default())),
            a2a_hub: Arc::clone(&a2a_hub),
            a2a_tracker: residuum_a2a::RemoteTaskTracker::load(
                infra.layout.a2a_outbound_json(),
                a2a_hub,
                Arc::clone(&infra.messenger),
                infra.layout.agent_inbox_dir(),
                chrono_tz::UTC,
            )
            .await,
            lifecycle: residuum_tools::LifecycleAccess::new(
                residuum_background::team::DirectoryHandle::unbound(),
                "test-agent",
            ),
            messenger: Arc::clone(&infra.messenger),
            layout: infra.layout.clone(),
            config_dir: infra.hub_root.clone(),
            hub_dir: infra.hub_root.clone(),
            tz: chrono_tz::UTC,
            depth_cap: 3,
            auto_mode: Some(residuum_agent::auto_mode::AutoModeGate::new_shared(
                "test-agent",
                residuum_config::AutoModeConfig::default(),
                residuum_inference::system_one::SystemOneService::new(None),
                UsageRecorder::new(&infra.layout),
            )),
            mailboxes: None,
            room_view: None,
        })
    }

    fn test_client_context() -> Arc<residuum_tracing_service::ClientContext> {
        Arc::new(residuum_tracing_service::ClientContext {
            version: "test".to_owned(),
            commit: None,
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            model_provider: None,
            model_name: None,
            active_subagents: Vec::new(),
            config_flags: BTreeMap::default(),
            agent: None,
        })
    }

    async fn init_test_layout() -> (tempfile::TempDir, WorkspaceLayout) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("agent");
        let layout = WorkspaceLayout::new(&root);
        let team = layout.team();
        tokio::fs::create_dir_all(layout.root())
            .await
            .expect("root");
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .expect("mem");
        tokio::fs::create_dir_all(team.wiki_dir())
            .await
            .expect("wiki");
        tokio::fs::write(layout.soul_md(), "You are Test Agent.")
            .await
            .expect("soul");
        tokio::fs::write(team.agents_md(), "Rules.")
            .await
            .expect("agents");
        (dir, layout)
    }

    async fn build_room_harness(model_url: &str) -> RoomTestHarness {
        let (dir, layout) = init_test_layout().await;

        let bus = residuum_bus::spawn_broker();
        let publisher = bus.publisher();
        let registry = Arc::new(SessionRegistry::load(layout.resume_points_json()).await);
        let store = Arc::new(SessionStore::new(layout.sessions_dir()));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            publisher.clone(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits::from(&BackgroundConfig::default()),
        ));
        let checkpoints = residuum_checkpoints::testing::test_engine();
        let (_, span_buf) = residuum_core::util::telemetry::SpanBufferLayer::new(
            &residuum_core::util::telemetry::SpanBufferConfig::default(),
        );
        let tracing_service = Arc::new(residuum_tracing_service::TracingService::new(
            residuum_config::TracingConfig::default(),
            span_buf,
        ));
        let client_context = test_client_context();
        let session_runtime = Arc::new(SessionRuntime::new(
            Arc::clone(&registry),
            Arc::clone(&store),
            Arc::new(Semaphore::new(4)),
            &BackgroundConfig::default(),
            SessionRuntimeHandles {
                publisher: publisher.clone(),
                tz: chrono_tz::UTC,
                messenger: Arc::clone(&messenger),
                checkpoints,
                tracing_service: Arc::clone(&tracing_service),
                tracing_client_context: Arc::clone(&client_context),
            },
        ));
        let search_index = Arc::new(MemoryIndex::empty().expect("mem index"));
        let searcher = Arc::new(HybridSearcher::new(
            Arc::clone(&search_index),
            None,
            None,
            residuum_config::SearchConfig::default(),
        ));
        let merge_writer = Arc::new(MemoryMergeWriter::new(
            Reflector::disabled(chrono_tz::UTC),
            layout.clone(),
            search_index,
            None,
            None,
        ));
        let tool_factory = build_test_tool_factory(&TestInfra {
            hub_root: dir.path().to_path_buf(),
            layout: layout.clone(),
            publisher: publisher.clone(),
            registry: Arc::clone(&registry),
            messenger: Arc::clone(&messenger),
            searcher,
            tracing_service,
            client_context,
        })
        .await;

        let spawn_context = build_test_spawn_context(
            model_url,
            &layout,
            tool_factory,
            session_runtime,
            &registry,
            &publisher,
            merge_writer,
            &messenger,
        );
        let listener_handle = spawn_listener(Arc::clone(&spawn_context), &bus)
            .await
            .expect("spawn listener");
        let (_tx, spawn_context_rx) = watch::channel(spawn_context);
        let participant = GatewayRoomParticipant {
            agent_messenger: messenger,
            session_registry: registry,
            bus_handle: bus,
            publisher,
            spawn_context_rx,
            rooms_root: dir.path().join("rooms"),
            tz: chrono_tz::UTC,
        };
        RoomTestHarness {
            _dir: dir,
            layout,
            participant,
            listener_handle,
        }
    }

    #[expect(clippy::too_many_arguments, reason = "test harness helper")]
    fn build_test_spawn_context(
        model_url: &str,
        layout: &WorkspaceLayout,
        tool_factory: Arc<residuum_tools::SessionTools>,
        session_runtime: Arc<SessionRuntime>,
        registry: &Arc<SessionRegistry>,
        publisher: &Publisher,
        merge_writer: Arc<MemoryMergeWriter>,
        messenger: &Arc<AgentMessenger>,
    ) -> Arc<SpawnContext> {
        Arc::new(SpawnContext {
            background_config: BackgroundConfig::default(),
            main_provider_specs: vec![mock_provider_spec(model_url)],
            model_catalog: Arc::new(tokio::sync::RwLock::new(ModelCatalog::default())),
            http_client: SharedHttpClient::new(&HttpClientConfig::default()).expect("http client"),
            max_tokens: 1024,
            retry_config: residuum_config::RetryConfig::default(),
            options: CompletionOptions::default(),
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
            layout: layout.clone(),
            role_overrides: HashMap::new(),
            session_tools: tool_factory,
            session_runtime,
            session_registry: Arc::clone(registry),
            publisher: publisher.clone(),
            skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
            mcp_registry: residuum_mcp::McpRegistry::new_shared(),
            observer: Arc::new(Observer::disabled(chrono_tz::UTC)),
            merge_writer,
            messenger: Arc::clone(messenger),
            bg_tier_active_index: BackgroundTierActiveIndex::default(),
            room_roster: None,
        })
    }

    #[tokio::test]
    async fn room_turn_runs_in_isolated_session_without_touching_recent_messages() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "role": "assistant", "content": "Room reply on main model." } }]
            })))
            .mount(&server)
            .await;

        let h = build_room_harness(&server.uri()).await;
        tokio::fs::write(h.layout.recent_messages_json(), b"[{\"sentinel\":1}]")
            .await
            .expect("write sentinel");

        let (tool_tx, _tool_rx) = mpsc::unbounded_channel();
        let output = h
            .participant
            .run_room_turn(LocalRoomTurnRequest {
                room_id: "room_01jcv8k9m2p4q7".to_owned(),
                room_name: "Arch Sync".to_owned(),
                room_topic: Some("Isolation".to_owned()),
                prompt: "[Owner]: Status?".to_owned(),
                attachments: Vec::new(),
                tool_status_tx: tool_tx,
            })
            .await
            .expect("room turn succeeds");

        assert_eq!(output.content, "Room reply on main model.");
        let live = h.participant.session_registry.list_live();
        assert!(
            live.iter().any(|s| {
                s.address == room_session_address("room_01jcv8k9m2p4q7")
                    && s.model == ModelSelector::MAIN
            }),
            "room turn must run at external-room-<id> on ModelSelector::MAIN: {live:?}"
        );
        let raw_recent = tokio::fs::read_to_string(h.layout.recent_messages_json())
            .await
            .expect("read sentinel");
        assert_eq!(
            raw_recent, "[{\"sentinel\":1}]",
            "1:1 recent_messages.json must never be modified by room turns"
        );

        let reqs = server.received_requests().await.expect("received requests");
        let first_req = reqs.first().expect("one request");
        let req_body: serde_json::Value =
            serde_json::from_slice(&first_req.body).expect("json body");
        assert_eq!(
            req_body.get("model").and_then(serde_json::Value::as_str),
            Some("main-test-model")
        );

        h.participant
            .close_room_session("room_01jcv8k9m2p4q7")
            .await
            .expect("close session");
        wait::until_true("room session completes and clears registry", || {
            h.participant
                .session_registry
                .get(&room_session_address("room_01jcv8k9m2p4q7"))
                .is_none()
        })
        .await;
        h.listener_handle.abort();
    }

    #[tokio::test]
    async fn generate_minutes_calls_main_provider_chain() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "role": "assistant", "content": "# Meeting Minutes\n\n## Summary\nDone." } }]
            })))
            .mount(&server)
            .await;

        let h = build_room_harness(&server.uri()).await;
        let minutes = h
            .participant
            .generate_minutes("Summarize this room.".to_owned())
            .await
            .expect("generate minutes");
        assert!(minutes.contains("## Summary"));
        h.listener_handle.abort();
    }

    #[tokio::test]
    async fn cancel_room_turn_stops_in_flight_turn_without_touching_recent_messages() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let reached_gate = Arc::new(residuum_core::testing::gate::Gate::closed("room-cancel"));
        let gate_for_srv = Arc::clone(&reached_gate);
        let srv = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0_u8; 4096];
                if stream.read(&mut buf).await.unwrap_or(0) > 0 {
                    gate_for_srv.entry().pass().await;
                    let body = r#"{"choices":[{"message":{"role":"assistant","content":"late"}}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(resp.as_bytes()).await.ok();
                }
            }
        });

        let h = build_room_harness(&format!("http://{addr}")).await;
        tokio::fs::write(h.layout.recent_messages_json(), b"[{\"sentinel\":1}]")
            .await
            .expect("write sentinel");

        let participant = Arc::new(h.participant);
        let participant_bg = Arc::clone(&participant);
        let (tool_tx, _tool_rx) = mpsc::unbounded_channel();
        let turn_handle = tokio::spawn(async move {
            participant_bg
                .run_room_turn(LocalRoomTurnRequest {
                    room_id: "room_01jcv8k9m2p4q7".to_owned(),
                    room_name: "Cancel Sync".to_owned(),
                    room_topic: None,
                    prompt: "[Owner]: Hold on".to_owned(),
                    attachments: Vec::new(),
                    tool_status_tx: tool_tx,
                })
                .await
        });

        reached_gate.until_held(1).await;
        participant
            .cancel_room_turn("room_01jcv8k9m2p4q7")
            .await
            .expect("cancel_room_turn succeeds");
        let res = wait::guarded("canceled room turn returns", turn_handle)
            .await
            .expect("join turn");
        assert!(res.is_err(), "canceled room turn must return Err: {res:?}");
        assert_eq!(
            tokio::fs::read_to_string(h.layout.recent_messages_json())
                .await
                .expect("read sentinel"),
            "[{\"sentinel\":1}]"
        );
        reached_gate.open_all();
        srv.abort();
        h.listener_handle.abort();
    }
}
