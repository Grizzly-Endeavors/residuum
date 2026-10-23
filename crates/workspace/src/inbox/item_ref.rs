//! Refs: one string that names an inbox item in any agent's workspace.
//!
//! A ref is `<agent>/<kind>/<id>`: the agent's folder name, `user` for the
//! agent's `inbox/user/` (messages the agent sent the user) or `agent` for its
//! `inbox/agent/` (messages to the agent), and the item's filename stem. An
//! active item and its archived self share the ref, since archiving keeps the id.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use residuum_config::WorkspaceLayout;
use serde::{Deserialize, Serialize};

/// Which of an agent's two inboxes an item lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InboxKind {
    /// `inbox/user/`: what the agent sent the user.
    User,
    /// `inbox/agent/`: what was sent to the agent.
    Agent,
}

impl InboxKind {
    /// The kind's segment in a ref: `user` or `agent`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }

    /// The directory holding this kind's items, active or archived.
    #[must_use]
    pub fn items_dir(self, layout: &WorkspaceLayout, archived: bool) -> PathBuf {
        match (self, archived) {
            (Self::User, false) => layout.user_inbox_dir(),
            (Self::User, true) => layout.user_inbox_archive_dir(),
            (Self::Agent, false) => layout.agent_inbox_dir(),
            (Self::Agent, true) => layout.agent_inbox_archive_dir(),
        }
    }

    /// The directory holding this kind's per-item attachment directories,
    /// active or archived.
    #[must_use]
    pub fn attachments_dir(self, layout: &WorkspaceLayout, archived: bool) -> PathBuf {
        match (self, archived) {
            (Self::User, false) => layout.user_inbox_attachments_dir(),
            (Self::User, true) => layout.user_inbox_archive_attachments_dir(),
            (Self::Agent, false) => layout.agent_inbox_attachments_dir(),
            (Self::Agent, true) => layout.agent_inbox_archive_attachments_dir(),
        }
    }
}

impl fmt::Display for InboxKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A ref that couldn't be parsed or built. The message names the ref and what
/// is wrong with it, so it can be shown to an agent or a user as is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboxRefError {
    /// Not three `/`-separated parts.
    #[error("'{0}' is not an inbox item ref; a ref looks like '<agent>/<user|agent>/<id>'")]
    Malformed(String),
    /// The agent part is not a valid agent folder name.
    #[error("inbox item ref '{reference}' names an invalid agent: {problem}")]
    InvalidAgent {
        /// The ref as given.
        reference: String,
        /// What is wrong with the agent name.
        problem: String,
    },
    /// The kind part is neither `user` nor `agent`.
    #[error("inbox item ref '{reference}' has kind '{kind}'; it must be 'user' or 'agent'")]
    InvalidKind {
        /// The ref as given.
        reference: String,
        /// The kind part as given.
        kind: String,
    },
    /// The id part can't be an item's filename stem.
    #[error("inbox item ref '{reference}' has an invalid id: {problem}")]
    InvalidId {
        /// The ref as given.
        reference: String,
        /// What is wrong with the id.
        problem: String,
    },
}

/// One inbox item in one agent's workspace: `<agent>/<user|agent>/<id>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InboxRef {
    agent: String,
    kind: InboxKind,
    id: String,
}

impl InboxRef {
    /// A ref to item `id` in `agent`'s `kind` inbox.
    ///
    /// # Errors
    /// [`InboxRefError::InvalidAgent`] when `agent` is not a valid agent folder
    /// name, [`InboxRefError::InvalidId`] when `id` is not a safe filename stem.
    pub fn new(
        agent: impl Into<String>,
        kind: InboxKind,
        id: impl Into<String>,
    ) -> Result<Self, InboxRefError> {
        let agent = agent.into();
        let id = id.into();
        let reference = || format!("{agent}/{kind}/{id}");
        if let Err(problem) = residuum_config::paths::validate_agent_name(&agent) {
            return Err(InboxRefError::InvalidAgent {
                reference: reference(),
                problem,
            });
        }
        if let Err(problem) = validate_item_id(&id) {
            return Err(InboxRefError::InvalidId {
                reference: reference(),
                problem,
            });
        }
        Ok(Self { agent, kind, id })
    }

    /// Parse `<agent>/<user|agent>/<id>`.
    ///
    /// # Errors
    /// [`InboxRefError::Malformed`] when `raw` is not three `/`-separated
    /// parts, and the errors of [`Self::new`] for a bad part.
    pub fn parse(raw: &str) -> Result<Self, InboxRefError> {
        let mut parts = raw.split('/');
        let (Some(agent), Some(kind), Some(id), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(InboxRefError::Malformed(raw.to_string()));
        };
        let kind = match kind {
            "user" => InboxKind::User,
            "agent" => InboxKind::Agent,
            other => {
                return Err(InboxRefError::InvalidKind {
                    reference: raw.to_string(),
                    kind: other.to_string(),
                });
            }
        };
        Self::new(agent, kind, id)
    }

    /// The agent folder name whose workspace holds the item.
    #[must_use]
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// Which of the agent's inboxes holds the item.
    #[must_use]
    pub fn kind(&self) -> InboxKind {
        self.kind
    }

    /// The item's filename stem.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// An item id must be usable as a filename stem in one directory: not empty,
/// not `.`, no path separators or NUL, and no `..`.
pub(super) fn validate_item_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("the id is empty".to_string());
    }
    if id == "." {
        return Err("the id is '.'".to_string());
    }
    if id.contains(['/', '\\', '\0']) {
        return Err(format!("'{id}' contains a path separator or NUL"));
    }
    if id.contains("..") {
        return Err(format!("'{id}' contains '..'"));
    }
    Ok(())
}

impl fmt::Display for InboxRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.agent, self.kind, self.id)
    }
}

impl FromStr for InboxRef {
    type Err = InboxRefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for InboxRef {
    type Error = InboxRefError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<InboxRef> for String {
    fn from(value: InboxRef) -> Self {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_round_trip() {
        for raw in [
            "atlas/user/20260225_daily_digest",
            "scout-2/agent/20260225_hello_2",
            "a/agent/20260225",
        ] {
            let parsed = InboxRef::parse(raw).unwrap();
            assert_eq!(parsed.to_string(), raw);
            assert_eq!(parsed, raw.parse::<InboxRef>().unwrap());
        }
        let parsed = InboxRef::parse("atlas/user/x").unwrap();
        assert_eq!(parsed.agent(), "atlas");
        assert_eq!(parsed.kind(), InboxKind::User);
        assert_eq!(parsed.id(), "x");
    }

    #[test]
    fn serde_uses_the_string_form() {
        let r = InboxRef::new("atlas", InboxKind::Agent, "note").unwrap();
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "\"atlas/agent/note\"");
        assert_eq!(serde_json::from_str::<InboxRef>(&json).unwrap(), r);
        assert!(serde_json::from_str::<InboxRef>("\"atlas/inbox/note\"").is_err());
    }

    #[test]
    fn parse_rejects_wrong_shape() {
        for raw in ["", "atlas", "atlas/user", "atlas/user/a/b", "/user/x"] {
            assert!(InboxRef::parse(raw).is_err(), "{raw:?} should be rejected");
        }
        assert!(matches!(
            InboxRef::parse("atlas/user"),
            Err(InboxRefError::Malformed(_))
        ));
        assert!(matches!(
            InboxRef::parse("atlas/user/a/b"),
            Err(InboxRefError::Malformed(_))
        ));
    }

    #[test]
    fn parse_rejects_bad_parts() {
        assert!(matches!(
            InboxRef::parse("atlas/inbox/x"),
            Err(InboxRefError::InvalidKind { .. })
        ));
        assert!(matches!(
            InboxRef::parse("Atlas Desk/user/x"),
            Err(InboxRefError::InvalidAgent { .. })
        ));
        assert!(matches!(
            InboxRef::parse("hub/user/x"),
            Err(InboxRefError::InvalidAgent { .. })
        ));
        assert!(matches!(
            InboxRef::parse("atlas/user/"),
            Err(InboxRefError::InvalidId { .. })
        ));
        assert!(matches!(
            InboxRef::parse("atlas/user/.."),
            Err(InboxRefError::InvalidId { .. })
        ));
        assert!(matches!(
            InboxRef::parse("atlas/user/."),
            Err(InboxRefError::InvalidId { .. })
        ));
        assert!(matches!(
            InboxRef::new("atlas", InboxKind::User, "a\\b"),
            Err(InboxRefError::InvalidId { .. })
        ));
        assert!(matches!(
            InboxRef::new("atlas", InboxKind::User, "a/b"),
            Err(InboxRefError::InvalidId { .. })
        ));
    }

    #[test]
    fn errors_read_as_sentences_naming_the_ref() {
        let err = InboxRef::parse("atlas/inbox/x").unwrap_err().to_string();
        assert!(err.contains("atlas/inbox/x"), "{err}");
        assert!(err.contains("'user' or 'agent'"), "{err}");
    }
}
