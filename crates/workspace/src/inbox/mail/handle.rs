//! A late-bound, weak [`MailboxDirectory`], for the agents' tools.

use residuum_core::util::LateBound;

use super::{Mailbox, MailboxDirectory};

/// A late-bound, weak reference to the hub's [`MailboxDirectory`], which the
/// agent host binds itself into on construction.
///
/// The handle is itself a [`MailboxDirectory`]: before it is bound, or once
/// the host is gone, it knows no mailboxes.
pub type MailboxDirectoryHandle = LateBound<dyn MailboxDirectory>;

impl MailboxDirectory for MailboxDirectoryHandle {
    fn mailboxes(&self) -> Vec<Mailbox> {
        self.get()
            .map(|directory| directory.mailboxes())
            .unwrap_or_default()
    }

    fn mailbox(&self, agent: &str) -> Option<Mailbox> {
        self.get().and_then(|directory| directory.mailbox(agent))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::testing::StaticMailboxDirectory;

    fn mailbox(agent: &str) -> Mailbox {
        Mailbox {
            agent: agent.to_string(),
            dir: std::path::PathBuf::from("/tmp").join(agent),
            timezone: chrono_tz::UTC,
        }
    }

    #[test]
    fn unbound_handle_knows_no_mailboxes() {
        let handle = MailboxDirectoryHandle::unbound();
        assert_eq!(handle.mailboxes(), Vec::<Mailbox>::new());
        assert!(handle.mailbox("atlas").is_none());
    }

    #[test]
    fn bound_handle_reads_through_and_forgets_a_dropped_directory() {
        let directory: Arc<dyn MailboxDirectory> =
            Arc::new(StaticMailboxDirectory::new([mailbox("atlas")]));
        let handle = MailboxDirectoryHandle::unbound();
        handle.bind(Arc::downgrade(&directory));
        assert_eq!(handle.mailboxes(), vec![mailbox("atlas")]);
        assert_eq!(handle.mailbox("atlas"), Some(mailbox("atlas")));
        assert!(handle.mailbox("nova").is_none());

        drop(directory);
        assert_eq!(
            handle.mailboxes(),
            Vec::<Mailbox>::new(),
            "the host is gone"
        );
    }
}
