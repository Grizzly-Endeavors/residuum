use super::events::{
    A2aTaskSignalEvent, AgentResultEvent, ConversationTypingEvent, ErrorEvent, InlineOutputEvent,
    IntermediateEvent, MainConversationEvent, MessageEvent, NoticeEvent, NotificationEvent,
    OutboundA2aTaskEvent, PostTurnActivityEvent, ResponseEvent, SessionEvent, SessionResponseEvent,
    SessionStartFailedEvent, SpawnRequestEvent, TurnLifecycleEvent, UserInboxAddedEvent,
    WorkbenchEvent, WorkspaceEvent,
};
use super::pulse_fire::PulseFireEvent;
use super::types::{EndpointName, NotifyName, TopicId};

pub trait Topic {
    fn topic_id(&self) -> TopicId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    Lossless,
    Lossy,
}

pub trait Carries<E: Clone + Send + Sync + 'static>: Topic {
    const DELIVERY_MODE: DeliveryMode;
}

/// What is delivered to one named endpoint.
///
/// For a chat interface (Telegram, Discord, Teams) it carries the turns that
/// interface started: the reply, turn lifecycle transitions (its typing
/// indicator), intermediate model text and a failure, plus conversation
/// sessions' output and typing. The web UI's endpoint carries only what the
/// agent posts to it with `send_message`; the web follows every turn through
/// [`MainConversation`] instead.
pub struct Endpoint(pub EndpointName);

impl Topic for Endpoint {
    fn topic_id(&self) -> TopicId {
        TopicId::Endpoint(self.0.clone())
    }
}

impl Carries<ResponseEvent> for Endpoint {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<TurnLifecycleEvent> for Endpoint {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<IntermediateEvent> for Endpoint {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<SessionResponseEvent> for Endpoint {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<ConversationTypingEvent> for Endpoint {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<ErrorEvent> for Endpoint {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct MainConversation;

impl Topic for MainConversation {
    fn topic_id(&self) -> TopicId {
        TopicId::MainConversation
    }
}

impl Carries<MainConversationEvent> for MainConversation {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct Background;

impl Topic for Background {
    fn topic_id(&self) -> TopicId {
        TopicId::Background
    }
}

impl Carries<AgentResultEvent> for Background {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<SpawnRequestEvent> for Background {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct Sessions;

impl Topic for Sessions {
    fn topic_id(&self) -> TopicId {
        TopicId::Sessions
    }
}

impl Carries<SessionEvent> for Sessions {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<SessionStartFailedEvent> for Sessions {
    // The only word the owner gets that a start they are waiting on failed:
    // a dropped one leaves the web UI's New session panel showing Starting,
    // and a pulse fire waiting on its session showing as running.
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct UserMessage;

impl Topic for UserMessage {
    fn topic_id(&self) -> TopicId {
        TopicId::UserMessage
    }
}

impl Carries<MessageEvent> for UserMessage {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct Notification(pub NotifyName);

impl Topic for Notification {
    fn topic_id(&self) -> TopicId {
        TopicId::Notification(self.0.clone())
    }
}

impl Carries<NotificationEvent> for Notification {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<NoticeEvent> for Notification {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<InlineOutputEvent> for Notification {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<ErrorEvent> for Notification {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<OutboundA2aTaskEvent> for Notification {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<PulseFireEvent> for Notification {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}
impl Carries<PostTurnActivityEvent> for Notification {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct Inbox;

impl Topic for Inbox {
    fn topic_id(&self) -> TopicId {
        TopicId::Inbox
    }
}

impl Carries<NotificationEvent> for Inbox {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct UserInbox;

impl Topic for UserInbox {
    fn topic_id(&self) -> TopicId {
        TopicId::UserInbox
    }
}

impl Carries<UserInboxAddedEvent> for UserInbox {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct A2aTaskSignal;

impl Topic for A2aTaskSignal {
    fn topic_id(&self) -> TopicId {
        TopicId::A2aTaskSignal
    }
}

impl Carries<A2aTaskSignalEvent> for A2aTaskSignal {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossless;
}

pub struct Workbench;

impl Topic for Workbench {
    fn topic_id(&self) -> TopicId {
        TopicId::Workbench
    }
}

impl Carries<WorkbenchEvent> for Workbench {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossy;
}

pub struct Workspace;

impl Topic for Workspace {
    fn topic_id(&self) -> TopicId {
        TopicId::Workspace
    }
}

impl Carries<WorkspaceEvent> for Workspace {
    const DELIVERY_MODE: DeliveryMode = DeliveryMode::Lossy;
}

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
    fn main_conversation_topic_id() {
        assert_eq!(MainConversation.topic_id(), TopicId::MainConversation);
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
    fn user_inbox_topic_id() {
        assert_eq!(UserInbox.topic_id(), TopicId::UserInbox);
    }

    #[test]
    fn a2a_task_signal_topic_id() {
        assert_eq!(A2aTaskSignal.topic_id(), TopicId::A2aTaskSignal);
    }

    #[test]
    fn every_route_has_the_expected_delivery_mode() {
        assert_eq!(
            <Endpoint as Carries<ResponseEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Endpoint as Carries<TurnLifecycleEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
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
            <Endpoint as Carries<ConversationTypingEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Endpoint as Carries<ErrorEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <MainConversation as Carries<MainConversationEvent>>::DELIVERY_MODE,
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
            <Sessions as Carries<SessionStartFailedEvent>>::DELIVERY_MODE,
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
            <Notification as Carries<OutboundA2aTaskEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Notification as Carries<PulseFireEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );
        assert_eq!(
            <Notification as Carries<PostTurnActivityEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <Inbox as Carries<NotificationEvent>>::DELIVERY_MODE,
            DeliveryMode::Lossless
        );

        assert_eq!(
            <UserInbox as Carries<UserInboxAddedEvent>>::DELIVERY_MODE,
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
