use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, PoisonError, RwLock};

use crate::messaging::{AgentMessenger, DeliveryOutcome, SendError};
use residuum_agent::TeamRoster;
use residuum_bus::SessionAddress;
use residuum_bus::session::{MAIN_ADDRESS, TEAMMATE_SENDER_CATEGORY, TEAMMATE_SENDER_PREFIX};
use residuum_config::canonicalize_display_name;

use super::directory::{DirectoryHandle, TeamDirectory};
use super::types::{AgentState, AgentSummary, LifecycleError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamTarget {
    pub agent: String,
    pub session: Option<String>,
}

impl TeamTarget {
    #[must_use]
    pub fn address(&self) -> String {
        match &self.session {
            Some(session) => format!("{TEAMMATE_SENDER_PREFIX}{}/{session}", self.agent),
            None => format!("{TEAMMATE_SENDER_PREFIX}{}", self.agent),
        }
    }

    fn local_address(&self) -> &str {
        self.session.as_deref().unwrap_or(MAIN_ADDRESS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TeamAddressError {
    #[error(
        "the address 'agent:' names no teammate. Use \"agent:<name>\" for a teammate's main or \
         \"agent:<name>/<session-address>\" for one of its sessions."
    )]
    MissingName,
    #[error("{0}. Use \"agent:<name>\" or \"agent:<name>/<session-address>\".")]
    InvalidName(String),
    #[error(
        "the address '{0}' has an empty session address after the '/'. Use \"agent:<name>\" for \
         the teammate's main or \"agent:<name>/<session-address>\" for one of its sessions."
    )]
    MissingSession(String),
}

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
    if let Err(problem) = canonicalize_display_name(name) {
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

#[derive(Debug, Clone, thiserror::Error)]
pub enum TeamSendError {
    #[error("'agent:{0}' is you. Use \"main\" or a session address to reach your own agent.")]
    ToSelf(String),
    #[error("no teammate named '{name}'. {}", teammates_phrase(teammates))]
    UnknownAgent {
        name: String,
        teammates: Vec<String>,
    },
    #[error("{}", not_running_text(name, state))]
    NotRunning { name: String, state: String },
    #[error(
        "teammate '{agent}' has no session '{session}'. Use the exact address from a message it \
         sent you."
    )]
    NoSuchSession { agent: String, session: String },
    #[error("{target} is busy, try again shortly")]
    Busy { target: String },
    #[error("{0}")]
    Delivery(SendError),
}

fn not_running_text(name: &str, state: &str) -> String {
    if state == "starting" {
        format!("teammate '{name}' is starting; nothing was queued. Try again shortly.")
    } else {
        format!(
            "teammate '{name}' is {state}; nothing was queued. Tell the user if this message \
             matters; they can start it from Home in the web UI."
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

pub struct TeamRouter {
    messengers: RwLock<BTreeMap<String, Arc<AgentMessenger>>>,
    directory: DirectoryHandle,
}

impl TeamRouter {
    #[must_use]
    pub fn new_shared(directory: DirectoryHandle) -> Arc<Self> {
        Arc::new(Self {
            messengers: RwLock::new(BTreeMap::new()),
            directory,
        })
    }

    pub fn register(&self, name: &str, messenger: Arc<AgentMessenger>) {
        self.messengers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_string(), messenger);
    }

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

    fn directory(&self) -> Option<Arc<dyn TeamDirectory>> {
        self.directory.get()
    }

    fn members(&self) -> Vec<AgentSummary> {
        self.directory()
            .map(|directory| directory.list())
            .unwrap_or_default()
    }

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
        let (slug, messenger) = self.reachable_messenger(me, &target.agent)?;
        if slug == me {
            return Err(TeamSendError::ToSelf(me.to_string()));
        }

        let qualified = qualify_sender(me, sender);
        let outcome = messenger
            .send(
                target.local_address(),
                residuum_bus::AgentMessageEvent::new(
                    SessionAddress::from(qualified),
                    TEAMMATE_SENDER_CATEGORY,
                    content,
                    hop_count,
                ),
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
                    own.note_refused_send(sender, &target.address(), hop_count)
                        .await;
                }
                Err(TeamSendError::Delivery(error))
            }
        }
    }

    fn reachable_messenger(
        &self,
        me: &str,
        name: &str,
    ) -> Result<(String, Arc<AgentMessenger>), TeamSendError> {
        let unknown = || TeamSendError::UnknownAgent {
            name: name.to_string(),
            teammates: self
                .members()
                .into_iter()
                .filter(|member| member.name != me)
                .map(|member| member.label().to_string())
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
        let slug = summary.name.clone();
        let shown = summary.label().to_string();
        match (summary.state, self.messenger(&slug)) {
            (AgentState::Running, Some(messenger)) => Ok((slug, messenger)),
            (AgentState::Running, None) => Err(TeamSendError::NotRunning {
                name: shown,
                state: "stopping".to_string(),
            }),
            (state, _) => Err(TeamSendError::NotRunning {
                name: shown,
                state: state.to_string(),
            }),
        }
    }
}

fn qualify_sender(me: &str, sender: &SessionAddress) -> String {
    if sender.as_ref() == MAIN_ADDRESS {
        format!("{TEAMMATE_SENDER_PREFIX}{me}")
    } else {
        format!("{TEAMMATE_SENDER_PREFIX}{me}/{sender}")
    }
}

#[derive(Clone)]
pub struct TeamLink {
    me: String,
    router: Arc<TeamRouter>,
}

impl TeamLink {
    #[must_use]
    pub fn new(me: impl Into<String>, router: Arc<TeamRouter>) -> Self {
        Self {
            me: me.into(),
            router,
        }
    }

    #[must_use]
    pub fn alone(me: impl Into<String>) -> Self {
        Self::new(me, TeamRouter::new_shared(DirectoryHandle::unbound()))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.me
    }

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

    #[must_use]
    pub fn members(&self) -> Vec<AgentSummary> {
        self.router.members()
    }

    #[must_use]
    pub fn teammates(&self) -> Vec<AgentSummary> {
        self.members()
            .into_iter()
            .filter(|member| member.name != self.me)
            .collect()
    }
}

impl TeamRoster for TeamLink {
    fn prompt_block(&self) -> Option<String> {
        let teammates = self.teammates();
        if teammates.is_empty() {
            return None;
        }
        let me = self
            .router
            .directory()
            .and_then(|directory| directory.summary(&self.me).ok())
            .map_or_else(|| self.me.clone(), |summary| summary.label().to_string());
        let mut block = format!(
            "You are \"{me}\". Teammates (message_agent to=\"agent:<name>\"; only running ones receive):"
        );
        for teammate in &teammates {
            let role = teammate.role.as_deref().unwrap_or("no role line yet");
            _ = write!(
                block,
                "\n- {} ({}): {role}",
                teammate.label(),
                teammate.state
            );
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
    }

    #[test]
    fn malformed_teammate_addresses_say_what_is_wrong() {
        for (bad, needle) in [
            ("agent:", "names no teammate"),
            ("agent:/x", "names no teammate"),
            ("agent:Bad_Name", "letters"),
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
        let actual = &link.teammates();
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
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
