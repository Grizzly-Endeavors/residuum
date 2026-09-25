//! Discord interface adapter.
//!
//! Implements the serenity `EventHandler` trait to receive messages and
//! publish them onto the bus for agent processing.
//!
//! - Direct messages always reach the agent; in server channels only
//!   messages that @mention the bot do.
//! - The owner is whoever first DMs the bot; others are refused unless
//!   `[discord] respond_to_others` is on.
//! - Replies go back to the channel a message came from; proactive output
//!   goes to the owner's DM unless `send_message` names a conversation.
//!   Notices and errors only ever go to the owner.
//!
//! Supports:
//! - Slash commands from the shared command registry (owner only)
//! - Attachment downloading to the workspace inbox

mod channels;
mod handler;
pub(crate) mod subscriber;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use anyhow::Context as _;
use serenity::model::id::{ChannelId, UserId};
use serenity::prelude::*;

use crate::bus::{EndpointName, Publisher};
use crate::config::DiscordConfig;
use crate::gateway::event_loop::AdapterSenders;
use crate::interfaces::chat_state::{ChatRef, ChatStateStore};
use crate::interfaces::context_buffer::ContextBuffer;
use crate::interfaces::reply_targets::ReplyTargets;

use self::handler::DiscordHandler;

/// Endpoint and interface name for Discord on the bus.
const ENDPOINT: &str = "discord";

/// State shared by the event handler and the outbound subscriber.
pub(super) struct DiscordState {
    respond_to_others: bool,
    /// The owner and every conversation the bot has seen, keyed by channel ID.
    store: ChatStateStore<ChatRef>,
    /// Channel each in-flight turn should answer in, by correlation ID.
    reply_targets: ReplyTargets<ChannelId>,
    /// The bot's own user, learned when the gateway connection is ready.
    bot_id: OnceLock<UserId>,
    /// Server channel labels already looked up, e.g. `"#builds (Eng Team)"`.
    channel_labels: Mutex<HashMap<ChannelId, String>>,
    /// Unmentioned server messages held for the next @mention, by channel.
    context_buffer: ContextBuffer,
    /// For notifying main when a conversation session's output can't be
    /// delivered (see `subscriber::deliver_session_response`).
    publisher: Publisher,
}

impl DiscordState {
    fn labels(&self) -> MutexGuard<'_, HashMap<ChannelId, String>> {
        // Plain map of owned strings; a panic mid-insert cannot corrupt it.
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

    /// Where output for `correlation_id` goes: the channel that started the
    /// turn, or the owner's DM for proactive output.
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

/// Discord interface adapter that routes DMs and server mentions to the agent.
pub struct DiscordInterface {
    cfg: DiscordConfig,
    senders: AdapterSenders,
    workspace_dir: PathBuf,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl DiscordInterface {
    /// Create a new Discord interface adapter.
    #[must_use]
    pub(crate) fn new(
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

    /// Start the Discord gateway connection.
    ///
    /// This blocks until the connection is closed, a shutdown signal is
    /// received, or an error occurs. A corrupt saved Discord state file is
    /// moved aside and started fresh rather than treated as an error (see
    /// [`ChatStateStore::load`]). Building the client retries with backoff
    /// on a transient failure (network not up yet, a momentary API error)
    /// instead of failing immediately — see
    /// [`crate::interfaces::boot_retry`].
    ///
    /// # Errors
    /// Returns an error if the saved Discord state cannot be read (a
    /// permissions problem, not corrupt content), the bus subscription
    /// fails, the serenity client still can't be built after retrying, or
    /// the connection fails.
    pub(crate) async fn start(self) -> anyhow::Result<()> {
        let intents = GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::GUILDS
            | GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;
        let layout = crate::workspace::layout::WorkspaceLayout::new(&self.workspace_dir);

        let (chat_store, chat_state_notice) =
            ChatStateStore::load(layout.discord_state_json()).await?;
        if let Some(notice) = chat_state_notice {
            crate::gateway::helpers::publish_notice(&self.senders.publisher, notice).await;
        }
        let state = Arc::new(DiscordState {
            respond_to_others: self.cfg.respond_to_others,
            store: chat_store,
            reply_targets: ReplyTargets::default(),
            bot_id: OnceLock::new(),
            channel_labels: Mutex::new(HashMap::new()),
            context_buffer: ContextBuffer::new(self.cfg.context_messages),
            publisher: self.senders.publisher.clone(),
        });
        let subs = crate::interfaces::BaseSubscribers::new(
            &self.senders.bus_handle,
            EndpointName::from(ENDPOINT),
        )
        .await
        .context("failed to subscribe to discord bus topics")?;

        let handler = DiscordHandler {
            state: Arc::clone(&state),
            publisher: self.senders.publisher.clone(),
            inbox_dir: layout.agent_inbox_dir(),
            reload_tx: self.senders.reload,
            command_tx: self.senders.command,
            stop_tx: self.senders.stop,
            session_registry: self.senders.session_registry,
            tz: self.tz,
        };

        // Building the client talks to the Discord API to validate the
        // token. A network blip or a momentary API error here used to
        // leave the adapter dead until a config reload; retry with backoff
        // instead, and let a shutdown signal cut retries short.
        let mut connect_shutdown_rx = self.shutdown_rx.clone();
        let mut client = tokio::select! {
            result = crate::interfaces::boot_retry::retry_connect(&self.senders.publisher, "Discord", || {
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
        let outbound = tokio::spawn(subscriber::run_discord_subscriber(
            subs,
            Arc::clone(&client.http),
            state,
        ));

        // Monitor shutdown signal and cleanly disconnect shards
        let shard_manager = Arc::clone(&client.shard_manager);
        let mut shutdown_rx = self.shutdown_rx;
        tokio::spawn(async move {
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
