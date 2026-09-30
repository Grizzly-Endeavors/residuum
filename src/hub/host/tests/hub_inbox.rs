//! The cross-agent inbox against a real host: real agents, one running and
//! one stopped, read and changed through the hub's HTTP wiring.

use super::*;

/// The `agent/id` of every item of a `GET /api/hub/inbox` body, in order.
fn listed(body: &str) -> Vec<String> {
    let page: Value = serde_json::from_str(body).unwrap();
    array_at(&page, "items")
        .iter()
        .map(|item| format!("{}/{}", str_at(item, "agent"), str_at(item, "id")))
        .collect()
}

#[tokio::test]
async fn the_hub_inbox_lists_and_changes_a_running_and_a_stopped_agents_items() {
    let hub = Fixture::new(&["scout", "quiet"], "").await;
    hub.host.start("scout").await.unwrap();
    assert_eq!(hub.state_of("quiet"), AgentState::Stopped);
    hub.add_inbox_item("scout", "20260930_pelican");
    hub.add_inbox_item("quiet", "20260930_heron");

    let (status, body) = hub.get("/api/hub/inbox").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        listed(&body),
        ["scout/20260930_pelican", "quiet/20260930_heron"],
        "both agents' items, the newer id first at a shared time"
    );
    let page: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        str_at(array_at(&page, "items").first().unwrap(), "at"),
        "2026-09-30T08:15:00Z",
        "the stored local time is read in the hub's timezone"
    );

    let client = &hub.http;
    for agent in ["scout", "quiet"] {
        let id = if agent == "scout" {
            "20260930_pelican"
        } else {
            "20260930_heron"
        };
        let read = client
            .put(hub.url(&format!("/api/hub/inbox/{agent}/{id}/read")))
            .send()
            .await
            .unwrap();
        assert_eq!(read.status().as_u16(), 200, "{agent}");
    }
    let (_, unread) = hub.get("/api/hub/inbox/unread").await;
    assert_eq!(
        serde_json::from_str::<Value>(&unread).unwrap(),
        json!({ "total": 0, "by_agent": { "quiet": 0, "scout": 0 } })
    );

    let archived = client
        .post(hub.url("/api/hub/inbox/quiet/20260930_heron/archive"))
        .send()
        .await
        .unwrap();
    assert_eq!(archived.status().as_u16(), 200);
    let (_, active) = hub.get("/api/hub/inbox").await;
    assert_eq!(listed(&active), ["scout/20260930_pelican"]);
    let (_, in_archive) = hub.get("/api/hub/inbox?status=archived").await;
    assert_eq!(listed(&in_archive), ["quiet/20260930_heron"]);
}

#[tokio::test]
async fn the_hub_inbox_reads_item_times_in_the_hubs_current_timezone() {
    let hub = Fixture::new(&["quiet"], "").await;
    hub.add_inbox_item("quiet", "20260930_heron");

    let mut config = hub.host.hub_config();
    config.timezone = chrono_tz::Asia::Tokyo;
    hub.host.hub_config_changed(config);

    let (status, body) = hub.get("/api/hub/inbox").await;
    assert_eq!(status, 200, "{body}");
    let page: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        str_at(array_at(&page, "items").first().unwrap(), "at"),
        "2026-09-30T08:15:00+09:00"
    );
}
