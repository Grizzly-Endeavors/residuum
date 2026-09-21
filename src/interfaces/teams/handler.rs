//! Inbound activity handling: authentication, access control, and dispatch to the agent.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};

use crate::inference::{ImageData, MessageSender};
use crate::interfaces::attachment::{
    AttachmentInfo, finalize_attachment, format_failed_attachment_line, save_attachment_bytes,
};
use crate::interfaces::types::MessageOrigin;

use super::TeamsRuntime;
use super::activity::{Activity, Attachment, ChannelAccount, ConversationKind};
use super::auth::AuthError;
use super::context_buffer::{BufferedMessage, render_context};
use super::store::{ConversationRef, Owner};

/// Attachment content type Teams uses for files shared in a chat.
const FILE_DOWNLOAD_INFO: &str = "application/vnd.microsoft.teams.file.download.info";

/// `POST /api/teams/messages` — the bot's messaging endpoint.
///
/// Authenticates and queues the activity, then acknowledges immediately; the
/// Bot Connector retries anything not acknowledged within ~15 seconds, so
/// slow work (attachment downloads) must not happen on this path.
pub(super) async fn messages_endpoint(
    State(rt): State<Arc<TeamsRuntime>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let activity: Activity = match serde_json::from_slice(&body) {
        Ok(activity) => activity,
        Err(e) => {
            tracing::warn!(error = %e, "teams request body is not a valid activity");
            return StatusCode::BAD_REQUEST;
        }
    };
    let Some(service_url) = activity.service_url.as_deref() else {
        tracing::warn!(kind = %activity.kind, "teams activity has no serviceUrl");
        return StatusCode::BAD_REQUEST;
    };

    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if let Err(e) = rt.validator.validate(authorization, service_url).await {
        if let AuthError::KeyFetch(reason) = &e {
            // Our failure, not the caller's: 503 makes the connector retry later.
            tracing::error!(error = %reason, "could not fetch Microsoft's signing keys; teams messages cannot be verified");
            return StatusCode::SERVICE_UNAVAILABLE;
        }
        tracing::warn!(reason = %e, "rejected unauthenticated teams request");
        return StatusCode::UNAUTHORIZED;
    }

    if activity.channel_id.as_deref() != Some("msteams") {
        tracing::warn!(channel = ?activity.channel_id, "ignoring non-Teams activity");
        return StatusCode::BAD_REQUEST;
    }
    if activity.tenant_id() != Some(rt.cfg.tenant_id.as_str()) {
        tracing::warn!(
            tenant = ?activity.tenant_id(),
            "rejected teams activity from a tenant other than the configured one"
        );
        return StatusCode::FORBIDDEN;
    }

    if rt.inbound_tx.try_send(activity).is_err() {
        tracing::warn!("teams inbound queue full; asking the connector to retry");
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    StatusCode::OK
}

/// Process one authenticated activity. Runs on the single inbound worker so
/// messages reach the agent in the order Teams delivered them.
pub(super) async fn process_activity(rt: &TeamsRuntime, activity: Activity) {
    match activity.kind.as_str() {
        "message" => handle_message(rt, activity).await,
        "conversationUpdate" => handle_membership(rt, &activity).await,
        "installationUpdate" => handle_installation(rt, &activity).await,
        other => tracing::debug!(kind = %other, "ignoring teams activity type"),
    }
}

/// Stable ID for a conversation: channel thread replies carry
/// `;messageid=…` suffixes that identify the thread, not the channel.
fn base_conversation_id(conversation_id: &str) -> &str {
    conversation_id
        .split_once(';')
        .map_or(conversation_id, |(base, _)| base)
}

fn conversation_ref(activity: &Activity) -> Option<ConversationRef> {
    Some(ConversationRef {
        conversation_id: activity.conversation.as_ref()?.id.clone(),
        service_url: activity.service_url.clone()?,
        kind: activity.conversation_kind(),
        label: activity.location_label(),
    })
}

fn is_bot(activity: &Activity, account: &ChannelAccount) -> bool {
    activity
        .recipient
        .as_ref()
        .is_some_and(|bot| bot.id == account.id)
}

/// Keep the store's reference for a conversation current, keyed by its base ID.
async fn remember_conversation(rt: &TeamsRuntime, reference: &ConversationRef) {
    let stored = ConversationRef {
        conversation_id: base_conversation_id(&reference.conversation_id).to_string(),
        ..reference.clone()
    };
    if let Err(e) = rt.store.remember(stored).await {
        tracing::warn!(error = %e, conversation = %reference.label, "failed to save teams conversation reference");
    }
}

async fn handle_membership(rt: &TeamsRuntime, activity: &Activity) {
    let Some(reference) = conversation_ref(activity) else {
        return;
    };
    if activity.members_added.iter().any(|m| is_bot(activity, m)) {
        tracing::info!(conversation = %reference.label, "teams bot added to a conversation");
        remember_conversation(rt, &reference).await;
    }
    if activity.members_removed.iter().any(|m| is_bot(activity, m)) {
        tracing::info!(conversation = %reference.label, "teams bot removed from a conversation");
        let base = base_conversation_id(&reference.conversation_id);
        if let Err(e) = rt.store.forget(base).await {
            tracing::warn!(error = %e, "failed to forget teams conversation");
        }
        rt.buffer.drain(base);
    }
}

async fn handle_installation(rt: &TeamsRuntime, activity: &Activity) {
    let Some(reference) = conversation_ref(activity) else {
        return;
    };
    match activity.action.as_deref() {
        Some(action) if action.starts_with("add") => {
            tracing::info!(conversation = %reference.label, "teams app installed");
            remember_conversation(rt, &reference).await;
        }
        Some(action) if action.starts_with("remove") => {
            tracing::info!(conversation = %reference.label, "teams app uninstalled");
            if let Err(e) = rt
                .store
                .forget(base_conversation_id(&reference.conversation_id))
                .await
            {
                tracing::warn!(error = %e, "failed to forget teams conversation");
            }
        }
        _ => {}
    }
}

/// Who sent a message, relative to the bot's owner.
enum Standing {
    Owner,
    Other { owner: Option<Owner> },
}

async fn standing_of(rt: &TeamsRuntime, sender_aad: Option<&str>) -> Standing {
    let owner = rt.store.owner().await;
    match (&owner, sender_aad) {
        (Some(o), Some(aad)) if o.aad_object_id == aad => Standing::Owner,
        _ => Standing::Other { owner },
    }
}

/// A message the bot has decided to act on.
struct Incoming {
    activity: Activity,
    from: ChannelAccount,
    sender_name: String,
    reference: ConversationRef,
    base_id: String,
    text: String,
}

async fn handle_message(rt: &TeamsRuntime, activity: Activity) {
    let (Some(from), Some(reference)) = (activity.from.clone(), conversation_ref(&activity)) else {
        tracing::debug!("teams message without sender or conversation, ignoring");
        return;
    };
    let sender_name = from.name.clone().unwrap_or_else(|| "someone".to_string());
    remember_conversation(rt, &reference).await;
    if reference.kind == ConversationKind::Personal {
        claim_owner_if_unset(rt, &from, &sender_name, &reference).await;
    }

    let incoming = Incoming {
        base_id: base_conversation_id(&reference.conversation_id).to_string(),
        text: activity.text_without_bot_mention(),
        activity,
        from,
        sender_name,
        reference,
    };

    // In shared conversations the bot only acts on @mentions; everything
    // else is held as context for the next one.
    if incoming.reference.kind != ConversationKind::Personal && !incoming.activity.mentions_bot() {
        buffer_for_context(rt, &incoming);
        return;
    }

    let Some(standing) = permitted_standing(rt, &incoming).await else {
        return;
    };
    if let Some(command) = incoming.text.strip_prefix('/') {
        let reply = run_command(rt, command, &standing, &incoming.sender_name).await;
        rt.send_text(&incoming.reference, &reply).await;
        return;
    }
    publish_to_agent(rt, incoming).await;
}

fn buffer_for_context(rt: &TeamsRuntime, incoming: &Incoming) {
    let shared = shared_file_names(&incoming.activity.attachments);
    let body = [incoming.text.as_str(), shared.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if body.is_empty() {
        return;
    }
    rt.buffer.record(
        &incoming.base_id,
        BufferedMessage {
            sender: incoming.sender_name.clone(),
            text: body,
            at: crate::time::now_local(rt.tz),
        },
    );
}

/// The sender's standing if they may use the bot; otherwise tells them why
/// not and returns `None`.
async fn permitted_standing(rt: &TeamsRuntime, incoming: &Incoming) -> Option<Standing> {
    let standing = standing_of(rt, incoming.from.aad_object_id.as_deref()).await;
    let Standing::Other { owner } = &standing else {
        return Some(standing);
    };
    if rt.cfg.respond_to_others {
        return Some(standing);
    }
    tracing::info!(
        sender = %incoming.sender_name,
        conversation = %incoming.reference.label,
        "teams message from someone other than the owner; respond_to_others is off"
    );
    let reply = match owner {
        Some(owner) => format!("I only take requests from {}.", owner.name),
        None => "I'm not set up yet. My owner needs to send me a direct message first.".to_string(),
    };
    rt.send_text(&incoming.reference, &reply).await;
    None
}

async fn run_command(
    rt: &TeamsRuntime,
    command: &str,
    standing: &Standing,
    sender_name: &str,
) -> String {
    if !matches!(standing, Standing::Owner) {
        return "Only my owner can run commands.".to_string();
    }
    let (name, args) = match command.split_once(' ') {
        Some((name, args)) => (name, Some(args.trim())),
        None => (command, None),
    };
    let dispatch = crate::interfaces::CommandDispatch {
        reload_tx: &rt.reload_tx,
        command_tx: &rt.command_tx,
        stop_tx: &rt.stop_tx,
        inbox_dir: &rt.inbox_dir,
        tz: rt.tz,
    };
    crate::interfaces::run_chat_command(name, args, &dispatch, super::ENDPOINT, sender_name).await
}

async fn publish_to_agent(rt: &TeamsRuntime, incoming: Incoming) {
    let Incoming {
        activity,
        from,
        sender_name,
        reference,
        base_id,
        text: mut content,
    } = incoming;

    let images = collect_attachments(rt, &activity.attachments, &mut content, &sender_name).await;
    if content.trim().is_empty() && images.is_empty() {
        tracing::debug!(conversation = %reference.label, "teams message had no content, dropping");
        return;
    }

    let background = match reference.kind {
        ConversationKind::Personal => None,
        ConversationKind::GroupChat | ConversationKind::Channel => {
            render_context(&reference.label, &rt.buffer.drain(&base_id))
        }
    };
    let correlation_id = format!(
        "teams-{}",
        activity
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    );
    rt.track_reply_target(&correlation_id, reference.clone());

    let event = crate::bus::MessageEvent {
        id: correlation_id,
        content,
        origin: MessageOrigin {
            endpoint: super::ENDPOINT.to_string(),
            sender: Some(MessageSender {
                name: sender_name.clone(),
                id: from.aad_object_id.unwrap_or(from.id),
                interface: super::ENDPOINT.to_string(),
                location: Some(reference.label.clone()),
            }),
        },
        timestamp: crate::time::now_local(rt.tz),
        images,
        context: background,
    };
    if let Err(e) = rt
        .publisher
        .publish(crate::bus::topics::UserMessage, event)
        .await
    {
        tracing::error!(error = %e, sender = %sender_name, "failed to publish teams message to the agent");
        rt.send_text(
            &reference,
            "Something went wrong handing your message to the agent. Please try again.",
        )
        .await;
    }
}

async fn claim_owner_if_unset(
    rt: &TeamsRuntime,
    from: &ChannelAccount,
    sender_name: &str,
    reference: &ConversationRef,
) {
    let Some(aad) = &from.aad_object_id else {
        return;
    };
    let owner = Owner {
        aad_object_id: aad.clone(),
        name: sender_name.to_string(),
        dm_conversation_id: base_conversation_id(&reference.conversation_id).to_string(),
    };
    match rt.store.claim_owner(owner).await {
        Ok(true) => {
            tracing::info!(owner = %sender_name, "teams owner set from first direct message");
        }
        Ok(false) => {}
        Err(e) => tracing::error!(error = %e, "failed to save teams owner"),
    }
}

/// `[shared file: a.pdf, b.png]` for files posted without mentioning the bot.
fn shared_file_names(attachments: &[Attachment]) -> String {
    let names: Vec<&str> = attachments
        .iter()
        .filter(|a| a.content_type == FILE_DOWNLOAD_INFO || a.content_type.starts_with("image/"))
        .map(|a| a.name.as_deref().unwrap_or("image"))
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("[shared file: {}]", names.join(", "))
    }
}

/// Download files and inline images into the inbox, appending a line per
/// attachment to `content` and returning images to show the model.
async fn collect_attachments(
    rt: &TeamsRuntime,
    attachments: &[Attachment],
    content: &mut String,
    sender_name: &str,
) -> Vec<ImageData> {
    let mut images = Vec::new();
    for attachment in attachments {
        let (url, filename, needs_bot_token) = if attachment.content_type == FILE_DOWNLOAD_INFO {
            let Some(url) = attachment
                .content
                .as_ref()
                .and_then(|c| c.get("downloadUrl"))
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            (url, attachment.name.as_deref().unwrap_or("file"), false)
        } else if attachment.content_type.starts_with("image/") {
            let Some(url) = attachment.content_url.as_deref() else {
                continue;
            };
            (url, attachment.name.as_deref().unwrap_or("image.png"), true)
        } else {
            // Cards and the HTML rendering of the message body carry no file.
            continue;
        };

        let content_type = if needs_bot_token {
            Some(attachment.content_type.clone())
        } else {
            Some(crate::interfaces::attachment::detect_mime_type(
                std::path::Path::new(filename),
            ))
        };
        let bytes = if needs_bot_token {
            rt.connector.download(url).await.map_err(|e| e.to_string())
        } else {
            fetch_public(&rt.http, url).await
        };
        let info = AttachmentInfo {
            filename: filename.to_string(),
            size: bytes
                .as_ref()
                .map_or(0, |b| u32::try_from(b.len()).unwrap_or(u32::MAX)),
            content_type,
        };
        let saved = match bytes {
            Ok(bytes) => save_attachment_bytes(&info, &bytes, &rt.inbox_dir).await,
            Err(e) => Err(e),
        };
        match saved {
            Ok(saved) => {
                if let Some(image) = finalize_attachment(
                    &saved,
                    &info,
                    content,
                    sender_name,
                    &rt.inbox_dir,
                    rt.tz,
                    "Teams",
                )
                .await
                {
                    images.push(image);
                }
            }
            Err(reason) => {
                tracing::warn!(filename = %info.filename, error = %reason, "failed to download teams attachment");
                content.push('\n');
                content.push_str(&format_failed_attachment_line(&info, &reason));
            }
        }
    }
    images
}

async fn fetch_public(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = http
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("download failed: {e}"))?;
    response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("download failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_id_strips_thread_suffix() {
        assert_eq!(
            base_conversation_id("19:abc@thread.tacv2;messageid=1700"),
            "19:abc@thread.tacv2"
        );
        assert_eq!(base_conversation_id("a:personal"), "a:personal");
    }

    #[test]
    fn shared_files_are_named_but_cards_are_not() {
        let attachments = vec![
            Attachment {
                content_type: FILE_DOWNLOAD_INFO.to_string(),
                content_url: None,
                name: Some("plan.pdf".to_string()),
                content: None,
            },
            Attachment {
                content_type: "text/html".to_string(),
                content_url: None,
                name: None,
                content: None,
            },
        ];
        assert_eq!(shared_file_names(&attachments), "[shared file: plan.pdf]");
        assert_eq!(shared_file_names(&[]), "");
    }
}
