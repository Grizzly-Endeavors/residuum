//! `/api/hub/mail/...`: the user's conversations with every agent.
//!
//! The routes read and change the agents' inbox files through
//! [`residuum_workspace::inbox::mail`], so they answer with any agent in any
//! state. A thread is named in a path by the percent-encoded ref of any of its
//! messages, and answers with its canonical `thread_id` (the root's ref).
//! Failures are `{ "error": message }`: `400` for a request that can't be
//! used (a blank body, no recipients, an unknown agent or upload), `404` for a
//! ref that names no message, and `500` when the files can't be read or
//! changed. A thread whose messages from agents were read, archived or
//! restored is counted again in those agents' overviews.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use super::error::json_error;
use crate::AgentDirectory;
use crate::mail::{
    AgentMailboxes, MailMessage, MailReplyResponse, MailSendResponse, MailThread, MailThreadPage,
    MailThreadSummary,
};
use crate::overview::TeamOverview;
use crate::uploads::{UploadError, UploadStore};
use residuum_workspace::inbox::InboxRef;
use residuum_workspace::inbox::mail::{
    self as domain, MailFolder, MailIndex, MailboxDirectory, OutgoingMail, SendMailError,
    ThreadFilter, ThreadUpdateError, UserMailSource,
};

/// How many threads a page holds when the request names no limit.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// The most threads a page holds; a larger limit is treated as this.
pub const MAX_PAGE_SIZE: usize = 200;

/// What the mail routes read and tell.
#[derive(Clone)]
pub(super) struct MailState {
    /// Where the agents' files are.
    pub directory: Arc<dyn AgentDirectory>,
    /// Told when the hub changes an agent's messages, so its unread count is
    /// read again.
    pub overview: Arc<TeamOverview>,
    /// The staged files a send or reply attaches.
    pub uploads: UploadStore,
}

impl MailState {
    fn mailboxes(&self) -> AgentMailboxes<'_> {
        AgentMailboxes(self.directory.as_ref())
    }
}

/// The thread list, a thread, its actions, and send.
pub(super) fn routes(state: MailState) -> Router {
    Router::new()
        .route("/api/hub/mail/threads", get(list_threads))
        .route("/api/hub/mail/threads/{thread}", get(get_thread))
        .route(
            "/api/hub/mail/threads/{thread}/reply",
            post(reply_to_thread),
        )
        .route("/api/hub/mail/threads/{thread}/read", post(read_thread))
        .route(
            "/api/hub/mail/threads/{thread}/archive",
            post(archive_thread),
        )
        .route(
            "/api/hub/mail/threads/{thread}/restore",
            post(restore_thread),
        )
        .route("/api/hub/mail/send", post(send))
        .with_state(state)
}

/// The query of `GET /api/hub/mail/threads`, kept as text so a value that
/// can't be used is answered with a JSON error that says which one.
#[derive(Deserialize)]
struct ListParams {
    folder: Option<String>,
    agent: Option<String>,
    before: Option<String>,
    limit: Option<String>,
}

/// Where a page stopped, carried in `next`: the latest time and the id of its
/// last thread, as `<unix milliseconds>:<thread_id>`. A ref may hold `:`, so
/// the id is everything after the first one.
#[derive(Debug, PartialEq, Eq)]
struct Cursor {
    latest_at_ms: i64,
    thread_id: InboxRef,
}

impl Cursor {
    fn after(thread: &domain::MailThread) -> Self {
        Self {
            latest_at_ms: thread.latest_at().timestamp_millis(),
            thread_id: thread.thread_id().clone(),
        }
    }

    fn render(&self) -> String {
        format!("{}:{}", self.latest_at_ms, self.thread_id)
    }

    fn parse(raw: &str) -> Option<Self> {
        let (at, id) = raw.split_once(':')?;
        Some(Self {
            latest_at_ms: at.parse().ok()?,
            thread_id: InboxRef::parse(id).ok()?,
        })
    }

    /// Whether `thread` comes after this cursor in the list's order: newest
    /// first, then by thread id.
    fn precedes(&self, thread: &domain::MailThread) -> bool {
        let at = thread.latest_at().timestamp_millis();
        at < self.latest_at_ms || (at == self.latest_at_ms && *thread.thread_id() > self.thread_id)
    }
}

/// A usable list request.
struct ListQuery {
    filter: ThreadFilter,
    before: Option<Cursor>,
    limit: usize,
}

impl ListParams {
    fn into_query(self, mailboxes: &AgentMailboxes<'_>) -> Result<ListQuery, String> {
        let folder = match self.folder.as_deref() {
            None | Some("inbox") => MailFolder::Inbox,
            Some("sent") => MailFolder::Sent,
            Some("archived") => MailFolder::Archived,
            Some(other) => {
                return Err(format!(
                    "the folder must be 'inbox', 'sent' or 'archived', not '{other}'"
                ));
            }
        };
        if let Some(agent) = &self.agent
            && mailboxes.mailbox(agent).is_none()
        {
            return Err(format!("there is no agent named '{agent}'"));
        }
        let limit = match self.limit.as_deref().map(str::parse::<usize>) {
            None => DEFAULT_PAGE_SIZE,
            Some(Ok(0)) => return Err("the limit must be at least 1".to_string()),
            Some(Ok(limit)) => limit.min(MAX_PAGE_SIZE),
            Some(Err(e)) => {
                let raw = self.limit.as_deref().unwrap_or_default();
                return Err(format!(
                    "the limit must be a whole number, not '{raw}' ({e})"
                ));
            }
        };
        let before = self
            .before
            .as_deref()
            .map(|raw| {
                Cursor::parse(raw)
                    .ok_or_else(|| "the before cursor isn't one this list returned".to_string())
            })
            .transpose()?;
        Ok(ListQuery {
            filter: ThreadFilter {
                folder: Some(folder),
                agent: self.agent,
            },
            before,
            limit,
        })
    }
}

/// `GET /api/hub/mail/threads` — one page of threads, newest first.
async fn list_threads(
    State(state): State<MailState>,
    params: Result<Query<ListParams>, QueryRejection>,
) -> Response {
    let mailboxes = state.mailboxes();
    let query = match params {
        Ok(Query(params)) => match params.into_query(&mailboxes) {
            Ok(query) => query,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
        },
        Err(rejection) => return json_error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    let index = MailIndex::load(&mailboxes).await;
    let mut threads = index
        .threads(&query.filter)
        .filter(|thread| query.before.as_ref().is_none_or(|c| c.precedes(thread)));
    let page: Vec<&domain::MailThread> = threads.by_ref().take(query.limit).collect();
    let next = match (threads.next(), page.last()) {
        (Some(_), Some(last)) => Some(Cursor::after(last).render()),
        _ => None,
    };
    Json(MailThreadPage {
        threads: page.into_iter().map(MailThreadSummary::of).collect(),
        next,
    })
    .into_response()
}

/// A request the routes refuse: the status and the plain message the user
/// sees as `{ "error": message }`.
struct Refusal {
    status: StatusCode,
    message: String,
}

impl Refusal {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// The `404` for a ref that names no message.
    fn no_message(raw: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            format!("There is no message '{raw}'. It may have been deleted."),
        )
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        json_error(self.status, self.message)
    }
}

/// The message ref in a path; one that doesn't parse names no message.
fn message_ref(raw: &str) -> Result<InboxRef, Refusal> {
    InboxRef::parse(raw).map_err(|_unparsed| Refusal::no_message(raw))
}

/// The thread of `index` holding the message `item`, which the path named as
/// `raw`.
fn thread_of<'a>(
    index: &'a MailIndex,
    item: &InboxRef,
    raw: &str,
) -> Result<&'a domain::MailThread, Refusal> {
    index
        .thread_containing(item)
        .ok_or_else(|| Refusal::no_message(raw))
}

/// `GET /api/hub/mail/threads/{ref}` — the whole thread holding the message.
async fn get_thread(
    State(state): State<MailState>,
    Path(raw): Path<String>,
) -> Result<Json<MailThread>, Refusal> {
    let item = message_ref(&raw)?;
    let index = MailIndex::load(&state.mailboxes()).await;
    let thread = thread_of(&index, &item, &raw)?;
    Ok(Json(MailThread::of(thread)))
}

/// Which thread change a route makes.
#[derive(Clone, Copy)]
enum ThreadChange {
    Read,
    Archive,
    Restore,
}

impl ThreadChange {
    fn verb(self) -> &'static str {
        match self {
            Self::Read => "mark the thread read",
            Self::Archive => "archive the thread",
            Self::Restore => "restore the thread",
        }
    }
}

/// Make `change` to the thread holding the message `raw` names, then count
/// again the unread messages of every agent that wrote in it.
async fn change_thread(
    state: &MailState,
    raw: &str,
    change: ThreadChange,
) -> Result<StatusCode, Refusal> {
    let item = message_ref(raw)?;
    let mailboxes = state.mailboxes();
    let index = MailIndex::load(&mailboxes).await;
    let thread = thread_of(&index, &item, raw)?;
    let result: Result<(), ThreadUpdateError> = match change {
        ThreadChange::Read => domain::mark_thread_read(&mailboxes, thread).await,
        ThreadChange::Archive => domain::archive_thread(&mailboxes, thread).await,
        ThreadChange::Restore => domain::restore_thread(&mailboxes, thread).await,
    };
    // Some messages may have changed even when others failed, so the counts
    // are read again either way.
    let writers: BTreeSet<&str> = thread
        .messages()
        .filter_map(|message| match &message.from {
            domain::MailSender::Agent(agent) => Some(agent.as_str()),
            domain::MailSender::User => None,
        })
        .collect();
    for agent in writers {
        state.overview.inbox_changed(agent).await;
    }
    match result {
        Ok(()) => {
            tracing::debug!(thread = %thread.thread_id(), action = change.verb(), "hub changed a mail thread");
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => {
            tracing::error!(thread = %thread.thread_id(), action = change.verb(), error = %e, failures = e.failures.len(), "couldn't change every message of a mail thread");
            Err(Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Couldn't {}: {e}", change.verb()),
            ))
        }
    }
}

/// `POST /api/hub/mail/threads/{ref}/read`.
async fn read_thread(
    State(state): State<MailState>,
    Path(raw): Path<String>,
) -> Result<StatusCode, Refusal> {
    change_thread(&state, &raw, ThreadChange::Read).await
}

/// `POST /api/hub/mail/threads/{ref}/archive`.
async fn archive_thread(
    State(state): State<MailState>,
    Path(raw): Path<String>,
) -> Result<StatusCode, Refusal> {
    change_thread(&state, &raw, ThreadChange::Archive).await
}

/// `POST /api/hub/mail/threads/{ref}/restore`.
async fn restore_thread(
    State(state): State<MailState>,
    Path(raw): Path<String>,
) -> Result<StatusCode, Refusal> {
    change_thread(&state, &raw, ThreadChange::Restore).await
}

/// The body of `POST /api/hub/mail/send`.
#[derive(Deserialize)]
struct SendRequest {
    to: Vec<String>,
    #[serde(default)]
    subject: String,
    body: String,
    #[serde(default)]
    attachments: Vec<String>,
}

/// The body of `POST /api/hub/mail/threads/{ref}/reply`.
#[derive(Deserialize)]
struct ReplyRequest {
    body: String,
    #[serde(default)]
    attachments: Vec<String>,
}

/// The request body, or the `400` that says why it can't be read.
fn request_body<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, Refusal> {
    request.map(|Json(body)| body).map_err(|rejection| {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            format!("The message couldn't be read: {}", rejection.body_text()),
        )
    })
}

/// The staged files `ids` name, in order, each once.
async fn staged_files(uploads: &UploadStore, ids: &[String]) -> Result<Vec<PathBuf>, Refusal> {
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        match uploads.resolve(id).await {
            Ok(staged) => files.push(staged.path),
            Err(UploadError::InvalidId(_) | UploadError::NotFound(_)) => {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "An attached file (upload '{id}') isn't available anymore. Attach it again and send."
                    ),
                ));
            }
            Err(e) => {
                tracing::error!(upload = %id, error = %e, "couldn't read a staged upload a message attaches");
                return Err(Refusal::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Couldn't read an attached file. Try sending again.",
                ));
            }
        }
    }
    Ok(files)
}

/// Remove the staged uploads a sent message copied. One that can't be removed
/// is logged; the store drops it when it expires.
async fn consume_uploads(uploads: &UploadStore, ids: &[String]) {
    let unique: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    for id in unique {
        if let Err(e) = uploads.remove(id).await {
            tracing::warn!(upload = %id, error = %e, "couldn't remove a staged upload after sending it; it expires on its own");
        }
    }
}

/// Send `mail`, and consume the uploads `upload_ids` once it went.
async fn send_and_consume(
    state: &MailState,
    mail: OutgoingMail,
    upload_ids: &[String],
) -> Result<domain::SentMail, Refusal> {
    match domain::send_mail(&state.mailboxes(), mail).await {
        Ok(sent) => {
            consume_uploads(&state.uploads, upload_ids).await;
            Ok(sent)
        }
        Err(e) => Err(send_refusal(&e)),
    }
}

fn send_refusal(error: &SendMailError) -> Refusal {
    match error {
        SendMailError::NoRecipients => Refusal::new(
            StatusCode::BAD_REQUEST,
            "Choose at least one agent to send the message to.",
        ),
        SendMailError::UnknownRecipient(agent) => Refusal::new(
            StatusCode::BAD_REQUEST,
            format!("There is no agent named '{agent}'."),
        ),
        SendMailError::BlankBody => Refusal::new(
            StatusCode::BAD_REQUEST,
            "Write a message before sending it.",
        ),
        SendMailError::Attachment(_) | SendMailError::Write(_) => {
            tracing::error!(error = %error, "couldn't send mail");
            Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("The message wasn't sent: {error}. Try again."),
            )
        }
    }
}

/// `POST /api/hub/mail/send` — a new message to one or more agents.
async fn send(
    State(state): State<MailState>,
    request: Result<Json<SendRequest>, JsonRejection>,
) -> Result<Json<MailSendResponse>, Refusal> {
    let request = request_body(request)?;
    let attachments = staged_files(&state.uploads, &request.attachments).await?;
    let mail = OutgoingMail {
        to: request.to,
        title: request.subject,
        body: request.body,
        source: UserMailSource::Web,
        in_reply_to: None,
        attachments,
    };
    let sent = send_and_consume(&state, mail, &request.attachments).await?;
    // A message that answers nothing starts its own thread.
    Ok(Json(MailSendResponse {
        thread_id: sent.message.to_string(),
    }))
}

/// `POST /api/hub/mail/threads/{ref}/reply` — the reply-all to the thread
/// holding the message, answering its latest message.
async fn reply_to_thread(
    State(state): State<MailState>,
    Path(raw): Path<String>,
    request: Result<Json<ReplyRequest>, JsonRejection>,
) -> Result<Json<MailReplyResponse>, Refusal> {
    let request = request_body(request)?;
    let item = message_ref(&raw)?;
    let before = MailIndex::load(&state.mailboxes()).await;
    let thread = thread_of(&before, &item, &raw)?;
    let attachments = staged_files(&state.uploads, &request.attachments).await?;
    let mail = OutgoingMail::reply(thread, request.body, attachments, UserMailSource::Web);
    let sent = send_and_consume(&state, mail, &request.attachments).await?;

    let after = MailIndex::load(&state.mailboxes()).await;
    after
        .thread_containing(&sent.message)
        .and_then(|t| t.messages().find(|m| m.item_ref == sent.message))
        .map(|message| {
            Json(MailReplyResponse {
                message: MailMessage::of(message),
            })
        })
        .ok_or_else(|| {
            tracing::error!(thread = %thread.thread_id(), message = %sent.message, "a sent reply couldn't be read back");
            Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The reply was sent, but it couldn't be read back. Reload the thread to see it.",
            )
        })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;

    fn cursor_for(at: DateTime<chrono::Utc>, thread_id: &str) -> String {
        Cursor {
            latest_at_ms: at.timestamp_millis(),
            thread_id: InboxRef::parse(thread_id).unwrap(),
        }
        .render()
    }

    #[test]
    fn a_cursor_survives_a_round_trip_even_when_the_id_holds_colons() {
        let raw = cursor_for(
            DateTime::from_timestamp(1_790_000_000, 0).unwrap(),
            "scout/user/20260930_a:b:c",
        );
        let cursor = Cursor::parse(&raw).unwrap();
        assert_eq!(cursor.latest_at_ms, 1_790_000_000_000);
        assert_eq!(cursor.thread_id.to_string(), "scout/user/20260930_a:b:c");
        assert_eq!(cursor.render(), raw);
    }

    #[test]
    fn a_cursor_needs_a_time_and_a_ref() {
        for raw in [
            "",
            "scout/user/id",
            "soon:scout/user/id",
            "1790000000:",
            "1790000000:scout",
            "1790000000",
        ] {
            assert_eq!(Cursor::parse(raw), None, "{raw:?} isn't a cursor");
        }
    }
}
