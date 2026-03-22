//! Communication interfaces between the user and the agent.

pub mod attachment;
pub mod chunking;
pub mod cli;
pub mod discord;
pub mod telegram;
pub mod types;
pub mod webhook;
pub mod websocket;

use std::path::Path;

use crate::bus::{
    BusError, BusHandle, EndpointName, ErrorEvent, IntermediateEvent, NoticeEvent, NotifyName,
    ResponseEvent, Subscriber, TurnLifecycleEvent, topics,
};

/// Common subscriber fields shared by Discord, Telegram, and similar interfaces.
pub(crate) struct BaseSubscribers {
    pub(crate) response: Subscriber<ResponseEvent>,
    pub(crate) turn_lifecycle: Subscriber<TurnLifecycleEvent>,
    pub(crate) intermediate: Subscriber<IntermediateEvent>,
    pub(crate) notice: Subscriber<NoticeEvent>,
    pub(crate) error: Subscriber<ErrorEvent>,
}

impl BaseSubscribers {
    pub(crate) async fn new(bus_handle: &BusHandle, ep: EndpointName) -> Result<Self, BusError> {
        let system_topic = || topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL));
        Ok(Self {
            response: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            turn_lifecycle: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            intermediate: bus_handle.subscribe(topics::Endpoint(ep)).await?,
            notice: bus_handle.subscribe(system_topic()).await?,
            error: bus_handle.subscribe(system_topic()).await?,
        })
    }
}

pub(crate) async fn inbox_add_from_command(
    inbox_dir: &Path,
    body: &str,
    source: &str,
    tz: chrono_tz::Tz,
    ok_response: String,
) -> String {
    let title: String = body
        .lines()
        .next()
        .unwrap_or("Inbox message")
        .chars()
        .take(60)
        .collect();
    match crate::inbox::quick_add(inbox_dir, &title, body, source, tz).await {
        Ok(_) => ok_response,
        Err(e) => format!("failed to add inbox item: {e}"),
    }
}

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
