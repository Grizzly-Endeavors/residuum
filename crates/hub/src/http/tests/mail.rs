//! Tests for `/api/hub/mail/...`, against the fake directory's agents in
//! every state: `scout` running, `quiet` stopped, and `broken` failed when a
//! test adds it.

use chrono::{NaiveDate, NaiveDateTime};

use super::*;
use crate::uploads::UploadError;

fn at(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 9, day)
        .unwrap()
        .and_hms_opt(hour, minute, 0)
        .unwrap()
}

/// A message an agent sent the user, as `add_agent_message` writes it.
struct AgentMessage<'a> {
    agent: &'a str,
    id: &'a str,
    when: NaiveDateTime,
    read: bool,
    archived: bool,
    in_reply_to: Option<&'a str>,
    /// One `note.txt` with these bytes, when set.
    attachment: Option<&'a [u8]>,
}

impl<'a> AgentMessage<'a> {
    fn new(agent: &'a str, id: &'a str, when: NaiveDateTime) -> Self {
        Self {
            agent,
            id,
            when,
            read: false,
            archived: false,
            in_reply_to: None,
            attachment: None,
        }
    }
}

/// Save `message` in its agent's user inbox (or its archive).
async fn add_agent_message(h: &Harness, message: AgentMessage<'_>) {
    use residuum_workspace::inbox::InboxKind;
    let layout = WorkspaceLayout::new(h.agent_dir(message.agent));
    let dir = InboxKind::User.items_dir(&layout, message.archived);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let mut attachments = Vec::new();
    if let Some(bytes) = message.attachment {
        let item_dir = InboxKind::User
            .attachments_dir(&layout, message.archived)
            .join(message.id);
        tokio::fs::create_dir_all(&item_dir).await.unwrap();
        tokio::fs::write(item_dir.join("note.txt"), bytes)
            .await
            .unwrap();
        attachments.push(PathBuf::from(format!(
            "inbox/user/attachments/{}/note.txt",
            message.id
        )));
    }
    let item = residuum_workspace::inbox::InboxItem {
        title: format!("title of {}", message.id),
        body: format!("**body** of {}", message.id),
        source: "agent".to_string(),
        timestamp: message.when,
        read: message.read,
        attachments,
        in_reply_to: message.in_reply_to.map(str::to_string),
    };
    residuum_workspace::testing::save_item_for_tests(&dir, &format!("{}.json", message.id), &item)
        .await
        .unwrap();
}

/// The plain message an agent sent at `when`.
async fn add_simple(h: &Harness, agent: &str, id: &str, when: NaiveDateTime) {
    add_agent_message(h, AgentMessage::new(agent, id, when)).await;
}

/// `ref` escaped as one path segment.
fn enc(item: &str) -> String {
    item.replace('/', "%2F")
}

/// Stage `bytes` as `filename` and return the upload id.
async fn stage(h: &Harness, filename: &str, bytes: &[u8]) -> String {
    h.uploads.stage(filename, bytes).await.unwrap().id
}

/// The thread ids of a page, in order.
fn listed(page: &Value) -> Vec<String> {
    page["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["thread_id"].as_str().unwrap().to_string())
        .collect()
}

/// The `error` of a JSON error body.
fn error_of(body: &Value) -> &str {
    body["error"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON error, got {body}"))
}

async fn send_mail(h: &Harness, body: Value) -> Value {
    h.expect(
        Method::POST,
        "/api/hub/mail/send",
        Some(body),
        StatusCode::OK,
    )
    .await
}

#[tokio::test]
async fn the_inbox_holds_every_agents_threads_newest_first_in_every_state() {
    let h = Harness::new();
    h.add_failed_agent();
    add_simple(&h, "scout", "20260930_alpha", at(30, 8, 0)).await;
    add_agent_message(
        &h,
        AgentMessage {
            attachment: Some(b"hello"),
            ..AgentMessage::new("quiet", "20260930_beta", at(30, 8, 15))
        },
    )
    .await;
    add_agent_message(
        &h,
        AgentMessage {
            read: true,
            ..AgentMessage::new("broken", "20260929_gamma", at(29, 23, 59))
        },
    )
    .await;

    let page = h.get_expect("/api/hub/mail/threads", StatusCode::OK).await;

    assert_eq!(
        listed(&page),
        [
            "quiet/user/20260930_beta",
            "scout/user/20260930_alpha",
            "broken/user/20260929_gamma"
        ],
        "newest first, across a stopped, a running and a failed agent"
    );
    assert_eq!(page["next"], Value::Null);
    assert_eq!(
        page["threads"][0],
        json!({
            "thread_id": "quiet/user/20260930_beta",
            "subject": "title of 20260930_beta",
            "participants": ["quiet"],
            "message_count": 1,
            "latest_at": "2026-09-30T08:15:00Z",
            "snippet": "body of 20260930_beta",
            "unread": true,
            "has_attachments": true,
            "in_inbox": true,
            "in_sent": false,
            "archived": false,
        })
    );
    assert_eq!(page["threads"][2]["unread"], false);
}

#[tokio::test]
async fn the_snippet_is_the_start_of_the_latest_message_as_plain_text() {
    let h = Harness::new();
    let layout = WorkspaceLayout::new(h.agent_dir("scout"));
    tokio::fs::create_dir_all(layout.user_inbox_dir())
        .await
        .unwrap();
    let item = residuum_workspace::inbox::InboxItem {
        title: "Long".to_string(),
        body: format!(
            "# Heading\n\n[link](https://example.com) {}",
            "word ".repeat(60)
        ),
        source: "agent".to_string(),
        timestamp: at(30, 8, 0),
        read: false,
        attachments: Vec::new(),
        in_reply_to: None,
    };
    residuum_workspace::testing::save_item_for_tests(&layout.user_inbox_dir(), "long.json", &item)
        .await
        .unwrap();

    let page = h.get_expect("/api/hub/mail/threads", StatusCode::OK).await;

    let snippet = page["threads"][0]["snippet"].as_str().unwrap();
    assert!(snippet.starts_with("Heading link word word"), "{snippet}");
    assert_eq!(snippet.chars().count(), crate::mail::SNIPPET_CHARS);
    assert!(snippet.ends_with('…'), "{snippet}");
}

#[tokio::test]
async fn a_sent_message_nobody_answered_is_only_in_sent_and_an_answered_one_is_in_both() {
    let h = Harness::new();
    let lonely = send_mail(
        &h,
        json!({ "to": ["quiet"], "subject": "Anyone?", "body": "hello" }),
    )
    .await;
    let lonely = lonely["thread_id"].as_str().unwrap().to_string();
    let answered = send_mail(
        &h,
        json!({ "to": ["scout"], "subject": "Plan", "body": "the plan" }),
    )
    .await;
    let answered = answered["thread_id"].as_str().unwrap().to_string();
    add_agent_message(
        &h,
        AgentMessage {
            in_reply_to: Some(&answered),
            ..AgentMessage::new("scout", "20991231_re_plan", at(30, 9, 0))
        },
    )
    .await;

    let inbox = h
        .get_expect("/api/hub/mail/threads?folder=inbox", StatusCode::OK)
        .await;
    let sent = h
        .get_expect("/api/hub/mail/threads?folder=sent", StatusCode::OK)
        .await;
    let archived = h
        .get_expect("/api/hub/mail/threads?folder=archived", StatusCode::OK)
        .await;

    assert_eq!(listed(&inbox), std::slice::from_ref(&answered));
    let mut sent_ids = listed(&sent);
    sent_ids.sort();
    let mut want = vec![answered.clone(), lonely.clone()];
    want.sort();
    assert_eq!(sent_ids, want, "both threads hold a message of the user's");
    assert!(listed(&archived).is_empty(), "{archived}");
    let both = inbox["threads"][0].clone();
    assert_eq!(both["in_inbox"], true);
    assert_eq!(both["in_sent"], true);
    assert_eq!(both["message_count"], 2);
    assert_eq!(both["subject"], "Plan");
}

#[tokio::test]
async fn the_agent_filter_narrows_the_list_and_bad_queries_are_json_400s() {
    let h = Harness::new();
    add_simple(&h, "scout", "s1", at(30, 8, 0)).await;
    add_simple(&h, "quiet", "q1", at(30, 9, 0)).await;

    let scout = h
        .get_expect("/api/hub/mail/threads?agent=scout", StatusCode::OK)
        .await;
    assert_eq!(listed(&scout), ["scout/user/s1"]);

    for (query, wanted) in [
        ("agent=nobody", "nobody"),
        ("folder=trash", "folder"),
        ("limit=0", "at least 1"),
        ("limit=many", "whole number"),
        ("before=yesterday", "cursor"),
    ] {
        let body = h
            .get_expect(
                &format!("/api/hub/mail/threads?{query}"),
                StatusCode::BAD_REQUEST,
            )
            .await;
        assert!(error_of(&body).contains(wanted), "{query}: {body}");
    }
}

#[tokio::test]
async fn paging_visits_every_thread_once_in_order_even_when_times_tie() {
    let h = Harness::new();
    for (agent, id, minute) in [
        ("scout", "a", 0),
        ("quiet", "a", 0),
        ("scout", "b", 0),
        ("quiet", "c", 5),
        ("scout", "d", 10),
    ] {
        add_simple(&h, agent, id, at(30, 8, minute)).await;
    }
    let whole = h.get_expect("/api/hub/mail/threads", StatusCode::OK).await;
    assert_eq!(
        listed(&whole),
        [
            "scout/user/d",
            "quiet/user/c",
            "quiet/user/a",
            "scout/user/a",
            "scout/user/b"
        ],
        "newest first, then by thread id"
    );

    let mut seen = Vec::new();
    let mut uri = "/api/hub/mail/threads?limit=2".to_string();
    loop {
        let page = h.get_expect(&uri, StatusCode::OK).await;
        seen.extend(listed(&page));
        match page["next"].as_str() {
            Some(next) => {
                let next: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
                uri = format!("/api/hub/mail/threads?limit=2&before={next}");
            }
            None => break,
        }
    }
    assert_eq!(seen, listed(&whole), "every thread once, in order");
}

#[tokio::test]
async fn the_default_page_is_fifty_and_a_larger_limit_is_capped_at_two_hundred() {
    let h = Harness::new();
    for i in 0..210 {
        add_simple(&h, "scout", &format!("item{i:03}"), at(30, 8, 0)).await;
    }

    let default = h.get_expect("/api/hub/mail/threads", StatusCode::OK).await;
    let capped = h
        .get_expect("/api/hub/mail/threads?limit=1000", StatusCode::OK)
        .await;

    assert_eq!(listed(&default).len(), 50);
    assert!(default["next"].is_string());
    assert_eq!(listed(&capped).len(), 200);
    assert!(capped["next"].is_string());
}

#[tokio::test]
async fn a_thread_is_found_by_the_ref_of_any_of_its_messages() {
    let h = Harness::new();
    add_simple(&h, "scout", "root", at(30, 8, 0)).await;
    add_agent_message(
        &h,
        AgentMessage {
            in_reply_to: Some("scout/user/root"),
            ..AgentMessage::new("quiet", "answer", at(30, 9, 0))
        },
    )
    .await;

    for named in ["scout/user/root", "quiet/user/answer"] {
        let thread = h
            .get_expect(
                &format!("/api/hub/mail/threads/{}", enc(named)),
                StatusCode::OK,
            )
            .await;
        assert_eq!(thread["thread_id"], "scout/user/root", "{named}");
        assert_eq!(thread["subject"], "title of root");
        assert_eq!(thread["participants"], json!(["quiet", "scout"]));
        assert_eq!(thread["recipients_for_reply"], json!(["quiet", "scout"]));
        assert_eq!(
            thread["messages"][1],
            json!({
                "ref": "quiet/user/answer",
                "from": { "kind": "agent", "agent": "quiet" },
                "to": [],
                "at": "2026-09-30T09:00:00Z",
                "title": "title of answer",
                "body": "**body** of answer",
                "attachments": [],
                "in_reply_to": "scout/user/root",
                "read": false,
                "archived": false,
                "seen_by": [],
            }),
            "{named}"
        );
    }
    assert_eq!(
        h.get_expect("/api/hub/mail/threads/scout%2Fuser%2Froot", StatusCode::OK)
            .await["messages"][0]["in_reply_to"],
        Value::Null
    );
}

#[tokio::test]
async fn an_unknown_or_unparseable_ref_is_a_json_404_on_every_thread_route() {
    let h = Harness::new();
    add_simple(&h, "scout", "root", at(30, 8, 0)).await;

    for raw in ["scout%2Fuser%2Fghost", "not-a-ref", "nobody%2Fuser%2Froot"] {
        let base = format!("/api/hub/mail/threads/{raw}");
        let body = h.get_expect(&base, StatusCode::NOT_FOUND).await;
        assert!(error_of(&body).contains("no message"), "{raw}: {body}");
        for action in ["read", "archive", "restore"] {
            h.post_expect(&format!("{base}/{action}"), StatusCode::NOT_FOUND)
                .await;
        }
        h.expect(
            Method::POST,
            &format!("{base}/reply"),
            Some(json!({ "body": "hi" })),
            StatusCode::NOT_FOUND,
        )
        .await;
        h.expect(
            Method::POST,
            &format!("{base}/reply"),
            Some(json!({ "not_a_body": true })),
            StatusCode::NOT_FOUND,
        )
        .await;
    }
}

#[tokio::test]
async fn one_send_to_several_agents_is_one_message_with_each_agents_seen() {
    let h = Harness::new();
    let upload = stage(&h, "plan.md", b"# plan").await;

    let sent = send_mail(
        &h,
        json!({
            "to": ["scout", "quiet"],
            "subject": "Kickoff",
            "body": "Read the plan.",
            "attachments": [upload],
        }),
    )
    .await;
    let thread_id = sent["thread_id"].as_str().unwrap().to_string();
    assert!(thread_id.starts_with("quiet/agent/"), "{thread_id}");
    assert!(
        matches!(
            h.uploads.resolve(&upload).await,
            Err(UploadError::NotFound(_))
        ),
        "the staged upload is consumed by the send"
    );

    // scout reads its copy.
    let id = thread_id.rsplit('/').next().unwrap().to_string();
    residuum_workspace::inbox::mark_read(
        &WorkspaceLayout::new(h.agent_dir("scout")).agent_inbox_dir(),
        &id,
    )
    .await
    .unwrap();

    let thread = h
        .get_expect(
            &format!("/api/hub/mail/threads/{}", enc(&thread_id)),
            StatusCode::OK,
        )
        .await;
    let messages = thread["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "one message, not one per copy: {thread}");
    let message = &messages[0];
    assert_eq!(message["from"], json!({ "kind": "user" }));
    assert_eq!(message["to"], json!(["quiet", "scout"]));
    assert_eq!(message["title"], "Kickoff");
    assert_eq!(message["read"], true);
    assert_eq!(
        message["seen_by"],
        json!([
            { "agent": "quiet", "seen": false },
            { "agent": "scout", "seen": true },
        ])
    );
    let attachment = &message["attachments"][0];
    assert_eq!(attachment["filename"], "plan.md");
    assert_eq!(attachment["size"], 6);
    assert_eq!(attachment["agent"], "quiet");
    assert_eq!(
        attachment["path"],
        format!("inbox/agent/attachments/{id}/plan.md")
    );
    assert_eq!(
        attachment["url"],
        format!("/api/agents/quiet/inbox/agent/{id}/attachments/0")
    );
    // quiet is stopped: the file router serves the bytes.
    let (_served_headers, served) = h
        .request_ok(Method::GET, attachment["url"].as_str().unwrap(), "")
        .await;
    assert_eq!(served, b"# plan");
    // Each recipient holds its own copy of the file.
    for agent in ["scout", "quiet"] {
        let (_headers, bytes) = h
            .request_ok(
                Method::GET,
                &format!("/api/agents/{agent}/inbox/agent/{id}/attachments/0"),
                "",
            )
            .await;
        assert_eq!(bytes, b"# plan", "{agent}");
    }
}

#[tokio::test]
async fn a_send_that_cannot_go_is_a_json_400_and_writes_nothing() {
    let h = Harness::new();
    let upload = stage(&h, "kept.txt", b"kept").await;
    let unknown_upload = "0".repeat(32);

    for (body, wanted) in [
        (
            json!({ "to": [], "subject": "s", "body": "b" }),
            "at least one agent",
        ),
        (
            json!({ "to": ["nobody"], "subject": "s", "body": "b" }),
            "nobody",
        ),
        (
            json!({ "to": ["scout"], "subject": "s", "body": "  " }),
            "Write a message",
        ),
        (
            json!({ "to": ["scout"], "subject": "s", "body": "b", "attachments": [upload, unknown_upload] }),
            "isn't available",
        ),
        (
            json!({ "to": ["scout"], "body": "b", "attachments": ["../../etc"] }),
            "isn't available",
        ),
        (json!({ "subject": "s" }), "couldn't be read"),
    ] {
        let reply = h
            .expect(
                Method::POST,
                "/api/hub/mail/send",
                Some(body.clone()),
                StatusCode::BAD_REQUEST,
            )
            .await;
        assert!(error_of(&reply).contains(wanted), "{body}: {reply}");
    }
    assert!(
        !WorkspaceLayout::new(h.agent_dir("scout"))
            .agent_inbox_dir()
            .exists(),
        "nothing was written"
    );
    assert!(
        h.uploads.resolve(&upload).await.is_ok(),
        "a refused send keeps the uploads it named"
    );
}

#[tokio::test]
async fn a_reply_goes_to_everyone_in_the_thread_and_answers_its_latest_message() {
    let h = Harness::new();
    add_simple(&h, "scout", "root", at(30, 8, 0)).await;
    add_agent_message(
        &h,
        AgentMessage {
            in_reply_to: Some("scout/user/root"),
            ..AgentMessage::new("quiet", "answer", at(30, 9, 0))
        },
    )
    .await;
    let upload = stage(&h, "notes.txt", b"notes").await;

    let reply = h
        .expect(
            Method::POST,
            "/api/hub/mail/threads/quiet%2Fuser%2Fanswer/reply",
            Some(json!({ "body": "Thanks, both.", "attachments": [upload] })),
            StatusCode::OK,
        )
        .await;

    let message = &reply["message"];
    assert_eq!(message["from"], json!({ "kind": "user" }));
    assert_eq!(message["to"], json!(["quiet", "scout"]));
    assert_eq!(message["title"], "Re: title of root");
    assert_eq!(message["in_reply_to"], "quiet/user/answer");
    assert_eq!(message["body"], "Thanks, both.");
    assert_eq!(message["attachments"][0]["filename"], "notes.txt");
    assert!(h.uploads.resolve(&upload).await.is_err(), "consumed");

    let thread = h
        .get_expect("/api/hub/mail/threads/scout%2Fuser%2Froot", StatusCode::OK)
        .await;
    let messages = thread["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages[2]["ref"], message["ref"],
        "the reply is in the thread"
    );

    let blank = h
        .expect(
            Method::POST,
            "/api/hub/mail/threads/scout%2Fuser%2Froot/reply",
            Some(json!({ "body": "" })),
            StatusCode::BAD_REQUEST,
        )
        .await;
    assert!(error_of(&blank).contains("Write a message"), "{blank}");
}

#[tokio::test]
async fn read_archive_and_restore_change_the_users_messages_in_every_agent_state() {
    let h = Harness::new();
    h.add_failed_agent();
    for name in EVERY_STATE {
        add_simple(&h, name, "root", at(30, 8, 0)).await;
        add_agent_message(
            &h,
            AgentMessage {
                in_reply_to: Some(&format!("{name}/user/root")),
                ..AgentMessage::new(name, "more", at(30, 9, 0))
            },
        )
        .await;
        let base = format!("/api/hub/mail/threads/{name}%2Fuser%2Fmore");

        h.expect(
            Method::POST,
            &format!("{base}/read"),
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
        let thread = h.get_expect(&base, StatusCode::OK).await;
        assert!(
            thread["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["read"] == true),
            "{name}: every message read: {thread}"
        );

        h.expect(
            Method::POST,
            &format!("{base}/archive"),
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
        let archived = h
            .get_expect(
                &format!("/api/hub/mail/threads?folder=archived&agent={name}"),
                StatusCode::OK,
            )
            .await;
        assert_eq!(listed(&archived), [format!("{name}/user/root")], "{name}");
        let inbox = h
            .get_expect(
                &format!("/api/hub/mail/threads?agent={name}"),
                StatusCode::OK,
            )
            .await;
        assert!(listed(&inbox).is_empty(), "{name}: {inbox}");
        assert!(
            WorkspaceLayout::new(h.agent_dir(name))
                .user_inbox_archive_dir()
                .join("root.json")
                .exists(),
            "{name}: moved to the archive on disk"
        );

        h.expect(
            Method::POST,
            &format!("{base}/restore"),
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
        let restored = h
            .get_expect(
                &format!("/api/hub/mail/threads?agent={name}"),
                StatusCode::OK,
            )
            .await;
        assert_eq!(listed(&restored), [format!("{name}/user/root")], "{name}");
    }
}

#[tokio::test]
async fn a_users_message_attachment_and_an_agents_both_serve_with_the_agent_stopped() {
    let h = Harness::new();
    add_agent_message(
        &h,
        AgentMessage {
            attachment: Some(b"from quiet"),
            ..AgentMessage::new("quiet", "report", at(30, 8, 0))
        },
    )
    .await;
    let upload = stage(&h, "answer.txt", b"from you").await;
    h.expect(
        Method::POST,
        "/api/hub/mail/threads/quiet%2Fuser%2Freport/reply",
        Some(json!({ "body": "see attached", "attachments": [upload] })),
        StatusCode::OK,
    )
    .await;

    let thread = h
        .get_expect(
            "/api/hub/mail/threads/quiet%2Fuser%2Freport",
            StatusCode::OK,
        )
        .await;
    let urls: Vec<(String, String)> = thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let attachment = &m["attachments"][0];
            (
                attachment["url"].as_str().unwrap().to_string(),
                attachment["path"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        urls[0],
        (
            "/api/agents/quiet/inbox/report/attachments/0".to_string(),
            "inbox/user/attachments/report/note.txt".to_string()
        )
    );
    assert!(urls[1].0.contains("/inbox/agent/"), "{urls:?}");
    for ((url, _), want) in urls.iter().zip([b"from quiet".as_slice(), b"from you"]) {
        let (_headers, bytes) = h.request_ok(Method::GET, url, "").await;
        assert_eq!(bytes, want, "{url}");
    }

    // Archiving the thread keeps both links working.
    h.expect(
        Method::POST,
        "/api/hub/mail/threads/quiet%2Fuser%2Freport/archive",
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    let archived = h
        .get_expect(
            "/api/hub/mail/threads/quiet%2Fuser%2Freport",
            StatusCode::OK,
        )
        .await;
    let url = archived["messages"][0]["attachments"][0]["url"]
        .as_str()
        .unwrap()
        .to_string();
    let (_headers, bytes) = h.request_ok(Method::GET, &url, "").await;
    assert_eq!(bytes, b"from quiet");
    assert_eq!(
        archived["messages"][0]["attachments"][0]["path"],
        "archive/inbox/user/attachments/report/note.txt"
    );
}

#[tokio::test]
async fn message_times_follow_the_hubs_timezone_across_dst_edges() {
    let h = Harness::new();
    *h.directory.timezone.lock().unwrap() = "America/New_York".parse().unwrap();
    let new_york = |month, day, hour, minute| {
        NaiveDate::from_ymd_opt(2026, month, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    };
    add_simple(&h, "scout", "winter", new_york(1, 15, 10, 0)).await;
    add_simple(&h, "scout", "repeated", new_york(11, 1, 1, 30)).await;
    add_simple(&h, "scout", "skipped", new_york(3, 8, 2, 30)).await;

    let page = h.get_expect("/api/hub/mail/threads", StatusCode::OK).await;

    let times: Vec<(&str, &str)> = page["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["thread_id"].as_str().unwrap(),
                t["latest_at"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        times,
        [
            ("scout/user/repeated", "2026-11-01T01:30:00-04:00"),
            ("scout/user/skipped", "2026-03-08T03:30:00-04:00"),
            ("scout/user/winter", "2026-01-15T10:00:00-05:00"),
        ],
        "an ambiguous time takes the earlier offset, a nonexistent one moves forward by the gap"
    );
}

#[tokio::test]
async fn ids_with_non_ascii_letters_work_through_percent_encoded_refs() {
    let h = Harness::new();
    add_simple(&h, "scout", "20260225_café_résumé", at(27, 14, 30)).await;
    let base = "/api/hub/mail/threads/scout%2Fuser%2F20260225_caf%C3%A9_r%C3%A9sum%C3%A9";

    let thread = h.get_expect(base, StatusCode::OK).await;
    assert_eq!(thread["thread_id"], "scout/user/20260225_café_résumé");
    h.expect(
        Method::POST,
        &format!("{base}/read"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    h.expect(
        Method::POST,
        &format!("{base}/archive"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    h.expect(
        Method::POST,
        &format!("{base}/restore"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    let page = h.get_expect("/api/hub/mail/threads", StatusCode::OK).await;
    assert_eq!(page["threads"][0]["unread"], false);
}

#[tokio::test]
async fn the_old_inbox_routes_are_gone() {
    let h = Harness::new();
    add_simple(&h, "scout", "item1", at(30, 8, 0)).await;
    for (method, uri) in [
        (Method::GET, "/api/hub/inbox"),
        (Method::GET, "/api/hub/inbox/unread"),
        (Method::PUT, "/api/hub/inbox/scout/item1/read"),
        (Method::POST, "/api/hub/inbox/scout/item1/archive"),
    ] {
        let (status, _) = h.call(method.clone(), uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    for (method, route) in [
        (Method::GET, "inbox"),
        (Method::GET, "inbox/archive"),
        (Method::PUT, "inbox/item1/read"),
        (Method::POST, "inbox/item1/archive"),
        (Method::POST, "inbox/item1/restore"),
    ] {
        let uri = format!("/api/agents/quiet/{route}");
        let (status, _) = h.call(method.clone(), &uri, None).await;
        assert_ne!(status, StatusCode::OK, "{method} {uri}");
    }
}
