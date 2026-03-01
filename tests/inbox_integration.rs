//! End-to-end integration tests for the inbox subsystem (Phase 4).
//!
//! Tests the full inbox lifecycle using temporary directories.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod inbox_integration {
    use std::path::PathBuf;

    use chrono::NaiveDate;
    use tempfile::tempdir;

    use residuum::inbox::{
        self, InboxItem, archive_item, count_unread, generate_filename, list_items, load_item,
        mark_read, save_item,
    };

    fn make_item(title: &str, read: bool) -> InboxItem {
        InboxItem {
            title: title.to_string(),
            body: format!("Body for {title}"),
            source: "test".to_string(),
            timestamp: NaiveDate::from_ymd_opt(2026, 2, 25)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
            read,
            attachments: Vec::new(),
        }
    }

    fn make_item_at(title: &str, hour: u32, read: bool) -> InboxItem {
        InboxItem {
            title: title.to_string(),
            body: format!("Body for {title}"),
            source: "test".to_string(),
            timestamp: NaiveDate::from_ymd_opt(2026, 2, 25)
                .unwrap()
                .and_hms_opt(hour, 0, 0)
                .unwrap(),
            read,
            attachments: Vec::new(),
        }
    }

    // ── Full lifecycle ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn full_lifecycle_add_list_read_archive() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        let archive_dir = dir.path().join("archive/inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        // Add
        let filename = "20260225_lifecycle_test.json";
        save_item(&inbox_dir, filename, &make_item("lifecycle test", false))
            .await
            .unwrap();

        // List
        let items = list_items(&inbox_dir).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "20260225_lifecycle_test");
        assert!(!items[0].1.read);

        // Read (marks as read)
        let read_item = mark_read(&inbox_dir, "20260225_lifecycle_test")
            .await
            .unwrap();
        assert!(read_item.read);

        // Verify persisted read state
        let reloaded = load_item(&inbox_dir.join(filename)).await.unwrap();
        assert!(reloaded.read);

        // Archive
        archive_item(&inbox_dir, &archive_dir, "20260225_lifecycle_test")
            .await
            .unwrap();
        assert!(!inbox_dir.join(filename).exists(), "source should be gone");
        assert!(archive_dir.join(filename).exists(), "should be in archive");

        // List should be empty now
        let remaining = list_items(&inbox_dir).await.unwrap();
        assert!(remaining.is_empty(), "inbox should be empty after archive");
    }

    // ── count_unread correctness ─────────────────────────────────────────────

    #[tokio::test]
    async fn count_unread_correctness() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path();

        assert_eq!(count_unread(inbox_dir), 0, "empty dir = 0");

        save_item(inbox_dir, "a.json", &make_item("a", false))
            .await
            .unwrap();
        save_item(inbox_dir, "b.json", &make_item("b", false))
            .await
            .unwrap();
        save_item(inbox_dir, "c.json", &make_item("c", true))
            .await
            .unwrap();

        assert_eq!(count_unread(inbox_dir), 2, "2 unread, 1 read");

        mark_read(inbox_dir, "a").await.unwrap();
        assert_eq!(count_unread(inbox_dir), 1, "1 unread after marking a");

        mark_read(inbox_dir, "b").await.unwrap();
        assert_eq!(count_unread(inbox_dir), 0, "0 unread after marking all");
    }

    // ── list_items ignores archive and non-JSON ──────────────────────────────

    #[tokio::test]
    async fn list_items_ignores_archive_and_non_json() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        // Valid inbox item
        save_item(&inbox_dir, "valid.json", &make_item("valid", false))
            .await
            .unwrap();

        // Non-JSON files
        tokio::fs::write(inbox_dir.join("photo.png"), b"fake image")
            .await
            .unwrap();
        tokio::fs::write(inbox_dir.join("notes.txt"), b"some notes")
            .await
            .unwrap();

        // Archive subdirectory with JSON
        let archive = inbox_dir.join("archive");
        tokio::fs::create_dir_all(&archive).await.unwrap();
        save_item(&archive, "archived.json", &make_item("archived", false))
            .await
            .unwrap();

        let items = list_items(&inbox_dir).await.unwrap();
        assert_eq!(items.len(), 1, "should only include top-level .json files");
        assert_eq!(items[0].1.title, "valid");
    }

    // ── Attachments round-trip ───────────────────────────────────────────────

    #[tokio::test]
    async fn attachments_roundtrip_empty() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path();

        save_item(inbox_dir, "empty.json", &make_item("no attach", false))
            .await
            .unwrap();
        let loaded = load_item(&inbox_dir.join("empty.json")).await.unwrap();
        assert!(loaded.attachments.is_empty());
    }

    #[tokio::test]
    async fn attachments_roundtrip_populated() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path();

        let mut with_attach = make_item("with attach", false);
        with_attach.attachments = vec![
            PathBuf::from("inbox/photo.jpg"),
            PathBuf::from("inbox/doc.pdf"),
        ];
        save_item(inbox_dir, "with.json", &with_attach)
            .await
            .unwrap();
        let loaded = load_item(&inbox_dir.join("with.json")).await.unwrap();
        assert_eq!(loaded.attachments.len(), 2);
        assert_eq!(loaded.attachments[0], PathBuf::from("inbox/photo.jpg"));
        assert_eq!(loaded.attachments[1], PathBuf::from("inbox/doc.pdf"));
    }

    // ── Filename generation ──────────────────────────────────────────────────

    #[test]
    fn generate_filename_same_title_same_day() {
        let a = generate_filename("daily report", chrono_tz::UTC);
        let b = generate_filename("daily report", chrono_tz::UTC);
        assert_eq!(a, b, "same title same day should produce same filename");
        assert!(a.ends_with("_daily_report.json"));
    }

    #[test]
    fn generate_filename_sanitization() {
        let name = generate_filename("Hello/World\\Test!!!", chrono_tz::UTC);
        assert!(!name.contains('/'));
        assert!(!name.contains('\\'));
        assert!(!name.contains('!'));
        assert!(
            !name.contains("__"),
            "consecutive underscores should be collapsed"
        );
    }

    // ── Multiple items sorted correctly ──────────────────────────────────────

    #[tokio::test]
    async fn list_items_sorted_newest_first() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path();

        save_item(inbox_dir, "early.json", &make_item_at("early", 8, false))
            .await
            .unwrap();
        save_item(inbox_dir, "late.json", &make_item_at("late", 20, false))
            .await
            .unwrap();
        save_item(inbox_dir, "mid.json", &make_item_at("mid", 14, false))
            .await
            .unwrap();

        let items = list_items(inbox_dir).await.unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].1.title, "late", "newest first");
        assert_eq!(items[1].1.title, "mid");
        assert_eq!(items[2].1.title, "early", "oldest last");
    }

    // ── Archive errors on nonexistent ────────────────────────────────────────

    #[tokio::test]
    async fn archive_nonexistent_errors() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        let archive_dir = dir.path().join("archive/inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let result = archive_item(&inbox_dir, &archive_dir, "ghost").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "error should mention not found: {err}"
        );
    }

    // ── Companion inbox item structure ───────────────────────────────────────

    #[tokio::test]
    async fn companion_inbox_item_structure() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path();

        // Simulate what the Discord handler does
        let companion = inbox::InboxItem {
            title: "Discord attachment: photo.jpg".to_string(),
            body: "From: testuser\nSize: 4096 bytes\nContent-Type: image/jpeg".to_string(),
            source: "discord".to_string(),
            timestamp: NaiveDate::from_ymd_opt(2026, 2, 25)
                .unwrap()
                .and_hms_opt(15, 0, 0)
                .unwrap(),
            read: false,
            attachments: vec![PathBuf::from("inbox/20260225_150000_photo.jpg")],
        };
        save_item(inbox_dir, "companion.json", &companion)
            .await
            .unwrap();

        let loaded = load_item(&inbox_dir.join("companion.json")).await.unwrap();
        assert_eq!(loaded.title, "Discord attachment: photo.jpg");
        assert_eq!(loaded.source, "discord");
        assert!(!loaded.read);
        assert_eq!(loaded.attachments.len(), 1);
        assert!(loaded.body.contains("testuser"));
        assert!(loaded.body.contains("4096 bytes"));
        assert!(loaded.body.contains("image/jpeg"));
    }
}
