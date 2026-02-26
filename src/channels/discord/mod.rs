//! Discord channel adapter (DM-only).
//!
//! Feature-gated behind `--features discord`. Implements the serenity `EventHandler`
//! trait to receive DMs and route them through the standard `RoutedMessage` pipeline.
//!
//! Supports:
//! - Hot-reloadable presence via `PRESENCE.toml`
//! - Slash commands mirroring the CLI command set
//! - Attachment downloading to the workspace inbox

mod handler;
mod reply;

use std::path::PathBuf;

use serenity::prelude::*;
use tokio::sync::mpsc;

use crate::channels::types::RoutedMessage;
use crate::config::DiscordConfig;
use crate::gateway::server::ServerCommand;

use self::handler::DiscordHandler;

/// Discord channel adapter that routes DMs to the agent inbound channel.
pub struct DiscordChannel {
    cfg: DiscordConfig,
    inbound_tx: mpsc::Sender<RoutedMessage>,
    workspace_dir: PathBuf,
    reload_sender: tokio::sync::watch::Sender<bool>,
    command_tx: mpsc::Sender<ServerCommand>,
    tz: chrono_tz::Tz,
}

impl DiscordChannel {
    /// Create a new Discord channel adapter.
    ///
    /// # Arguments
    /// - `cfg`: Discord bot configuration (token).
    /// - `inbound_tx`: Channel for routing messages to the agent.
    /// - `workspace_dir`: Path to the workspace root (for PRESENCE.toml and inbox).
    /// - `reload_sender`: Watch channel to trigger config reload.
    /// - `command_tx`: Channel for dispatching named server commands.
    /// - `tz`: Timezone for inbox item timestamps.
    #[must_use]
    pub fn new(
        cfg: DiscordConfig,
        inbound_tx: mpsc::Sender<RoutedMessage>,
        workspace_dir: PathBuf,
        reload_sender: tokio::sync::watch::Sender<bool>,
        command_tx: mpsc::Sender<ServerCommand>,
        tz: chrono_tz::Tz,
    ) -> Self {
        Self {
            cfg,
            inbound_tx,
            workspace_dir,
            reload_sender,
            command_tx,
            tz,
        }
    }

    /// Start the Discord gateway connection.
    ///
    /// This blocks until the connection is closed or an error occurs.
    ///
    /// # Errors
    /// Returns an error if the serenity client cannot be built or the connection fails.
    pub async fn start(self) -> Result<(), serenity::Error> {
        let intents = GatewayIntents::DIRECT_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

        let presence_path = self.workspace_dir.join("PRESENCE.toml");
        let inbox_dir = self.workspace_dir.join("inbox");

        let handler = DiscordHandler {
            inbound_tx: self.inbound_tx,
            presence_path,
            inbox_dir,
            reload_sender: self.reload_sender,
            command_tx: self.command_tx,
            tz: self.tz,
        };

        let mut client = Client::builder(&self.cfg.token, intents)
            .event_handler(handler)
            .await?;

        client.start().await
    }
}

#[cfg(test)]
mod tests {
    use super::handler::{help_text, status_text};
    use super::*;

    #[test]
    fn help_text_contains_commands() {
        let text = help_text();
        assert!(text.contains("/help"), "should mention /help");
        assert!(text.contains("/status"), "should mention /status");
        assert!(text.contains("/reload"), "should mention /reload");
        assert!(text.contains("/observe"), "should mention /observe");
        assert!(text.contains("/reflect"), "should mention /reflect");
    }

    #[test]
    fn status_text_contains_version() {
        let text = status_text();
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "should contain package version"
        );
        assert!(text.contains("Online"), "should show online status");
    }

    #[test]
    fn slash_command_names() {
        // Platform-specific commands always present
        let platform_cmds = ["help", "status", "reload"];
        for name in platform_cmds {
            let text = match name {
                "help" => help_text(),
                "status" => status_text(),
                "reload" => "Reloading configuration...".to_string(),
                _ => "Unknown".to_string(),
            };
            assert!(
                !text.contains("Unknown"),
                "command '{name}' should have a known handler"
            );
        }

        // Server commands derived from shared registry
        let server_cmds: Vec<_> = crate::channels::cli::commands::server_commands().collect();
        assert!(
            !server_cmds.is_empty(),
            "should have at least one server command"
        );
        for info in &server_cmds {
            assert!(
                !info.name.is_empty(),
                "server command name should not be empty"
            );
        }
    }
}
