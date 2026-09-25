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

use crate::background::registry::{SessionRegistry, conversation_session_address};
use crate::bus::{
    BusError, BusHandle, EndpointName, ErrorEvent, IntermediateEvent, NoticeEvent, NotifyName,
    ResponseEvent, SessionResponseEvent, Subscriber, TurnLifecycleEvent, topics,
};
use crate::interfaces::types::{ConversationContext, ConversationKind};

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

/// Channels a chat adapter needs to carry out slash-command side effects.
pub(crate) struct CommandDispatch<'a> {
    pub(crate) reload_tx: &'a tokio::sync::watch::Sender<crate::gateway::types::ReloadSignal>,
    pub(crate) command_tx: &'a tokio::sync::mpsc::Sender<crate::gateway::types::ServerCommand>,
    pub(crate) stop_tx: &'a tokio::sync::mpsc::Sender<crate::gateway::types::StopRequest>,
    /// Looked up to stop a conversation session's turn — see
    /// [`dispatch_stop_request`] — never touched by any other command.
    pub(crate) session_registry: &'a SessionRegistry,
    pub(crate) inbox_dir: &'a Path,
    pub(crate) tz: chrono_tz::Tz,
}

/// Run a slash command typed into a chat interface and return the reply text.
///
/// `interface` (e.g. `"telegram"`) labels logs and the inbox source;
/// `sender_name` is who typed it. `conversation` is the conversation the
/// command was typed in, exactly as it would appear on an ordinary message
/// from the same sender — `None` only for an interface with no conversation
/// concept. It decides `/stop`'s target; every other command ignores it.
pub(crate) async fn run_chat_command(
    name: &str,
    args: Option<&str>,
    dispatch: &CommandDispatch<'_>,
    interface: &str,
    sender_name: &str,
    conversation: Option<&ConversationContext>,
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
            dispatch_stop_request(
                dispatch.stop_tx,
                dispatch.session_registry,
                interface,
                conversation,
                &source,
            )
            .await
        }
        None => result.response,
    }
}

/// Where a `/stop` command should be aimed, resolved from the conversation
/// it was typed in — the same routing rule as an ordinary message (see
/// [`crate::interfaces::types::MessageOrigin::belongs_to_main`]): the
/// owner's own DM (or an interface with no conversation concept, i.e. the
/// web UI) targets main, every other admitted conversation — a group chat,
/// a channel, or a non-owner DM — targets that conversation's own session
/// instead. Every chat interface already restricts commands to the owner,
/// so `conversation.is_owner` is always true by the time a command reaches
/// this; it's still checked so the rule reads the same as the one for
/// ordinary messages rather than silently relying on that.
enum StopTarget {
    /// Stop the main agent's current turn.
    Main,
    /// Stop this conversation's own session, addressed the same way
    /// [`crate::background::ConversationRouter`] addresses it.
    Conversation(crate::bus::SessionAddress),
}

fn resolve_stop_target(endpoint: &str, conversation: Option<&ConversationContext>) -> StopTarget {
    match conversation {
        Some(ctx) if !(ctx.kind == ConversationKind::Personal && ctx.is_owner) => {
            StopTarget::Conversation(conversation_session_address(endpoint, &ctx.id))
        }
        _ => StopTarget::Main,
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

/// Dispatch a `/stop` command to whichever turn it targets and wait for the
/// outcome: the main agent's, or the session behind the conversation it was
/// typed in — see [`resolve_stop_target`] and [`StopTarget`].
///
/// A conversation session is stopped through the session registry
/// directly (the same synchronous, atomic check-and-cancel the web UI's
/// per-session stop uses via
/// [`crate::background::registry::SessionRegistry::stop_if_running`]), so
/// there is no channel for a stale request to sit in — the reply always
/// reflects the session's actual state at the moment this call happens.
///
/// Main is stopped through `stop_tx` instead: unlike `dispatch_server_command`,
/// this doesn't go through `command_tx` — that pipeline only drains between
/// turns, too late to stop one in progress. `stop_tx` reaches the active
/// turn's own select loop directly, which answers every request it reads
/// (including one that arrives with nothing running); a request that
/// arrives in the window between two turns is drained and answered by
/// `handle_idle_stop_request` or `drain_stale_stop_requests`, so it can
/// never be held and applied to a turn other than the one running when it
/// was handled.
#[tracing::instrument(skip_all, fields(source = %source))]
pub(crate) async fn dispatch_stop_request(
    stop_tx: &tokio::sync::mpsc::Sender<crate::gateway::types::StopRequest>,
    session_registry: &SessionRegistry,
    endpoint: &str,
    conversation: Option<&ConversationContext>,
    source: &str,
) -> String {
    match resolve_stop_target(endpoint, conversation) {
        StopTarget::Conversation(address) => {
            tracing::info!(source = %source, %address, "stop requested for conversation session");
            if session_registry.stop_if_running(&address) {
                tracing::info!(%address, "stopped the conversation session's running turn");
                "stopped the current turn.".to_string()
            } else {
                "nothing is running right now.".to_string()
            }
        }
        StopTarget::Main => dispatch_main_stop_request(stop_tx, source).await,
    }
}

/// Stop the main agent's currently running turn and wait for the outcome.
async fn dispatch_main_stop_request(
    stop_tx: &tokio::sync::mpsc::Sender<crate::gateway::types::StopRequest>,
    source: &str,
) -> String {
    use std::time::Duration;
    tracing::info!(source = %source, "stop requested for main");
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
    use crate::background::registry::{
        SessionCategory, SessionInfo, SessionRegistry, SessionState,
    };
    use crate::bus::{EventTrigger, MessageEvent, SessionAddress, Subscriber};
    use crate::config::BackgroundModelTier;
    use crate::gateway::types::StopRequest;
    use tokio_util::sync::CancellationToken;

    fn session_info(address: &str, state: SessionState) -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from(address),
            run_id: "run-1".to_string(),
            category: SessionCategory::External,
            trigger: EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state,
            spawner: None,
            depth: 1,
            purpose: "chat-1".to_string(),
            agent_skill: None,
            model_tier: BackgroundModelTier::Medium,
            conversation_target: None,
            started_at: chrono::Utc::now(),
            usage: crate::agent::usage::SessionUsageTotals::default(),
        }
    }

    fn group_chat(id: &str) -> ConversationContext {
        ConversationContext {
            id: id.to_string(),
            kind: ConversationKind::GroupChat,
            is_owner: true,
        }
    }

    fn owner_dm(id: &str) -> ConversationContext {
        ConversationContext {
            id: id.to_string(),
            kind: ConversationKind::Personal,
            is_owner: true,
        }
    }

    #[tokio::test]
    async fn group_chat_stop_reaches_the_conversation_session_and_not_main() {
        let registry = SessionRegistry::new();
        let address = conversation_session_address("discord", "chan-1");
        let token = CancellationToken::new();
        let _rx = registry
            .register(
                session_info(address.as_ref(), SessionState::Running),
                token.clone(),
            )
            .unwrap();
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel::<StopRequest>(1);

        let reply = dispatch_stop_request(
            &stop_tx,
            &registry,
            "discord",
            Some(&group_chat("chan-1")),
            "test",
        )
        .await;

        assert_eq!(reply, "stopped the current turn.");
        assert!(
            token.is_cancelled(),
            "the conversation session's turn must be the one stopped"
        );
        assert!(
            stop_rx.try_recv().is_err(),
            "main's stop channel must never be touched by a conversation's /stop"
        );
    }

    #[tokio::test]
    async fn owner_dm_stop_reaches_main_and_never_touches_a_session() {
        let registry = SessionRegistry::new();
        // Registered at the address a (wrongly) session-targeted stop would
        // hit, so a regression that ignores `belongs_to_main` shows up here.
        let address = conversation_session_address("discord", "dm-1");
        let token = CancellationToken::new();
        let _rx = registry
            .register(
                session_info(address.as_ref(), SessionState::Running),
                token.clone(),
            )
            .unwrap();
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel::<StopRequest>(1);
        tokio::spawn(async move {
            let req = stop_rx.recv().await.unwrap();
            req.result_tx.unwrap().send(true).ok();
        });

        let reply = dispatch_stop_request(
            &stop_tx,
            &registry,
            "discord",
            Some(&owner_dm("dm-1")),
            "test",
        )
        .await;

        assert_eq!(reply, "stopped the current turn.");
        assert!(
            !token.is_cancelled(),
            "the owner's own DM must stop main, never a conversation session"
        );
    }

    #[tokio::test]
    async fn web_ui_stop_with_no_conversation_reaches_main() {
        let registry = SessionRegistry::new();
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel::<StopRequest>(1);
        tokio::spawn(async move {
            let req = stop_rx.recv().await.unwrap();
            req.result_tx.unwrap().send(true).ok();
        });

        let reply = dispatch_stop_request(&stop_tx, &registry, "ws", None, "test").await;

        assert_eq!(reply, "stopped the current turn.");
    }

    #[tokio::test]
    async fn stop_with_nothing_running_in_the_conversation_reports_that_and_is_discarded() {
        let registry = SessionRegistry::new();
        let address = conversation_session_address("discord", "chan-2");
        let token = CancellationToken::new();
        let _rx = registry
            .register(
                session_info(address.as_ref(), SessionState::Idle),
                token.clone(),
            )
            .unwrap();
        let (stop_tx, _stop_rx) = tokio::sync::mpsc::channel::<StopRequest>(1);

        let reply = dispatch_stop_request(
            &stop_tx,
            &registry,
            "discord",
            Some(&group_chat("chan-2")),
            "test",
        )
        .await;

        assert_eq!(reply, "nothing is running right now.");
        assert!(
            !token.is_cancelled(),
            "a stop request with nothing running must not be held and applied to whatever \
             the session runs next"
        );
        assert_eq!(
            registry.get(&address).unwrap().state,
            SessionState::Idle,
            "the idle session must be left exactly as it was"
        );
    }

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
