//! What the hub learns about its running agents as it happens.
//!
//! Each running agent has a **watcher** (see [`watcher`]): a task the host
//! starts with the agent and stops when the agent stops. It reads the agent's
//! own bus, and publishes what it finds on the hub's [`AgentChangeFeed`] as
//! [`AgentChange`]s. The activity tracker's turn hook publishes there too, as
//! [`AgentChangeKind::TurnEnded`].
//!
//! | Source on the agent's bus | Change |
//! |---|---|
//! | Sessions: started, state changed, completed | [`AgentChangeKind::SessionStarted`], [`SessionStateChanged`](AgentChangeKind::SessionStateChanged), [`SessionCompleted`](AgentChangeKind::SessionCompleted) |
//! | The system notification channel: outbound A2A task changes | [`AgentChangeKind::OutboundTaskChanged`] |
//! | `UserInbox`: an item saved by `user_inbox_add` | [`AgentChangeKind::UserInboxAdded`] |
//! | The agent's workspace change feed, for the files in [`WatchedPath`] | [`AgentChangeKind::WatchedPathChanged`] |
//! | The workspace change feed's resync, and its outages | [`AgentChangeKind::Resync`] |
//!
//! Every session event, lifecycle or turn, is also relayed on its own
//! broadcast for the session relay: see [`AgentChangeFeed::subscribe_sessions`].
//!
//! The consumers of the feed ([`AgentChangeFeed::subscribe`]) recompute what
//! they show from disk and the agent's session registry. Changes say which
//! part to recompute; [`AgentChangeKind::Resync`] says all of it.

mod changes;
mod watcher;

pub use changes::{
    AgentChange, AgentChangeFeed, AgentChangeKind, AgentChangeReceiver, AgentSessionEvent,
    MainTurnEnded, WatchedPath,
};
pub(crate) use watcher::AgentWatcher;
