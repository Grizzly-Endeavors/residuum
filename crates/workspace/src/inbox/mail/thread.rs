//! Assembling threads from every agent's mail items.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use residuum_config::WorkspaceLayout;

use super::send::is_user_mail_source;
use super::{Mailbox, MailboxDirectory};
use crate::inbox::{InboxItem, InboxKind, InboxRef, list_items_or_empty};

/// Who wrote a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailSender {
    /// The user.
    User,
    /// The agent with this folder name.
    Agent(String),
}

/// Whether one recipient of a user message has seen its copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailSeen {
    /// The recipient's folder name.
    pub agent: String,
    /// The recipient's copy is read or archived.
    pub seen: bool,
}

/// One attachment file of a message, where it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailAttachment {
    /// The item copy holding the file: its agent owns the file.
    pub item: InboxRef,
    /// The file's position in the item's attachment list, which the
    /// attachment-serving route takes.
    pub index: usize,
    /// The file's name.
    pub filename: String,
    /// The file's path relative to the owning agent's workspace, in the
    /// active or archived attachment directory as the item is now.
    pub path: PathBuf,
    /// The file's size in bytes.
    pub size: u64,
    /// The file's MIME type, from its extension.
    pub content_type: String,
}

impl MailAttachment {
    /// The folder name of the agent whose workspace holds the file.
    #[must_use]
    pub fn agent(&self) -> &str {
        self.item.agent()
    }
}

/// One message of a thread. A user message sent to several agents is one
/// message with a copy per recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMessage {
    /// The message's ref: for a user message, its first recipient's copy
    /// (by agent name). A ref to any copy resolves to the message.
    pub item_ref: InboxRef,
    /// Every copy's ref, sorted by agent name; just `item_ref` for an agent's message.
    pub copies: Vec<InboxRef>,
    /// Who wrote it.
    pub from: MailSender,
    /// The recipients of a user message, sorted; empty for an agent's
    /// message, which goes to the user.
    pub to: Vec<String>,
    /// When it was written.
    pub at: DateTime<Tz>,
    /// Its title.
    pub title: String,
    /// Its body, as markdown.
    pub body: String,
    /// Its attachments whose files exist (from the first copy, for a user
    /// message); a missing file is left out and logged.
    pub attachments: Vec<MailAttachment>,
    /// The message it answers, when that is stored as a valid ref. It may
    /// name a message that no longer exists, which makes this one its
    /// thread's root.
    pub in_reply_to: Option<InboxRef>,
    /// Whether the user read it; always true for the user's own message.
    pub read: bool,
    /// Whether the user archived it. A user message is never archived: only
    /// the user's mailbox (agents' messages) is, and an agent archiving its
    /// copy shows in `seen_by`.
    pub archived: bool,
    /// Per recipient of a user message, whether it has seen its copy; empty
    /// for an agent's message.
    pub seen_by: Vec<MailSeen>,
}

/// A mail folder. A thread can be in Inbox and Sent at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailFolder {
    /// Threads with an active message from an agent.
    Inbox,
    /// Threads with a message from the user.
    Sent,
    /// Threads with messages from agents, all archived.
    Archived,
}

/// A conversation: a root message and every message whose `in_reply_to`
/// chain reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailThread {
    root: MailMessage,
    replies: Vec<MailMessage>,
    participants: Vec<String>,
    folders: Folders,
    unread: bool,
    has_attachments: bool,
}

/// Which folders a thread is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Folders {
    inbox: bool,
    sent: bool,
    archived: bool,
}

impl MailThread {
    fn new(root: MailMessage, replies: Vec<MailMessage>) -> Self {
        let messages = || std::iter::once(&root).chain(replies.iter());
        let mut participants = BTreeSet::new();
        let (mut in_inbox, mut in_sent, mut any_from_agent) = (false, false, false);
        let (mut unread, mut has_attachments) = (false, false);
        for message in messages() {
            has_attachments |= !message.attachments.is_empty();
            match &message.from {
                MailSender::User => {
                    in_sent = true;
                    participants.extend(message.to.iter().cloned());
                }
                MailSender::Agent(agent) => {
                    any_from_agent = true;
                    participants.insert(agent.clone());
                    if !message.archived {
                        in_inbox = true;
                        unread |= !message.read;
                    }
                }
            }
        }
        Self {
            participants: participants.into_iter().collect(),
            folders: Folders {
                inbox: in_inbox,
                sent: in_sent,
                archived: any_from_agent && !in_inbox,
            },
            unread,
            has_attachments,
            root,
            replies,
        }
    }

    /// The thread's id: its root message's ref.
    #[must_use]
    pub fn thread_id(&self) -> &InboxRef {
        &self.root.item_ref
    }

    /// The root message's title.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.root.title
    }

    /// Every agent in the thread (each one that wrote, and each recipient of
    /// the user's messages), sorted.
    #[must_use]
    pub fn participants(&self) -> &[String] {
        &self.participants
    }

    /// Who a reply-all goes to: every agent that wrote in the thread and every
    /// agent a message of the user's went to, sorted. The same set as
    /// [`Self::participants`].
    #[must_use]
    pub fn recipients_for_reply(&self) -> &[String] {
        &self.participants
    }

    /// The root message.
    #[must_use]
    pub fn root(&self) -> &MailMessage {
        &self.root
    }

    /// Every message: the root first, then the replies oldest first.
    pub fn messages(&self) -> impl Iterator<Item = &MailMessage> {
        std::iter::once(&self.root).chain(self.replies.iter())
    }

    /// How many messages the thread holds.
    #[must_use]
    pub fn message_count(&self) -> usize {
        1 + self.replies.len()
    }

    /// The message written last: the greatest time, with a tie going to the
    /// later message in [`Self::messages`] order. Not necessarily the last
    /// one listed, since a reply can carry an earlier time than its root.
    #[must_use]
    pub fn latest(&self) -> &MailMessage {
        self.messages()
            .max_by(|a, b| a.at.cmp(&b.at))
            .unwrap_or(&self.root)
    }

    /// When the latest message was written.
    #[must_use]
    pub fn latest_at(&self) -> DateTime<Tz> {
        self.latest().at
    }

    /// Whether `item` is a copy of one of the thread's messages.
    #[must_use]
    pub fn contains(&self, item: &InboxRef) -> bool {
        self.messages().any(|m| m.copies.contains(item))
    }

    /// In Inbox: an agent's message in it is active.
    #[must_use]
    pub fn in_inbox(&self) -> bool {
        self.folders.inbox
    }

    /// In Sent: the user wrote a message in it.
    #[must_use]
    pub fn in_sent(&self) -> bool {
        self.folders.sent
    }

    /// Archived: it has agents' messages and all of them are archived.
    #[must_use]
    pub fn archived(&self) -> bool {
        self.folders.archived
    }

    /// Whether the thread is in `folder`.
    #[must_use]
    pub fn in_folder(&self, folder: MailFolder) -> bool {
        match folder {
            MailFolder::Inbox => self.folders.inbox,
            MailFolder::Sent => self.folders.sent,
            MailFolder::Archived => self.folders.archived,
        }
    }

    /// An active agent's message in it is unread.
    #[must_use]
    pub fn unread(&self) -> bool {
        self.unread
    }

    /// A message in it has an attachment.
    #[must_use]
    pub fn has_attachments(&self) -> bool {
        self.has_attachments
    }
}

/// Which threads [`MailIndex::threads`] lists. The default lists every thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadFilter {
    /// Only threads in this folder.
    pub folder: Option<MailFolder>,
    /// Only threads this agent takes part in (by folder name).
    pub agent: Option<String>,
}

impl ThreadFilter {
    /// Whether `thread` passes the filter.
    #[must_use]
    pub fn matches(&self, thread: &MailThread) -> bool {
        self.folder.is_none_or(|folder| thread.in_folder(folder))
            && self
                .agent
                .as_deref()
                .is_none_or(|agent| thread.participants().iter().any(|p| p == agent))
    }
}

/// An inbox directory that couldn't be read, so its items are missing from
/// the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailLoadProblem {
    /// The agent whose directory it is.
    pub agent: String,
    /// The directory.
    pub dir: PathBuf,
    /// What went wrong.
    pub error: String,
}

/// Every agent's mail, assembled into threads.
#[derive(Debug, Clone, Default)]
pub struct MailIndex {
    /// Newest first by latest message.
    threads: Vec<MailThread>,
    /// Every copy's ref, to the index of its thread in `threads`.
    thread_of: HashMap<InboxRef, usize>,
    problems: Vec<MailLoadProblem>,
}

impl MailIndex {
    /// Read every agent's mail items through `directory` and assemble the
    /// threads.
    ///
    /// An inbox directory that can't be read is left out, logged, and listed
    /// in [`Self::problems`]; a missing one (an agent that never ran) is empty.
    pub async fn load(directory: &dyn MailboxDirectory) -> Self {
        let mut items = Vec::new();
        let mut problems = Vec::new();
        for mailbox in directory.mailboxes() {
            load_mailbox(&mailbox, &mut items, &mut problems).await;
        }
        let mut index = Self::assemble(items);
        index.problems = problems;
        index
    }

    /// The threads `filter` passes, newest first by latest message.
    pub fn threads(&self, filter: &ThreadFilter) -> impl Iterator<Item = &MailThread> + '_ {
        let filter = filter.clone();
        self.threads.iter().filter(move |t| filter.matches(t))
    }

    /// The thread holding the message `item` names: the ref of any copy of any
    /// message in it, the root or a reply.
    #[must_use]
    pub fn thread_containing(&self, item: &InboxRef) -> Option<&MailThread> {
        self.thread_of
            .get(item)
            .and_then(|&index| self.threads.get(index))
    }

    /// The inbox directories that couldn't be read.
    #[must_use]
    pub fn problems(&self) -> &[MailLoadProblem] {
        &self.problems
    }

    fn assemble(items: Vec<LoadedItem>) -> Self {
        let messages = merge_copies(items);
        let by_ref: HashMap<&InboxRef, usize> = messages
            .iter()
            .enumerate()
            .flat_map(|(i, m)| m.copies.iter().map(move |r| (r, i)))
            .collect();
        let mut parent: Vec<Option<usize>> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                m.in_reply_to
                    .as_ref()
                    .and_then(|r| by_ref.get(r).copied())
                    .filter(|&p| p != i)
            })
            .collect();
        drop(by_ref);
        break_cycles(&messages, &mut parent);
        let depth = depths(&parent);

        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..messages.len() {
            groups.entry(root_of(&parent, i)).or_default().push(i);
        }

        let mut slots: Vec<Option<MailMessage>> = messages.into_iter().map(Some).collect();
        let mut threads = Vec::with_capacity(groups.len());
        for (root, mut members) in groups {
            members.retain(|&m| m != root);
            let take = |taken: &mut Vec<Option<MailMessage>>, i: usize| {
                taken.get_mut(i).and_then(Option::take)
            };
            let Some(root_message) = take(&mut slots, root) else {
                continue;
            };
            let mut replies: Vec<(usize, MailMessage)> = members
                .into_iter()
                .filter_map(|i| {
                    let d = depth.get(i).copied().unwrap_or_default();
                    take(&mut slots, i).map(|m| (d, m))
                })
                .collect();
            replies.sort_by(|(da, a), (db, b)| message_order(a, *da, b, *db));
            threads.push(MailThread::new(
                root_message,
                replies.into_iter().map(|(_, m)| m).collect(),
            ));
        }
        threads.sort_by(|a, b| {
            b.latest_at()
                .cmp(&a.latest_at())
                .then_with(|| a.thread_id().cmp(b.thread_id()))
        });

        let thread_of = threads
            .iter()
            .enumerate()
            .flat_map(|(i, t)| {
                t.messages()
                    .flat_map(|m| m.copies.iter().cloned())
                    .map(move |r| (r, i))
                    .collect::<Vec<_>>()
            })
            .collect();
        Self {
            threads,
            thread_of,
            problems: Vec::new(),
        }
    }
}

/// Replies in time order; within one stored minute a parent before its
/// replies, then by ref so the order is stable.
fn message_order(a: &MailMessage, depth_a: usize, b: &MailMessage, depth_b: usize) -> Ordering {
    a.at.cmp(&b.at)
        .then(depth_a.cmp(&depth_b))
        .then_with(|| a.item_ref.cmp(&b.item_ref))
}

/// Cut one link of every `in_reply_to` cycle, at its oldest message, which
/// becomes a root.
fn break_cycles(messages: &[MailMessage], parent: &mut [Option<usize>]) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Visit {
        New,
        OnPath,
        Done,
    }
    let mut visit = vec![Visit::New; parent.len()];
    for start in 0..parent.len() {
        let mut path = Vec::new();
        let mut current = start;
        loop {
            match visit.get(current).copied() {
                Some(Visit::New) => {
                    if let Some(v) = visit.get_mut(current) {
                        *v = Visit::OnPath;
                    }
                    path.push(current);
                    match parent.get(current).copied().flatten() {
                        Some(next) => current = next,
                        None => break,
                    }
                }
                Some(Visit::OnPath) => {
                    let cycle_start = path.iter().position(|&p| p == current).unwrap_or(0);
                    let oldest = path
                        .get(cycle_start..)
                        .unwrap_or_default()
                        .iter()
                        .copied()
                        .min_by(|&a, &b| match (messages.get(a), messages.get(b)) {
                            (Some(ma), Some(mb)) => message_order(ma, 0, mb, 0),
                            _ => a.cmp(&b),
                        });
                    if let Some(slot) = oldest.and_then(|o| parent.get_mut(o)) {
                        *slot = None;
                    }
                    break;
                }
                Some(Visit::Done) | None => break,
            }
        }
        for p in path {
            if let Some(v) = visit.get_mut(p) {
                *v = Visit::Done;
            }
        }
    }
}

/// The root a message's parent chain ends at. `parent` has no cycles.
fn root_of(parent: &[Option<usize>], mut i: usize) -> usize {
    while let Some(p) = parent.get(i).copied().flatten() {
        i = p;
    }
    i
}

/// Each message's distance from its root. `parent` has no cycles.
fn depths(parent: &[Option<usize>]) -> Vec<usize> {
    (0..parent.len())
        .map(|mut i| {
            let mut depth = 0;
            while let Some(p) = parent.get(i).copied().flatten() {
                depth += 1;
                i = p;
            }
            depth
        })
        .collect()
}

/// One mail item read from disk.
struct LoadedItem {
    item_ref: InboxRef,
    item: InboxItem,
    archived: bool,
    at: DateTime<Tz>,
    attachments: Vec<MailAttachment>,
}

/// Read one agent's four inbox directories into `items`: its user inbox, and
/// the user-authored items of its agent inbox, active and archived. An item
/// both active and archived (placed by hand) counts once, as active.
async fn load_mailbox(
    mailbox: &Mailbox,
    items: &mut Vec<LoadedItem>,
    problems: &mut Vec<MailLoadProblem>,
) {
    let layout = WorkspaceLayout::new(&mailbox.dir);
    for kind in [InboxKind::User, InboxKind::Agent] {
        let mut seen_ids = BTreeSet::new();
        for archived in [false, true] {
            let dir = kind.items_dir(&layout, archived);
            let listed = match list_items_or_empty(&dir).await {
                Ok(listed) => listed,
                Err(e) => {
                    tracing::warn!(
                        agent = %mailbox.agent,
                        dir = %dir.display(),
                        error = %e,
                        "couldn't read an inbox directory; its mail is left out"
                    );
                    problems.push(MailLoadProblem {
                        agent: mailbox.agent.clone(),
                        dir,
                        error: format!("{e:#}"),
                    });
                    continue;
                }
            };
            for (id, item) in listed {
                if kind == InboxKind::Agent && !is_user_mail_source(&item.source) {
                    continue;
                }
                let item_ref = match InboxRef::new(&mailbox.agent, kind, &id) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::debug!(agent = %mailbox.agent, id = %id, error = %e, "inbox item has no valid ref; left out of mail");
                        continue;
                    }
                };
                if !seen_ids.insert(id.clone()) {
                    tracing::debug!(item = %item_ref, "inbox item is both active and archived; mail shows the active one");
                    continue;
                }
                let attachments = resolve_attachments(&layout, &item_ref, archived, &item).await;
                items.push(LoadedItem {
                    at: residuum_core::time::local_to_instant(mailbox.timezone, item.timestamp),
                    item_ref,
                    item,
                    archived,
                    attachments,
                });
            }
        }
    }
}

/// The files of `item`'s attachments that exist, in its active or archived
/// attachment directory. Only each stored path's file name is used, since the
/// directory part goes stale when the item is archived.
async fn resolve_attachments(
    layout: &WorkspaceLayout,
    item_ref: &InboxRef,
    archived: bool,
    item: &InboxItem,
) -> Vec<MailAttachment> {
    let kind = item_ref.kind();
    let on_disk = kind.attachments_dir(layout, archived).join(item_ref.id());
    let relative = kind
        .attachments_dir(&WorkspaceLayout::new(PathBuf::new()), archived)
        .join(item_ref.id());
    let mut resolved = Vec::with_capacity(item.attachments.len());
    for (index, stored) in item.attachments.iter().enumerate() {
        let Some(filename) = stored.file_name().and_then(|f| f.to_str()) else {
            tracing::debug!(item = %item_ref, stored = %stored.display(), "inbox attachment entry has no filename, omitting");
            continue;
        };
        let path = on_disk.join(filename);
        match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_file() => resolved.push(MailAttachment {
                item: item_ref.clone(),
                index,
                filename: filename.to_string(),
                path: relative.join(filename),
                size: meta.len(),
                content_type: residuum_bus::attachment::detect_mime_type(&path),
            }),
            Ok(_) => {
                tracing::debug!(item = %item_ref, path = %path.display(), "inbox attachment is not a file, omitting");
            }
            Err(e) => {
                tracing::debug!(item = %item_ref, path = %path.display(), error = %e, "inbox attachment file missing on disk, omitting");
            }
        }
    }
    resolved
}

/// What makes agent-inbox items copies of one user message: the same id,
/// words and time. Same id alone could be two unrelated notes that happened
/// to get the same name in two workspaces.
type CopyKey = (String, String, String, DateTime<Utc>);

/// Turn loaded items into messages: each user-inbox item is one; the
/// user-authored agent-inbox copies of one send merge into one.
fn merge_copies(items: Vec<LoadedItem>) -> Vec<MailMessage> {
    let mut messages = Vec::new();
    let mut copies: BTreeMap<CopyKey, Vec<LoadedItem>> = BTreeMap::new();
    for loaded in items {
        match loaded.item_ref.kind() {
            InboxKind::User => messages.push(agent_message(loaded)),
            InboxKind::Agent => {
                let key = (
                    loaded.item_ref.id().to_string(),
                    loaded.item.title.clone(),
                    loaded.item.body.clone(),
                    loaded.at.with_timezone(&Utc),
                );
                copies.entry(key).or_default().push(loaded);
            }
        }
    }
    messages.extend(copies.into_values().filter_map(user_message));
    messages
}

fn parse_reply_ref(loaded: &LoadedItem) -> Option<InboxRef> {
    let raw = loaded.item.in_reply_to.as_deref()?;
    match InboxRef::parse(raw) {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::debug!(item = %loaded.item_ref, error = %e, "inbox item's in_reply_to is not a valid ref; it starts its own thread");
            None
        }
    }
}

fn agent_message(loaded: LoadedItem) -> MailMessage {
    let in_reply_to = parse_reply_ref(&loaded);
    MailMessage {
        from: MailSender::Agent(loaded.item_ref.agent().to_string()),
        copies: vec![loaded.item_ref.clone()],
        item_ref: loaded.item_ref,
        to: Vec::new(),
        at: loaded.at,
        title: loaded.item.title,
        body: loaded.item.body,
        attachments: loaded.attachments,
        in_reply_to,
        read: loaded.item.read,
        archived: loaded.archived,
        seen_by: Vec::new(),
    }
}

fn user_message(mut copies: Vec<LoadedItem>) -> Option<MailMessage> {
    copies.sort_by(|a, b| a.item_ref.cmp(&b.item_ref));
    let seen_by = copies
        .iter()
        .map(|c| MailSeen {
            agent: c.item_ref.agent().to_string(),
            seen: c.item.read || c.archived,
        })
        .collect();
    let to = copies
        .iter()
        .map(|c| c.item_ref.agent().to_string())
        .collect();
    let refs = copies.iter().map(|c| c.item_ref.clone()).collect();
    let first = copies.into_iter().next()?;
    let in_reply_to = parse_reply_ref(&first);
    Some(MailMessage {
        item_ref: first.item_ref,
        copies: refs,
        from: MailSender::User,
        to,
        at: first.at,
        title: first.item.title,
        body: first.item.body,
        attachments: first.attachments,
        in_reply_to,
        read: true,
        archived: false,
        seen_by,
    })
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
    async fn agent_message_and_cross_agent_replies_form_one_thread() {
        let fx = Fixture::new(&["atlas", "scout"]);
        let root = fx
            .user_item("atlas", "digest", "Daily digest", 9, None)
            .await;
        let reply = fx
            .user_mail(
                "send1",
                &["atlas", "scout"],
                "Re: Daily digest",
                10,
                Some(&root),
            )
            .await;
        fx.user_item("scout", "scout_note", "Re: Daily digest", 11, Some(&reply))
            .await;

        let index = fx.index().await;
        let all: Vec<_> = index.threads(&ThreadFilter::default()).collect();
        assert_eq!(all.len(), 1, "{all:#?}");
        let thread = all[0];
        assert_eq!(thread.thread_id(), &root);
        assert_eq!(thread.subject(), "Daily digest");
        assert_eq!(thread.participants(), ["atlas", "scout"]);
        assert_eq!(thread.recipients_for_reply(), ["atlas", "scout"]);
        let senders: Vec<_> = thread.messages().map(|m| m.from.clone()).collect();
        assert_eq!(
            senders,
            [
                MailSender::Agent("atlas".into()),
                MailSender::User,
                MailSender::Agent("scout".into())
            ]
        );
        assert!(thread.in_inbox() && thread.in_sent() && !thread.archived());
        assert!(thread.unread());
        let user = thread.messages().nth(1).unwrap();
        assert_eq!(user.to, ["atlas", "scout"]);
        assert_eq!(user.copies.len(), 2);
        assert_eq!(index.thread_containing(&user.copies[1]), Some(thread));
        assert_eq!(
            index
                .thread_containing(&user.copies[1])
                .unwrap()
                .thread_id(),
            &root,
            "a copy of a reply resolves to the thread"
        );
    }

    #[tokio::test]
    async fn a_reply_to_an_archived_item_threads_under_it() {
        let fx = Fixture::new(&["atlas"]);
        let root = fx.user_item("atlas", "old", "Old news", 9, None).await;
        fx.archive_user_item("atlas", "old").await;
        fx.user_item("atlas", "new", "Re: Old news", 10, Some(&root))
            .await;

        let index = fx.index().await;
        let thread = index.thread_containing(&root).unwrap();
        assert_eq!(thread.message_count(), 2);
        assert!(thread.root().archived);
        assert!(
            thread.in_inbox(),
            "a new active agent reply brings the thread back to Inbox"
        );
        assert!(!thread.archived());
    }

    #[tokio::test]
    async fn a_missing_or_invalid_parent_makes_its_own_root() {
        let fx = Fixture::new(&["atlas"]);
        let gone = InboxRef::new("atlas", InboxKind::User, "deleted").unwrap();
        let orphan = fx
            .user_item("atlas", "orphan", "Orphan", 9, Some(&gone))
            .await;
        fx.write_raw_reply("atlas", "bad", "Bad ref", 10, "not a ref")
            .await;

        let index = fx.index().await;
        assert_eq!(index.threads(&ThreadFilter::default()).count(), 2);
        let thread = index.thread_containing(&orphan).unwrap();
        assert_eq!(thread.message_count(), 1);
        assert_eq!(thread.root().in_reply_to.as_ref(), Some(&gone));
    }

    #[tokio::test]
    async fn self_references_and_cycles_do_not_hang_or_lose_messages() {
        let fx = Fixture::new(&["atlas"]);
        let a = InboxRef::new("atlas", InboxKind::User, "a").unwrap();
        let b = InboxRef::new("atlas", InboxKind::User, "b").unwrap();
        let s = InboxRef::new("atlas", InboxKind::User, "s").unwrap();
        fx.user_item("atlas", "a", "A", 9, Some(&b)).await;
        fx.user_item("atlas", "b", "B", 10, Some(&a)).await;
        fx.user_item("atlas", "s", "S", 11, Some(&s)).await;

        let index = fx.index().await;
        assert_eq!(index.threads(&ThreadFilter::default()).count(), 2);
        let cycle = index
            .thread_containing(&a)
            .expect("the oldest message of a cycle is its root");
        assert_eq!(cycle.message_count(), 2);
        assert_eq!(index.thread_containing(&s).unwrap().message_count(), 1);
    }

    #[tokio::test]
    async fn multi_recipient_seen_state_follows_each_copy() {
        let fx = Fixture::new(&["atlas", "scout", "drifter"]);
        let sent = fx
            .user_mail("hello", &["atlas", "scout", "drifter"], "Hello", 9, None)
            .await;
        fx.mark_agent_copy_read("atlas", "hello").await;
        fx.archive_agent_copy("scout", "hello").await;

        let index = fx.index().await;
        let thread = index.thread_containing(&sent).unwrap();
        let message = thread.root();
        assert_eq!(message.to, ["atlas", "drifter", "scout"]);
        assert_eq!(
            message.seen_by,
            [
                MailSeen {
                    agent: "atlas".into(),
                    seen: true
                },
                MailSeen {
                    agent: "drifter".into(),
                    seen: false
                },
                MailSeen {
                    agent: "scout".into(),
                    seen: true
                },
            ]
        );
        assert!(message.read && !message.archived);
    }

    #[tokio::test]
    async fn a_thread_nobody_answered_is_only_in_sent() {
        let fx = Fixture::new(&["atlas"]);
        let sent = fx.user_mail("q", &["atlas"], "Question", 9, None).await;

        let index = fx.index().await;
        let thread = index.thread_containing(&sent).unwrap();
        assert!(thread.in_sent() && !thread.in_inbox() && !thread.archived());
        assert!(!thread.unread());
        let inbox = ThreadFilter {
            folder: Some(MailFolder::Inbox),
            agent: None,
        };
        assert_eq!(index.threads(&inbox).count(), 0);
        let sent_folder = ThreadFilter {
            folder: Some(MailFolder::Sent),
            agent: None,
        };
        assert_eq!(index.threads(&sent_folder).count(), 1);
    }

    #[tokio::test]
    async fn non_mail_agent_inbox_items_never_appear() {
        let fx = Fixture::new(&["atlas"]);
        for source in [
            "pulse",
            "cron:backup",
            "webhook",
            "artifact:x",
            "system",
            "discord",
        ] {
            fx.agent_item_with_source(
                "atlas",
                &format!("n_{}", source.replace(':', "_")),
                source,
                9,
            )
            .await;
        }
        fx.agent_item_with_source("atlas", "from_cli", "cli", 9)
            .await;

        let index = fx.index().await;
        let all: Vec<_> = index.threads(&ThreadFilter::default()).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].thread_id().id(), "from_cli");
    }

    #[tokio::test]
    async fn copies_with_the_same_id_but_different_words_stay_separate() {
        let fx = Fixture::new(&["atlas", "scout"]);
        fx.agent_item("atlas", "note", "Note", "one body", 9).await;
        fx.agent_item("scout", "note", "Note", "another body", 9)
            .await;

        let index = fx.index().await;
        assert_eq!(index.threads(&ThreadFilter::default()).count(), 2);
    }

    #[tokio::test]
    async fn threads_sort_newest_first_and_filter_by_agent() {
        let fx = Fixture::new(&["atlas", "scout"]);
        let old = fx.user_item("atlas", "old", "Old", 8, None).await;
        let new = fx.user_item("scout", "new", "New", 12, None).await;
        let mid = fx.user_item("atlas", "mid", "Mid", 10, None).await;
        fx.user_item("atlas", "late_reply", "Re: Old", 13, Some(&old))
            .await;

        let index = fx.index().await;
        let order: Vec<_> = index
            .threads(&ThreadFilter::default())
            .map(|t| t.thread_id().clone())
            .collect();
        assert_eq!(order, [old.clone(), new.clone(), mid.clone()]);
        let atlas = ThreadFilter {
            folder: None,
            agent: Some("atlas".into()),
        };
        let atlas_threads: Vec<_> = index
            .threads(&atlas)
            .map(|t| t.thread_id().clone())
            .collect();
        assert_eq!(atlas_threads, [old, mid]);
    }

    #[tokio::test]
    async fn a_reply_written_the_same_minute_follows_its_parent() {
        let fx = Fixture::new(&["atlas"]);
        let root = fx.user_item("atlas", "zzz_root", "Root", 9, None).await;
        fx.user_item("atlas", "aaa_reply", "Re: Root", 9, Some(&root))
            .await;

        let index = fx.index().await;
        let thread = index.thread_containing(&root).unwrap();
        let ids: Vec<_> = thread
            .messages()
            .map(|m| m.item_ref.id().to_string())
            .collect();
        assert_eq!(ids, ["zzz_root", "aaa_reply"]);
    }

    #[tokio::test]
    async fn latest_is_the_newest_message_even_when_a_reply_predates_its_root() {
        let fx = Fixture::new(&["atlas"]);
        let root = fx.user_item("atlas", "root", "Root", 10, None).await;
        fx.user_item("atlas", "early_reply", "Re: Root", 9, Some(&root))
            .await;

        let index = fx.index().await;
        let thread = index.thread_containing(&root).unwrap();
        assert_eq!(thread.latest().item_ref, root);
        assert_eq!(thread.latest_at(), thread.root().at);
    }

    #[tokio::test]
    async fn attachments_resolve_where_the_item_is_now() {
        let fx = Fixture::new(&["atlas"]);
        let root = fx.user_item("atlas", "files", "Files", 9, None).await;
        fx.attach_to_user_item("atlas", "files", "notes.md", b"# hi")
            .await;
        fx.archive_user_item("atlas", "files").await;

        let index = fx.index().await;
        let thread = index.thread_containing(&root).unwrap();
        assert!(thread.has_attachments());
        let attachment = &thread.root().attachments[0];
        assert_eq!(attachment.agent(), "atlas");
        assert_eq!(attachment.index, 0);
        assert_eq!(attachment.filename, "notes.md");
        assert_eq!(attachment.size, 4);
        assert_eq!(
            attachment.path,
            PathBuf::from("archive/inbox/user/attachments/files/notes.md")
        );
    }

    #[tokio::test]
    async fn an_unreadable_inbox_is_reported_not_fatal() {
        let fx = Fixture::new(&["atlas", "scout"]);
        fx.user_item("scout", "fine", "Fine", 9, None).await;
        let layout = WorkspaceLayout::new(fx.dir("atlas"));
        tokio::fs::create_dir_all(layout.user_inbox_dir().parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(layout.user_inbox_dir(), b"not a directory")
            .await
            .unwrap();

        let index = fx.index().await;
        assert_eq!(index.threads(&ThreadFilter::default()).count(), 1);
        assert_eq!(index.problems().len(), 1);
        assert_eq!(index.problems()[0].agent, "atlas");
    }
}
