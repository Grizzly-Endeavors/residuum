//! Communication interfaces between the user and the agent.

pub mod attachment;
pub mod chunking;
pub mod cli;
pub mod discord;
pub mod presence;
pub mod telegram;
pub mod types;
pub mod webhook;
pub mod websocket;

/// Dispatch a named server command and wait for its reply.
///
/// Creates a oneshot channel, sends the command, and waits up to 10 seconds
/// for a response. Falls back to `fallback` on timeout or channel close.
pub(crate) async fn dispatch_server_command(
    command_tx: &tokio::sync::mpsc::Sender<crate::gateway::types::ServerCommand>,
    name: &'static str,
    args: Option<String>,
    fallback: String,
    source: &str,
) -> String {
    use std::time::Duration;
    tracing::info!(command = %name, source = %source, "server command dispatched");
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if let Err(e) = command_tx.try_send(crate::gateway::types::ServerCommand {
        name: name.to_string(),
        args,
        reply_tx: Some(reply_tx),
    }) {
        tracing::warn!(command = %name, error = %e, "failed to dispatch server command");
    }
    match tokio::time::timeout(Duration::from_secs(10), reply_rx).await {
        Ok(Ok(msg)) => msg,
        Ok(Err(_)) => {
            tracing::warn!(command = %name, "server command reply channel closed before response");
            fallback
        }
        Err(_) => {
            tracing::warn!(command = %name, timeout_secs = 10, "server command timed out waiting for reply");
            fallback
        }
    }
}
