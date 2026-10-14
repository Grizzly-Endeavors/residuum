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

    fn display_name(&self) -> &'static str {
        "Microsoft Teams"
    }

    fn explain_failure(&self, reason: &str) -> Option<String> {
        explain_connector_failure(reason, &self.cfg.app_id)
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

/// The owner's fix for a send failure whose cause Microsoft names with a
/// known Entra error code. `app_id` is the bot's configured app (bot) ID.
fn explain_connector_failure(reason: &str, app_id: &str) -> Option<String> {
    // AADSTS7000229: the bot's app registration has no service principal in
    // the tenant, so Entra refuses it a Bot Connector token and every reply
    // fails.
    if reason.contains("AADSTS7000229") {
        return Some(format!(
            "Microsoft won't let the bot sign in because its app isn't set up in your \
             organization's Microsoft account directory. Someone with admin rights there can fix \
             it with this Azure CLI command, then send the bot a message to check: \
             az ad sp create --id {app_id}"
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_ID: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn missing_service_principal_names_the_command_for_this_bot() {
        let reason = "could not get a bot token from Microsoft: 401 Unauthorized: \
                      {\"error\":\"invalid_client\",\"error_description\":\"AADSTS7000229: \
                      The client application 11111111-2222-3333-4444-555555555555 is missing \
                      service principal in the tenant.\"}";

        let fix = explain_connector_failure(reason, APP_ID).expect("a known cause has a fix");

        assert!(
            fix.contains(&format!("az ad sp create --id {APP_ID}")),
            "the fix should name the command for this bot: {fix}"
        );
        assert!(
            !fix.contains("AADSTS"),
            "the owner-facing fix should not repeat Microsoft's error code: {fix}"
        );
        assert!(
            !fix.contains('`'),
            "web UI errors render as plain text, so markdown would show literally: {fix}"
        );
    }

    #[test]
    fn unrecognized_failure_has_no_fix() {
        let reason = "bot connector returned 403 Forbidden: {\"error\":{\"code\":\"BotDisabled\"}}";

        assert_eq!(explain_connector_failure(reason, APP_ID), None);
    }
}
