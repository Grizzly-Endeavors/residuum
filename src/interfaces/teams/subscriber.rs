//! Delivers agent output from the bus to Teams conversations.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::interfaces::attachment::FileAttachment;
use crate::interfaces::outbound::ChatOutbound;

use super::TeamsRuntime;
use super::connector::typing_activity;
use super::store::ConversationRef;

/// Per-message text budget. Teams rejects activities over ~28 KB; this
/// leaves room for the JSON envelope and multi-byte characters.
pub(super) const MAX_MESSAGE_BYTES: usize = 20_000;
/// Teams shows a typing indicator for roughly three seconds.
const TYPING_INTERVAL: Duration = Duration::from_secs(3);

pub(super) async fn run_teams_subscriber(
    rt: Arc<TeamsRuntime>,
    subs: crate::interfaces::BaseSubscribers,
) {
    crate::interfaces::outbound::run(subs, rt).await;
}

#[async_trait]
impl ChatOutbound for Arc<TeamsRuntime> {
    type Target = ConversationRef;

    fn name(&self) -> &'static str {
        "teams"
    }

    fn unknown_conversation_reason(&self) -> &'static str {
        "the bot no longer knows that conversation"
    }

    fn publisher(&self) -> &crate::bus::Publisher {
        &self.publisher
    }

    async fn reply_target(&self, correlation_id: &str) -> Option<ConversationRef> {
        self.target_for(correlation_id).await
    }

    fn release_reply(&self, correlation_id: &str) {
        self.reply_targets.release(correlation_id);
    }

    async fn conversation_target(&self, conversation_id: &str) -> Option<ConversationRef> {
        self.store.conversation(conversation_id).await
    }

    async fn describe(&self, _conversation_id: &str, target: &ConversationRef) -> String {
        target.label.clone()
    }

    fn target_label(&self, target: &ConversationRef) -> String {
        target.label.clone()
    }

    fn start_typing(&self, target: ConversationRef) -> tokio::sync::watch::Sender<()> {
        let rt = Arc::clone(self);
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(());
        crate::util::spawn_in_span(async move {
            loop {
                if let Err(e) = rt
                    .connector
                    .send_activity(&target, &typing_activity())
                    .await
                {
                    tracing::trace!(error = %e, "teams typing indicator failed");
                }
                tokio::select! {
                    () = tokio::time::sleep(TYPING_INTERVAL) => {}
                    // Resolves with an error once the sender is dropped at turn end.
                    _ = stop_rx.changed() => break,
                }
            }
        });
        stop_tx
    }

    async fn send(
        &self,
        target: &ConversationRef,
        content: &str,
        attachment: Option<&FileAttachment>,
    ) -> Result<(), String> {
        let text = match attachment {
            Some(attachment) => attachment_fallback(content, attachment),
            None if content.is_empty() => return Ok(()),
            None => content.to_string(),
        };
        self.try_send_text(target, &text)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Teams cannot send files. The text the owner sees names the file and where
/// it was saved.
fn attachment_fallback(content: &str, attachment: &FileAttachment) -> String {
    tracing::warn!(
        file = %attachment.path.display(),
        "teams cannot deliver file attachments; sending the text with a note"
    );
    format!(
        "{content}\n\n_I made a file for you ({}), but I can't send files over Teams yet. \
         It's saved at `{}` and available in the web UI._",
        attachment.filename,
        attachment.path.display()
    )
    .trim_start()
    .to_string()
}
