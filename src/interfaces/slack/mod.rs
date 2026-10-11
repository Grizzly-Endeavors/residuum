//! Slack interface — Socket Mode, DM-only MVP (design: docs/design/slack-interface.md).
//!
//! One interface instance per agent, each with its own app tokens and socket,
//! mirroring the Telegram adapter's shape. Inbound DM events deliver into the
//! agent's main conversation; outbound replies ride the conversation-only
//! contract shared by every external chat interface.
pub(crate) mod api;
pub(crate) mod events;
pub(crate) mod format;
pub(crate) mod handler;
pub(crate) mod socket;
pub(crate) mod subscriber;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;

use crate::bus::Publisher;
use crate::config::SlackConfig;
use crate::gateway::event_loop::AdapterSenders;
use crate::interfaces::chat_state::{ChatRef, ChatStateStore};
use crate::interfaces::conversations::{ConversationSource, KnownConversation};
use crate::interfaces::reply_targets::ReplyTargets;

/// Endpoint and interface name for Slack on the bus. Telegram keeps its
/// equivalent in `telegram/mod.rs`; hoisted here from `handler.rs` once the
/// outbound subscriber (plan Task 6) needed it too.
pub(crate) const ENDPOINT: &str = "slack";

/// Minimum gap between `chat.postMessage` attempts. Slack's slowest Web API
/// budget is roughly one call per second per method; household DM volume
/// never approaches that, and 1 s keeps a chunked reply streaming naturally.
const POST_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// State shared by the inbound handler and the outbound subscriber —
/// `telegram/mod.rs`'s `TelegramState`, minus the group machinery Slack's
/// DM-only MVP does not have (context buffering, group membership).
pub(crate) struct SlackState {
    /// The owner and every DM channel the bot has seen, keyed by channel id,
    /// persisted in `slack_state.json`.
    pub(crate) store: ChatStateStore<ChatRef>,
    /// Channel each in-flight turn should answer in, by correlation id,
    /// recorded by the handler at dispatch (`telegram/handler.rs`'s
    /// `reply_targets.track`).
    pub(crate) reply_targets: ReplyTargets<String>,
    /// For notifying main when output cannot be delivered (see
    /// `interfaces/outbound.rs`).
    pub(crate) publisher: Publisher,
}

impl SlackState {
    /// Where output for `correlation_id` goes: the channel that started the
    /// turn, or the owner's DM for proactive output — Telegram's
    /// `target_for` verbatim. Like Telegram, Slack does not parse the
    /// correlation at reply time; the handler recorded the mapping when the
    /// message was dispatched, and only the proactive fallback resolves here.
    pub(crate) async fn target_for(&self, correlation_id: &str) -> Option<String> {
        match self.reply_targets.get(correlation_id) {
            Some(channel) => Some(channel),
            None => self.owner_dm().await,
        }
    }

    /// The owner's DM channel, or `None` before any owner is bound. Unlike
    /// Telegram's chat ids there is nothing to parse — a Slack channel id is
    /// already the string `post_message` takes.
    pub(crate) async fn owner_dm(&self) -> Option<String> {
        self.store
            .owner()
            .await
            .map(|owner| owner.dm_conversation_id)
    }
}

#[async_trait::async_trait]
impl ConversationSource for SlackState {
    async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>> {
        Ok(self.store.known_conversations().await)
    }

    async fn owner_dm_conversation_id(&self) -> Option<String> {
        self.store.owner().await.map(|o| o.dm_conversation_id)
    }
}

/// Slack interface adapter: Socket Mode inbound, `chat.postMessage` outbound.
///
/// [`SlackInterface::start`] builds the whole pipeline — shared
/// [`SlackState`], inbound handler task, rate-limited sender, bus subscriber,
/// socket task — exactly the way [`crate::interfaces::telegram`]'s
/// `TelegramInterface::start` does, and tears it all down when it returns.
pub struct SlackInterface {
    cfg: SlackConfig,
    senders: AdapterSenders,
    workspace_dir: PathBuf,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl SlackInterface {
    /// Create a new Slack interface adapter.
    #[must_use]
    pub(crate) fn new(
        cfg: SlackConfig,
        senders: AdapterSenders,
        workspace_dir: PathBuf,
        tz: chrono_tz::Tz,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            cfg,
            senders,
            workspace_dir,
            tz,
            shutdown_rx,
        }
    }

    /// Start the Slack interface: Socket Mode client plus outbound
    /// subscriber, running until a shutdown signal arrives.
    ///
    /// A corrupt saved `slack_state.json` is moved aside and started fresh
    /// rather than treated as an error — surfaced as a notice through the
    /// same path Telegram uses (see [`ChatStateStore::load`]). Verifying the
    /// app token can open a Socket Mode connection retries with backoff
    /// instead of failing immediately — see
    /// [`crate::interfaces::boot_retry`].
    ///
    /// # Errors
    /// Returns an error if the saved state cannot be read (a permissions
    /// problem, not corrupt content), the app token still can't connect
    /// after retrying, or the bus subscription fails.
    pub(crate) async fn start(self) -> anyhow::Result<()> {
        let api =
            api::ReqwestSlackApi::new(self.cfg.bot_token.clone(), self.cfg.app_token.clone())?;
        self.start_with_api(api).await
    }

    /// [`Self::start`] with the Web API client injected — the seam the
    /// end-to-end test drives the real pipeline through, everything but
    /// live Slack.
    async fn start_with_api<A>(self, api: A) -> anyhow::Result<()>
    where
        A: api::SlackApi + Clone + 'static,
    {
        let layout = crate::workspace::layout::WorkspaceLayout::new(&self.workspace_dir);
        let (chat_store, chat_state_notice) =
            ChatStateStore::load(layout.slack_state_json()).await?;
        if let Some(notice) = chat_state_notice {
            crate::gateway::helpers::publish_notice(&self.senders.publisher, notice).await;
        }
        // The one state object both directions share: the handler records
        // conversations and correlation→channel mappings here, and the
        // subscriber reads them back when a turn's output arrives.
        let state = Arc::new(SlackState {
            store: chat_store,
            reply_targets: ReplyTargets::default(),
            publisher: self.senders.publisher.clone(),
        });
        let _registration = self
            .senders
            .conversations
            .register(ENDPOINT, Arc::clone(&state) as Arc<dyn ConversationSource>);

        // Verify the app token before entering the socket loop. A network
        // blip or a momentary API error here used to leave the adapter dead
        // until a config reload; retry with backoff instead, and let a
        // shutdown signal cut retries short. The URL this probe opens is
        // discarded — `run_socket` opens a fresh connection per cycle, the
        // same way Telegram's `get_me` verifies without consuming updates.
        let connect_api = api.clone();
        let mut connect_shutdown_rx = self.shutdown_rx.clone();
        tokio::select! {
            result = crate::interfaces::boot_retry::retry_connect(
                &self.senders.publisher,
                "Slack",
                || connect_api.open_connection(),
            ) => {
                if result.is_none() {
                    anyhow::bail!("slack socket mode connection could not be opened after repeated retries");
                }
            }
            _ = connect_shutdown_rx.changed() => {
                tracing::info!("slack adapter received shutdown signal while connecting");
                return Ok(());
            }
        }

        // Inbound: the handler task owns dedup, owner binding, and bus
        // dispatch; the socket forwards each acked envelope into its channel.
        let inbound =
            handler::SlackInbound::spawn(self.cfg.respond_to_others, Arc::clone(&state), self.tz);
        // Outbound: the rate-limited sender serializes posts; the subscriber
        // translates bus events through it.
        let subs = crate::interfaces::BaseSubscribers::new(
            &self.senders.bus_handle,
            crate::bus::EndpointName::from(ENDPOINT),
        )
        .await
        .context("failed to subscribe to slack bus topics")?;
        let sender = Arc::new(api::RateLimitedSender {
            api: api.clone(),
            min_interval: POST_MIN_INTERVAL,
            last: tokio::sync::Mutex::new(None),
        });
        let _outbound = AbortOnDrop(crate::util::spawn_in_span(
            subscriber::run_slack_subscriber(subs, Arc::clone(&sender), Arc::clone(&state)),
        ));

        // The socket task reconnects forever on its own; start() stays alive
        // to hold the teardown guards until shutdown, surfacing an exit if
        // the socket task ever dies instead.
        let mut socket = AbortOnDrop(crate::util::spawn_in_span(socket::run_socket(
            api,
            inbound.forwarder(),
        )));
        let mut shutdown_rx = self.shutdown_rx;
        tokio::select! {
            _ = shutdown_rx.wait_for(|v| *v) => {
                tracing::info!("slack adapter received shutdown signal");
            }
            result = &mut socket.0 => {
                match result {
                    // run_socket only returns on a failed client build; it
                    // reconnects through everything else.
                    Ok(Ok(())) => tracing::info!("slack socket task exited"),
                    Ok(Err(e)) => return Err(e.context("slack socket task failed")),
                    Err(e) => return Err(anyhow::Error::new(e).context("slack socket task panicked")),
                }
            }
        }
        Ok(())
    }
}

/// Aborts the wrapped task when dropped — `telegram/handler.rs`'s
/// `OutboundTask` pattern. The interface's socket and subscriber tasks both
/// die with `start`, so a stopped or restarted adapter never leaves a second
/// one running beside its replacement.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end: the real wired interface against a loopback fake Slack.
    //!
    //! Only the Web API boundary is faked — a `SlackApi` whose
    //! `open_connection` hands out the fake's ws URL and whose
    //! `post_message` records its calls, mirroring `api.rs`'s tests. The
    //! socket, ack POSTs, handler, dedup, owner binding, state file,
    //! subscriber, chunker, and mrkdwn converter all run for real.
    use std::sync::Arc;

    use axum::Router;
    use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
    use axum::extract::{Path, State};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use crate::background::registry::SessionRegistry;
    use crate::bus::{
        EndpointName, MessageEvent, NoticeEvent, NotifyName, ResponseEvent, SYSTEM_CHANNEL,
        Subscriber, topics,
    };
    use crate::config::SlackConfig;
    use crate::gateway::event_loop::AdapterSenders;
    use crate::gateway::types::GatewayCore;
    use crate::interfaces::chunking::chunk_text;
    use crate::testing::wait;

    use super::ENDPOINT;
    use super::api::{SlackApi, SlackApiError};
    use super::subscriber::SLACK_MAX_CHARS;

    /// What the fake Slack surfaces to the driver.
    #[derive(Debug, Clone)]
    enum Observed {
        /// A Socket Mode ws connection was accepted; the driver scripts it.
        Connected {
            cmds: mpsc::UnboundedSender<ServerCmd>,
        },
        /// An ack POST landed on the fake's `/ack/{id}` route.
        Ack { id: String, body: String },
    }

    /// Driver → live fake connection (the `socket.rs` tests' pattern).
    #[derive(Debug)]
    enum ServerCmd {
        /// Send one text frame (an envelope JSON).
        Text(String),
    }

    #[derive(Clone)]
    struct FakeSlackShared {
        obs: mpsc::UnboundedSender<Observed>,
    }

    /// The fake Socket Mode endpoint: accepts ws connections the driver
    /// scripts and records ack POSTs. It never sends unsolicited frames.
    async fn socket_mode_route(
        upgrade: WebSocketUpgrade,
        State(shared): State<FakeSlackShared>,
    ) -> impl IntoResponse {
        upgrade.on_upgrade(move |mut socket: WebSocket| async move {
            let (cmds, mut cmd_rx) = mpsc::unbounded_channel();
            shared
                .obs
                .send(Observed::Connected { cmds })
                .expect("the driver is still observing");
            loop {
                tokio::select! {
                    command = cmd_rx.recv() => {
                        match command {
                            Some(ServerCmd::Text(text)) => {
                                if socket.send(WsMessage::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    frame = socket.recv() => {
                        match frame {
                            None | Some(Err(_) | Ok(WsMessage::Close(_))) => break,
                            Some(Ok(_)) => {}
                        }
                    }
                }
            }
        })
    }

    async fn ack_route(
        State(shared): State<FakeSlackShared>,
        Path(id): Path<String>,
        body: String,
    ) -> &'static str {
        shared
            .obs
            .send(Observed::Ack { id, body })
            .expect("the driver is still observing");
        "ok"
    }

    /// A running fake Slack on a loopback port.
    struct FakeSlack {
        ws_url: String,
        http_base: String,
        obs_rx: mpsc::UnboundedReceiver<Observed>,
    }

    async fn spawn_fake_slack() -> FakeSlack {
        let (obs_tx, obs_rx) = mpsc::unbounded_channel();
        let router = Router::new()
            .route("/ws", get(socket_mode_route))
            .route("/ack/{id}", post(ack_route))
            .with_state(FakeSlackShared { obs: obs_tx });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake slack");
        let addr = listener.local_addr().expect("fake slack address");
        tokio::spawn(async move {
            axum::serve(listener, router).await.ok();
        });
        FakeSlack {
            ws_url: format!("ws://{addr}/ws"),
            http_base: format!("http://{addr}"),
            obs_rx,
        }
    }

    /// The Web API seam, faked at the fake server's boundaries: Socket Mode
    /// opens hand out the fake's ws URL, `chat.postMessage` records its
    /// calls — `api.rs`'s test fake, pointed at this server.
    #[derive(Clone, Default)]
    struct RecordingApi {
        ws_url: String,
        posted: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    #[async_trait::async_trait]
    impl SlackApi for RecordingApi {
        async fn post_message(&self, channel: &str, text: &str) -> Result<(), SlackApiError> {
            self.posted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((channel.to_string(), text.to_string()));
            Ok(())
        }

        async fn open_connection(&self) -> anyhow::Result<String> {
            Ok(self.ws_url.clone())
        }
    }

    /// Everything one end-to-end run owns. [`EndToEnd::boot`] starts the
    /// real interface; tests may touch the workspace (plant a corrupt state
    /// file) between building the rig and booting it.
    struct EndToEnd {
        api: RecordingApi,
        core: GatewayCore,
        http_base: String,
        user_messages: Subscriber<MessageEvent>,
        notices: Subscriber<NoticeEvent>,
        shutdown_tx: tokio::sync::watch::Sender<bool>,
        interface: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
        obs_rx: mpsc::UnboundedReceiver<Observed>,
        workspace: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    async fn end_to_end() -> EndToEnd {
        let fake = spawn_fake_slack().await;
        let dir = tempfile::tempdir().unwrap();
        let (core, _receivers) =
            GatewayCore::new(dir.path().to_path_buf(), dir.path().to_path_buf());
        let user_messages = core
            .bus_handle
            .subscribe(topics::UserMessage)
            .await
            .unwrap();
        let notices = core
            .bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let api = RecordingApi {
            ws_url: fake.ws_url.clone(),
            ..RecordingApi::default()
        };
        EndToEnd {
            api,
            core,
            http_base: fake.http_base,
            user_messages,
            notices,
            shutdown_tx,
            interface: None,
            obs_rx: fake.obs_rx,
            workspace: dir.path().to_path_buf(),
            _dir: dir,
        }
    }

    impl EndToEnd {
        /// Boot the real interface: real bus, real state file under the
        /// temp workspace, real handler/subscriber/sender — only the
        /// `SlackApi` is the recording fake.
        fn boot(&mut self) {
            let senders = AdapterSenders {
                publisher: self.core.publisher.clone(),
                bus_handle: self.core.bus_handle.clone(),
                reload: self.core.reload_tx.clone(),
                command: self.core.command_tx.clone(),
                stop: self.core.stop_tx.clone(),
                session_registry: Arc::new(SessionRegistry::new()),
                conversations: crate::interfaces::conversations::ConversationDirectory::default(),
            };
            let iface = super::SlackInterface::new(
                SlackConfig {
                    bot_token: "xoxb-e2e".to_string(),
                    app_token: "xapp-e2e".to_string(),
                    respond_to_others: false,
                },
                senders,
                self.workspace.clone(),
                chrono_tz::UTC,
                self.shutdown_tx.subscribe(),
            );
            let api = self.api.clone();
            self.interface = Some(tokio::spawn(async move { iface.start_with_api(api).await }));
        }

        /// Take the interface task out, for teardown joins. The
        /// wait-on-something-while-watching-the-interface helpers below
        /// borrow the task in place instead.
        fn take_interface(&mut self) -> tokio::task::JoinHandle<anyhow::Result<()>> {
            self.interface.take().expect("boot() was called first")
        }

        /// The next ws observation, failing fast if the interface dies.
        /// The task handle is borrowed disjointly from the observation
        /// channel (destructured, not through a method, so the two select
        /// arms can hold both borrows at once).
        async fn next(&mut self, what: &str) -> Observed {
            let Self {
                obs_rx, interface, ..
            } = self;
            let interface = interface.as_mut().expect("boot() was called first");
            tokio::select! {
                biased;
                observed = wait::guarded(what, obs_rx.recv()) => {
                    observed.unwrap_or_else(|| panic!("the fake slack went away while waiting for {what}"))
                }
                result = interface => {
                    panic!("the slack interface exited while waiting for {what}: {result:?}")
                }
            }
        }

        /// The next `UserMessage` published by the real handler, failing
        /// fast if the interface dies first (same disjoint-borrow shape).
        async fn next_user_message(&mut self, what: &str) -> MessageEvent {
            let Self {
                user_messages,
                interface,
                ..
            } = self;
            let interface = interface.as_mut().expect("boot() was called first");
            tokio::select! {
                biased;
                event = wait::guarded(what, user_messages.recv()) => {
                    event
                        .unwrap_or_else(|e| panic!("bus error while waiting for {what}: {e}"))
                        .expect("the bus stays up")
                }
                result = interface => {
                    panic!("the slack interface exited while waiting for {what}: {result:?}")
                }
            }
        }

        /// The next system notice, failing fast if the interface dies first.
        async fn next_notice(&mut self, what: &str) -> NoticeEvent {
            let Self {
                notices, interface, ..
            } = self;
            let interface = interface.as_mut().expect("boot() was called first");
            tokio::select! {
                biased;
                notice = wait::guarded(what, notices.recv()) => {
                    notice
                        .unwrap_or_else(|e| panic!("bus error while waiting for {what}: {e}"))
                        .expect("the bus stays up")
                }
                result = interface => {
                    panic!("the slack interface exited while waiting for {what}: {result:?}")
                }
            }
        }

        async fn connected(&mut self, what: &str) -> mpsc::UnboundedSender<ServerCmd> {
            match self.next(what).await {
                Observed::Connected { cmds } => cmds,
                Observed::Ack { .. } => panic!("expected a socket connection for {what}"),
            }
        }

        async fn ack_post(&mut self, what: &str) -> (String, String) {
            match self.next(what).await {
                Observed::Ack { id, body } => (id, body),
                Observed::Connected { .. } => panic!("expected an ack POST for {what}"),
            }
        }

        /// The events-api fixture rewired: identity set, ack pointed at the
        /// fake's own POST route (the `socket.rs` tests' rewire rule).
        fn dm_envelope(&self, id: &str) -> String {
            let mut envelope: serde_json::Value =
                serde_json::from_str(include_str!("fixtures/envelope-events-api-message.json"))
                    .expect("fixture parses");
            let object = envelope
                .as_object_mut()
                .expect("envelope fixture is an object");
            object.insert(
                "envelope_id".to_string(),
                serde_json::Value::String(id.into()),
            );
            object.insert(
                "response_url".to_string(),
                serde_json::Value::String(format!("{}/ack/{id}", self.http_base)),
            );
            serde_json::to_string(&envelope).expect("rewired envelope serializes")
        }

        /// Recorded `chat.postMessage` calls.
        fn posted(&self) -> Vec<(String, String)> {
            self.api
                .posted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        /// Publish an agent reply for `correlation_id`, as the agent runtime
        /// would, and wait until every chunk of it has been posted.
        async fn reply_and_wait(
            &self,
            correlation_id: &str,
            content: &str,
        ) -> Vec<(String, String)> {
            self.core
                .publisher
                .publish(
                    topics::Endpoint(EndpointName::from(ENDPOINT)),
                    ResponseEvent {
                        correlation_id: correlation_id.to_string(),
                        content: content.to_string(),
                        timestamp: chrono::Utc::now().naive_utc(),
                        attachment: None,
                        conversation: None,
                    },
                )
                .await
                .unwrap();
            let expected = self.posted().len() + chunk_text(content, SLACK_MAX_CHARS).len();
            wait::until_true(format!("{expected} chat.postMessage call(s)"), || {
                self.posted().len() >= expected
            })
            .await;
            self.posted()
        }
    }

    /// The spec's acceptance criteria, executable: a DM arrives over the
    /// socket, is acked, and reaches the bus as a `UserMessage`; an agent
    /// reply published to the endpoint topic lands at Slack as
    /// `chat.postMessage`, mrkdwn-converted, in the channel that started
    /// the turn.
    #[tokio::test]
    async fn a_dm_round_trips_through_the_real_interface() {
        let mut e2e = end_to_end().await;
        e2e.boot();

        let conn = e2e.connected("the socket mode connection").await;
        conn.send(ServerCmd::Text(
            include_str!("fixtures/envelope-hello.json").to_string(),
        ))
        .expect("the scripted connection is open");
        conn.send(ServerCmd::Text(e2e.dm_envelope("env-e2e-1")))
            .expect("the scripted connection is open");

        // Inbound, half one: the ack POST lands at the fake before anything
        // else happens.
        let (ack_id, ack_body) = e2e.ack_post("the ack for the DM envelope").await;
        assert_eq!(ack_id, "env-e2e-1");
        assert_eq!(ack_body, "{}", "the ack body is the empty object");

        // Inbound, half two: the DM is a UserMessage on the bus, correlation
        // identifying channel + ts, owner-bound.
        let event = e2e.next_user_message("the DM published to the bus").await;
        assert_eq!(event.content, "hello from slack");
        assert_eq!(event.id, "slack-D0001-1728558000.000100");
        assert_eq!(event.origin.endpoint, "slack");
        assert!(
            event
                .origin
                .conversation
                .as_ref()
                .expect("conversation context present")
                .is_owner,
            "the first human DM binds the owner"
        );

        // Owner binding ran for real: slack_state.json exists in the
        // workspace the interface was given.
        wait::until_true("the slack state file is written", || {
            e2e.workspace.join("slack_state.json").exists()
        })
        .await;

        // Outbound: an agent reply to that correlation reaches Slack through
        // the subscriber, chunker, and mrkdwn converter.
        let posted = e2e.reply_and_wait(&event.id, "**bold** and `code`").await;
        assert_eq!(
            posted,
            vec![("D0001".to_string(), "*bold* and `code`".to_string())],
            "the reply goes to the DM channel, converted to mrkdwn"
        );

        // A longer reply exercises the chunker through the same pipeline.
        let long = "*emphasis* on a long reply\n\nx".to_string() + &"x".repeat(8600);
        let long_posted = e2e.reply_and_wait(&event.id, &long).await;
        assert!(
            long_posted.len() > 1,
            "a long reply is chunked: {long_posted:?}"
        );
        assert!(
            long_posted.iter().all(|(channel, _)| channel == "D0001"),
            "every chunk goes to the DM channel: {long_posted:?}"
        );
        assert!(
            long_posted
                .iter()
                .all(|(_, text)| text.len() <= SLACK_MAX_CHARS),
            "no chunk exceeds the Slack budget: {long_posted:?}"
        );

        // Teardown: shutdown takes the whole interface down with it.
        e2e.shutdown_tx.send(true).unwrap();
        wait::guarded("the interface exits on shutdown", e2e.take_interface())
            .await
            .expect("the task itself joined")
            .expect("start_with_api returns Ok on shutdown");
    }

    /// A corrupt `slack_state.json` surfaces the same notice Telegram's
    /// corrupt `telegram_state.json` does — moved aside, fresh start, the
    /// owner told — and the interface comes up anyway.
    #[tokio::test]
    async fn a_corrupt_state_file_notices_and_still_boots() {
        let mut e2e = end_to_end().await;
        // Plant the corrupt file before boot: the interface reads it at
        // start, before its first socket connection.
        std::fs::write(e2e.workspace.join("slack_state.json"), "{ not json").unwrap();
        e2e.boot();

        let notice = e2e.next_notice("the corrupt-state notice").await;
        assert!(
            notice.message.contains("corrupt"),
            "the notice names the problem: {}",
            notice.message
        );
        assert!(
            notice.message.contains("slack_state.json"),
            "the notice names the file: {}",
            notice.message
        );

        // The interface still boots: the socket connects on the same run.
        e2e.connected("the socket mode connection despite the corrupt state")
            .await;

        // And the corrupt file was moved aside, not kept in place.
        assert!(
            !e2e.workspace.join("slack_state.json").exists()
                || e2e.workspace.join("slack_state.json.corrupt").exists(),
            "the corrupt file was moved aside for reference"
        );

        e2e.shutdown_tx.send(true).unwrap();
        let exit = wait::guarded("the interface exits on shutdown", e2e.take_interface()).await;
        if let Err(e) = exit {
            panic!("the interface task failed to join on shutdown: {e}");
        }
    }
}
