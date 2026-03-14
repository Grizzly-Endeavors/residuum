//! Telegram interface adapter (DM-only).
//!
//! Uses the teloxide Bot API for long-polling message reception and publishes
//! private messages onto the bus for agent processing.

mod handler;
pub(crate) mod subscriber;

use std::path::PathBuf;

use crate::config::TelegramConfig;
use crate::gateway::event_loop::AdapterSenders;

/// Telegram interface adapter that routes private messages to the agent inbound channel.
pub struct TelegramInterface {
    cfg: TelegramConfig,
    senders: AdapterSenders,
    workspace_dir: PathBuf,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl TelegramInterface {
    /// Create a new Telegram interface adapter.
    #[must_use]
    pub(crate) fn new(
        cfg: TelegramConfig,
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
            self.senders,
            self.workspace_dir,
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
