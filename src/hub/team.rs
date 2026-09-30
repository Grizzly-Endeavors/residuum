//! Teamwork messaging between the hub's agents.
//!
//! [`TeamRouter`] is the hub-level service that carries a `message_agent`
//! call addressed `agent:<name>` (a teammate's main) or
//! `agent:<name>/<session-address>` (one of its sessions) from the sending
//! agent to the target agent's [`AgentMessenger`], which delivers it exactly
//! as it delivers a local message: an interrupt or a new turn for main, an
//! interrupt for a live session, a resume for a completed one.
//!
//! Every running agent registers its messenger at start and unregisters at
//! stop, so nothing is ever queued for an agent that isn't running. Each
//! agent holds a [`TeamLink`], its handle on the router: it knows the
//! agent's own name, sends on its behalf, and reads the roster that
//! `list_agents` and the prompt's `TEAM` block show.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, PoisonError, RwLock};

use crate::background::messaging::{AgentMessenger, DeliveryOutcome, SendError};
use crate::background::registry::{MAIN_ADDRESS, TEAMMATE_SENDER_CATEGORY, TEAMMATE_SENDER_PREFIX};
use crate::bus::SessionAddress;
use crate::config::paths::validate_agent_name;

use super::directory::{AgentDirectory, DirectoryHandle};
use super::types::{AgentState, AgentSummary, LifecycleError};

/// Where a teammate message is going: an agent, and one of its sessions
/// (`None` for its main).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamTarget {
    /// The teammate's name.
    pub agent: String,
    /// The teammate's session address, or `None` for its main.
    pub session: Option<String>,
}

impl TeamTarget {
    /// The address form this target was parsed from.
    #[must_use]
    pub fn address(&self) -> String {
        match &self.session {
            Some(session) => format!("{TEAMMATE_SENDER_PREFIX}{}/{session}", self.agent),
            None => format!("{TEAMMATE_SENDER_PREFIX}{}", self.agent),
        }
    }

    /// The address on the target agent's own messenger.
    fn local_address(&self) -> &str {
        self.session.as_deref().unwrap_or(MAIN_ADDRESS)
    }
}

/// A teammate address that doesn't parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TeamAddressError {
    /// `agent:` with nothing after it.
    #[error(
        "the address 'agent:' names no teammate. Use \"agent:<name>\" for a teammate's main or \
         \"agent:<name>/<session-address>\" for one of its sessions."
    )]
    MissingName,
    /// The agent name breaks the agent-name rules.
    #[error("{0}. Use \"agent:<name>\" or \"agent:<name>/<session-address>\".")]
    InvalidName(String),
    /// `agent:<name>/` with no session address after the slash.
    #[error(
        "the address '{0}' has an empty session address after the '/'. Use \"agent:<name>\" for \
         the teammate's main or \"agent:<name>/<session-address>\" for one of its sessions."
    )]
    MissingSession(String),
}

/// Parse a `message_agent` destination as a teammate address.
///
/// Returns `None` when `to` is not in the `agent:` form at all (a local
/// address or an `a2a:` remote agent), so callers fall through to their
/// other address forms. `agent:<name>/main` is the teammate's main.
#[must_use]
pub fn parse_team_address(to: &str) -> Option<Result<TeamTarget, TeamAddressError>> {
    let rest = to.strip_prefix(TEAMMATE_SENDER_PREFIX)?;
    let (name, session) = match rest.split_once('/') {
        Some((name, session)) => (name, Some(session)),
        None => (rest, None),
    };
    if name.is_empty() {
        return Some(Err(TeamAddressError::MissingName));
    }
    if let Err(problem) = validate_agent_name(name) {
        return Some(Err(TeamAddressError::InvalidName(problem)));
    }
    let session = match session {
        Some("") => return Some(Err(TeamAddressError::MissingSession(to.to_string()))),
        Some(MAIN_ADDRESS) | None => None,
        Some(session) => Some(session.to_string()),
    };
    Some(Ok(TeamTarget {
        agent: name.to_string(),
        session,
    }))
}

/// A teammate message couldn't be delivered. The text is written for the
/// sending agent: it says what happened and what to do.
#[derive(Debug, Clone, thiserror::Error)]
pub enum TeamSendError {
    /// The address names the sender itself.
    #[error("'agent:{0}' is you. Use \"main\" or a session address to reach your own agent.")]
    ToSelf(String),
    /// No agent has this name.
    #[error("no teammate named '{name}'. {}", teammates_phrase(teammates))]
    UnknownAgent {
        /// The name that was addressed.
        name: String,
        /// The other agents on the team.
        teammates: Vec<String>,
    },
    /// The teammate exists but isn't running. Nothing was queued.
    #[error("{}", not_running_text(name, state))]
    NotRunning {
        /// The teammate's name.
        name: String,
        /// Its state, in words.
        state: String,
    },
    /// The teammate is running but has no such session.
    #[error(
        "teammate '{agent}' has no session '{session}'. Use the exact address from a message it \
         sent you."
    )]
    NoSuchSession {
        /// The teammate's name.
        agent: String,
        /// The session address that was addressed.
        session: String,
    },
    /// The target's interrupt channel is full.
    #[error("{target} is busy, try again shortly")]
    Busy {
        /// The address that was busy.
        target: String,
    },
    /// Delivery failed inside the teammate (publish failure, hop limit).
    #[error("{0}")]
    Delivery(SendError),
}

fn not_running_text(name: &str, state: &str) -> String {
    if state == "starting" {
        format!("teammate '{name}' is starting; nothing was queued. Try again shortly.")
    } else {
        format!(
            "teammate '{name}' is {state}; nothing was queued. Tell the user if this message \
             matters; they can start it from the team view."
        )
    }
}

fn teammates_phrase(teammates: &[String]) -> String {
    if teammates.is_empty() {
        "You have no teammates.".to_string()
    } else {
        format!(
            "Your teammates are: {}. Use list_agents for their state.",
            teammates.join(", ")
        )
    }
}

/// The hub's team router: each running agent's messenger, by agent name.
pub struct TeamRouter {
    messengers: RwLock<BTreeMap<String, Arc<AgentMessenger>>>,
    /// The agent host, bound once it exists (it is built from the services
    /// that hold this router). Read for existence and state.
    directory: DirectoryHandle,
}

impl TeamRouter {
    /// An empty router that checks names and states against `directory`.
    #[must_use]
    pub fn new_shared(directory: DirectoryHandle) -> Arc<Self> {
        Arc::new(Self {
            messengers: RwLock::new(BTreeMap::new()),
            directory,
        })
    }

    /// Register a starting agent's messenger. A restarted agent replaces its
    /// previous registration.
    pub fn register(&self, name: &str, messenger: Arc<AgentMessenger>) {
        self.messengers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_string(), messenger);
    }

    /// Remove a stopping agent's messenger, so nothing more is delivered to
    /// it.
    pub fn unregister(&self, name: &str) {
        self.messengers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
    }

    fn messenger(&self, name: &str) -> Option<Arc<AgentMessenger>> {
        self.messengers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    fn directory(&self) -> Option<Arc<dyn AgentDirectory>> {
        self.directory.get()
    }

    /// Every agent on the team, sorted by name. Empty until the directory is
    /// bound.
    fn members(&self) -> Vec<AgentSummary> {
        self.directory()
            .map(|directory| directory.list())
            .unwrap_or_default()
    }

    /// Send `content` from `sender` (an address in agent `me`) to `target`,
    /// carrying `hop_count`.
    async fn send(
        &self,
        me: &str,
        sender: &SessionAddress,
        target: &TeamTarget,
        content: String,
        hop_count: u32,
    ) -> Result<DeliveryOutcome, TeamSendError> {
        if target.agent == me {
            return Err(TeamSendError::ToSelf(me.to_string()));
        }
        let messenger = self.reachable_messenger(me, &target.agent)?;

        let qualified = qualify_sender(me, sender);
        let outcome = messenger
            .send(
                target.local_address(),
                SessionAddress::from(qualified),
                TEAMMATE_SENDER_CATEGORY.to_string(),
                content,
                hop_count,
            )
            .await;
        match outcome {
            Ok(DeliveryOutcome::Unknown) => Err(TeamSendError::NoSuchSession {
                agent: target.agent.clone(),
                session: target.local_address().to_string(),
            }),
            Ok(delivered) => Ok(delivered),
            Err(SendError::Busy(_)) => Err(TeamSendError::Busy {
                target: target.address(),
            }),
            Err(error) => {
                if matches!(error, SendError::HopLimitExceeded { .. })
                    && let Some(own) = self.messenger(me)
                {
                    // The target's messenger noted the refusal in its own
                    // session; the sender's side gets the same note.
                    own.note_refused_send(sender, &target.address(), hop_count)
                        .await;
                }
                Err(TeamSendError::Delivery(error))
            }
        }
    }

    /// The running teammate's messenger, or the error that says why there
    /// isn't one.
    fn reachable_messenger(
        &self,
        me: &str,
        name: &str,
    ) -> Result<Arc<AgentMessenger>, TeamSendError> {
        let unknown = || TeamSendError::UnknownAgent {
            name: name.to_string(),
            teammates: self
                .members()
                .into_iter()
                .map(|member| member.name)
                .filter(|member| member != me)
                .collect(),
        };
        let Some(directory) = self.directory() else {
            return Err(unknown());
        };
        let summary = match directory.summary(name) {
            Ok(summary) => summary,
            Err(LifecycleError::NotFound(_)) => return Err(unknown()),
            Err(error) => {
                tracing::warn!(error = %error, teammate = %name, "couldn't look up a teammate to deliver a message");
                return Err(unknown());
            }
        };
        match (summary.state, self.messenger(name)) {
            (AgentState::Running, Some(messenger)) => Ok(messenger),
            // A running agent whose messenger is gone is between its stop
            // request and its state change.
            (AgentState::Running, None) => Err(TeamSendError::NotRunning {
                name: name.to_string(),
                state: "stopping".to_string(),
            }),
            (state, _) => Err(TeamSendError::NotRunning {
                name: name.to_string(),
                state: state.to_string(),
            }),
        }
    }
}

/// The sender's fully qualified address as a teammate sees it:
/// `agent:<me>` for main, `agent:<me>/<session>` for a session.
fn qualify_sender(me: &str, sender: &SessionAddress) -> String {
    if sender.as_ref() == MAIN_ADDRESS {
        format!("{TEAMMATE_SENDER_PREFIX}{me}")
    } else {
        format!("{TEAMMATE_SENDER_PREFIX}{me}/{sender}")
    }
}

/// An agent's handle on the team router: its own name, sending on its
/// behalf, and the roster.
#[derive(Clone)]
pub struct TeamLink {
    me: String,
    router: Arc<TeamRouter>,
}

impl TeamLink {
    /// The link agent `me` holds on `router`.
    #[must_use]
    pub fn new(me: impl Into<String>, router: Arc<TeamRouter>) -> Self {
        Self {
            me: me.into(),
            router,
        }
    }

    /// A link to a team of one: no directory, no teammates.
    #[must_use]
    pub fn alone(me: impl Into<String>) -> Self {
        Self::new(me, TeamRouter::new_shared(DirectoryHandle::unbound()))
    }

    /// The agent's own name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.me
    }

    /// Send `content` to a teammate from `sender`, an address in this agent.
    ///
    /// # Errors
    ///
    /// [`TeamSendError`] when the teammate is unknown, not running, has no
    /// such session, is busy, or the hub's hop limit refuses the message.
    pub async fn send(
        &self,
        sender: &SessionAddress,
        target: &TeamTarget,
        content: String,
        hop_count: u32,
    ) -> Result<DeliveryOutcome, TeamSendError> {
        self.router
            .send(&self.me, sender, target, content, hop_count)
            .await
    }

    /// Every agent on the team, this one included, sorted by name.
    #[must_use]
    pub fn members(&self) -> Vec<AgentSummary> {
        self.router.members()
    }

    /// The other agents on the team, sorted by name.
    #[must_use]
    pub fn teammates(&self) -> Vec<AgentSummary> {
        self.members()
            .into_iter()
            .filter(|member| member.name != self.me)
            .collect()
    }

    /// The prompt's `TEAM` block, or `None` when this agent has no
    /// teammates. Built from the same roster `list_agents` returns, so it
    /// reflects state changes on the next turn.
    #[must_use]
    pub fn prompt_block(&self) -> Option<String> {
        let teammates = self.teammates();
        if teammates.is_empty() {
            return None;
        }
        let mut block = format!(
            "You are \"{}\". Teammates (message_agent to=\"agent:<name>\"; only running ones receive):",
            self.me
        );
        for teammate in &teammates {
            let role = teammate.role.as_deref().unwrap_or("no role line yet");
            _ = write!(block, "\n- {} ({}): {role}", teammate.name, teammate.state);
        }
        Some(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_teammates_main_parses_as_a_bare_name() {
        let target = parse_team_address("agent:writer").unwrap().unwrap();
        assert_eq!(
            target,
            TeamTarget {
                agent: "writer".to_string(),
                session: None
            }
        );
        assert_eq!(target.address(), "agent:writer");
    }

    #[test]
    fn a_teammates_session_parses_after_the_first_slash() {
        let target = parse_team_address("agent:writer/spawned-draft-3f9a")
            .unwrap()
            .unwrap();
        assert_eq!(target.agent, "writer");
        assert_eq!(target.session.as_deref(), Some("spawned-draft-3f9a"));
        assert_eq!(target.address(), "agent:writer/spawned-draft-3f9a");
    }

    #[test]
    fn a_session_address_may_itself_contain_slashes() {
        let target = parse_team_address("agent:writer/a/b").unwrap().unwrap();
        assert_eq!(target.session.as_deref(), Some("a/b"));
    }

    #[test]
    fn the_main_session_is_the_teammates_main() {
        let target = parse_team_address("agent:writer/main").unwrap().unwrap();
        assert_eq!(target.session, None);
    }

    #[test]
    fn other_address_forms_are_not_teammate_addresses() {
        for other in [
            "main",
            "spawned-a-1",
            "a2a:writer",
            "artifact:wiki",
            "owner",
        ] {
            assert!(parse_team_address(other).is_none(), "{other}");
        }
    }

    #[test]
    fn a_remote_agent_and_a_teammate_never_parse_as_each_other() {
        assert!(parse_team_address("a2a:agent:writer").is_none());
        assert!(parse_team_address("agent-writer").is_none());
        assert!(!crate::a2a::client::config::is_valid_agent_name(
            "agent:writer"
        ));
        assert!(!crate::a2a::client::config::is_valid_agent_name("writer/x"));
    }

    #[test]
    fn malformed_teammate_addresses_say_what_is_wrong() {
        for (bad, needle) in [
            ("agent:", "names no teammate"),
            ("agent:/x", "names no teammate"),
            ("agent:Bad_Name", "lowercase"),
            ("agent:-x", "hyphen"),
            ("agent:writer/", "empty session address"),
        ] {
            let error = parse_team_address(bad).unwrap().unwrap_err().to_string();
            assert!(error.contains(needle), "{bad}: {error}");
        }
    }

    #[test]
    fn the_sender_is_qualified_by_agent_for_main_and_for_sessions() {
        assert_eq!(
            qualify_sender("alpha", &SessionAddress::from(MAIN_ADDRESS)),
            "agent:alpha"
        );
        assert_eq!(
            qualify_sender("alpha", &SessionAddress::from("spawned-x-1")),
            "agent:alpha/spawned-x-1"
        );
    }

    #[tokio::test]
    async fn a_team_of_one_has_no_teammates_and_no_block() {
        let link = TeamLink::alone("solo");
        assert!(link.teammates().is_empty());
        assert!(link.prompt_block().is_none());
        let target = TeamTarget {
            agent: "ghost".to_string(),
            session: None,
        };
        let error = link
            .send(&SessionAddress::from(MAIN_ADDRESS), &target, "hi".into(), 1)
            .await
            .unwrap_err();
        assert!(matches!(error, TeamSendError::UnknownAgent { .. }));
        assert!(error.to_string().contains("no teammates"));
    }

    #[tokio::test]
    async fn addressing_yourself_is_refused_with_the_local_form() {
        let link = TeamLink::alone("solo");
        let target = TeamTarget {
            agent: "solo".to_string(),
            session: None,
        };
        let error = link
            .send(&SessionAddress::from(MAIN_ADDRESS), &target, "hi".into(), 1)
            .await
            .unwrap_err();
        assert!(matches!(error, TeamSendError::ToSelf(_)));
        assert!(error.to_string().contains("\"main\""));
    }
}
