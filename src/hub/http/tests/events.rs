//! Tests for `GET /api/hub/events` and the hub socket's `team_event` frames.

use super::*;
use crate::hub::team_events::{
    NewTeamEvent, TeamEvent, TeamEventKind, TeamEventLevel, TeamEventPlace, TeamEventTarget,
};

/// Record an entry about `scout`, the way the hub's recorder does.
fn record(h: &Harness, summary: &str, level: TeamEventLevel) -> TeamEvent {
    h.team_events.record(NewTeamEvent {
        at: Utc::now(),
        agent: Some("scout".to_string()),
        kind: TeamEventKind::AgentReplied,
        level,
        summary: summary.to_string(),
        target: Some(TeamEventTarget::AgentPlace {
            agent: "scout".to_string(),
            place: TeamEventPlace::Chat,
        }),
    })
}

fn record_many(h: &Harness, count: u32) {
    for n in 1..=count {
        record(h, &format!("entry {n}"), TeamEventLevel::Info);
    }
}

/// The ids of a page's events, in order.
fn ids(page: &Value) -> Vec<u64> {
    page["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["id"].as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn an_empty_log_answers_with_its_boot_id_and_no_events() {
    let h = Harness::new();
    let page = h.get_expect("/api/hub/events", StatusCode::OK).await;
    assert_eq!(
        page,
        json!({ "boot_id": TEST_BOOT_ID, "events": [], "next_before": null })
    );
}

#[tokio::test]
async fn an_entry_is_served_in_the_contract_shape() {
    let h = Harness::new();
    let entry = record(
        &h,
        "scout replied in your conversation",
        TeamEventLevel::Warn,
    );

    let page = h.get_expect("/api/hub/events", StatusCode::OK).await;

    assert_eq!(
        page["events"][0],
        json!({
            "id": 1,
            "at": entry.at,
            "agent": "scout",
            "kind": "agent_replied",
            "level": "warn",
            "summary": "scout replied in your conversation",
            "target": { "kind": "agent_place", "agent": "scout", "place": "chat" },
        })
    );
}

#[tokio::test]
async fn events_come_newest_first_fifty_at_a_time_and_page_back_with_before() {
    let h = Harness::new();
    record_many(&h, 120);

    let first = h.get_expect("/api/hub/events", StatusCode::OK).await;
    assert_eq!(ids(&first).len(), 50);
    assert_eq!(ids(&first).first(), Some(&120));
    assert_eq!(first["next_before"], 71);

    let second = h
        .get_expect("/api/hub/events?before=71&limit=60", StatusCode::OK)
        .await;
    assert_eq!(ids(&second), (11..=70).rev().collect::<Vec<_>>());
    assert_eq!(second["next_before"], 11);

    let last = h
        .get_expect("/api/hub/events?before=11", StatusCode::OK)
        .await;
    assert_eq!(ids(&last), (1..=10).rev().collect::<Vec<_>>());
    assert_eq!(last["next_before"], Value::Null);
}

#[tokio::test]
async fn after_returns_the_entries_a_client_has_not_seen() {
    let h = Harness::new();
    record_many(&h, 10);

    let newer = h
        .get_expect("/api/hub/events?after=7", StatusCode::OK)
        .await;
    assert_eq!(ids(&newer), [10, 9, 8]);
    assert_eq!(newer["next_before"], Value::Null);

    let none = h
        .get_expect("/api/hub/events?after=10", StatusCode::OK)
        .await;
    assert_eq!(ids(&none), Vec::<u64>::new());

    // More newer entries than fit: the newest come first, and before walks
    // back to the client's position.
    let capped = h
        .get_expect("/api/hub/events?after=2&limit=3", StatusCode::OK)
        .await;
    assert_eq!(ids(&capped), [10, 9, 8]);
    assert_eq!(capped["next_before"], 8);
    let rest = h
        .get_expect("/api/hub/events?after=2&before=8", StatusCode::OK)
        .await;
    assert_eq!(ids(&rest), [7, 6, 5, 4, 3]);
    assert_eq!(rest["next_before"], Value::Null);
}

#[tokio::test]
async fn a_limit_above_the_maximum_is_treated_as_the_maximum() {
    let h = Harness::new();
    record_many(&h, 250);
    let page = h
        .get_expect("/api/hub/events?limit=100000", StatusCode::OK)
        .await;
    assert_eq!(ids(&page).len(), 200);
    assert_eq!(page["next_before"], 51);
}

#[tokio::test]
async fn a_query_that_cannot_be_read_is_a_json_error() {
    let h = Harness::new();
    for query in [
        "before=newest",
        "after=-1",
        "before=1.5",
        "limit=0",
        "limit=many",
        "limit=-3",
    ] {
        let error = h
            .get_expect(&format!("/api/hub/events?{query}"), StatusCode::BAD_REQUEST)
            .await;
        let message = error["error"].as_str().unwrap_or_default();
        assert!(!message.is_empty(), "{query} explains itself: {error}");
    }
    let named = h
        .get_expect("/api/hub/events?before=newest", StatusCode::BAD_REQUEST)
        .await;
    assert!(
        named["error"].as_str().unwrap().contains("before"),
        "the error names the field: {named}"
    );
}

/// The next frame of `socket`, which must be a `team_event`.
async fn next_team_event(socket: &mut ClientSocket) -> Value {
    let frame = next_frame(socket).await;
    assert_eq!(frame["type"], "team_event", "{frame}");
    frame
}

#[tokio::test]
async fn each_new_entry_reaches_the_hub_socket_as_a_team_event_frame() {
    let h = Harness::new();
    record(&h, "before the page opened", TeamEventLevel::Info);
    let addr = h.serve().await;
    let mut socket = connect_hub(addr).await;
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");

    let first = record(&h, "first", TeamEventLevel::Info);
    let second = record(&h, "second", TeamEventLevel::Error);

    // A page reads what came before it connected from the events route, so
    // the first frame is the first entry recorded after it connected.
    let first_frame = next_team_event(&mut socket).await;
    assert_eq!(first_frame["boot_id"], TEST_BOOT_ID);
    assert_eq!(first_frame["event"], serde_json::to_value(&first).unwrap());
    assert_eq!(first.id, 2);
    let second_frame = next_team_event(&mut socket).await;
    assert_eq!(
        second_frame["event"],
        serde_json::to_value(&second).unwrap()
    );
}

#[tokio::test]
async fn the_boot_id_is_the_same_in_hub_boot_the_events_route_and_the_frames() {
    let h = Harness::new();
    let addr = h.serve().await;
    let mut socket = connect(addr, "/api/hub/ws").await;
    let boot = next_frame(&mut socket).await;
    assert_eq!(next_frame(&mut socket).await["type"], "system_one_status");
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");
    record(&h, "an entry", TeamEventLevel::Info);
    let frame = next_team_event(&mut socket).await;
    let page = h.get_expect("/api/hub/events", StatusCode::OK).await;

    assert_eq!(boot["boot_id"], TEST_BOOT_ID);
    assert_eq!(frame["boot_id"], boot["boot_id"]);
    assert_eq!(page["boot_id"], boot["boot_id"]);
}

#[tokio::test]
async fn a_warning_sent_to_one_connection_is_not_a_team_event() {
    let h = Harness::new();
    let addr = h.serve().await;
    let mut socket = connect_hub(addr).await;
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");

    // A watch request the hub refuses is answered with a warning to this
    // connection alone.
    send_client(
        &mut socket,
        &json!({ "type": "watch_team", "prefixes": ["wiki"] }),
    )
    .await;
    let warning = next_frame(&mut socket).await;
    assert_eq!(warning["type"], "notice");
    assert_eq!(warning["level"], "warn");

    let page = h.get_expect("/api/hub/events", StatusCode::OK).await;
    assert_eq!(ids(&page), Vec::<u64>::new(), "nothing was logged");
    // The next frame is the probe entry's, so no frame came for the warning.
    let probe = record(&h, "probe", TeamEventLevel::Info);
    let frame = next_team_event(&mut socket).await;
    assert_eq!(frame["event"]["id"], probe.id);
}

#[tokio::test]
async fn a_socket_that_falls_behind_the_log_gets_a_snapshot_then_the_entries_still_buffered() {
    let h = Harness::new();
    let addr = h.serve().await;
    let mut socket = connect_hub(addr).await;
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");

    // Recorded without yielding, so the connection can't read any of them
    // until the burst is over and its receiver has overflowed.
    record_many(&h, 300);

    let resnapshot = next_frame(&mut socket).await;
    assert_eq!(resnapshot["type"], "agents_snapshot");
    let oldest_kept = next_team_event(&mut socket).await;
    assert_eq!(oldest_kept["event"]["id"], 45, "the 44 oldest were lost");
    let mut last = oldest_kept["event"]["id"].as_u64().unwrap();
    while last < 300 {
        last = next_team_event(&mut socket).await["event"]["id"]
            .as_u64()
            .unwrap();
    }
    // The client reads what it missed from the route.
    let page = h
        .get_expect("/api/hub/events?after=0&limit=200", StatusCode::OK)
        .await;
    assert_eq!(ids(&page).first(), Some(&300));
}
