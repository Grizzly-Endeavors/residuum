//! Microsoft Teams interface.
//!
//! Teams delivers messages as HTTP requests carrying Bot Framework activities to a public
//! HTTPS endpoint, so unlike Discord and Telegram this adapter runs its own
//! small HTTP listener (`[teams] port`, default 7701) instead of dialing out.
//! It is deliberately separate from the gateway port: whatever tunnel makes
//! it public then exposes only this signature-checked endpoint, never the
//! unauthenticated config and secrets API on the gateway.
//!
//! - Direct messages always reach the agent.
//! - In group chats and channels only @mentions do; other messages are held
//!   in memory and handed over as background context on the next mention.
//! - Replies go back to the conversation a message came from; proactive
//!   output (scheduled results, `send_message`) goes to the owner's DM.

mod activity;
mod auth;
mod connector;
mod handler;
mod store;
mod subscriber;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::bus::{EndpointName, Publisher};
use crate::config::TeamsConfig;
use crate::gateway::event_loop::AdapterSenders;
use crate::gateway::types::{ReloadSignal, ServerCommand, StopRequest};
use crate::interfaces::context_buffer::ContextBuffer;
use crate::interfaces::conversations::{ConversationSource, KnownConversation};
use crate::interfaces::reply_targets::ReplyTargets;

use self::auth::TokenValidator;
use self::connector::ConnectorClient;
use self::store::{ConversationRef, TeamsStore};

/// Endpoint and interface name for Teams on the bus.
pub(crate) const ENDPOINT: &str = "teams";
/// Path Microsoft posts activities to; configured as the bot's messaging endpoint.
pub(crate) const MESSAGES_PATH: &str = "/api/teams/messages";
/// Activities waiting for the inbound worker before the endpoint sheds load.
const INBOUND_QUEUE: usize = 64;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// State shared by the HTTP endpoint, the inbound worker, and the outbound subscriber.
pub(super) struct TeamsRuntime {
    cfg: TeamsConfig,
    http: reqwest::Client,
    validator: TokenValidator,
    connector: ConnectorClient,
    store: TeamsStore,
    buffer: ContextBuffer,
    /// Conversation each in-flight turn should answer in, by correlation ID.
    reply_targets: ReplyTargets<ConversationRef>,
    inbound_tx: tokio::sync::mpsc::Sender<activity::Activity>,
    publisher: Publisher,
    reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: tokio::sync::mpsc::Sender<ServerCommand>,
    stop_tx: tokio::sync::mpsc::Sender<StopRequest>,
    inbox_dir: PathBuf,
    tz: chrono_tz::Tz,
}

impl TeamsRuntime {
    /// Where output for `correlation_id` goes: the conversation that started
    /// the turn, or the owner's DM for proactive output.
    async fn target_for(&self, correlation_id: &str) -> Option<ConversationRef> {
        match self.reply_targets.get(correlation_id) {
            Some(target) => Some(target),
            None => self.owner_dm().await,
        }
    }

    async fn owner_dm(&self) -> Option<ConversationRef> {
        let owner = self.store.owner().await?;
        self.store.conversation(&owner.dm_conversation_id).await
    }

    /// Send a markdown message, logging (not propagating) failure.
    async fn send_text(&self, target: &ConversationRef, text: &str) {
        if let Err(e) = self.try_send_text(target, text).await {
            tracing::error!(error = %e, conversation = %target.label, "failed to send teams message");
        }
    }

    /// Send a markdown message in chunks, stopping at the first failure.
    async fn try_send_text(
        &self,
        target: &ConversationRef,
        text: &str,
    ) -> Result<(), connector::ConnectorError> {
        for chunk in crate::interfaces::chunking::chunk_text(text, subscriber::MAX_MESSAGE_BYTES) {
            self.connector
                .send_activity(target, &connector::message_activity(&chunk))
                .await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ConversationSource for TeamsRuntime {
    async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>> {
        Ok(self.store.known_conversations().await)
    }

    async fn owner_dm_conversation_id(&self) -> Option<String> {
        self.store.owner().await.map(|o| o.dm_conversation_id)
    }
}

/// The Teams adapter: messaging listener, inbound worker, and outbound subscriber.
pub struct TeamsInterface {
    cfg: TeamsConfig,
    senders: AdapterSenders,
    bind: String,
    layout: crate::workspace::layout::WorkspaceLayout,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl TeamsInterface {
    /// Create the adapter; `bind` is the gateway's bind address.
    #[must_use]
    pub fn new(
        cfg: TeamsConfig,
        senders: AdapterSenders,
        bind: String,
        workspace_dir: PathBuf,
        tz: chrono_tz::Tz,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            cfg,
            senders,
            bind,
            layout: crate::workspace::layout::WorkspaceLayout::new(workspace_dir),
            tz,
            shutdown_rx,
        }
    }

    /// Run until the shutdown signal fires.
    ///
    /// # Errors
    /// Returns an error if the saved Teams state cannot be loaded, the bus
    /// subscription fails, or the listener port cannot be bound.
    pub async fn start(self) -> anyhow::Result<()> {
        use anyhow::Context as _;

        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .context("failed to build teams http client")?;
        let store = TeamsStore::load(self.layout.teams_state_json()).await?;
        let subs = crate::interfaces::BaseSubscribers::new(
            &self.senders.bus_handle,
            EndpointName::from(ENDPOINT),
        )
        .await
        .context("failed to subscribe to teams bus topics")?;
        let (inbound_tx, mut inbound_rx) = tokio::sync::mpsc::channel(INBOUND_QUEUE);

        let rt = Arc::new(TeamsRuntime {
            validator: TokenValidator::new(http.clone(), self.cfg.app_id.clone()),
            connector: ConnectorClient::new(
                http.clone(),
                &self.cfg.tenant_id,
                self.cfg.app_id.clone(),
                self.cfg.app_password.clone(),
            ),
            buffer: ContextBuffer::new(self.cfg.context_messages),
            http,
            store,
            reply_targets: ReplyTargets::default(),
            inbound_tx,
            publisher: self.senders.publisher,
            reload_tx: self.senders.reload,
            command_tx: self.senders.command,
            stop_tx: self.senders.stop,
            inbox_dir: self.layout.agent_inbox_dir(),
            tz: self.tz,
            cfg: self.cfg,
        });

        let addr = format!("{}:{}", self.bind, rt.cfg.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .with_context(|| format!("failed to bind the teams listener on {addr}"))?;
        tracing::info!(
            addr = %addr,
            path = MESSAGES_PATH,
            respond_to_others = rt.cfg.respond_to_others,
            "teams interface listening"
        );

        let app = axum::Router::new()
            .route(
                MESSAGES_PATH,
                axum::routing::post(handler::messages_endpoint),
            )
            .with_state(Arc::clone(&rt));

        let worker_rt = Arc::clone(&rt);
        let worker = tokio::spawn(async move {
            while let Some(activity) = inbound_rx.recv().await {
                handler::process_activity(&worker_rt, activity).await;
            }
        });
        let outbound = tokio::spawn(subscriber::run_teams_subscriber(Arc::clone(&rt), subs));
        let _registration = self
            .senders
            .conversations
            .register(ENDPOINT, Arc::clone(&rt) as Arc<dyn ConversationSource>);

        let mut shutdown_rx = self.shutdown_rx;
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                // Either a shutdown signal or the sender going away ends the adapter.
                if shutdown_rx.changed().await.is_err() {
                    tracing::debug!("teams shutdown sender dropped; stopping listener");
                }
            })
            .await;

        tracing::info!("teams interface stopped");
        worker.abort();
        outbound.abort();
        served.context("teams listener failed")
    }
}
