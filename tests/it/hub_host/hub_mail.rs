//! Mail against a real host: real agents, one running and one stopped, read,
//! changed and written through the hub's HTTP wiring.
#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

use super::*;

/// The thread ids of a `GET /api/hub/mail/threads` body, in order.
fn listed(body: &str) -> Vec<String> {
    let page: Value = serde_json::from_str(body).unwrap();
    array_at(&page, "threads")
        .iter()
        .map(|thread| str_at(thread, "thread_id").to_string())
        .collect()
}

#[tokio::test]
async fn mail_lists_and_changes_a_running_and_a_stopped_agents_threads() {
    let hub = Fixture::new(&["scout", "quiet"], "").await;
    hub.host.start("scout").await.unwrap();
    assert_eq!(hub.state_of("quiet"), AgentState::Stopped);
    hub.add_inbox_item("scout", "20260930_pelican");
    hub.add_inbox_item("quiet", "20260930_heron");

    let (status, body) = hub.get("/api/hub/mail/threads").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        listed(&body),
        ["quiet/user/20260930_heron", "scout/user/20260930_pelican"],
        "both agents' threads, by thread id at a shared time"
    );
    let page: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        str_at(array_at(&page, "threads").first().unwrap(), "latest_at"),
        "2026-09-30T08:15:00Z",
        "the stored local time is read in the hub's timezone"
    );

    let client = &hub.http;
    for thread in [
        "scout%2Fuser%2F20260930_pelican",
        "quiet%2Fuser%2F20260930_heron",
    ] {
        let read = client
            .post(hub.url(&format!("/api/hub/mail/threads/{thread}/read")))
            .send()
            .await
            .unwrap();
        assert_eq!(read.status().as_u16(), 204, "{thread}");
    }
    let (_, after_read) = hub.get("/api/hub/mail/threads").await;
    let page: Value = serde_json::from_str(&after_read).unwrap();
    assert!(
        array_at(&page, "threads")
            .iter()
            .all(|t| t.get("unread") == Some(&Value::Bool(false))),
        "{after_read}"
    );

    let archived = client
        .post(hub.url("/api/hub/mail/threads/quiet%2Fuser%2F20260930_heron/archive"))
        .send()
        .await
        .unwrap();
    assert_eq!(archived.status().as_u16(), 204);
    let (_, inbox) = hub.get("/api/hub/mail/threads").await;
    assert_eq!(listed(&inbox), ["scout/user/20260930_pelican"]);
    let (_, in_archive) = hub.get("/api/hub/mail/threads?folder=archived").await;
    assert_eq!(listed(&in_archive), ["quiet/user/20260930_heron"]);
}

#[tokio::test]
async fn a_message_sent_to_a_running_and_a_stopped_agent_lands_in_both_inboxes() {
    let hub = Fixture::new(&["scout", "quiet"], "").await;
    hub.host.start("scout").await.unwrap();

    let sent = hub
        .http
        .post(hub.url("/api/hub/mail/send"))
        .json(&json!({ "to": ["scout", "quiet"], "subject": "Kickoff", "body": "Start." }))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status().as_u16(), 200);
    let sent: Value = sent.json().await.unwrap();
    let thread_id = str_at(&sent, "thread_id").to_string();
    let id = thread_id.rsplit('/').next().unwrap().to_string();
    for agent in ["scout", "quiet"] {
        assert!(
            hub.root
                .path()
                .join(agent)
                .join("inbox/agent")
                .join(format!("{id}.json"))
                .exists(),
            "{agent} holds a copy"
        );
    }

    let (status, body) = hub.get("/api/hub/mail/threads?folder=sent").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(listed(&body), [thread_id]);
}

#[tokio::test]
async fn mail_reads_message_times_in_the_hubs_current_timezone() {
    let hub = Fixture::new(&["quiet"], "").await;
    hub.add_inbox_item("quiet", "20260930_heron");

    let mut config = hub.host.hub_config();
    config.timezone = chrono_tz::Asia::Tokyo;
    hub.host.hub_config_changed(config);

    let (status, body) = hub.get("/api/hub/mail/threads").await;
    assert_eq!(status, 200, "{body}");
    let page: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        str_at(array_at(&page, "threads").first().unwrap(), "latest_at"),
        "2026-09-30T08:15:00+09:00"
    );
}

#[tokio::test]
async fn an_active_items_text_is_read_for_a_push_and_a_moved_item_has_none() {
    use residuum::hub::mail::active_text;

    let hub = Fixture::new(&["quiet"], "").await;
    hub.add_inbox_item("quiet", "20260930_heron");
    let directory = hub.host.as_ref();

    assert_eq!(
        active_text(directory, "quiet", "20260930_heron")
            .await
            .unwrap(),
        Some(("Pelican".to_string(), "seen at the pier".to_string()))
    );

    let archived = hub
        .http
        .post(hub.url("/api/hub/mail/threads/quiet%2Fuser%2F20260930_heron/archive"))
        .send()
        .await
        .unwrap();
    assert_eq!(archived.status().as_u16(), 204);
    assert_eq!(
        active_text(directory, "quiet", "20260930_heron")
            .await
            .unwrap(),
        None,
        "the user moved it before the push was worded"
    );
    assert!(
        active_text(directory, "nobody", "20260930_heron")
            .await
            .is_err()
    );
    assert!(active_text(directory, "quiet", "../escape").await.is_err());
}
