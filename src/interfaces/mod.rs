//! Communication interfaces between the user and the agent.

pub mod attachment;
pub(crate) mod chat_state;
pub mod chunking;
pub mod commands;
pub(crate) mod context_buffer;
pub(crate) mod conversations;
pub mod discord;
pub(crate) mod reply_targets;
pub mod teams;
pub mod telegram;
pub mod types;
pub mod webhook;
pub mod websocket;

use std::path::Path;

use crate::bus::{
    BusError, BusHandle, EndpointName, ErrorEvent, IntermediateEvent, NoticeEvent, NotifyName,
    ResponseEvent, SessionResponseEvent, Subscriber, TurnLifecycleEvent, topics,
};

/// Common subscriber fields shared by Discord, Telegram, and similar interfaces.
pub(crate) struct BaseSubscribers {
    pub(crate) response: Subscriber<ResponseEvent>,
    /// Conversation sessions' turn output for this endpoint (distinct from
    /// `response`, which carries the main agent's replies and `send_message`
    /// posts — see [`crate::bus::SessionResponseEvent`]).
    pub(crate) session_response: Subscriber<SessionResponseEvent>,
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
            session_response: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            turn_lifecycle: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            intermediate: bus_handle.subscribe(topics::Endpoint(ep)).await?,
            notice: bus_handle.subscribe(system_topic()).await?,
            error: bus_handle.subscribe(system_topic()).await?,
        })
    }
}

/// Tell `main` that a conversation session's turn output could not be
/// delivered to its own conversation.
///
/// A session never talks to the owner directly and its own conversation
/// can't show a delivery failure either (the failure is precisely that the
/// interface couldn't reach it), so this is the only way the failure becomes
/// visible to the agent system at all — `main` decides whether the owner
/// needs to hear about it. Logged at `error` regardless of whether the
/// notice itself reaches main.
pub(crate) async fn notify_main_of_undeliverable_session_output(
    publisher: &crate::bus::Publisher,
    session_address: &crate::bus::SessionAddress,
    conversation_id: &str,
    reason: &str,
) {
    tracing::error!(
        session = %session_address,
        conversation = %conversation_id,
        reason,
        "conversation session's output could not be delivered; notifying main"
    );
    let event = crate::bus::MessageEvent::from_background(format!(
        "[Delivery Failed] session {session_address}'s reply to conversation {conversation_id} \
         could not be delivered: {reason}"
    ));
    if let Err(e) = publisher.publish(topics::UserMessage, event).await {
        tracing::warn!(error = %e, "failed to notify main about undeliverable session output");
    }
}

/// Tell `main` that a participant's inbound message could not be delivered
/// to its conversation session.
///
/// Mirrors [`notify_main_of_undeliverable_session_output`] for the opposite
/// direction: an inbound message a session's interrupt channel refused
/// (saturated) rather than an outbound reply the interface couldn't post.
/// Without this, a busy session drops the message with nothing but a log
/// line — the participant sees no reply and nothing in the agent system ever
/// learns their message went nowhere. Logged at `error` regardless of
/// whether the notice itself reaches main.
pub(crate) async fn notify_main_of_undeliverable_conversation_message(
    publisher: &crate::bus::Publisher,
    session_address: &crate::bus::SessionAddress,
    conversation_id: &str,
    reason: &str,
) {
    tracing::error!(
        session = %session_address,
        conversation = %conversation_id,
        reason,
        "a participant's message could not be delivered to its conversation session"
    );
    let event = crate::bus::MessageEvent::from_background(format!(
        "[Delivery Failed] a participant's message to conversation {conversation_id} \
         (session {session_address}) could not be delivered: {reason}"
    ));
    if let Err(e) = publisher.publish(topics::UserMessage, event).await {
        tracing::warn!(
            error = %e,
            "failed to notify main about an undeliverable conversation message"
        );
    }
}

/// Channels a chat adapter needs to carry out slash-command side effects.
pub(crate) struct CommandDispatch<'a> {
    pub(crate) reload_tx: &'a tokio::sync::watch::Sender<crate::gateway::types::ReloadSignal>,
    pub(crate) command_tx: &'a tokio::sync::mpsc::Sender<crate::gateway::types::ServerCommand>,
    pub(crate) stop_tx: &'a tokio::sync::mpsc::Sender<crate::gateway::types::StopRequest>,
    pub(crate) inbox_dir: &'a Path,
    pub(crate) tz: chrono_tz::Tz,
}

/// Run a slash command typed into a chat interface and return the reply text.
///
/// `interface` (e.g. `"telegram"`) labels logs and the inbox source;
/// `sender_name` is who typed it.
pub(crate) async fn run_chat_command(
    name: &str,
    args: Option<&str>,
    dispatch: &CommandDispatch<'_>,
    interface: &str,
    sender_name: &str,
) -> String {
    let result = commands::execute_command(name, args, &commands::CommandContext::default());
    let source = format!("{interface} command");
    match result.side_effect {
        Some(commands::CommandSideEffect::Reload) => {
            tracing::info!(interface, "reload requested via chat command");
            if dispatch
                .reload_tx
                .send(crate::gateway::types::ReloadSignal::Root)
                .is_err()
            {
                tracing::warn!(command = %name, interface, "reload_tx closed, reload dropped");
            }
            result.response
        }
        Some(commands::CommandSideEffect::ServerCommand {
            name: server_command,
            args: server_args,
        }) => {
            dispatch_server_command(
                dispatch.command_tx,
                server_command,
                server_args,
                result.response,
                &source,
            )
            .await
        }
        Some(commands::CommandSideEffect::InboxAdd(body)) => {
            inbox_add_from_command(
                dispatch.inbox_dir,
                &body,
                &format!("{interface}:{sender_name}"),
                dispatch.tz,
                result.response,
            )
            .await
        }
        Some(commands::CommandSideEffect::Stop) => {
            dispatch_stop_request(dispatch.stop_tx, &source).await
        }
        None => result.response,
    }
}

#[tracing::instrument(skip_all, fields(source = %source))]
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
#[tracing::instrument(skip_all, fields(command = %name, source = %source))]
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
        Ok(Ok(Ok(msg))) => msg,
        Ok(Ok(Err(reason))) => reason,
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

/// Dispatch a stop request for the currently running turn and wait for the outcome.
///
/// Unlike `dispatch_server_command`, this doesn't go through `command_tx` —
/// that pipeline only drains between turns, too late to stop one in
/// progress. `stop_tx` reaches the active turn's own select loop directly.
#[tracing::instrument(skip_all, fields(source = %source))]
pub(crate) async fn dispatch_stop_request(
    stop_tx: &tokio::sync::mpsc::Sender<crate::gateway::types::StopRequest>,
    source: &str,
) -> String {
    use std::time::Duration;
    tracing::info!(source = %source, "stop requested");
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    if let Err(e) = stop_tx.try_send(crate::gateway::types::StopRequest {
        reply_to: None,
        result_tx: Some(result_tx),
    }) {
        tracing::warn!(source = %source, error = %e, "failed to dispatch stop request");
        return "couldn't reach the agent to stop it — try again in a moment.".to_string();
    }
    match tokio::time::timeout(Duration::from_secs(5), result_rx).await {
        Ok(Ok(true)) => "stopped the current turn.".to_string(),
        Ok(Ok(false)) => "nothing is running right now.".to_string(),
        Ok(Err(_)) => {
            tracing::warn!(source = %source, "stop reply channel closed before response");
            "nothing is running right now.".to_string()
        }
        Err(_) => {
            tracing::warn!(source = %source, "stop request timed out waiting for reply");
            "couldn't confirm the stop in time — it may still have worked.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{MessageEvent, SessionAddress, Subscriber};

    #[tokio::test]
    async fn undeliverable_session_output_reaches_main_as_background_input() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();

        notify_main_of_undeliverable_session_output(
            &publisher,
            &SessionAddress::from("external-discord-0001"),
            "chan-1",
            "the bot is no longer in that channel",
        )
        .await;

        let event = sub.recv().await.unwrap().unwrap();
        assert!(event.content.contains("external-discord-0001"));
        assert!(event.content.contains("chan-1"));
        assert!(event.content.contains("no longer in that channel"));
        assert!(
            event.origin.belongs_to_main(),
            "the notice must reach main, never a conversation session"
        );
    }
}
