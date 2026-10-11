//! Tests for `/api/hub/inbox...`, against the fake directory's agents in
//! every state.

use chrono::{NaiveDate, NaiveDateTime};

use super::*;

fn at(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 9, day)
        .unwrap()
        .and_hms_opt(hour, minute, 0)
        .unwrap()
}

/// Save an active item `id` of `agent` with no attachments.
async fn add_item(h: &Harness, agent: &str, id: &str, when: NaiveDateTime, read: bool) {
    let inbox = WorkspaceLayout::new(h.agent_dir(agent)).user_inbox_dir();
    tokio::fs::create_dir_all(&inbox).await.unwrap();
    let item = crate::inbox::InboxItem {
        title: format!("title of {id}"),
        body: format!("body of {id}"),
        source: "test".to_string(),
        timestamp: when,
        read,
        attachments: Vec::new(),
    };
    crate::inbox::save_item(&inbox, &format!("{id}.json"), &item)
        .await
        .unwrap();
}

/// Save an archived item `id` of `agent`.
async fn add_archived_item(h: &Harness, agent: &str, id: &str, when: NaiveDateTime) {
    let archive = WorkspaceLayout::new(h.agent_dir(agent)).user_inbox_archive_dir();
    tokio::fs::create_dir_all(&archive).await.unwrap();
    let item = crate::inbox::InboxItem {
        title: format!("title of {id}"),
        body: format!("body of {id}"),
        source: "test".to_string(),
        timestamp: when,
        read: true,
        attachments: Vec::new(),
    };
    crate::inbox::save_item(&archive, &format!("{id}.json"), &item)
        .await
        .unwrap();
}

/// `agent/id` of every item in a listing, in order.
fn listed(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            format!(
                "{}/{}",
                item["agent"].as_str().unwrap(),
                item["id"].as_str().unwrap()
            )
        })
        .collect()
}

/// The `error` of a JSON error body.
fn error_of(body: &Value) -> &str {
    body["error"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON error, got {body}"))
}

#[tokio::test]
async fn the_hub_inbox_merges_every_agents_items_newest_first_in_every_state() {
    let h = Harness::new();
    h.add_failed_agent();
    add_item(&h, "scout", "20260930_alpha", at(30, 8, 0), false).await;
    seed_inbox_item(&h.agent_dir("quiet"), "20260930_beta", "Beta", b"hello").await;
    add_item(&h, "broken", "20260929_gamma", at(29, 23, 59), true).await;

    let page = h.get_expect("/api/hub/inbox", StatusCode::OK).await;

    assert_eq!(
        listed(&page),
        [
            "quiet/20260930_beta",
            "scout/20260930_alpha",
            "broken/20260929_gamma"
        ],
        "newest first, across a stopped, a running and a failed agent"
    );
    assert_eq!(page["next_cursor"], Value::Null);
    assert_eq!(
        page["items"][0],
        json!({
            "agent": "quiet",
            "id": "20260930_beta",
            "title": "Beta",
            "body": "body of Beta",
            "source": "test",
            "at": "2026-09-30T08:15:00Z",
            "read": false,
            "attachments": [{
                "filename": "note.txt",
                "mime_type": "text/plain",
                "size": 5,
                "url": "/api/agents/quiet/inbox/20260930_beta/attachments/0",
            }],
        })
    );

    // The attachment link is the per-agent route, which serves a stopped agent.
    let (_headers, bytes) = h
        .request_ok(
            Method::GET,
            "/api/agents/quiet/inbox/20260930_beta/attachments/0",
            "",
        )
        .await;
    assert_eq!(bytes, b"hello");
}

#[tokio::test]
async fn an_agent_with_no_inbox_directory_contributes_nothing() {
    let h = Harness::new();
    add_item(&h, "scout", "only", at(30, 9, 0), false).await;

    let page = h.get_expect("/api/hub/inbox", StatusCode::OK).await;

    assert_eq!(
        listed(&page),
        ["scout/only"],
        "quiet has never had an inbox"
    );
}

#[tokio::test]
async fn the_agent_and_status_filters_narrow_the_listing() {
    let h = Harness::new();
    add_item(&h, "scout", "active_scout", at(30, 9, 0), false).await;
    add_item(&h, "quiet", "active_quiet", at(30, 10, 0), false).await;
    add_archived_item(&h, "scout", "old_scout", at(29, 9, 0)).await;
    add_archived_item(&h, "quiet", "old_quiet", at(29, 10, 0)).await;

    let by_agent = h
        .get_expect("/api/hub/inbox?agent=quiet", StatusCode::OK)
        .await;
    assert_eq!(listed(&by_agent), ["quiet/active_quiet"]);

    let archived = h
        .get_expect("/api/hub/inbox?status=archived", StatusCode::OK)
        .await;
    assert_eq!(listed(&archived), ["quiet/old_quiet", "scout/old_scout"]);

    let explicit_active = h
        .get_expect("/api/hub/inbox?status=active&agent=scout", StatusCode::OK)
        .await;
    assert_eq!(listed(&explicit_active), ["scout/active_scout"]);
}

#[tokio::test]
async fn a_listing_that_cannot_be_read_is_a_json_error() {
    let h = Harness::new();

    let unknown = h
        .get_expect("/api/hub/inbox?agent=ghost", StatusCode::NOT_FOUND)
        .await;
    assert_eq!(error_of(&unknown), "no agent named 'ghost'");

    for (query, mentions) in [
        ("status=bogus", "status"),
        ("limit=0", "limit"),
        ("limit=many", "limit"),
        ("limit=-1", "limit"),
        ("before=garbage", "before"),
        ("before=x:scout:id", "before"),
        ("before=5::id", "before"),
    ] {
        let body = h
            .get_expect(&format!("/api/hub/inbox?{query}"), StatusCode::BAD_REQUEST)
            .await;
        assert!(
            error_of(&body).contains(mentions),
            "{query}: the error names the parameter: {body}"
        );
    }
}

#[tokio::test]
async fn paging_visits_every_item_once_in_order_even_when_times_and_ids_tie() {
    let h = Harness::new();
    // `same` has one id in two agents at one instant, so only the agent can order them.
    for (agent, id, when) in [
        ("scout", "a", at(30, 9, 0)),
        ("scout", "b", at(30, 9, 1)),
        ("scout", "same", at(30, 9, 2)),
        ("quiet", "same", at(30, 9, 2)),
        ("quiet", "c", at(30, 9, 2)),
        ("quiet", "d", at(30, 9, 3)),
        ("scout", "e", at(30, 9, 4)),
    ] {
        add_item(&h, agent, id, when, false).await;
    }

    let everything = listed(
        &h.get_expect("/api/hub/inbox?limit=200", StatusCode::OK)
            .await,
    );
    assert_eq!(
        everything,
        [
            "scout/e",
            "quiet/d",
            "scout/same",
            "quiet/same",
            "quiet/c",
            "scout/b",
            "scout/a"
        ],
        "time first, then id, then agent, newest first"
    );

    let mut seen = Vec::new();
    let mut uri = "/api/hub/inbox?limit=3".to_string();
    let mut page_count = 0;
    loop {
        let page = h.get_expect(&uri, StatusCode::OK).await;
        seen.extend(listed(&page));
        page_count += 1;
        match page["next_cursor"].as_str() {
            Some(cursor) => uri = format!("/api/hub/inbox?limit=3&before={cursor}"),
            None => break,
        }
    }
    assert_eq!(page_count, 3, "seven items at three a page");
    assert_eq!(seen, everything, "every item once, in the same order");
}

#[tokio::test]
async fn a_cursor_keeps_working_after_its_item_is_archived() {
    let h = Harness::new();
    for (id, minute) in [("a", 0), ("b", 1), ("c", 2)] {
        add_item(&h, "scout", id, at(30, 9, minute), false).await;
    }
    let first = h.get_expect("/api/hub/inbox?limit=1", StatusCode::OK).await;
    assert_eq!(listed(&first), ["scout/c"]);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();

    h.post_expect("/api/hub/inbox/scout/c/archive", StatusCode::OK)
        .await;
    let second = h
        .get_expect(&format!("/api/hub/inbox?before={cursor}"), StatusCode::OK)
        .await;

    assert_eq!(
        listed(&second),
        ["scout/b", "scout/a"],
        "the page after a removed item still starts where it should"
    );
}

#[tokio::test]
async fn the_default_page_is_fifty_and_a_larger_limit_is_capped_at_two_hundred() {
    let h = Harness::new();
    for i in 0..201_u32 {
        add_item(
            &h,
            "scout",
            &format!("item_{i:03}"),
            at(1 + i / 60 % 28, i / 60 % 24, i % 60),
            false,
        )
        .await;
    }

    let default = h.get_expect("/api/hub/inbox", StatusCode::OK).await;
    assert_eq!(default["items"].as_array().unwrap().len(), 50);
    assert!(default["next_cursor"].is_string());

    let capped = h
        .get_expect("/api/hub/inbox?limit=1000", StatusCode::OK)
        .await;
    assert_eq!(capped["items"].as_array().unwrap().len(), 200);
    let cursor = capped["next_cursor"].as_str().unwrap();

    let rest = h
        .get_expect(
            &format!("/api/hub/inbox?limit=1000&before={cursor}"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(rest["items"].as_array().unwrap().len(), 1);
    assert_eq!(rest["next_cursor"], Value::Null);
}

#[tokio::test]
async fn unread_counts_every_agent_including_those_with_none() {
    let h = Harness::new();
    h.add_failed_agent();
    add_item(&h, "scout", "one", at(30, 9, 0), false).await;
    add_item(&h, "scout", "two", at(30, 9, 1), false).await;
    add_item(&h, "scout", "seen", at(30, 9, 2), true).await;
    add_item(&h, "quiet", "three", at(30, 9, 3), false).await;
    add_archived_item(&h, "quiet", "archived", at(29, 9, 0)).await;

    let unread = h.get_expect("/api/hub/inbox/unread", StatusCode::OK).await;

    assert_eq!(
        unread,
        json!({ "total": 3, "by_agent": { "broken": 0, "quiet": 1, "scout": 2 } }),
        "archived and read items don't count, and a stopped or failed agent does"
    );

    h.expect(
        Method::PUT,
        "/api/hub/inbox/scout/one/read",
        None,
        StatusCode::OK,
    )
    .await;
    let after = h.get_expect("/api/hub/inbox/unread", StatusCode::OK).await;
    assert_eq!(after["total"], 2);
    assert_eq!(after["by_agent"]["scout"], 1);
}

#[tokio::test]
async fn read_archive_and_restore_round_trip_in_every_agent_state() {
    let h = Harness::new();
    h.add_failed_agent();

    for name in EVERY_STATE {
        seed_inbox_item(&h.agent_dir(name), "item1", "First", b"hello").await;
        let item = format!("/api/hub/inbox/{name}/item1");
        let layout = WorkspaceLayout::new(h.agent_dir(name));
        let attachment = json!([{
            "filename": "note.txt",
            "mime_type": "text/plain",
            "size": 5,
            "url": format!("/api/agents/{name}/inbox/item1/attachments/0"),
        }]);

        let read = h
            .expect(Method::PUT, &format!("{item}/read"), None, StatusCode::OK)
            .await;
        assert_eq!(read["item"]["agent"], name);
        assert_eq!(read["item"]["id"], "item1");
        assert_eq!(read["item"]["read"], true, "{name}");
        assert_eq!(read["item"]["attachments"], attachment, "{name}");
        assert!(
            crate::inbox::load_item(&layout.user_inbox_dir().join("item1.json"))
                .await
                .unwrap()
                .read,
            "{name}: the read flag is saved"
        );

        let archived = h
            .post_expect(&format!("{item}/archive"), StatusCode::OK)
            .await;
        assert_eq!(archived["item"]["read"], true, "{name}");
        assert_eq!(
            archived["item"]["attachments"], attachment,
            "{name}: the attachment is still served after archiving"
        );
        assert!(
            layout
                .user_inbox_archive_attachments_dir()
                .join("item1/note.txt")
                .exists(),
            "{name}: the attachment moved with the item"
        );
        let active = h
            .get_expect(&format!("/api/hub/inbox?agent={name}"), StatusCode::OK)
            .await;
        assert!(listed(&active).is_empty(), "{name}: no longer active");
        let in_archive = h
            .get_expect(
                &format!("/api/hub/inbox?agent={name}&status=archived"),
                StatusCode::OK,
            )
            .await;
        assert_eq!(listed(&in_archive), [format!("{name}/item1")], "{name}");
        let (_headers, bytes) = h
            .request_ok(
                Method::GET,
                &format!("/api/agents/{name}/inbox/item1/attachments/0"),
                "",
            )
            .await;
        assert_eq!(bytes, b"hello", "{name}: the archived attachment downloads");

        let restored = h
            .post_expect(&format!("{item}/restore"), StatusCode::OK)
            .await;
        assert_eq!(restored["item"]["id"], "item1", "{name}");
        assert_eq!(restored["item"]["attachments"], attachment, "{name}");
        let active_again = h
            .get_expect(&format!("/api/hub/inbox?agent={name}"), StatusCode::OK)
            .await;
        assert_eq!(listed(&active_again), [format!("{name}/item1")], "{name}");
        assert!(
            layout
                .user_inbox_attachments_dir()
                .join("item1/note.txt")
                .exists(),
            "{name}: the attachment came back with the item"
        );
    }
}

#[tokio::test]
async fn marking_an_archived_item_read_changes_it_in_the_archive() {
    let h = Harness::new();
    let archive = WorkspaceLayout::new(h.agent_dir("scout")).user_inbox_archive_dir();
    tokio::fs::create_dir_all(&archive).await.unwrap();
    let item = crate::inbox::InboxItem {
        title: "Old".to_string(),
        body: "old body".to_string(),
        source: "test".to_string(),
        timestamp: at(1, 8, 0),
        read: false,
        attachments: Vec::new(),
    };
    crate::inbox::save_item(&archive, "old.json", &item)
        .await
        .unwrap();

    let read = h
        .expect(
            Method::PUT,
            "/api/hub/inbox/scout/old/read",
            None,
            StatusCode::OK,
        )
        .await;

    assert_eq!(read["item"]["read"], true);
    assert!(
        crate::inbox::load_item(&archive.join("old.json"))
            .await
            .unwrap()
            .read,
        "the archived item is the one that was changed"
    );
}

#[tokio::test]
async fn item_calls_with_an_unknown_agent_or_item_are_json_404s() {
    let h = Harness::new();
    add_item(&h, "scout", "here", at(30, 9, 0), false).await;
    add_archived_item(&h, "scout", "gone", at(29, 9, 0)).await;

    for (method, uri, message) in [
        (
            Method::PUT,
            "/api/hub/inbox/ghost/here/read",
            "no agent named 'ghost'",
        ),
        (
            Method::POST,
            "/api/hub/inbox/ghost/here/archive",
            "no agent named 'ghost'",
        ),
        (
            Method::POST,
            "/api/hub/inbox/ghost/here/restore",
            "no agent named 'ghost'",
        ),
        (
            Method::PUT,
            "/api/hub/inbox/scout/nothing/read",
            "scout has no inbox item 'nothing'",
        ),
        (
            Method::POST,
            "/api/hub/inbox/scout/nothing/archive",
            "scout has no active inbox item 'nothing'",
        ),
        (
            Method::POST,
            "/api/hub/inbox/scout/nothing/restore",
            "scout has no archived inbox item 'nothing'",
        ),
        (
            Method::POST,
            "/api/hub/inbox/scout/gone/archive",
            "scout has no active inbox item 'gone'",
        ),
        (
            Method::POST,
            "/api/hub/inbox/scout/here/restore",
            "scout has no archived inbox item 'here'",
        ),
        (
            Method::PUT,
            "/api/hub/inbox/quiet/here/read",
            "quiet has no inbox item 'here'",
        ),
    ] {
        let body = h
            .expect(method.clone(), uri, None, StatusCode::NOT_FOUND)
            .await;
        assert_eq!(error_of(&body), message, "{method} {uri}");
    }
}

#[tokio::test]
async fn an_id_that_is_not_a_bare_item_id_is_a_json_400() {
    let h = Harness::new();

    for id in ["..%2Fsecret", "a%2Fb", "%2E%2E", "a%5Cb"] {
        let body = h
            .expect(
                Method::PUT,
                &format!("/api/hub/inbox/scout/{id}/read"),
                None,
                StatusCode::BAD_REQUEST,
            )
            .await;
        assert!(
            error_of(&body).contains("isn't an inbox item id"),
            "{id}: {body}"
        );
    }
}

#[tokio::test]
async fn a_move_onto_a_different_item_with_the_same_id_is_a_409_that_loses_nothing() {
    let h = Harness::new();
    add_item(&h, "scout", "dup", at(30, 9, 0), false).await;
    add_archived_item(&h, "scout", "dup", at(29, 9, 0)).await;

    let archive = h
        .expect(
            Method::POST,
            "/api/hub/inbox/scout/dup/archive",
            None,
            StatusCode::CONFLICT,
        )
        .await;
    let restore = h
        .expect(
            Method::POST,
            "/api/hub/inbox/scout/dup/restore",
            None,
            StatusCode::CONFLICT,
        )
        .await;

    assert!(error_of(&archive).contains("already holds a different item"));
    assert!(error_of(&restore).contains("already holds a different item"));
    let layout = WorkspaceLayout::new(h.agent_dir("scout"));
    assert_eq!(
        crate::inbox::load_item(&layout.user_inbox_dir().join("dup.json"))
            .await
            .unwrap()
            .timestamp,
        at(30, 9, 0),
        "the active item is untouched"
    );
    assert_eq!(
        crate::inbox::load_item(&layout.user_inbox_archive_dir().join("dup.json"))
            .await
            .unwrap()
            .timestamp,
        at(29, 9, 0),
        "the archived item is untouched"
    );
}

#[tokio::test]
async fn an_unreadable_inbox_fails_the_listing_and_names_the_agent() {
    let h = Harness::new();
    // A file where the inbox directory should be can't be listed.
    let inbox = WorkspaceLayout::new(h.agent_dir("scout")).user_inbox_dir();
    tokio::fs::create_dir_all(inbox.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&inbox, b"not a directory").await.unwrap();

    let body = h
        .get_expect("/api/hub/inbox", StatusCode::INTERNAL_SERVER_ERROR)
        .await;

    assert!(
        error_of(&body).contains("couldn't read scout's inbox"),
        "{body}"
    );
}

#[tokio::test]
async fn item_times_follow_the_hubs_timezone_across_dst_edges() {
    let h = Harness::new();
    *h.directory.timezone.lock().unwrap() = "America/New_York".parse().unwrap();
    let new_york = |month, day, hour, minute| {
        NaiveDate::from_ymd_opt(2026, month, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    };
    add_item(&h, "scout", "winter", new_york(1, 15, 10, 0), false).await;
    add_item(&h, "scout", "repeated", new_york(11, 1, 1, 30), false).await;
    add_item(&h, "scout", "skipped", new_york(3, 8, 2, 30), false).await;

    let page = h.get_expect("/api/hub/inbox", StatusCode::OK).await;

    let times: Vec<(&str, &str)> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["id"].as_str().unwrap(), item["at"].as_str().unwrap()))
        .collect();
    assert_eq!(
        times,
        [
            ("repeated", "2026-11-01T01:30:00-04:00"),
            ("skipped", "2026-03-08T03:30:00-04:00"),
            ("winter", "2026-01-15T10:00:00-05:00"),
        ],
        "an ambiguous time takes the earlier offset, a nonexistent one moves forward by the gap"
    );
}

#[tokio::test]
async fn two_items_with_one_title_on_one_day_both_survive_with_their_attachments() {
    let h = Harness::new();
    let layout = WorkspaceLayout::new(h.agent_dir("quiet"));
    let sources = tempfile::tempdir().unwrap();
    let mut ids = Vec::new();
    for (body, content) in [("first", b"one".as_slice()), ("second", b"two".as_slice())] {
        let source = sources.path().join(format!("{body}.txt"));
        tokio::fs::write(&source, content).await.unwrap();
        tokio::fs::create_dir_all(layout.user_inbox_dir())
            .await
            .unwrap();
        let (filename, failures) = crate::inbox::quick_add_with_attachments(
            &layout.user_inbox_dir(),
            &layout.user_inbox_attachments_dir(),
            "Daily report",
            body,
            "agent",
            chrono_tz::UTC,
            &[source],
        )
        .await
        .unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        ids.push(filename.trim_end_matches(".json").to_string());
    }
    assert_ne!(ids[0], ids[1], "the second item got its own id");

    let page = h
        .get_expect("/api/hub/inbox?agent=quiet", StatusCode::OK)
        .await;

    assert_eq!(page["items"].as_array().unwrap().len(), 2, "{page}");
    for (id, body, content) in [
        (&ids[0], "first", b"one".as_slice()),
        (&ids[1], "second", b"two".as_slice()),
    ] {
        let item = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id.as_str())
            .unwrap_or_else(|| panic!("{id} is listed"));
        assert_eq!(item["body"], body);
        let url = item["attachments"][0]["url"].as_str().unwrap();
        let (_headers, bytes) = h.request_ok(Method::GET, url, "").await;
        assert_eq!(bytes, content, "{id} kept its own attachment");
    }

    // Archive the first, then save a third with the same title: it doesn't take the archived id.
    h.post_expect(
        &format!("/api/hub/inbox/quiet/{}/archive", ids[0]),
        StatusCode::OK,
    )
    .await;
    let third = crate::inbox::quick_add(
        &layout.user_inbox_dir(),
        "Daily report",
        "third",
        "agent",
        chrono_tz::UTC,
    )
    .await
    .unwrap();
    assert!(
        ![&ids[0], &ids[1]].contains(&&third.trim_end_matches(".json").to_string()),
        "the third item's id is new"
    );
    let archived = h
        .get_expect("/api/hub/inbox?agent=quiet&status=archived", StatusCode::OK)
        .await;
    assert_eq!(listed(&archived), [format!("quiet/{}", ids[0])]);
}

#[tokio::test]
async fn ids_saved_before_ids_were_unique_keep_working() {
    let h = Harness::new();
    // The date-and-slug ids earlier versions wrote, one with non-ASCII letters.
    let legacy = ["20260227_deploy_completed", "20260225_café_résumé"];
    let encoded = [
        "20260227_deploy_completed",
        "20260225_caf%C3%A9_r%C3%A9sum%C3%A9",
    ];
    for id in legacy {
        add_item(&h, "scout", id, at(27, 14, 30), false).await;
    }

    for (id, path_id) in legacy.into_iter().zip(encoded) {
        let base = format!("/api/hub/inbox/scout/{path_id}");
        let read = h
            .expect(Method::PUT, &format!("{base}/read"), None, StatusCode::OK)
            .await;
        assert_eq!(read["item"]["id"], id);
        h.post_expect(&format!("{base}/archive"), StatusCode::OK)
            .await;
        let restored = h
            .post_expect(&format!("{base}/restore"), StatusCode::OK)
            .await;
        assert_eq!(restored["item"]["id"], id);
    }
    let page = h.get_expect("/api/hub/inbox", StatusCode::OK).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
}
