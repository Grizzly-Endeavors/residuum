mod channels;
mod handler;
pub(crate) mod subscriber;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use anyhow::Context as _;
use serenity::model::id::{ChannelId, UserId};
use serenity::prelude::*;

use crate::AdapterSenders;
use crate::chat_state::{ChatRef, ChatStateStore};
use crate::context_buffer::ContextBuffer;
use crate::reply_targets::ReplyTargets;
use residuum_bus::{EndpointName, Publisher};
use residuum_config::DiscordConfig;

use self::handler::DiscordHandler;

const ENDPOINT: &str = "discord";

pub(super) struct DiscordState {
    respond_to_others: bool,
    deliver_intermediate: bool,
    store: ChatStateStore<ChatRef>,
    reply_targets: ReplyTargets<ChannelId>,
    bot_id: OnceLock<UserId>,
    channel_labels: Mutex<HashMap<ChannelId, String>>,
    context_buffer: ContextBuffer,
    publisher: Publisher,
}

impl DiscordState {
    fn labels(&self) -> MutexGuard<'_, HashMap<ChannelId, String>> {
        self.channel_labels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn cached_label(&self, channel_id: ChannelId) -> Option<String> {
        self.labels().get(&channel_id).cloned()
    }

    fn cache_label(&self, channel_id: ChannelId, label: String) {
        self.labels().insert(channel_id, label);
    }

    async fn target_for(&self, correlation_id: &str) -> Option<ChannelId> {
        match self.reply_targets.get(correlation_id) {
            Some(channel) => Some(channel),
            None => self.owner_dm().await,
        }
    }

    async fn owner_dm(&self) -> Option<ChannelId> {
        let owner = self.store.owner().await?;
        match owner.dm_conversation_id.parse::<u64>() {
            Ok(id) if id != 0 => Some(ChannelId::new(id)),
            _ => {
                tracing::error!(
                    channel = %owner.dm_conversation_id,
                    "saved discord owner DM channel is not a valid channel ID"
                );
                None
            }
        }
    }
}

pub struct DiscordInterface {
    cfg: DiscordConfig,
    senders: AdapterSenders,
    workspace_dir: PathBuf,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl DiscordInterface {
    #[must_use]
    pub fn new(
        cfg: DiscordConfig,
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

    pub async fn start(self) -> anyhow::Result<()> {
        let intents = GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::GUILDS
            | GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;
        let layout = residuum_config::WorkspaceLayout::new(&self.workspace_dir);

        let (chat_store, chat_state_notice) =
            ChatStateStore::load(layout.discord_state_json()).await?;
        if let Some(notice) = chat_state_notice {
            residuum_bus::notice::publish_notice(&self.senders.publisher, notice).await;
        }
        let state = Arc::new(DiscordState {
            respond_to_others: self.cfg.respond_to_others,
            deliver_intermediate: self.cfg.deliver_intermediate,
            store: chat_store,
            reply_targets: ReplyTargets::default(),
            bot_id: OnceLock::new(),
            channel_labels: Mutex::new(HashMap::new()),
            context_buffer: ContextBuffer::new(self.cfg.context_messages),
            publisher: self.senders.publisher.clone(),
        });
        let subs =
            crate::BaseSubscribers::new(&self.senders.bus_handle, EndpointName::from(ENDPOINT))
                .await
                .context("failed to subscribe to discord bus topics")?;

        let handler = DiscordHandler {
            state: Arc::clone(&state),
            publisher: self.senders.publisher.clone(),
            inbox_dir: layout.agent_inbox_dir(),
            main_conversation: layout.recent_messages_json(),
            reload_tx: self.senders.reload,
            command_tx: self.senders.command,
            stop_tx: self.senders.stop,
            session_registry: self.senders.session_registry,
            tz: self.tz,
        };

        let mut connect_shutdown_rx = self.shutdown_rx.clone();
        let mut client = tokio::select! {
            result = crate::boot_retry::retry_connect(&self.senders.publisher, "Discord", || {
                Client::builder(&self.cfg.token, intents).event_handler(handler.clone())
            }) => {
                match result {
                    Some(client) => client,
                    None => {
                        anyhow::bail!("discord client could not be built after repeated retries")
                    }
                }
            }
            _ = connect_shutdown_rx.changed() => {
                tracing::info!("discord adapter received shutdown signal while connecting");
                return Ok(());
            }
        };

        let _registration = self.senders.conversations.register(
            ENDPOINT,
            Arc::new(channels::DiscordConversations {
                state: Arc::clone(&state),
                http: Arc::clone(&client.http),
            }),
        );
        let outbound = residuum_core::util::spawn_in_span(subscriber::run_discord_subscriber(
            subs,
            Arc::clone(&client.http),
            state,
        ));

        let shard_manager = Arc::clone(&client.shard_manager);
        let mut shutdown_rx = self.shutdown_rx;
        residuum_core::util::spawn_in_span(async move {
            if shutdown_rx.wait_for(|v| *v).await.is_ok() {
                tracing::info!("discord adapter received shutdown signal");
                shard_manager.shutdown_all().await;
            }
        });

        let result = client.start().await;
        outbound.abort();
        result.context("discord connection failed")
    }
}
