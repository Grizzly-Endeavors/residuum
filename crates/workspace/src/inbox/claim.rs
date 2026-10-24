//! Claiming inbox item ids on disk.
//!
//! [`ClaimedItem`] is the one way a new inbox item is created. Every writer of a
//! new item (the inbox tools, the notification inbox channel, the chat adapters'
//! attachment items, the A2A paths) claims an id first, so two writers can never
//! be handed the same id.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;

use residuum_config::WorkspaceLayout;

use super::{InboxItem, generate_filename, save_item};

/// An inbox item id claimed on disk, which its holder then saves as an item or releases.
///
/// Claiming creates `attachments/<id>/` in the inbox with a create-new operation,
/// so two claims racing for the same id cannot both win: the loser moves on to
/// the next suffix (`_2`, `_3`, ...). An id is taken when its item JSON or its
/// `attachments/<id>/` directory exists in the inbox or in the archive that pairs
/// with it (an archived item keeps its id). Claiming creates the inbox directory
/// when it is missing.
///
/// The holder owns the claimed directory from then on: files go into
/// [`Self::attachments_dir`], and [`Self::save`] writes the item's JSON, which
/// completes the claim. A claim that is dropped without being saved or released
/// (a cancelled task, a failed save) removes its directory, so nothing is left
/// behind. [`Self::release`] removes it explicitly, and [`Self::keep`] leaves it.
/// A saved item without attachments has no directory, only its JSON, so the JSON
/// is checked again after the directory is created.
///
/// To give one item the same id in several inboxes (a send to several
/// recipients), claim all of them at once with [`Self::claim_in_all`]: if the id
/// is taken in any one inbox, it releases what it claimed and retries the next
/// suffix, so the returned claims share one id.
#[derive(Debug)]
pub struct ClaimedItem {
    inbox_dir: PathBuf,
    /// Filename stem of the item, which also names its attachment directory.
    id: String,
    attachments_dir: PathBuf,
    /// Whether dropping the claim removes its attachments directory.
    armed: bool,
}

impl ClaimedItem {
    /// Claim an id for an item titled `title` at `now` in `inbox_dir`.
    ///
    /// The id is the title's [`generate_filename`] stem, with `_2`, `_3`, ...
    /// appended while it is taken.
    ///
    /// # Errors
    /// Returns an error if the attachments directory cannot be created, or if
    /// claiming a directory fails for any reason other than it already existing.
    pub async fn claim(inbox_dir: &Path, title: &str, now: NaiveDateTime) -> Result<Self, String> {
        let mut claimed = Self::claim_in_all(&[inbox_dir], title, now).await?;
        claimed.pop().ok_or_else(|| {
            format!(
                "claiming an inbox item id in {} returned no claim",
                inbox_dir.display()
            )
        })
    }

    /// Claim one id that is free in every inbox of `inbox_dirs`, returning one
    /// claim per inbox in the same order, all with the same id.
    ///
    /// When the id is taken in any one inbox, the claims made so far are
    /// released and the next suffix is tried, so the ids always agree.
    ///
    /// # Errors
    /// Returns an error if `inbox_dirs` is empty or names the same directory
    /// twice, if an attachments directory cannot be created, or if claiming a
    /// directory fails for any reason other than it already existing. Nothing is
    /// left claimed on error.
    pub async fn claim_in_all<P: AsRef<Path>>(
        inbox_dirs: &[P],
        title: &str,
        now: NaiveDateTime,
    ) -> Result<Vec<Self>, String> {
        Self::claim_in_all_with(inbox_dirs, title, now, Self::try_claim).await
    }

    /// [`Self::claim_in_all`] with the per-inbox claim step supplied, so a test
    /// can fail one inbox's claim without filesystem permissions.
    async fn claim_in_all_with<P, F>(
        inbox_dirs: &[P],
        title: &str,
        now: NaiveDateTime,
        claim_one: F,
    ) -> Result<Vec<Self>, String>
    where
        P: AsRef<Path>,
        F: AsyncFn(&Path, Option<&Path>, &str) -> Result<Option<Self>, String>,
    {
        let dirs: Vec<&Path> = inbox_dirs.iter().map(AsRef::<Path>::as_ref).collect();
        if dirs.is_empty() {
            return Err("no inbox to claim an item id in".to_string());
        }
        for dir in &dirs {
            let attachments_root = dir.join("attachments");
            tokio::fs::create_dir_all(&attachments_root)
                .await
                .map_err(|e| {
                    format!(
                        "failed to create attachments dir {}: {e}",
                        attachments_root.display()
                    )
                })?;
        }
        // Compare the inboxes as the filesystem resolves them, so one inbox spelled
        // two ways (relative, `..`, a symlink) is still rejected: its second claim
        // would always find the first one's directory and never succeed.
        let mut resolved: HashSet<PathBuf> = HashSet::with_capacity(dirs.len());
        let mut archives: Vec<Option<PathBuf>> = Vec::with_capacity(dirs.len());
        for dir in &dirs {
            let canonical = tokio::fs::canonicalize(dir).await.map_err(|e| {
                format!(
                    "failed to resolve inbox dir {} to claim an item id in: {e}",
                    dir.display()
                )
            })?;
            if !resolved.insert(canonical.clone()) {
                return Err(format!(
                    "inbox directory {} was given more than once to claim an item id in",
                    dir.display()
                ));
            }
            archives.push(archive_sibling(&canonical));
        }

        let base = generate_filename(title, now);
        let stem = base.trim_end_matches(".json");
        let mut suffix: u32 = 1;
        loop {
            let id = if suffix == 1 {
                stem.to_string()
            } else {
                format!("{stem}_{suffix}")
            };
            let mut claimed: Vec<Self> = Vec::with_capacity(dirs.len());
            let mut taken = false;
            for (dir, archive) in dirs.iter().zip(&archives) {
                match claim_one(dir, archive.as_deref(), &id).await {
                    Ok(Some(claim)) => claimed.push(claim),
                    Ok(None) => {
                        taken = true;
                        break;
                    }
                    Err(e) => {
                        release_all(claimed).await;
                        return Err(e);
                    }
                }
            }
            if !taken {
                return Ok(claimed);
            }
            release_all(claimed).await;
            suffix += 1;
        }
    }

    /// Claim `id` in `inbox_dir`, or `None` when it is taken.
    async fn try_claim(
        inbox_dir: &Path,
        archive_dir: Option<&Path>,
        id: &str,
    ) -> Result<Option<Self>, String> {
        if id_in_use(inbox_dir, archive_dir, id).await {
            return Ok(None);
        }
        Self::claim_dir(inbox_dir, archive_dir, id).await
    }

    /// Create `id`'s attachments directory in `inbox_dir`, or `None` when the id
    /// is taken once the directory is in place.
    ///
    /// [`ClaimedItem::save`] writes the item's JSON before it removes an empty
    /// directory, so a writer that saved between the check in [`Self::try_claim`]
    /// and the create above leaves the id held by its JSON alone. That JSON is
    /// checked again here, and the directory this claim just created is removed
    /// rather than overwriting the saved item.
    async fn claim_dir(
        inbox_dir: &Path,
        archive_dir: Option<&Path>,
        id: &str,
    ) -> Result<Option<Self>, String> {
        let attachments_dir = inbox_dir.join("attachments").join(id);
        match tokio::fs::create_dir(&attachments_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
            Err(e) => {
                return Err(format!(
                    "failed to claim inbox item id {id} at {}: {e}",
                    attachments_dir.display()
                ));
            }
        }
        if json_in_use(inbox_dir, archive_dir, id).await {
            if let Err(e) = tokio::fs::remove_dir(&attachments_dir).await {
                tracing::warn!(
                    path = %attachments_dir.display(),
                    error = %e,
                    "failed to remove a claimed attachments dir whose id a save had taken"
                );
            }
            return Ok(None);
        }
        Ok(Some(Self {
            inbox_dir: inbox_dir.to_path_buf(),
            id: id.to_string(),
            attachments_dir,
            armed: true,
        }))
    }

    /// The claimed id: the item's filename without `.json`.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The item's JSON filename, `<id>.json`.
    #[must_use]
    pub fn filename(&self) -> String {
        format!("{}.json", self.id)
    }

    /// The inbox directory the id is claimed in.
    #[must_use]
    pub fn inbox_dir(&self) -> &Path {
        &self.inbox_dir
    }

    /// The directory the item's files go into: `attachments/<id>/` in the inbox.
    #[must_use]
    pub fn attachments_dir(&self) -> &Path {
        &self.attachments_dir
    }

    /// The path to record in [`InboxItem::attachments`] for a file called
    /// `file_name` in [`Self::attachments_dir`]: relative to the workspace root,
    /// under the user or agent inbox the claim is in.
    ///
    /// The path is always `/`-separated, whatever the platform: the item JSON
    /// stores it, so a Windows item must read the same as one written anywhere
    /// else. Reads go through `Path`, which accepts either separator.
    ///
    /// An inbox outside the workspace layout has no workspace root to be
    /// relative to, so its paths are relative to the inbox directory.
    #[must_use]
    pub fn recorded_attachment_path(&self, file_name: &str) -> PathBuf {
        let attachments = workspace_relative_attachments_dir(&self.inbox_dir);
        PathBuf::from(format!("{attachments}/{}/{file_name}", self.id))
    }

    /// Write the item's JSON atomically under the claimed id, which completes
    /// the claim, and remove the attachments directory when nothing was put in it.
    ///
    /// A failed write leaves the claim unsaved, so dropping it (or
    /// [`Self::release`]) removes the directory.
    ///
    /// # Errors
    /// Returns an error if the item cannot be serialized or written.
    pub async fn save(&mut self, item: &InboxItem) -> anyhow::Result<()> {
        save_item(&self.inbox_dir, &self.filename(), item).await?;
        self.armed = false;
        self.remove_attachments_dir_if_empty().await;
        Ok(())
    }

    /// Remove the claimed directory and anything in it, for an item that will
    /// not be saved. A failed removal is logged, not returned: the failure that
    /// led here is already reported to the user.
    pub async fn release(mut self) {
        remove_claimed_dir(&self.attachments_dir).await;
        self.armed = false;
    }

    /// Leave the claimed directory and its files on disk, and keep the id taken
    /// without an item JSON. For a holder whose files are the only copy and
    /// whose item could not be saved.
    pub fn keep(mut self) {
        self.armed = false;
    }

    async fn remove_attachments_dir_if_empty(&self) {
        let dir = &self.attachments_dir;
        let removed = async {
            let empty = {
                let mut entries = tokio::fs::read_dir(dir).await?;
                entries.next_entry().await?.is_none()
            };
            if empty {
                tokio::fs::remove_dir(dir).await?;
            }
            Ok::<(), std::io::Error>(())
        }
        .await;
        match removed {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(
                    path = %dir.display(),
                    error = %e,
                    "failed to remove the empty attachments dir of a saved inbox item"
                );
            }
        }
    }
}

impl Drop for ClaimedItem {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Synchronous because a drop cannot await: this is the cleanup for a
        // cancelled task or a failed save, and the directory is small.
        match std::fs::remove_dir_all(&self.attachments_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(
                    path = %self.attachments_dir.display(),
                    error = %e,
                    "failed to remove the claimed attachments dir of an unsaved inbox item"
                );
            }
        }
    }
}

async fn release_all(claimed: Vec<ClaimedItem>) {
    for claim in claimed {
        claim.release().await;
    }
}

async fn remove_claimed_dir(dir: &Path) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(
                path = %dir.display(),
                error = %e,
                "failed to remove the claimed attachments dir of an inbox item"
            );
        }
    }
}

/// The archive directory that pairs with a workspace inbox directory:
/// `<root>/inbox/<kind>` archives to `<root>/archive/inbox/<kind>`, the layout
/// `WorkspaceLayout` defines. `None` for a directory that isn't laid out that
/// way, which then has no archive to check.
fn archive_sibling(inbox_dir: &Path) -> Option<PathBuf> {
    let kind = inbox_dir.file_name()?;
    let inbox = inbox_dir.parent()?;
    if inbox.file_name()? != "inbox" {
        return None;
    }
    Some(inbox.parent()?.join("archive").join("inbox").join(kind))
}

/// The user or agent inbox's attachments directory relative to the workspace
/// root (`inbox/<kind>/attachments`), taken from the layout rooted at the empty
/// path, joined with `/`. For an inbox directory outside the layout, plain
/// `attachments`.
fn workspace_relative_attachments_dir(inbox_dir: &Path) -> String {
    let layout = WorkspaceLayout::new(PathBuf::new());
    [
        (layout.user_inbox_dir(), layout.user_inbox_attachments_dir()),
        (
            layout.agent_inbox_dir(),
            layout.agent_inbox_attachments_dir(),
        ),
    ]
    .into_iter()
    .find(|(inbox, _)| inbox_dir.ends_with(inbox))
    .map_or_else(
        || "attachments".to_string(),
        |(_, attachments)| {
            attachments
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        },
    )
}

/// Whether `id` is taken in the inbox or, when there is one, its archive.
///
/// An id is taken by its item JSON, and by its `attachments/<id>/` directory in
/// either place: a claim creates that directory before its JSON is saved, so the
/// directory alone already belongs to the id.
async fn id_in_use(inbox_dir: &Path, archive_dir: Option<&Path>, id: &str) -> bool {
    if holds_id(inbox_dir, id).await {
        return true;
    }
    match archive_dir {
        Some(dir) => holds_id(dir, id).await,
        None => false,
    }
}

async fn holds_id(dir: &Path, id: &str) -> bool {
    json_in_use(dir, None, id).await || path_exists(&dir.join("attachments").join(id)).await
}

/// Whether an item JSON named `id` exists in the inbox or, when there is one, its archive.
async fn json_in_use(inbox_dir: &Path, archive_dir: Option<&Path>, id: &str) -> bool {
    let json = format!("{id}.json");
    if path_exists(&inbox_dir.join(&json)).await {
        return true;
    }
    match archive_dir {
        Some(dir) => path_exists(&dir.join(&json)).await,
        None => false,
    }
}

async fn path_exists(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::{list_items, load_item};
    use chrono::NaiveDate;

    fn test_now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 2, 25)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn item(title: &str) -> InboxItem {
        InboxItem {
            title: title.to_string(),
            body: format!("Body for {title}"),
            source: "test".to_string(),
            timestamp: test_now(),
            read: false,
            attachments: Vec::new(),
            in_reply_to: None,
        }
    }

    async fn workspace_inbox(root: &Path) -> PathBuf {
        let inbox = WorkspaceLayout::new(root).agent_inbox_dir();
        tokio::fs::create_dir_all(&inbox).await.unwrap();
        inbox
    }

    #[tokio::test]
    async fn claiming_the_same_title_twice_gives_distinct_ids_and_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let title = "Discord attachment: image.png";

        let first = ClaimedItem::claim(&inbox, title, test_now()).await.unwrap();
        let second = ClaimedItem::claim(&inbox, title, test_now()).await.unwrap();

        assert_eq!(first.id(), "20260225_discord_attachment_image_png");
        assert_eq!(second.id(), "20260225_discord_attachment_image_png_2");
        assert_eq!(
            first.filename(),
            "20260225_discord_attachment_image_png.json"
        );
        for claim in [&first, &second] {
            assert_eq!(claim.inbox_dir(), inbox);
            assert_eq!(
                claim.attachments_dir(),
                inbox.join("attachments").join(claim.id())
            );
            assert!(
                claim.attachments_dir().is_dir(),
                "claimed directory should exist for {}",
                claim.id()
            );
        }
    }

    #[tokio::test]
    async fn claim_skips_an_id_whose_directory_is_held_without_json() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        tokio::fs::create_dir_all(inbox.join("attachments/20260225_held"))
            .await
            .unwrap();

        let claimed = ClaimedItem::claim(&inbox, "held", test_now())
            .await
            .unwrap();

        assert_eq!(claimed.id(), "20260225_held_2");
    }

    #[tokio::test]
    async fn claim_skips_an_id_whose_item_json_exists() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        save_item(&inbox, "20260225_taken.json", &item("taken"))
            .await
            .unwrap();

        let claimed = ClaimedItem::claim(&inbox, "taken", test_now())
            .await
            .unwrap();

        assert_eq!(claimed.id(), "20260225_taken_2");
    }

    #[tokio::test]
    async fn claim_skips_an_id_held_by_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let inbox = workspace_inbox(dir.path()).await;
        tokio::fs::create_dir_all(layout.agent_inbox_archive_dir())
            .await
            .unwrap();
        save_item(
            &layout.agent_inbox_archive_dir(),
            "20260225_gone.json",
            &item("gone"),
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(
            layout
                .agent_inbox_archive_attachments_dir()
                .join("20260225_gone_2"),
        )
        .await
        .unwrap();

        let claimed = ClaimedItem::claim(&inbox, "gone", test_now())
            .await
            .unwrap();

        assert_eq!(
            claimed.id(),
            "20260225_gone_3",
            "an archived item keeps its id, JSON or attachments directory alike"
        );
    }

    #[tokio::test]
    async fn recorded_attachment_path_is_slash_separated_on_every_platform() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let claim = ClaimedItem::claim(&inbox, "slashes", test_now())
            .await
            .unwrap();

        let recorded = claim.recorded_attachment_path("report.pdf");

        assert_eq!(
            recorded.to_str(),
            Some(format!("inbox/agent/attachments/{}/report.pdf", claim.id()).as_str()),
        );
    }

    #[tokio::test]
    async fn recorded_user_attachment_path_is_slash_separated() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = WorkspaceLayout::new(dir.path()).user_inbox_dir();
        let claim = ClaimedItem::claim(&inbox, "user slashes", test_now())
            .await
            .unwrap();

        let recorded = claim.recorded_attachment_path("a.txt");

        assert_eq!(
            recorded.to_str(),
            Some(format!("inbox/user/attachments/{}/a.txt", claim.id()).as_str()),
        );
    }

    #[tokio::test]
    async fn claim_creates_a_missing_inbox_directory() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = WorkspaceLayout::new(dir.path()).user_inbox_dir();

        let claimed = ClaimedItem::claim(&inbox, "fresh", test_now())
            .await
            .unwrap();

        assert!(claimed.attachments_dir().is_dir());
    }

    #[tokio::test]
    async fn claim_fails_when_the_inbox_cannot_be_created() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("inbox");
        tokio::fs::write(&blocker, b"a file, not a directory")
            .await
            .unwrap();

        let err = ClaimedItem::claim(&blocker.join("agent"), "blocked", test_now())
            .await
            .unwrap_err();

        assert!(
            err.contains("failed to create attachments dir"),
            "the error names what failed: {err}"
        );
    }

    #[tokio::test]
    async fn dropping_an_unsaved_claim_removes_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let claim = ClaimedItem::claim(&inbox, "dropped", test_now())
            .await
            .unwrap();
        let attachments_dir = claim.attachments_dir().to_path_buf();
        tokio::fs::write(attachments_dir.join("partial.bin"), b"partial")
            .await
            .unwrap();

        drop(claim);

        assert!(
            !attachments_dir.exists(),
            "a dropped claim must not leave its directory or files behind"
        );
        let again = ClaimedItem::claim(&inbox, "dropped", test_now())
            .await
            .unwrap();
        assert_eq!(
            again.id(),
            "20260225_dropped",
            "the id is free again once the claim is gone"
        );
    }

    #[tokio::test]
    async fn cancelling_a_task_that_holds_a_claim_removes_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let (claimed_tx, claimed_rx) = tokio::sync::oneshot::channel();
        let task_inbox = inbox.clone();
        let task = tokio::spawn(async move {
            let claim = ClaimedItem::claim(&task_inbox, "cancelled", test_now())
                .await
                .unwrap();
            claimed_tx
                .send(claim.attachments_dir().to_path_buf())
                .unwrap();
            // Held until the task is aborted at this await.
            std::future::pending::<()>().await;
            drop(claim);
        });
        let attachments_dir = claimed_rx.await.unwrap();
        assert!(attachments_dir.is_dir(), "the task holds its claim");

        task.abort();
        let joined = task.await;

        assert!(joined.unwrap_err().is_cancelled());
        assert!(
            !attachments_dir.exists(),
            "a cancelled task must not leave its claimed directory behind"
        );
    }

    #[tokio::test]
    async fn a_failed_save_leaves_the_claim_to_be_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let mut claim = ClaimedItem::claim(&inbox, "unsavable", test_now())
            .await
            .unwrap();
        let attachments_dir = claim.attachments_dir().to_path_buf();
        // A directory where the JSON belongs makes the atomic rename fail.
        tokio::fs::create_dir(inbox.join(claim.filename()))
            .await
            .unwrap();

        let result = claim.save(&item("unsavable")).await;

        assert!(result.is_err(), "the save cannot replace a directory");
        drop(claim);
        assert!(
            !attachments_dir.exists(),
            "a claim whose save failed must not leave its directory behind"
        );
    }

    #[tokio::test]
    async fn a_kept_claim_leaves_its_files_and_the_id_taken() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let claim = ClaimedItem::claim(&inbox, "kept", test_now())
            .await
            .unwrap();
        let file = claim.attachments_dir().join("only-copy.bin");
        tokio::fs::write(&file, b"data").await.unwrap();

        claim.keep();

        assert!(file.is_file(), "keep leaves the files in place");
        let next = ClaimedItem::claim(&inbox, "kept", test_now())
            .await
            .unwrap();
        assert_eq!(next.id(), "20260225_kept_2", "the kept id stays taken");
    }

    #[tokio::test]
    async fn save_writes_the_json_and_removes_an_empty_attachments_dir() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let mut claim = ClaimedItem::claim(&inbox, "plain", test_now())
            .await
            .unwrap();
        let attachments_dir = claim.attachments_dir().to_path_buf();
        let json_path = inbox.join(claim.filename());

        claim.save(&item("plain")).await.unwrap();
        drop(claim);

        assert_eq!(load_item(&json_path).await.unwrap().title, "plain");
        assert!(
            !attachments_dir.exists(),
            "an item without files has no attachments dir"
        );
    }

    #[tokio::test]
    async fn save_keeps_an_attachments_dir_that_holds_files() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let mut claim = ClaimedItem::claim(&inbox, "with file", test_now())
            .await
            .unwrap();
        let file = claim.attachments_dir().join("report.pdf");
        tokio::fs::write(&file, b"pdf").await.unwrap();

        claim.save(&item("with file")).await.unwrap();
        drop(claim);

        assert!(file.is_file(), "a saved claim keeps its files");
    }

    #[tokio::test]
    async fn release_removes_the_claim_and_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let claim = ClaimedItem::claim(&inbox, "released", test_now())
            .await
            .unwrap();
        let attachments_dir = claim.attachments_dir().to_path_buf();
        tokio::fs::write(attachments_dir.join("partial.bin"), b"partial")
            .await
            .unwrap();

        claim.release().await;

        assert!(!attachments_dir.exists(), "release removes the directory");
        assert!(
            list_items(&inbox).await.unwrap().is_empty(),
            "a released claim saves no item"
        );
    }

    #[tokio::test]
    async fn release_is_a_no_op_when_the_directory_is_already_gone() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        let claim = ClaimedItem::claim(&inbox, "gone", test_now())
            .await
            .unwrap();
        tokio::fs::remove_dir_all(claim.attachments_dir())
            .await
            .unwrap();

        claim.release().await;
    }

    #[tokio::test]
    async fn claim_in_all_gives_every_inbox_the_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let user = layout.user_inbox_dir();
        let agent = layout.agent_inbox_dir();
        // The base id is free in the user inbox, which is claimed first, but taken
        // in the agent inbox: the user inbox's claim is released and both move on.
        tokio::fs::create_dir_all(agent.join("attachments/20260225_hello"))
            .await
            .unwrap();

        let claims = ClaimedItem::claim_in_all(&[&user, &agent], "hello", test_now())
            .await
            .unwrap();

        let ids: Vec<&str> = claims.iter().map(ClaimedItem::id).collect();
        assert_eq!(ids, ["20260225_hello_2", "20260225_hello_2"]);
        let claimed_inboxes: Vec<&Path> = claims.iter().map(ClaimedItem::inbox_dir).collect();
        assert_eq!(claimed_inboxes, [user.as_path(), agent.as_path()]);
        assert!(
            !user.join("attachments/20260225_hello").exists(),
            "the id that was only free in the user inbox is not left claimed there"
        );
        for claim in &claims {
            assert!(claim.attachments_dir().is_dir());
        }
    }

    #[tokio::test]
    async fn claim_in_all_rejects_an_empty_list_and_a_repeated_directory() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;

        let respelled = inbox.join("../agent");

        let none: [&Path; 0] = [];
        let empty = ClaimedItem::claim_in_all(&none, "x", test_now()).await;
        let repeated = ClaimedItem::claim_in_all(&[&inbox, &inbox], "x", test_now()).await;
        let respelled_repeat =
            ClaimedItem::claim_in_all(&[&inbox, &respelled], "x", test_now()).await;

        assert!(empty.is_err(), "no inbox to claim in");
        assert!(repeated.is_err(), "the same inbox twice would never agree");
        assert!(
            respelled_repeat.is_err(),
            "the same inbox spelled another way would never agree either"
        );
        assert!(
            !inbox.join("attachments/20260225_x").exists(),
            "a rejected request claims nothing"
        );
    }

    #[tokio::test]
    async fn claim_dir_releases_its_directory_when_a_save_took_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = workspace_inbox(dir.path()).await;
        // `claim_in_all` creates the attachments root before `claim_dir` runs.
        tokio::fs::create_dir_all(inbox.join("attachments"))
            .await
            .unwrap();
        // A saved item without attachments: its JSON is the only trace of the id.
        save_item(&inbox, "20260225_raced.json", &item("raced"))
            .await
            .unwrap();

        let claim = ClaimedItem::claim_dir(&inbox, None, "20260225_raced")
            .await
            .unwrap();

        assert!(claim.is_none(), "the saved item keeps its id");
        assert!(
            !inbox.join("attachments/20260225_raced").exists(),
            "the directory this claim created is removed"
        );
        assert_eq!(
            load_item(&inbox.join("20260225_raced.json"))
                .await
                .unwrap()
                .title,
            "raced",
            "the saved item is left as it was"
        );
    }

    #[tokio::test]
    async fn claim_in_all_releases_its_claims_when_a_later_inbox_fails() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let user = layout.user_inbox_dir();
        let agent = layout.agent_inbox_dir();
        tokio::fs::create_dir_all(&user).await.unwrap();
        tokio::fs::create_dir_all(&agent).await.unwrap();

        // The user inbox is claimed first, so its claim must be released when the
        // agent inbox's claim fails with an I/O error.
        let result = ClaimedItem::claim_in_all_with(
            &[&user, &agent],
            "failing",
            test_now(),
            async |inbox: &Path, archive: Option<&Path>, id: &str| {
                if inbox == agent.as_path() {
                    let io_err = std::io::Error::other("injected failure");
                    return Err(format!(
                        "failed to claim inbox item id {id} at {}: {io_err}",
                        inbox.join("attachments").join(id).display()
                    ));
                }
                ClaimedItem::try_claim(inbox, archive, id).await
            },
        )
        .await;

        let err = result.unwrap_err();
        assert!(
            err.contains("failed to claim inbox item id"),
            "the error names the failed claim: {err}"
        );
        assert!(
            !user.join("attachments/20260225_failing").exists(),
            "the user inbox's claim is released when the agent inbox fails"
        );
    }

    #[test]
    fn recorded_attachment_paths_follow_the_inbox_the_claim_is_in() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        let claim_in = |inbox: PathBuf| ClaimedItem {
            attachments_dir: inbox.join("attachments").join("item"),
            inbox_dir: inbox,
            id: "item".to_string(),
            armed: false,
        };

        assert_eq!(
            claim_in(layout.user_inbox_dir()).recorded_attachment_path("a.txt"),
            PathBuf::from("inbox/user/attachments/item/a.txt")
        );
        assert_eq!(
            claim_in(layout.agent_inbox_dir()).recorded_attachment_path("a.txt"),
            PathBuf::from("inbox/agent/attachments/item/a.txt")
        );
        assert_eq!(
            claim_in(PathBuf::from("/tmp/elsewhere")).recorded_attachment_path("a.txt"),
            PathBuf::from("attachments/item/a.txt"),
            "an inbox outside the layout records paths relative to itself"
        );
    }

    #[test]
    fn archive_sibling_follows_the_workspace_layout() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        assert_eq!(
            archive_sibling(&layout.user_inbox_dir()),
            Some(layout.user_inbox_archive_dir()),
            "the user inbox archives where the layout says"
        );
        assert_eq!(
            archive_sibling(&layout.agent_inbox_dir()),
            Some(layout.agent_inbox_archive_dir()),
            "the agent inbox archives where the layout says"
        );
        assert_eq!(
            archive_sibling(Path::new("/tmp/ws/notes")),
            None,
            "a directory outside the layout has no archive to check"
        );
    }
}
