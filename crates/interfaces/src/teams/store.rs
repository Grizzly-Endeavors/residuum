//! Teams conversation references and the persisted Teams state.
//!
//! Proactive messages (scheduled results, `send_message`) need a stored
//! conversation reference because Teams gives the bot no way to open a chat
//! with nothing to go on.

use serde::{Deserialize, Serialize};

use crate::chat_state::{ChatStateStore, ConversationRecord};
use residuum_bus::origin::ConversationKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConversationRef {
    pub(super) conversation_id: String,
    pub(super) service_url: String,
    pub(super) kind: ConversationKind,
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

pub(super) type TeamsStore = ChatStateStore<ConversationRef>;
