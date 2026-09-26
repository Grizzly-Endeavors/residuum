//! Typed topic definitions for the bus.
//!
//! Each topic struct is a routing domain that can carry multiple event types.
//! The [`Carries`] marker trait declares which event types are valid for a
//! topic, providing compile-time safety at publish/subscribe boundaries.

use super::events::{
    A2aTaskSignalEvent, AgentResultEvent, ConversationTypingEvent, ErrorEvent, InlineOutputEvent,
    IntermediateEvent, MessageEvent, NoticeEvent, NotificationEvent, OutboundA2aTaskEvent,
    PostTurnActivityEvent, ResponseEvent, SessionEvent, SessionResponseEvent, SpawnRequestEvent,
    ToolActivityEvent, TurnLifecycleEvent, TurnUsageEvent, WorkbenchEvent, WorkspaceEvent,
};
use super::types::{EndpointName, NotifyName, TopicId};

// ---------------------------------------------------------------------------
// Topic and Carries traits
// ---------------------------------------------------------------------------

/// A routing domain on the bus, identified by a runtime [`TopicId`].
pub trait Topic {
    /// The runtime key used by the broker for routing.
    fn topic_id(&self) -> TopicId;
}

/// How a `(topic, event)` route behaves when a subscriber can't keep up.
///
/// The broker (`bus::broker`) backs each subscription with a channel sized
/// and shaped for this mode: unbounded for [`Lossless`](Self::Lossless), a
/// bounded ring for [`Lossy`](Self::Lossy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Every event reaches every subscriber, however far behind it falls.
    /// For events whose loss would change behavior or hide something from
    /// the user — see `CLAUDE.md` "No Silent Failures": user messages,
    /// agent/tool results and tool-call events, turn and session
    /// lifecycle/state, notices and notifications, agent-to-agent signals,
    /// and spawn requests.
    Lossless,
    /// The newest event supersedes the last, so a slow subscriber missing
    /// intermediate ones loses nothing meaningful: streaming progress ticks,
    /// or a change feed that already carries its own resync signal for a
    /// missed batch. Drops are bounded, counted, and logged once when they
    /// start and once when they stop — never per event.
    Lossy,
}

/// Marker trait declaring that topic `Self` can carry events of type `E`.
///
/// This provides compile-time safety: publishing or subscribing to an
/// unsupported `(topic, event)` pair is a type error. Every impl must also
/// assign [`Self::DELIVERY_MODE`] explicitly — there is no default — so a
/// new topic/event pairing can't silently inherit the wrong one.
pub trait Carries<E: Clone + Send + Sync + 'static>: Topic {
    /// Whether this route can ever drop an event under backpressure.
    const DELIVERY_MODE: DeliveryMode;
}

// ---------------------------------------------------------------------------
// Topic structs
// ---------------------------------------------------------------------------

/// Interactive endpoint turn activity.
///
/// Carries responses, tool call/result activity, turn lifecycle transitions,
/// and intermediate model text for a specific named endpoint.
pub struct Endpoint(pub EndpointName);

impl Topic for Endpoint {
    fn topic_id(&self) -> TopicId {
        TopicId::Endpoint(self.0.clone())
    }
}

impl Carries<ResponseEvent> for Endpoint {
    // A turn's final output text — losing it means the user never sees the
    // agent's answer.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<ToolActivityEvent> for Endpoint {
    // Tool call/result activity the UI renders as it happens.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<TurnLifecycleEvent> for Endpoint {
    // Turn start/end drives visible turn state (e.g. the running indicator);
    // a dropped `Ended` would leave the UI showing a turn that never stops.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<TurnUsageEvent> for Endpoint {
    // Cumulative token-count ticks for a running-turn indicator — each one
    // supersedes the last, so missing intermediate ticks is invisible.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossy;
}
impl Carries<IntermediateEvent> for Endpoint {
    // Pre-tool-call text the agent chose to say; each one is rendered as its
    // own chat message, not a delta, so dropping one hides real content.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<SessionResponseEvent> for Endpoint {
    // A session's own turn output, delivered straight to its conversation —
    // same stakes as `ResponseEvent`.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<ConversationTypingEvent> for Endpoint {
    // A dropped `active: false` would leave a conversation's typing
    // indicator stuck on, same reasoning as `TurnLifecycleEvent`.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

/// Background task orchestration: spawn requests and task results.
pub struct Background;

impl Topic for Background {
    fn topic_id(&self) -> TopicId {
        TopicId::Background
    }
}

impl Carries<AgentResultEvent> for Background {
    // A background/subagent task's terminal result — the caller waiting on
    // it, and the web UI's session history, must see every one.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<SpawnRequestEvent> for Background {
    // Losing a spawn request means the requested session never runs at all.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

/// Agent session activity: lifecycle transitions and session-tagged turn
/// events, for surfaces that follow sessions live (the web UI).
pub struct Sessions;

impl Topic for Sessions {
    fn topic_id(&self) -> TopicId {
        TopicId::Sessions
    }
}

impl Carries<SessionEvent> for Sessions {
    // Carries session/turn lifecycle and state transitions the web UI must
    // never miss, plus a `TurnUsage` variant that is itself a progress tick.
    // The type is a single wire event mixing both, so it is classified
    // conservatively as a whole: losing a lifecycle/state variant would hide
    // real behavior from the user, and keeping the occasional `TurnUsage`
    // instance around costs nothing a slow-subscriber backlog warning
    // (`broker::LOSSLESS_BACKLOG_WARN_THRESHOLD`) doesn't already cover.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

/// Inbound user messages destined for the main agent loop.
pub struct UserMessage;

impl Topic for UserMessage {
    fn topic_id(&self) -> TopicId {
        TopicId::UserMessage
    }
}

impl Carries<MessageEvent> for UserMessage {
    // A user's own message to the agent — the single least acceptable thing
    // to drop on this whole bus.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

/// Push notifications for a named channel.
///
/// The well-known channel `"system"` (see [`super::types::SYSTEM_CHANNEL`])
/// carries operational notices and errors broadcast to all connected endpoints.
pub struct Notification(pub NotifyName);

impl Topic for Notification {
    fn topic_id(&self) -> TopicId {
        TopicId::Notification(self.0.clone())
    }
}

impl Carries<NotificationEvent> for Notification {
    // A push notification the user is meant to see or act on.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<NoticeEvent> for Notification {
    // An operational notice — transient, but still a specific fact the user
    // was meant to be told once.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<InlineOutputEvent> for Notification {
    // Multi-line command output rendered inline in the chat stream.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<ErrorEvent> for Notification {
    // An error tied to a specific turn — silently dropping it is exactly
    // the silent failure `CLAUDE.md` forbids.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<OutboundA2aTaskEvent> for Notification {
    // A dropped update would leave the sessions sidebar showing a task
    // that already finished, with a Stop button that no longer applies.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<PostTurnActivityEvent> for Notification {
    // A dropped `active: false` would leave the web UI's quiet indicator
    // stuck showing background work that already finished.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

/// The user inbox for incoming notifications.
pub struct Inbox;

impl Topic for Inbox {
    fn topic_id(&self) -> TopicId {
        TopicId::Inbox
    }
}

impl Carries<NotificationEvent> for Inbox {
    // The inbox is the durable fallback for a notification with nowhere
    // else to land — it must never itself drop the thing it's the fallback
    // for.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

/// Explicit task-outcome signals from a session's `a2a_task_update` tool
/// call, consumed by the A2A executor waiting on that session's address.
pub struct A2aTaskSignal;

impl Topic for A2aTaskSignal {
    fn topic_id(&self) -> TopicId {
        TopicId::A2aTaskSignal
    }
}

impl Carries<A2aTaskSignalEvent> for A2aTaskSignal {
    // The one signal the waiting A2A executor uses to close out a task —
    // dropping it strands the caller with no terminal status.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

/// Workbench artifact file changes, for web UI views showing an artifact live.
pub struct Workbench;

impl Topic for Workbench {
    fn topic_id(&self) -> TopicId {
        TopicId::Workbench
    }
}

impl Carries<WorkbenchEvent> for Workbench {
    // Derived from the workspace change feed below, which already tolerates
    // missed batches via `WorkspaceEvent::Resync` — a missed artifact update
    // is caught by the next one or a resync, never a permanent loss.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossy;
}

/// The workspace change feed: debounced batches of file changes anywhere in
/// the workspace, for WebSocket watchers and artifact reloads.
pub struct Workspace;

impl Topic for Workspace {
    fn topic_id(&self) -> TopicId {
        TopicId::Workspace
    }
}

impl Carries<WorkspaceEvent> for Workspace {
    // The change feed's own `Resync` variant exists precisely because a
    // missed batch is expected and self-healing: watchers reload what they
    // show instead of relying on every batch arriving.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossy;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_topic_id() {
        let ep = EndpointName::from("ws");
        assert_eq!(Endpoint(ep.clone()).topic_id(), TopicId::Endpoint(ep));
    }

    #[test]
    fn notification_topic_id() {
        let name = NotifyName::from("ntfy");
        assert_eq!(
            Notification(name.clone()).topic_id(),
            TopicId::Notification(name)
        );
    }

    #[test]
    fn user_message_topic_id() {
        assert_eq!(UserMessage.topic_id(), TopicId::UserMessage);
    }

    #[test]
    fn background_topic_id() {
        assert_eq!(Background.topic_id(), TopicId::Background);
    }

    #[test]
    fn sessions_topic_id() {
        assert_eq!(Sessions.topic_id(), TopicId::Sessions);
    }

    #[test]
    fn inbox_topic_id() {
        assert_eq!(Inbox.topic_id(), TopicId::Inbox);
    }

    #[test]
    fn a2a_task_signal_topic_id() {
        assert_eq!(A2aTaskSignal.topic_id(), TopicId::A2aTaskSignal);
    }

    /// Every `(topic, event)` route's delivery mode, spelled out in one
    /// place as a lock-file for the classification above: changing a
    /// route's mode, or adding/removing a route, must show up as a diff
    /// here too. A route can't compile without choosing a mode (see
    /// `Carries::DELIVERY_MODE`'s doc comment), but nothing stops a future
    /// edit from choosing the wrong one — this test is what catches that.
    #[test]
    fn every_route_has_the_expected_delivery_mode() {
        assert_eq!(
            <Endpoint as Carries<ResponseEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Endpoint as Carries<ToolActivityEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Endpoint as Carries<TurnLifecycleEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Endpoint as Carries<TurnUsageEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossy
        );
        assert_eq!(
            <Endpoint as Carries<IntermediateEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Endpoint as Carries<SessionResponseEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <Background as Carries<AgentResultEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Background as Carries<SpawnRequestEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <Sessions as Carries<SessionEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <UserMessage as Carries<MessageEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <Notification as Carries<NotificationEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Notification as Carries<NoticeEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Notification as Carries<InlineOutputEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Notification as Carries<ErrorEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <Inbox as Carries<NotificationEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <A2aTaskSignal as Carries<A2aTaskSignalEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <Workbench as Carries<WorkbenchEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossy
        );
        assert_eq!(
            <Workspace as Carries<WorkspaceEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossy
        );
    }
}
