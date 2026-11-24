use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;

use residuum_config::WorkspaceLayout;

use super::{InboxItem, generate_filename, save_item};

#[derive(Debug)]
pub struct ClaimedItem {
    inbox_dir: PathBuf,
    id: String,
    attachments_dir: PathBuf,
    armed: bool,
}

impl ClaimedItem {
    pub async fn claim(inbox_dir: &Path, title: &str, now: NaiveDateTime) -> Result<Self, String> {
        let mut claimed = Self::claim_in_all(&[inbox_dir], title, now).await?;
        claimed.pop().ok_or_else(|| {
            format!(
                "claiming an inbox item id in {} returned no claim",
                inbox_dir.display()
            )
        })
    }

    pub async fn claim_in_all<P: AsRef<Path>>(
        inbox_dirs: &[P],
        title: &str,
        now: NaiveDateTime,
    ) -> Result<Vec<Self>, String> {
        Self::claim_in_all_with(inbox_dirs, title, now, Self::try_claim).await
    }

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

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn filename(&self) -> String {
        format!("{}.json", self.id)
    }

    #[must_use]
    pub fn inbox_dir(&self) -> &Path {
        &self.inbox_dir
    }

    #[must_use]
    pub fn attachments_dir(&self) -> &Path {
        &self.attachments_dir
    }

    #[must_use]
    pub fn recorded_attachment_path(&self, file_name: &str) -> PathBuf {
        let attachments = workspace_relative_attachments_dir(&self.inbox_dir);
        PathBuf::from(format!("{attachments}/{}/{file_name}", self.id))
    }

    pub async fn save(&mut self, item: &InboxItem) -> anyhow::Result<()> {
        save_item(&self.inbox_dir, &self.filename(), item).await?;
        self.armed = false;
        self.remove_attachments_dir_if_empty().await;
        Ok(())
    }

    pub async fn release(mut self) {
        remove_claimed_dir(&self.attachments_dir).await;
        self.armed = false;
    }

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

fn archive_sibling(inbox_dir: &Path) -> Option<PathBuf> {
    let kind = inbox_dir.file_name()?;
    let inbox = inbox_dir.parent()?;
    if inbox.file_name()? != "inbox" {
        return None;
    }
    Some(inbox.parent()?.join("archive").join("inbox").join(kind))
}

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
        tokio::fs::create_dir_all(inbox.join("attachments"))
            .await
            .unwrap();
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
