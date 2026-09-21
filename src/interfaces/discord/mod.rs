//! Discord interface adapter (DM-only).
//!
//! Implements the serenity `EventHandler` trait to receive DMs and publish them
//! onto the bus for agent processing.
//!
//! - The owner is whoever first DMs the bot; others are refused unless
//!   `[discord] respond_to_others` is on.
//! - Replies go back to the DM a message came from; proactive output
//!   (scheduled results, `send_message`), notices, and errors go to the owner.
//!
//! Supports:
//! - Slash commands from the shared command registry (owner only)
//! - Attachment downloading to the workspace inbox

mod handler;
pub(crate) mod subscriber;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use serenity::model::id::ChannelId;
use serenity::prelude::*;

use crate::bus::EndpointName;
use crate::config::DiscordConfig;
use crate::gateway::event_loop::AdapterSenders;
use crate::interfaces::chat_state::{ChatRef, ChatStateStore};
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
}

impl DiscordState {
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

/// Discord interface adapter that routes DMs to the agent inbound channel.
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
    /// received, or an error occurs.
    ///
    /// # Errors
    /// Returns an error if the saved Discord state cannot be loaded, the bus
    /// subscription fails, the serenity client cannot be built, or the
    /// connection fails.
    pub(crate) async fn start(self) -> anyhow::Result<()> {
        let intents = GatewayIntents::DIRECT_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
        let layout = crate::workspace::layout::WorkspaceLayout::new(&self.workspace_dir);

        let state = Arc::new(DiscordState {
            respond_to_others: self.cfg.respond_to_others,
            store: ChatStateStore::load(layout.discord_state_json()).await?,
            reply_targets: ReplyTargets::default(),
        });
        let subs = crate::interfaces::BaseSubscribers::new(
            &self.senders.bus_handle,
            EndpointName::from(ENDPOINT),
        )
        .await
        .context("failed to subscribe to discord bus topics")?;

        let handler = DiscordHandler {
            state: Arc::clone(&state),
            publisher: self.senders.publisher,
            inbox_dir: layout.agent_inbox_dir(),
            reload_tx: self.senders.reload,
            command_tx: self.senders.command,
            stop_tx: self.senders.stop,
            tz: self.tz,
        };

        let mut client = Client::builder(&self.cfg.token, intents)
            .event_handler(handler)
            .await
            .context("failed to build the discord client")?;

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
