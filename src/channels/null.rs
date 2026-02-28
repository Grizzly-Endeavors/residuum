//! No-op reply handle for background turns (pulse, actions, sub-agents).

use async_trait::async_trait;

use super::types::ReplyHandle;

/// A reply handle that discards all output.
///
/// Used for background pulse and action turns where no user is watching,
/// and for sub-agent turns that run without a connected channel.
pub struct NullReplyHandle;

#[async_trait]
impl ReplyHandle for NullReplyHandle {
    async fn send_response(&self, _content: &str) {}

    async fn send_typing(&self) {}

    async fn send_system_event(&self, _source: &str, _content: &str) {}
}
