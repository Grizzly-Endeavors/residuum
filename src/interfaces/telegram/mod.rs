//! Telegram interface adapter (DM-only).
//!
//! Uses the teloxide Bot API for long-polling message reception and routes
//! private messages through the standard `RoutedMessage` pipeline.

mod handler;
mod reply;
#[expect(dead_code, reason = "subscriber will be wired in during bus migration")]
pub(crate) mod subscriber;

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::config::TelegramConfig;
use crate::gateway::types::{ReloadSignal, ServerCommand};
use crate::interfaces::types::RoutedMessage;

/// Telegram interface adapter that routes private messages to the agent inbound channel.
pub struct TelegramInterface {
    cfg: TelegramConfig,
    inbound_tx: mpsc::Sender<RoutedMessage>,
    workspace_dir: PathBuf,
    reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: mpsc::Sender<ServerCommand>,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl TelegramInterface {
    /// Create a new Telegram interface adapter.
    #[must_use]
    pub(crate) fn new(
        cfg: TelegramConfig,
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

    /// Start the Telegram long-polling loop.
    ///
    /// This blocks until a shutdown signal is received, an error occurs, or the
    /// task is cancelled.
    ///
    /// # Errors
    /// Returns an error if the bot cannot connect or polling fails fatally.
    pub(crate) async fn start(self) -> anyhow::Result<()> {
        handler::run_telegram_polling(
            &self.cfg.token,
            self.inbound_tx,
            self.workspace_dir,
            self.reload_tx,
            self.command_tx,
            self.tz,
            self.shutdown_rx,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::interfaces::cli::commands::{CommandContext, CommandSideEffect, execute_command};

    fn telegram_ctx() -> CommandContext<'static> {
        CommandContext {
            url: "",
            verbose: false,
            interface_name: "telegram",
        }
    }

    #[test]
    fn execute_help_returns_command_list() {
        let result = execute_command("help", None, &telegram_ctx());
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
        let result = execute_command("status", None, &telegram_ctx());
        assert!(
            result.response.contains("verbose"),
            "should contain status info: {}",
            result.response
        );
        assert!(result.side_effect.is_none());
    }

    #[test]
    fn execute_reload_returns_side_effect() {
        let result = execute_command("reload", None, &telegram_ctx());
        assert_eq!(result.side_effect, Some(CommandSideEffect::Reload));
    }

    #[test]
    fn execute_observe_returns_server_command() {
        let result = execute_command("observe", None, &telegram_ctx());
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
        let result = execute_command("inbox", Some("remember this"), &telegram_ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::InboxAdd("remember this".to_string()))
        );
    }

    #[test]
    fn inbox_without_text_returns_usage() {
        let result = execute_command("inbox", None, &telegram_ctx());
        assert!(
            result.response.contains("usage"),
            "empty /inbox should show usage: {}",
            result.response
        );
        assert!(result.side_effect.is_none());
    }
}
