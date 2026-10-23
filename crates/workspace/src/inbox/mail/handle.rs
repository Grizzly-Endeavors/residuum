//! A late-bound, weak [`MailboxDirectory`], for the agents' tools.

use std::sync::{Arc, OnceLock, Weak};

use super::{Mailbox, MailboxDirectory};

/// A late-bound, weak reference to the hub's [`MailboxDirectory`].
///
/// The hub's agent host is built after the shared services every agent
/// receives, and each agent's tools are built inside a host-started agent, so
/// the services carry this handle and the host binds itself into it on
/// construction. It is weak because the agents hold it and the host holds the
/// agents; a strong reference would keep the host alive from inside its own
/// agents.
///
/// The handle is itself a [`MailboxDirectory`]: before it is bound, or once
/// the host is gone, it knows no mailboxes.
#[derive(Clone, Default)]
pub struct MailboxDirectoryHandle {
    inner: Arc<OnceLock<Weak<dyn MailboxDirectory>>>,
}

impl MailboxDirectoryHandle {
    /// A handle that points at nothing until [`Self::bind`] is called.
    #[must_use]
    pub fn unbound() -> Self {
        Self::default()
    }

    /// Point the handle at `directory`. A handle binds once; later calls
    /// leave the first binding in place.
    pub fn bind(&self, directory: Weak<dyn MailboxDirectory>) {
        self.inner.set(directory).ok();
    }

    /// The directory, when it is bound and still alive (the hub is not
    /// shutting down).
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn MailboxDirectory>> {
        self.inner.get().and_then(Weak::upgrade)
    }
}

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
        assert!(handle.mailboxes().is_empty());
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
        assert!(handle.mailboxes().is_empty(), "the host is gone");
    }

    #[test]
    fn a_handle_binds_once() {
        let first: Arc<dyn MailboxDirectory> =
            Arc::new(StaticMailboxDirectory::new([mailbox("atlas")]));
        let second: Arc<dyn MailboxDirectory> =
            Arc::new(StaticMailboxDirectory::new([mailbox("nova")]));
        let handle = MailboxDirectoryHandle::unbound();
        handle.bind(Arc::downgrade(&first));
        handle.bind(Arc::downgrade(&second));
        assert!(handle.mailbox("atlas").is_some());
        assert!(handle.mailbox("nova").is_none());
    }
}
