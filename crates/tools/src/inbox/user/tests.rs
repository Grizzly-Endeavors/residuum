use super::*;
use crate::inbox::test_fixture::{Item, MailHub, listed_ids, pages};
use residuum_bus::testing::bus_barrier;
use residuum_config::WorkspaceLayout;
use residuum_core::testing::wait;

/// The workspace of an agent named `atlas` under `dir`, and its mail access
/// with no hub directory.
fn atlas(dir: &tempfile::TempDir) -> (WorkspaceLayout, MailAccess) {
    let layout = WorkspaceLayout::new(dir.path().join("atlas"));
    let mail = MailAccess::new(None, &layout, chrono_tz::UTC);
    (layout, mail)
}

#[test]
fn definitions_have_matching_names() {
    let dir = tempfile::tempdir().unwrap();
    let (_, mail) = atlas(&dir);
    let add = UserInboxAddTool::new(mail.clone());
    assert_eq!(add.definition().name, add.name());
    assert_eq!(add.name(), "user_inbox_add");
    let list = UserInboxListTool::new(mail);
    assert_eq!(list.definition().name, list.name());
    assert_eq!(list.name(), "user_inbox_list");
}

#[tokio::test]
async fn user_inbox_add_without_attachments_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (layout, mail) = atlas(&dir);
    let attachments_dir = layout.user_inbox_attachments_dir();

    let result = UserInboxAddTool::new(mail)
        .execute(serde_json::json!({"title": "plain reminder", "body": "just text"}))
        .await
        .unwrap();

    assert!(!result.is_error, "should succeed: {}", result.output);
    assert!(
        result
            .output
            .starts_with("Added item to user inbox with ID: "),
        "confirmation message format should be unchanged: {}",
        result.output
    );
    assert!(
        !result.output.contains("attachment(s) copied"),
        "confirmation message should not report a copy count when none were given: {}",
        result.output
    );
    let id = result
        .output
        .trim_start_matches("Added item to user inbox with ID: ");
    assert!(
        !attachments_dir.join(id).exists(),
        "an item without attachments leaves no attachments directory"
    );
}

#[tokio::test]
async fn user_inbox_add_requires_a_title_without_in_reply_to() {
    let dir = tempfile::tempdir().unwrap();
    let (layout, mail) = atlas(&dir);
    let tool = UserInboxAddTool::new(mail);
    for args in [
        serde_json::json!({"body": "no title"}),
        serde_json::json!({"title": "  ", "body": "blank title"}),
    ] {
        let result = tool.execute(args).await;
        let Err(ToolError::InvalidArguments(message)) = result else {
            panic!("a missing title is refused: {result:?}");
        };
        assert!(message.contains("title is required"), "{message}");
    }
    assert!(!layout.user_inbox_dir().exists(), "nothing was written");
}

#[tokio::test]
async fn user_inbox_add_reply_defaults_the_title_and_threads_the_item() {
    let hub = MailHub::new(&["atlas", "nova"]);
    hub.write(Item::user_mail(
        "nova",
        "ask",
        "Re: Weekly plan",
        "What's next?",
        8,
    ))
    .await;
    let tool = UserInboxAddTool::new(hub.access("atlas"));

    let result = tool
        .execute(serde_json::json!({"body": "Here is the plan.", "in_reply_to": "nova/agent/ask"}))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.output);
    assert!(
        result
            .output
            .contains("In reply to nova/agent/ask in the thread \"Re: Weekly plan\", titled \"Re: Weekly plan\"."),
        "{}",
        result.output
    );

    let items = hub.items("atlas", InboxKind::User).await;
    let [(id, item)] = items.as_slice() else {
        panic!("one item written to atlas's user inbox: {items:?}");
    };
    assert_eq!(item.title, "Re: Weekly plan", "Re: does not stack");
    assert_eq!(item.source, "agent");
    assert_eq!(item.in_reply_to.as_deref(), Some("nova/agent/ask"));

    // The reply is now part of the thread.
    let index = MailIndex::load(hub.access("atlas").view().directory.as_ref()).await;
    let reply_ref = inbox::InboxRef::new("atlas", InboxKind::User, id).unwrap();
    let thread = index.thread_containing(&reply_ref).unwrap();
    assert_eq!(thread.message_count(), 2);
    assert_eq!(thread.root().item_ref.to_string(), "nova/agent/ask");
}

#[tokio::test]
async fn user_inbox_add_reply_keeps_a_given_title() {
    let hub = MailHub::new(&["atlas"]);
    hub.write(Item::agent_message("atlas", "first", "Plan", "v1", 8))
        .await;
    UserInboxAddTool::new(hub.access("atlas"))
        .execute(serde_json::json!({"title": "Plan v2", "body": "v2", "in_reply_to": "first"}))
        .await
        .unwrap();
    let items = hub.items("atlas", InboxKind::User).await;
    let reply = items.iter().find(|(id, _)| id != "first").unwrap();
    assert_eq!(reply.1.title, "Plan v2");
    assert_eq!(
        reply.1.in_reply_to.as_deref(),
        Some("atlas/user/first"),
        "a bare id names an item of this agent's user inbox"
    );
}

#[tokio::test]
async fn user_inbox_add_refuses_a_bad_or_unknown_ref() {
    let hub = MailHub::new(&["atlas", "nova"]);
    let tool = UserInboxAddTool::new(hub.access("atlas"));

    let malformed = tool
        .execute(serde_json::json!({"body": "b", "in_reply_to": "nova/letters/x"}))
        .await;
    let Err(ToolError::InvalidArguments(message)) = malformed else {
        panic!("a malformed ref is refused: {malformed:?}");
    };
    assert!(message.contains("not a valid inbox ref"), "{message}");

    let unknown = tool
        .execute(serde_json::json!({"title": "t", "body": "b", "in_reply_to": "nova/user/missing"}))
        .await;
    let Err(ToolError::InvalidArguments(unknown_message)) = unknown else {
        panic!("an unknown ref is refused: {unknown:?}");
    };
    assert!(
        unknown_message.contains("no mail message nova/user/missing was found"),
        "{unknown_message}"
    );
    assert!(
        !unknown_message.contains(OWN_INBOX_ONLY_NOTE),
        "{unknown_message}"
    );
    assert!(
        hub.items("atlas", InboxKind::User).await.is_empty(),
        "nothing was written"
    );
}

#[tokio::test]
async fn user_inbox_add_without_a_directory_replies_only_within_this_agent() {
    let hub = MailHub::new(&["atlas", "nova"]);
    hub.write(Item::agent_message("nova", "theirs", "Theirs", "b", 8))
        .await;
    hub.write(Item::agent_message("atlas", "mine", "Mine", "b", 8))
        .await;
    let tool = UserInboxAddTool::new(hub.access_without_directory("atlas"));

    let theirs = tool
        .execute(serde_json::json!({"body": "b", "in_reply_to": "nova/user/theirs"}))
        .await;
    let Err(ToolError::InvalidArguments(message)) = theirs else {
        panic!("another agent's message is out of reach: {theirs:?}");
    };
    assert!(message.contains(OWN_INBOX_ONLY_NOTE), "{message}");

    let mine = tool
        .execute(serde_json::json!({"body": "b", "in_reply_to": "atlas/user/mine"}))
        .await
        .unwrap();
    assert!(
        mine.output.contains("titled \"Re: Mine\""),
        "{}",
        mine.output
    );
}

#[tokio::test]
async fn user_inbox_add_announces_the_saved_item_once() {
    let dir = tempfile::tempdir().unwrap();
    let (layout, mail) = atlas(&dir);
    let bus = residuum_bus::spawn_broker();
    let mut added = bus
        .subscribe::<_, UserInboxAddedEvent>(topics::UserInbox)
        .await
        .unwrap();

    let tool = UserInboxAddTool::new(mail).with_publisher(bus.publisher());
    let result = tool
        .execute(serde_json::json!({"title": "heads up", "body": "look"}))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.output);

    let announced = wait::guarded("the item is announced", added.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.output.ends_with(&announced.item_id),
        "the announced id {:?} is the one the tool reported: {}",
        announced.item_id,
        result.output
    );
    assert!(
        layout
            .user_inbox_dir()
            .join(format!("{}.json", announced.item_id))
            .is_file(),
        "the announced item is already saved"
    );
    bus_barrier(&bus).await;
    assert!(added.drain().is_empty(), "one item, one announcement");
}

#[tokio::test]
async fn user_inbox_add_that_fails_announces_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (layout, mail) = atlas(&dir);
    // A regular file where the inbox directory should be, so saving fails.
    tokio::fs::create_dir_all(layout.root()).await.unwrap();
    tokio::fs::write(layout.root().join("inbox"), b"not a directory")
        .await
        .unwrap();
    let bus = residuum_bus::spawn_broker();
    let mut added = bus
        .subscribe::<_, UserInboxAddedEvent>(topics::UserInbox)
        .await
        .unwrap();

    let tool = UserInboxAddTool::new(mail).with_publisher(bus.publisher());
    let result = tool
        .execute(serde_json::json!({"title": "lost", "body": "never saved"}))
        .await;
    assert!(result.is_err(), "saving into a file path fails");
    bus_barrier(&bus).await;
    assert!(
        added.drain().is_empty(),
        "an item that wasn't saved is not announced"
    );
}

#[tokio::test]
async fn user_inbox_add_with_attachments_copies_file() {
    let dir = tempfile::tempdir().unwrap();
    let (layout, mail) = atlas(&dir);
    let source_file = dir.path().join("export.csv");
    tokio::fs::write(&source_file, b"a,b,c").await.unwrap();

    let result = UserInboxAddTool::new(mail)
        .execute(serde_json::json!({
            "title": "weekly export",
            "body": "attached",
            "attachments": [source_file.to_string_lossy()],
        }))
        .await
        .unwrap();

    assert!(!result.is_error, "should succeed: {}", result.output);
    assert!(
        result.output.contains("1 attachment(s) copied"),
        "confirmation should report the attachment count: {}",
        result.output
    );
    let items = inbox::list_items(&layout.user_inbox_dir()).await.unwrap();
    let [(_, item)] = items.as_slice() else {
        panic!("one item saved: {items:?}");
    };
    assert_eq!(item.title, "weekly export");
    assert_eq!(item.attachments.len(), 1);
}

#[tokio::test]
async fn user_inbox_add_missing_attachment_source_still_saves_the_item() {
    let dir = tempfile::tempdir().unwrap();
    let (layout, mail) = atlas(&dir);
    let result = UserInboxAddTool::new(mail)
        .execute(serde_json::json!({
            "title": "broken",
            "body": "still saved",
            "attachments": [dir.path().join("does_not_exist.bin").to_string_lossy()],
        }))
        .await
        .unwrap();

    // A file that can't be attached is reported, but never fails the
    // whole item — the item itself is still saved.
    assert!(!result.is_error, "{}", result.output);
    assert!(
        result.output.contains("could not be copied"),
        "the failure should be reported: {}",
        result.output
    );
    let items = inbox::list_items(&layout.user_inbox_dir()).await.unwrap();
    assert_eq!(
        items.len(),
        1,
        "the item should be saved even though its attachment failed: {items:?}"
    );
}

/// Every agent's mail with one thread across two agents and an archived item.
async fn team_mail() -> MailHub {
    let hub = MailHub::new(&["atlas", "nova"]);
    hub.write(Item::agent_message(
        "atlas",
        "digest",
        "Morning digest",
        "b",
        7,
    ))
    .await;
    hub.write(Item::user_mail("nova", "ask", "Status?", "b", 8))
        .await;
    hub.write(
        Item::agent_message("nova", "answer", "Re: Status?", "b", 9).replying_to("nova/agent/ask"),
    )
    .await;
    hub.write(Item::agent_message("nova", "stale", "Old idea", "b", 6).archived())
        .await;
    hub
}

#[tokio::test]
async fn user_inbox_list_shows_every_agents_items_newest_first() {
    let hub = team_mail().await;
    let result = UserInboxListTool::new(hub.access("atlas"))
        .execute(serde_json::json!({}))
        .await
        .unwrap();
    let out = &result.output;
    assert!(!result.is_error, "{out}");
    let expected = "2 user inbox item(s), newest first:\n  \
         [unread] nova/user/answer — Re: Status? (nova, 2026-02-25T09:00), in reply to nova/agent/ask\n  \
         [unread] atlas/user/digest — Morning digest (atlas, 2026-02-25T07:00)";
    assert_eq!(out, expected);
}

#[tokio::test]
async fn user_inbox_list_filters_by_agent_and_includes_archived_on_request() {
    let hub = team_mail().await;
    let tool = UserInboxListTool::new(hub.access("atlas"));

    let out = tool
        .execute(serde_json::json!({"agent": "nova", "include_archived": true}))
        .await
        .unwrap()
        .output;
    assert!(out.starts_with("2 user inbox item(s)"), "{out}");
    assert!(out.contains("nova/user/answer"), "{out}");
    assert!(
        out.contains("[read, archived] nova/user/stale — Old idea (nova, 2026-02-25T06:00)"),
        "{out}"
    );
    assert!(!out.contains("atlas/"), "{out}");

    let result = tool
        .execute(serde_json::json!({"agent": "zeta"}))
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(
        result.output.contains("no agent named 'zeta'"),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn user_inbox_list_without_a_directory_lists_this_agent_and_says_so() {
    let hub = team_mail().await;
    let out = UserInboxListTool::new(hub.access_without_directory("atlas"))
        .execute(serde_json::json!({}))
        .await
        .unwrap()
        .output;
    assert!(out.starts_with("1 user inbox item(s)"), "{out}");
    assert!(out.contains("atlas/user/digest"), "{out}");
    assert!(out.ends_with(OWN_INBOX_ONLY_NOTE), "{out}");
}

#[tokio::test]
async fn user_inbox_list_pages_visit_every_item_once_even_when_times_tie() {
    let hub = MailHub::new(&["atlas"]);
    for (id, hour) in [
        ("item-0", 8),
        ("item-1", 8),
        ("item-2", 8),
        ("item-3", 9),
        ("item-4", 9),
        ("item-5", 10),
        ("item-6", 10),
    ] {
        hub.write(Item::agent_message("atlas", id, "t", "b", hour))
            .await;
    }
    let tool = UserInboxListTool::new(hub.access("atlas"));

    let outputs = pages(&tool, serde_json::json!({"limit": 3})).await;

    let ids: Vec<Vec<String>> = outputs.iter().map(|out| listed_ids(out)).collect();
    assert_eq!(
        ids,
        [
            vec![
                "atlas/user/item-5",
                "atlas/user/item-6",
                "atlas/user/item-3"
            ],
            vec![
                "atlas/user/item-4",
                "atlas/user/item-0",
                "atlas/user/item-1"
            ],
            vec!["atlas/user/item-2"],
        ],
        "newest first, a tie by ref, each item on exactly one page"
    );
    let [first, second, last] = outputs.as_slice() else {
        panic!("three pages: {outputs:?}");
    };
    assert!(
        first.starts_with("7 user inbox item(s), newest first:"),
        "{first}"
    );
    assert!(
        first.contains("Showing 3 of 7. For the next page, call again with before=\""),
        "{first}"
    );
    assert!(
        first.contains("the same query, agent and include_archived."),
        "{first}"
    );
    assert!(
        second.starts_with("4 user inbox item(s) after the cursor, newest first:"),
        "{second}"
    );
    assert!(second.contains("Showing 3 of 4."), "{second}");
    assert!(
        last.starts_with("1 user inbox item(s) after the cursor, newest first:"),
        "{last}"
    );
    assert!(
        !last.contains("Showing"),
        "the last page has no next: {last}"
    );
}

#[tokio::test]
async fn user_inbox_list_lists_fifty_by_default_and_clamps_the_limit_to_two_hundred() {
    let hub = MailHub::new(&["atlas"]);
    let total = MAX_LIMIT + 5;
    for n in 0..total {
        hub.write(Item::agent_message(
            "atlas",
            &format!("item-{n:03}"),
            "t",
            "b",
            8,
        ))
        .await;
    }
    let tool = UserInboxListTool::new(hub.access("atlas"));

    for (arguments, listed) in [
        (serde_json::json!({}), DEFAULT_LIMIT),
        (serde_json::json!({"limit": 3}), 3),
        (serde_json::json!({"limit": "2"}), 2),
        (serde_json::json!({"limit": MAX_LIMIT}), MAX_LIMIT),
        (serde_json::json!({"limit": 100_000}), MAX_LIMIT),
    ] {
        let out = tool.execute(arguments.clone()).await.unwrap().output;
        assert_eq!(listed_ids(&out).len(), listed, "{arguments}: {out}");
        assert!(
            out.contains(&format!("Showing {listed} of {total}.")),
            "{arguments}: {out}"
        );
    }
}

#[tokio::test]
async fn user_inbox_list_refuses_a_zero_limit_a_bad_limit_and_a_foreign_cursor() {
    let hub = team_mail().await;
    let tool = UserInboxListTool::new(hub.access("atlas"));
    for (arguments, expected) in [
        (serde_json::json!({"limit": 0}), "limit must be at least 1"),
        (
            serde_json::json!({"limit": -1}),
            "limit must be a whole number",
        ),
        (
            serde_json::json!({"limit": "many"}),
            "limit must be a whole number",
        ),
        (
            serde_json::json!({"before": "nonsense"}),
            "the before cursor isn't one this list returned",
        ),
        (
            serde_json::json!({"before": "soon:atlas/user/digest"}),
            "the before cursor isn't one this list returned",
        ),
    ] {
        let result = tool.execute(arguments.clone()).await;
        let Err(ToolError::InvalidArguments(message)) = result else {
            panic!("{arguments} is refused: {result:?}");
        };
        assert!(message.contains(expected), "{arguments}: {message}");
    }
}

#[tokio::test]
async fn user_inbox_list_query_matches_title_body_and_attachment_filenames_ignoring_case() {
    let hub = MailHub::new(&["atlas"]);
    hub.write(Item::agent_message(
        "atlas",
        "digest",
        "Weekly Digest",
        "plain text",
        8,
    ))
    .await;
    hub.write(Item::agent_message(
        "atlas",
        "figures",
        "Numbers",
        "Contains the Budget figures",
        9,
    ))
    .await;
    hub.write(
        Item::agent_message("atlas", "plan", "Plan", "see attached", 10)
            .with_attachment("Roadmap.PDF"),
    )
    .await;
    hub.write(Item::agent_message(
        "atlas",
        "other",
        "Unrelated",
        "nothing",
        11,
    ))
    .await;
    let tool = UserInboxListTool::new(hub.access("atlas"));

    for (query, expected) in [
        ("DIGEST", "atlas/user/digest"),
        ("budget figures", "atlas/user/figures"),
        ("roadmap.pdf", "atlas/user/plan"),
        ("pdf", "atlas/user/plan"),
    ] {
        let out = tool
            .execute(serde_json::json!({"query": query}))
            .await
            .unwrap()
            .output;
        assert_eq!(listed_ids(&out), [expected], "{query}: {out}");
        assert!(
            out.starts_with("1 user inbox item(s) match, newest first:"),
            "{query}: {out}"
        );
    }

    let none = tool
        .execute(serde_json::json!({"query": "zebra"}))
        .await
        .unwrap();
    assert!(!none.is_error, "{}", none.output);
    assert_eq!(none.output, "No user inbox items match \"zebra\".");

    let blank = tool
        .execute(serde_json::json!({"query": "  "}))
        .await
        .unwrap()
        .output;
    assert_eq!(listed_ids(&blank).len(), 4, "a blank query is ignored");
    assert!(
        blank.starts_with("4 user inbox item(s), newest first:"),
        "{blank}"
    );
}

#[tokio::test]
async fn user_inbox_list_query_combines_with_paging_and_the_agent_filter() {
    let hub = MailHub::new(&["atlas", "nova"]);
    for n in 0..5 {
        hub.write(Item::agent_message(
            "nova",
            &format!("alpha-{n}"),
            &format!("Alpha report {n}"),
            "b",
            8,
        ))
        .await;
    }
    for n in 0..2 {
        hub.write(Item::agent_message(
            "nova",
            &format!("beta-{n}"),
            "Beta report",
            "b",
            8,
        ))
        .await;
    }
    for n in 0..3 {
        hub.write(Item::agent_message(
            "atlas",
            &format!("alpha-{n}"),
            "Alpha notes",
            "b",
            8,
        ))
        .await;
    }
    let tool = UserInboxListTool::new(hub.access("atlas"));

    let nova = pages(
        &tool,
        serde_json::json!({"query": "ALPHA", "agent": "nova", "limit": 2}),
    )
    .await;
    let ids: Vec<Vec<String>> = nova.iter().map(|out| listed_ids(out)).collect();
    assert_eq!(
        ids,
        [
            vec!["nova/user/alpha-0", "nova/user/alpha-1"],
            vec!["nova/user/alpha-2", "nova/user/alpha-3"],
            vec!["nova/user/alpha-4"],
        ]
    );
    let [first, second, _] = nova.as_slice() else {
        panic!("three pages: {nova:?}");
    };
    assert!(
        first.starts_with("5 user inbox item(s) match, newest first:"),
        "{first}"
    );
    assert!(
        second.starts_with("3 user inbox item(s) match after the cursor, newest first:"),
        "{second}"
    );

    let everyone = pages(&tool, serde_json::json!({"query": "alpha", "limit": 4})).await;
    let all: Vec<String> = everyone.iter().flat_map(|out| listed_ids(out)).collect();
    assert_eq!(
        all,
        [
            "atlas/user/alpha-0",
            "atlas/user/alpha-1",
            "atlas/user/alpha-2",
            "nova/user/alpha-0",
            "nova/user/alpha-1",
            "nova/user/alpha-2",
            "nova/user/alpha-3",
            "nova/user/alpha-4",
        ],
        "every match once, across two pages"
    );
    assert_eq!(everyone.len(), 2);
}
