//! Changing a thread in the user's mailbox: reading, archiving and restoring it.
//!
//! Each changes the thread's user-inbox items (the agents' messages to the
//! user) and nothing else: the agents' copies of the user's messages belong to
//! the agents' own inboxes.

use std::fmt;
use std::path::Path;

use residuum_config::WorkspaceLayout;

use super::{MailMessage, MailSender, MailThread, MailboxDirectory};
use crate::inbox::{InboxKind, InboxRef, archive_item, mark_read, restore_item};

/// One item a thread change couldn't update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailItemFailure {
    /// The item.
    pub item: InboxRef,
    /// Why it couldn't be updated.
    pub reason: String,
}

/// Some of a thread's items couldn't be updated. Every other item was.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct ThreadUpdateError {
    /// How many items needed the change.
    pub attempted: usize,
    /// The items that didn't get it.
    pub failures: Vec<MailItemFailure>,
}

impl fmt::Display for ThreadUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} of {} messages in the thread couldn't be updated",
            self.failures.len(),
            self.attempted
        )?;
        for failure in &self.failures {
            write!(f, "; {}: {}", failure.item, failure.reason)?;
        }
        Ok(())
    }
}

/// Mark every unread message an agent sent in `thread` read, active or archived.
///
/// # Errors
/// [`ThreadUpdateError`] naming each item that couldn't be marked; the rest are.
pub async fn mark_thread_read(
    directory: &dyn MailboxDirectory,
    thread: &MailThread,
) -> Result<(), ThreadUpdateError> {
    update_agent_messages(
        directory,
        thread,
        |m| !m.read,
        |layout, message| async move {
            let dir = InboxKind::User.items_dir(&layout, message.archived);
            mark_read(&dir, message.item_ref.id()).await.map(|_| ())
        },
    )
    .await
}

/// Archive every active message an agent sent in `thread`.
///
/// # Errors
/// [`ThreadUpdateError`] naming each item that couldn't be archived; the rest are.
pub async fn archive_thread(
    directory: &dyn MailboxDirectory,
    thread: &MailThread,
) -> Result<(), ThreadUpdateError> {
    update_agent_messages(
        directory,
        thread,
        |m| !m.archived,
        |layout, message| async move { move_item(&layout, message, false).await },
    )
    .await
}

/// Restore every archived message an agent sent in `thread`.
///
/// # Errors
/// [`ThreadUpdateError`] naming each item that couldn't be restored; the rest are.
pub async fn restore_thread(
    directory: &dyn MailboxDirectory,
    thread: &MailThread,
) -> Result<(), ThreadUpdateError> {
    update_agent_messages(
        directory,
        thread,
        |m| m.archived,
        |layout, message| async move { move_item(&layout, message, true).await },
    )
    .await
}

async fn move_item(
    layout: &WorkspaceLayout,
    message: &MailMessage,
    from_archive: bool,
) -> anyhow::Result<()> {
    let active = InboxKind::User.items_dir(layout, false);
    let archive = InboxKind::User.items_dir(layout, true);
    let id = message.item_ref.id();
    if from_archive {
        restore_item(&archive, &active, id).await
    } else {
        archive_item(&active, &archive, id).await
    }
}

/// Apply `change` to each agent's message in `thread` that `needs` it, going
/// on past failures and reporting them together.
async fn update_agent_messages<'t, F, Fut>(
    directory: &dyn MailboxDirectory,
    thread: &'t MailThread,
    needs: impl Fn(&MailMessage) -> bool,
    change: F,
) -> Result<(), ThreadUpdateError>
where
    F: Fn(WorkspaceLayout, &'t MailMessage) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let mut attempted = 0;
    let mut failures = Vec::new();
    for message in thread.messages() {
        if !matches!(message.from, MailSender::Agent(_)) || !needs(message) {
            continue;
        }
        attempted += 1;
        let item = &message.item_ref;
        let result = match directory.mailbox(item.agent()) {
            Some(mailbox) => change(WorkspaceLayout::new(Path::new(&mailbox.dir)), message).await,
            None => Err(anyhow::anyhow!("agent '{}' no longer exists", item.agent())),
        };
        if let Err(e) = result {
            tracing::warn!(item = %item, error = %format!("{e:#}"), "couldn't update a mail thread item");
            failures.push(MailItemFailure {
                item: item.clone(),
                reason: format!("{e:#}"),
            });
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ThreadUpdateError {
            attempted,
            failures,
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::super::fixture::Fixture;
    use super::*;

    #[tokio::test]
    async fn reading_a_thread_marks_every_agent_message_read() {
        let fx = Fixture::new(&["atlas", "scout"]);
        let root = fx.user_item("atlas", "r", "Report", 9, None).await;
        let mail = fx
            .user_mail("m", &["atlas", "scout"], "Re: Report", 10, Some(&root))
            .await;
        let archived = fx
            .user_item("scout", "s", "Re: Report", 11, Some(&mail))
            .await;
        fx.archive_user_item("scout", "s").await;

        let index = fx.index().await;
        mark_thread_read(&fx.directory, index.thread(&root).unwrap())
            .await
            .unwrap();

        assert!(fx.load(&root).await.0.read);
        assert!(fx.load(&archived).await.0.read);
        assert!(
            !fx.load(&mail).await.0.read,
            "the agent's copy is the agent's to read"
        );
        assert!(!fx.index().await.thread(&root).unwrap().unread());
    }

    #[tokio::test]
    async fn archiving_a_thread_leaves_the_agents_copies_alone() {
        let fx = Fixture::new(&["atlas"]);
        let mail = fx.user_mail("m", &["atlas"], "Hello", 9, None).await;
        let reply = fx
            .user_item("atlas", "r", "Re: Hello", 10, Some(&mail))
            .await;

        let index = fx.index().await;
        archive_thread(&fx.directory, index.thread(&mail).unwrap())
            .await
            .unwrap();

        assert!(fx.load(&reply).await.1, "the agent's message is archived");
        assert!(
            !fx.load(&mail).await.1,
            "the user's message stays in the agent's inbox"
        );
        let after = fx.index().await;
        let thread = after.thread(&mail).unwrap();
        assert!(thread.archived() && !thread.in_inbox() && thread.in_sent());

        restore_thread(&fx.directory, thread).await.unwrap();
        assert!(!fx.load(&reply).await.1);
        assert!(fx.index().await.thread(&mail).unwrap().in_inbox());
    }

    #[tokio::test]
    async fn a_failed_item_is_reported_and_the_rest_still_change() {
        let fx = Fixture::new(&["atlas"]);
        let a = fx.user_item("atlas", "a", "A", 9, None).await;
        fx.user_item("atlas", "b", "Re: A", 10, Some(&a)).await;
        let index = fx.index().await;
        let thread = index.thread(&a).unwrap().clone();
        let b_path = WorkspaceLayout::new(fx.dir("atlas"))
            .user_inbox_dir()
            .join("b.json");
        tokio::fs::remove_file(b_path).await.unwrap();

        let err = archive_thread(&fx.directory, &thread).await.unwrap_err();
        assert_eq!(err.attempted, 2);
        assert_eq!(err.failures.len(), 1);
        assert_eq!(err.failures[0].item.id(), "b");
        assert!(err.to_string().starts_with("1 of 2 messages"), "{err}");
        assert!(fx.load(&a).await.1, "the other item was archived");
    }
}
