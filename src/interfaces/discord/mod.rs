//! Discord interface adapter (DM-only).
//!
//! Implements the serenity `EventHandler` trait to receive DMs and route them
//! through the standard `RoutedMessage` pipeline.
//!
//! Supports:
//! - Hot-reloadable presence via `PRESENCE.toml`
//! - Slash commands mirroring the CLI command set
//! - Attachment downloading to the workspace inbox

mod handler;
mod reply;

use std::path::PathBuf;
use std::sync::Arc;

use serenity::prelude::*;
use tokio::sync::mpsc;

use crate::config::DiscordConfig;
use crate::gateway::types::{ReloadSignal, ServerCommand};
use crate::interfaces::types::RoutedMessage;

use self::handler::DiscordHandler;

/// Discord interface adapter that routes DMs to the agent inbound channel.
pub struct DiscordInterface {
    cfg: DiscordConfig,
    inbound_tx: mpsc::Sender<RoutedMessage>,
    workspace_dir: PathBuf,
    reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: mpsc::Sender<ServerCommand>,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl DiscordInterface {
    /// Create a new Discord interface adapter.
    ///
    /// # Arguments
    /// - `cfg`: Discord bot configuration (token).
    /// - `inbound_tx`: Channel for routing messages to the agent.
    /// - `workspace_dir`: Path to the workspace root (for PRESENCE.toml and inbox).
    /// - `reload_tx`: Watch channel to trigger config reload.
    /// - `command_tx`: Channel for dispatching named server commands.
    /// - `tz`: Timezone for inbox item timestamps.
    /// - `shutdown_rx`: Watch channel signalling graceful shutdown.
    #[must_use]
    pub(crate) fn new(
        cfg: DiscordConfig,
        inbound_tx: mpsc::Sender<RoutedMessage>,
        workspace_dir: PathBuf,
        reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
        command_tx: mpsc::Sender<ServerCommand>,
        tz: chrono_tz::Tz,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            cfg,
            inbound_tx,
            workspace_dir,
            reload_tx,
            command_tx,
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

        let handler = DiscordHandler {
            inbound_tx: self.inbound_tx,
            presence_path,
            inbox_dir,
            reload_tx: self.reload_tx,
            command_tx: self.command_tx,
            tz: self.tz,
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

#[cfg(test)]
mod tests {
    use crate::interfaces::cli::commands::{
        CommandContext, CommandSideEffect, all_commands, execute_command,
    };

    fn discord_ctx() -> CommandContext<'static> {
        CommandContext {
            url: "",
            verbose: false,
            interface_name: "discord",
        }
    }

    #[test]
    fn execute_help_returns_command_list() {
        let result = execute_command("help", None, &discord_ctx());
        assert!(
            result.response.contains("help"),
            "should mention help: {}",
            result.response
        );
        assert!(
            result.response.contains("observe"),
            "should mention observe: {}",
            result.response
        );
        assert!(result.side_effect.is_none(), "help has no side effect");
    }

    #[test]
    fn execute_status_returns_text() {
        let result = execute_command("status", None, &discord_ctx());
        assert!(
            result.response.contains("verbose"),
            "should contain status info: {}",
            result.response
        );
        assert!(result.side_effect.is_none());
    }

    #[test]
    fn execute_reload_returns_side_effect() {
        let result = execute_command("reload", None, &discord_ctx());
        assert_eq!(result.side_effect, Some(CommandSideEffect::Reload));
    }

    #[test]
    fn execute_observe_returns_server_command() {
        let result = execute_command("observe", None, &discord_ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::ServerCommand {
                name: "observe",
                args: None
            })
        );
    }

    #[test]
    fn execute_inbox_with_text() {
        let result = execute_command("inbox", Some("remember this"), &discord_ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::InboxAdd("remember this".to_string()))
        );
    }

    #[test]
    fn all_commands_includes_inbox() {
        let cmds: Vec<_> = all_commands().collect();
        let names: Vec<_> = cmds.iter().map(|c| c.name).collect();
        assert!(names.contains(&"help"), "should include help");
        assert!(names.contains(&"observe"), "should include observe");
        assert!(names.contains(&"inbox"), "should include inbox");
    }

    #[test]
    fn inbox_command_takes_argument() {
        let inbox_cmd = all_commands().find(|c| c.name == "inbox");
        assert!(inbox_cmd.is_some(), "/inbox should be in the registry");
        assert!(
            inbox_cmd.is_some_and(|c| c.takes_arg),
            "/inbox should accept a text argument"
        );
    }

    #[test]
    fn inbox_without_text_returns_usage() {
        let result = execute_command("inbox", None, &discord_ctx());
        assert!(
            result.response.contains("usage"),
            "empty /inbox should show usage: {}",
            result.response
        );
        assert!(result.side_effect.is_none());
    }
}
