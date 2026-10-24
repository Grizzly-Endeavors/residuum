//! Sending the user's mail: one message, one copy per recipient.

use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::Utc;
use residuum_config::WorkspaceLayout;

use super::{MailThread, Mailbox, MailboxDirectory};
use crate::inbox::{ClaimedItem, InboxItem, InboxKind, InboxRef, copy_attachments, derive_title};

/// Where a user-authored agent-inbox item was written from. Only items with
/// one of these sources are mail; every other agent-inbox item is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserMailSource {
    /// The web app.
    Web,
    /// The terminal (`residuum` CLI or the `/inbox` chat command).
    Cli,
}

impl UserMailSource {
    /// The `source` stored on the item.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Cli => "cli",
        }
    }
}

/// Whether an agent-inbox item with this `source` is a message the user wrote.
#[must_use]
pub fn is_user_mail_source(source: &str) -> bool {
    [UserMailSource::Web, UserMailSource::Cli]
        .iter()
        .any(|s| s.as_str() == source)
}

/// The title of a reply to a thread with this subject: `Re: <subject>`, with
/// any `Re:` the subject already starts with dropped so they don't stack.
#[must_use]
pub fn reply_subject(subject: &str) -> String {
    let mut rest = subject.trim();
    while let Some(stripped) = rest
        .get(..3)
        .filter(|prefix| prefix.eq_ignore_ascii_case("re:"))
        .and_then(|_| rest.get(3..))
    {
        rest = stripped.trim_start();
    }
    if rest.is_empty() {
        "Re:".to_string()
    } else {
        format!("Re: {rest}")
    }
}

/// A message the user is sending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMail {
    /// The recipients' folder names; duplicates count once.
    pub to: Vec<String>,
    /// The title; a blank one becomes the body's first line.
    pub title: String,
    /// The body, as markdown.
    pub body: String,
    /// Where it was written.
    pub source: UserMailSource,
    /// The message it answers. Not checked: a ref that resolves to nothing
    /// makes the message its own thread.
    pub in_reply_to: Option<InboxRef>,
    /// Local files, copied into each recipient's copy.
    pub attachments: Vec<PathBuf>,
}

impl OutgoingMail {
    /// The reply-all to `thread`: to its [`MailThread::recipients_for_reply`],
    /// titled [`reply_subject`] of its subject, answering its latest message.
    #[must_use]
    pub fn reply(
        thread: &MailThread,
        body: impl Into<String>,
        attachments: Vec<PathBuf>,
        source: UserMailSource,
    ) -> Self {
        Self {
            to: thread.recipients_for_reply().to_vec(),
            title: reply_subject(thread.subject()),
            body: body.into(),
            source,
            in_reply_to: Some(thread.latest().item_ref.clone()),
            attachments,
        }
    }
}

/// A sent message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMail {
    /// The message's ref (its first recipient's copy), which
    /// [`super::MailIndex`] gives it too.
    pub message: InboxRef,
    /// Every copy's ref, sorted by agent name.
    pub copies: Vec<InboxRef>,
}

/// Why a message wasn't sent. Nothing was written when it is returned.
#[derive(Debug, thiserror::Error)]
pub enum SendMailError {
    /// The message has no recipient.
    #[error("a message needs at least one recipient")]
    NoRecipients,
    /// A recipient isn't a known agent.
    #[error("there is no agent named '{0}'")]
    UnknownRecipient(String),
    /// The body is empty or only whitespace.
    #[error("a message needs a body")]
    BlankBody,
    /// An attachment couldn't be copied.
    #[error("couldn't attach the files: {0}")]
    Attachment(String),
    /// Writing a copy failed.
    #[error("couldn't save the message: {0:#}")]
    Write(anyhow::Error),
}

/// Send `mail`: write one copy to each recipient's `inbox/agent/`, all with
/// one id claimed in every recipient's inbox at once (see
/// [`ClaimedItem::claim_in_all`]), each with its own copy of the attachments.
///
/// Every copy is written or none is: on a failure partway, the claims not yet
/// saved are released and the copies already saved are removed.
///
/// # Errors
/// [`SendMailError::NoRecipients`], [`SendMailError::BlankBody`] and
/// [`SendMailError::UnknownRecipient`] before anything is written;
/// [`SendMailError::Attachment`] or [`SendMailError::Write`] when writing fails.
#[tracing::instrument(skip_all, fields(recipients = mail.to.len(), source = mail.source.as_str()))]
pub async fn send_mail(
    directory: &dyn MailboxDirectory,
    mail: OutgoingMail,
) -> Result<SentMail, SendMailError> {
    send_mail_with(
        directory,
        mail,
        async |claim: &mut ClaimedItem, item: &InboxItem| claim.save(item).await,
    )
    .await
}

/// [`send_mail`] with the step that saves one recipient's copy supplied, so a
/// test can fail a later copy's save without filesystem permissions.
async fn send_mail_with<F>(
    directory: &dyn MailboxDirectory,
    mail: OutgoingMail,
    save_copy: F,
) -> Result<SentMail, SendMailError>
where
    F: AsyncFn(&mut ClaimedItem, &InboxItem) -> anyhow::Result<()>,
{
    let recipients: BTreeSet<&str> = mail.to.iter().map(String::as_str).collect();
    if recipients.is_empty() {
        return Err(SendMailError::NoRecipients);
    }
    if mail.body.trim().is_empty() {
        return Err(SendMailError::BlankBody);
    }
    let mailboxes = recipients
        .iter()
        .map(|agent| {
            directory
                .mailbox(agent)
                .ok_or_else(|| SendMailError::UnknownRecipient((*agent).to_string()))
        })
        .collect::<Result<Vec<Mailbox>, _>>()?;

    let title = if mail.title.trim().is_empty() {
        derive_title(&mail.body)
    } else {
        mail.title.clone()
    };
    let now = Utc::now();
    // The id's date comes from the first recipient's clock; every copy shares it.
    let id_time = mailboxes.first().map_or_else(
        || now.naive_utc(),
        |m| now.with_timezone(&m.timezone).naive_local(),
    );
    let inbox_dirs: Vec<PathBuf> = mailboxes
        .iter()
        .map(|m| WorkspaceLayout::new(&m.dir).agent_inbox_dir())
        .collect();
    let claims = ClaimedItem::claim_in_all(&inbox_dirs, &title, id_time)
        .await
        .map_err(|e| SendMailError::Write(anyhow::anyhow!(e)))?;
    let id = claims.first().map(|c| c.id().to_string()).ok_or_else(|| {
        SendMailError::Write(anyhow::anyhow!("claiming a mail id returned no claim"))
    })?;

    // A ref that can't be built fails here, while every claim is still
    // unsaved and dropping them leaves nothing behind.
    let mut copies = mailboxes
        .iter()
        .map(|m| InboxRef::new(&m.agent, InboxKind::Agent, &id))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| SendMailError::Write(e.into()))?;
    copies.sort();
    let Some(message) = copies.first().cloned() else {
        return Err(SendMailError::NoRecipients);
    };

    write_copies(&mailboxes, claims, &mail, &title, now, save_copy).await?;

    tracing::info!(message = %message, copies = copies.len(), "mail sent");
    Ok(SentMail { message, copies })
}

/// Fill each recipient's claim with its attachments, then save every copy.
/// Attachments are copied for every claim before any copy is saved, so an
/// attachment failure leaves only unsaved claims, which dropping removes. A
/// save that fails after earlier copies were saved removes those copies.
async fn write_copies<F>(
    mailboxes: &[Mailbox],
    claims: Vec<ClaimedItem>,
    mail: &OutgoingMail,
    title: &str,
    now: chrono::DateTime<Utc>,
    save_copy: F,
) -> Result<(), SendMailError>
where
    F: AsyncFn(&mut ClaimedItem, &InboxItem) -> anyhow::Result<()>,
{
    let mut filled = Vec::with_capacity(claims.len());
    for (mailbox, claim) in mailboxes.iter().zip(claims) {
        let (attachments, failures) = copy_attachments(&claim, &mail.attachments).await;
        if !failures.is_empty() {
            tracing::warn!(
                failures = failures.len(),
                "a mail attachment couldn't be copied; sending nothing"
            );
            return Err(SendMailError::Attachment(failures.join("; ")));
        }
        let item = InboxItem {
            title: title.to_string(),
            body: mail.body.clone(),
            source: mail.source.as_str().to_string(),
            timestamp: now.with_timezone(&mailbox.timezone).naive_local(),
            read: false,
            attachments,
            in_reply_to: mail.in_reply_to.as_ref().map(ToString::to_string),
        };
        filled.push((claim, item));
    }

    let mut saved: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(filled.len());
    for (mut claim, item) in filled {
        if let Err(e) = save_copy(&mut claim, &item).await {
            tracing::warn!(error = %e, "saving a mail copy failed; removing the copies already saved");
            remove_saved(&saved).await;
            return Err(SendMailError::Write(e));
        }
        saved.push((
            claim.inbox_dir().join(claim.filename()),
            claim.attachments_dir().to_path_buf(),
        ));
    }
    Ok(())
}

/// Remove saved copies (each item file and its attachment directory) of a
/// mail whose later copy failed to save.
async fn remove_saved(saved: &[(PathBuf, PathBuf)]) {
    for (item, attachments) in saved {
        for (path, result) in [
            (item, tokio::fs::remove_file(item).await),
            (attachments, tokio::fs::remove_dir_all(attachments).await),
        ] {
            if let Err(e) = result
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::error!(path = %path.display(), error = %e, "couldn't remove part of a mail that failed to send");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::Fixture;
    use super::super::{MailIndex, MailSender, ThreadFilter};
    use super::*;

    fn mail(to: &[&str], title: &str, body: &str) -> OutgoingMail {
        OutgoingMail {
            to: to.iter().map(ToString::to_string).collect(),
            title: title.to_string(),
            body: body.to_string(),
            source: UserMailSource::Web,
            in_reply_to: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn reply_subject_does_not_stack() {
        assert_eq!(reply_subject("Daily digest"), "Re: Daily digest");
        assert_eq!(reply_subject("Re: Daily digest"), "Re: Daily digest");
        assert_eq!(reply_subject("RE: re:Daily digest"), "Re: Daily digest");
        assert_eq!(reply_subject("  Re:  "), "Re:");
        assert_eq!(reply_subject("Résumé"), "Re: Résumé");
        assert_eq!(reply_subject("Ré"), "Re: Ré");
    }

    #[test]
    fn only_web_and_cli_are_user_mail() {
        assert!(is_user_mail_source("web"));
        assert!(is_user_mail_source("cli"));
        for other in ["agent", "pulse", "artifact:x", "discord", "Web"] {
            assert!(!is_user_mail_source(other), "{other}");
        }
    }

    #[tokio::test]
    async fn sending_writes_one_message_with_a_copy_per_recipient() {
        let fx = Fixture::new(&["atlas", "scout"]);
        let file = fx.dir("atlas").parent().unwrap().join("plan.md");
        tokio::fs::write(&file, b"plan").await.unwrap();
        let mut outgoing = mail(&["scout", "atlas", "scout"], "Plan", "Here it is");
        outgoing.attachments = vec![file];

        let sent = send_mail(&fx.directory, outgoing).await.unwrap();

        assert_eq!(sent.copies.len(), 2);
        assert_eq!(sent.message.agent(), "atlas");
        for copy in &sent.copies {
            let (item, archived) = fx.load(copy).await;
            assert!(!archived);
            assert_eq!(item.source, "web");
            assert_eq!(item.title, "Plan");
            let expected = PathBuf::from("inbox/agent/attachments")
                .join(copy.id())
                .join("plan.md");
            assert_eq!(item.attachments.as_slice(), std::slice::from_ref(&expected));
            assert!(fx.dir(copy.agent()).join(expected).is_file());
        }
        let index = MailIndex::load(&fx.directory).await;
        let thread = index.thread_containing(&sent.message).unwrap();
        assert_eq!(thread.message_count(), 1);
        assert_eq!(thread.root().to, ["atlas", "scout"]);
        assert!(thread.has_attachments());
    }

    #[tokio::test]
    async fn the_id_is_free_in_every_recipients_inbox_and_archive() {
        let fx = Fixture::new(&["atlas", "scout"]);
        let probe = send_mail(&fx.directory, mail(&["atlas"], "Hi", "probe"))
            .await
            .unwrap();
        let base = probe.message.id().to_string();
        fx.archive_agent_copy("atlas", &base).await;
        fx.agent_item_with_source("scout", &format!("{base}_2"), "pulse", 9)
            .await;

        let sent = send_mail(&fx.directory, mail(&["atlas", "scout"], "Hi", "real"))
            .await
            .unwrap();

        assert_eq!(sent.message.id(), format!("{base}_3"));
        assert!(sent.copies.iter().all(|c| c.id() == sent.message.id()));
    }

    #[tokio::test]
    async fn invalid_mail_is_refused_before_anything_is_written() {
        let fx = Fixture::new(&["atlas"]);
        assert!(matches!(
            send_mail(&fx.directory, mail(&[], "T", "b")).await,
            Err(SendMailError::NoRecipients)
        ));
        assert!(matches!(
            send_mail(&fx.directory, mail(&["atlas"], "T", "  \n")).await,
            Err(SendMailError::BlankBody)
        ));
        let unknown = send_mail(&fx.directory, mail(&["atlas", "ghost"], "T", "b")).await;
        assert!(
            matches!(&unknown, Err(SendMailError::UnknownRecipient(a)) if a == "ghost"),
            "{unknown:?}"
        );
        assert!(
            !WorkspaceLayout::new(fx.dir("atlas"))
                .agent_inbox_dir()
                .exists()
        );
    }

    #[tokio::test]
    async fn a_failed_attachment_sends_nothing() {
        let fx = Fixture::new(&["atlas", "scout"]);
        let mut outgoing = mail(&["atlas", "scout"], "T", "b");
        outgoing.attachments = vec![fx.dir("atlas").join("missing.txt")];

        let err = send_mail(&fx.directory, outgoing).await.unwrap_err();

        assert!(matches!(err, SendMailError::Attachment(_)), "{err:?}");
        for agent in ["atlas", "scout"] {
            let layout = WorkspaceLayout::new(fx.dir(agent));
            let left = crate::inbox::list_items_or_empty(&layout.agent_inbox_dir())
                .await
                .unwrap();
            assert!(left.is_empty(), "{agent}: {left:?}");
            let attachments = layout.agent_inbox_attachments_dir();
            let dirs = std::fs::read_dir(&attachments).map_or(0, Iterator::count);
            assert_eq!(dirs, 0, "{agent} keeps no attachment directory");
        }
    }

    #[tokio::test]
    async fn an_uncheckable_recipient_inbox_fails_the_send_before_anything_is_written() {
        let fx = Fixture::new(&["atlas", "scout"]);
        tokio::fs::create_dir_all(fx.dir("scout")).await.unwrap();
        tokio::fs::write(fx.dir("scout").join("inbox"), b"not a directory")
            .await
            .unwrap();

        let err = send_mail(&fx.directory, mail(&["atlas", "scout"], "T", "b"))
            .await
            .unwrap_err();

        assert!(matches!(err, SendMailError::Write(_)), "{err:?}");
        assert_nothing_left(&fx, "atlas");
    }

    /// `agent`'s agent inbox holds no item and no claimed attachment dir.
    fn assert_nothing_left(fx: &Fixture, agent: &str) {
        let layout = WorkspaceLayout::new(fx.dir(agent));
        let items = std::fs::read_dir(layout.agent_inbox_dir()).map_or(0, |entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .count()
        });
        assert_eq!(items, 0, "{agent} keeps no item");
        let claims =
            std::fs::read_dir(layout.agent_inbox_attachments_dir()).map_or(0, Iterator::count);
        assert_eq!(claims, 0, "{agent} keeps no claimed id");
    }

    #[tokio::test]
    async fn concurrent_sends_with_one_title_get_distinct_ids() {
        let fx = Fixture::new(&["atlas", "scout"]);
        let (a, b) = tokio::join!(
            send_mail(&fx.directory, mail(&["atlas", "scout"], "Status", "first")),
            send_mail(&fx.directory, mail(&["scout", "atlas"], "Status", "second")),
        );
        let (a, b) = (a.unwrap(), b.unwrap());

        assert_ne!(
            a.message.id(),
            b.message.id(),
            "each send claims its own id"
        );
        for (sent, body) in [(&a, "first"), (&b, "second")] {
            for copy in &sent.copies {
                assert_eq!(fx.load(copy).await.0.body, body, "{copy}");
            }
        }
        let index = MailIndex::load(&fx.directory).await;
        assert_eq!(
            index
                .threads(&ThreadFilter {
                    folder: Some(super::super::MailFolder::Sent),
                    agent: None,
                })
                .count(),
            2,
            "both messages are in Sent"
        );
    }

    #[tokio::test]
    async fn a_copy_that_cannot_be_saved_removes_the_copies_already_saved() {
        let fx = Fixture::new(&["atlas", "scout"]);
        let scout_inbox = WorkspaceLayout::new(fx.dir("scout")).agent_inbox_dir();
        let mut outgoing = mail(&["atlas", "scout"], "T", "b");
        let source = fx.dir("scout").join("note.txt");
        tokio::fs::create_dir_all(fx.dir("scout")).await.unwrap();
        tokio::fs::write(&source, b"hello").await.unwrap();
        outgoing.attachments = vec![source];

        // atlas's copy is saved first, then scout's save fails.
        let err = send_mail_with(
            &fx.directory,
            outgoing,
            async |claim: &mut ClaimedItem, item: &InboxItem| {
                if claim.inbox_dir() == scout_inbox.as_path() {
                    anyhow::bail!("injected save failure");
                }
                claim.save(item).await
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, SendMailError::Write(_)), "{err:?}");
        assert_nothing_left(&fx, "atlas");
        assert_nothing_left(&fx, "scout");
    }

    #[tokio::test]
    async fn a_blank_title_comes_from_the_body() {
        let fx = Fixture::new(&["atlas"]);
        let sent = send_mail(&fx.directory, mail(&["atlas"], " ", "First line\nmore"))
            .await
            .unwrap();
        assert_eq!(fx.load(&sent.message).await.0.title, "First line");
    }

    #[tokio::test]
    async fn reply_all_goes_to_everyone_and_answers_the_latest_message() {
        let fx = Fixture::new(&["atlas", "scout", "drifter"]);
        let root = fx.user_item("atlas", "r", "Report", 9, None).await;
        let ask = fx
            .user_mail("ask", &["atlas", "scout"], "Re: Report", 10, Some(&root))
            .await;
        let answer = fx
            .user_item("scout", "s", "Re: Report", 11, Some(&ask))
            .await;
        fx.user_item("drifter", "unrelated", "Other", 12, None)
            .await;

        let index = MailIndex::load(&fx.directory).await;
        let thread = index.thread_containing(&root).unwrap();
        let reply = OutgoingMail::reply(thread, "Thanks", Vec::new(), UserMailSource::Web);
        assert_eq!(reply.to, ["atlas", "scout"]);
        assert_eq!(reply.title, "Re: Report");
        assert_eq!(reply.in_reply_to.as_ref(), Some(&answer));

        let sent = send_mail(&fx.directory, reply).await.unwrap();
        let after = MailIndex::load(&fx.directory).await;
        let replied = after.thread_containing(&root).unwrap();
        assert_eq!(replied.message_count(), 4);
        assert_eq!(replied.latest().item_ref, sent.message);
        assert_eq!(replied.latest().from, MailSender::User);
        assert_eq!(after.threads(&ThreadFilter::default()).count(), 2);
    }
}
