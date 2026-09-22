//! Teams conversation references and the persisted Teams state.
//!
//! Proactive messages (scheduled results, `send_message`) need a stored
//! conversation reference because Teams gives the bot no way to open a chat
//! with nothing to go on.

use serde::{Deserialize, Serialize};

use crate::interfaces::chat_state::{ChatStateStore, ConversationRecord};
use crate::interfaces::types::ConversationKind;

/// Everything needed to post into a conversation later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ConversationRef {
    pub(super) conversation_id: String,
    pub(super) service_url: String,
    pub(super) kind: ConversationKind,
    /// Human-readable location, e.g. `"#builds (Eng Team)"`.
    pub(super) label: String,
}

impl ConversationRecord for ConversationRef {
    fn kind(&self) -> ConversationKind {
        self.kind
    }

    fn label(&self) -> &str {
        &self.label
    }
}

/// The Teams owner and every conversation reference, saved in `teams_state.json`.
pub(super) type TeamsStore = ChatStateStore<ConversationRef>;
