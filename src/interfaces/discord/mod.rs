//! Discord interface adapter (DM-only).
//!
//! Implements the serenity `EventHandler` trait to receive DMs and publish them
//! onto the bus for agent processing.
//!
//! Supports:
//! - Hot-reloadable presence via `PRESENCE.toml`
//! - Slash commands mirroring the CLI command set
//! - Attachment downloading to the workspace inbox

mod handler;
mod presence;
pub(crate) mod subscriber;

use std::path::PathBuf;
use std::sync::Arc;

use serenity::prelude::*;

use crate::config::DiscordConfig;
use crate::gateway::event_loop::AdapterSenders;

use self::handler::DiscordHandler;

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
    /// Returns an error if the serenity client cannot be built or the connection fails.
    pub(crate) async fn start(self) -> Result<(), serenity::Error> {
        let intents = GatewayIntents::DIRECT_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

        let presence_path = self.workspace_dir.join("PRESENCE.toml");
        let inbox_dir = self.workspace_dir.join("inbox");

        let channel_id = Arc::new(tokio::sync::Mutex::new(None));

        let handler = DiscordHandler {
            publisher: self.senders.publisher,
            bus_handle: self.senders.bus_handle,
            channel_id,
            presence_path,
            inbox_dir,
            reload_tx: self.senders.reload,
            command_tx: self.senders.command,
            tz: self.tz,
            shutdown_rx: self.shutdown_rx.clone(),
        };

        let mut client = Client::builder(&self.cfg.token, intents)
            .event_handler(handler)
            .await?;

        // Monitor shutdown signal and cleanly disconnect shards
        let shard_manager = Arc::clone(&client.shard_manager);
        let mut shutdown_rx = self.shutdown_rx;
        tokio::spawn(async move {
            if shutdown_rx.wait_for(|v| *v).await.is_ok() {
                tracing::info!("discord adapter received shutdown signal");
                shard_manager.shutdown_all().await;
            }
        });

        client.start().await
    }
}
