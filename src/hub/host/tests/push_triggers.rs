//! Web Push against a real hub: what agents do reaches registered devices as
//! encrypted pushes, each trigger fires once by its own rule, each device's
//! preferences decide whether it hears, and a device whose user is looking at
//! the app (reported over the hub WebSocket) is left alone.
//!
//! Each device is a fake push service that accepts everything, with the
//! browser keys that can read what reaches it.

use super::agent_watch::agent_bus;
use super::*;
use crate::bus::{NotifyName, OutboundA2aTaskEvent, SYSTEM_CHANNEL, UserInboxAddedEvent, topics};
use crate::hub::push::{Browser, PushPreferencesPatch, PutPushDeviceRequest, WebPushSubscription};

/// How long to wait to be sure nothing more is coming.
const QUIET: Duration = Duration::from_millis(500);

type HubSocketStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Every preference on.
fn all_on() -> PushPreferencesPatch {
    PushPreferencesPatch {
        inbox_item: Some(true),
        agent_failed: Some(true),
        outbound_unreachable: Some(true),
        reply_while_away: Some(true),
    }
}

/// A device the hub can push to.
struct Phone {
    id: String,
    server: MockServer,
    browser: Option<Browser>,
}

impl Phone {
    async fn register(hub: &Fixture, label: &str, preferences: PushPreferencesPatch) -> Self {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        let browser = Browser::new();
        let device = hub
            .services
            .push
            .upsert_device(PutPushDeviceRequest {
                subscription: WebPushSubscription {
                    endpoint: format!("{}/push/{label}", server.uri()),
                    keys: browser.keys(),
                },
                label: Some(label.to_string()),
                preferences: Some(preferences),
                previous_endpoint: None,
            })
            .await
            .unwrap();
        Self {
            id: device.id,
            server,
            browser: Some(browser),
        }
    }

    async fn pushes(&self) -> usize {
        self.server.received_requests().await.unwrap().len()
    }

    /// Wait until `count` pushes have arrived.
    async fn until_pushes(&self, count: usize) {
        wait::until(&format!("{count} push(es) to arrive"), || async {
            (self.pushes().await >= count).then_some(())
        })
        .await;
    }

    /// Wait out [`QUIET`], then say how many pushes have arrived in all.
    async fn pushes_after_quiet(&self) -> usize {
        tokio::time::sleep(QUIET).await;
        self.pushes().await
    }

    /// What the push that arrived first says. A browser's key opens one
    /// message, so this reads one push per device.
    async fn first_payload(&mut self) -> Value {
        let requests = self.server.received_requests().await.unwrap();
        let body = &requests.first().expect("a push arrived").body;
        let plaintext = self
            .browser
            .take()
            .expect("this device's first push was already read")
            .decrypt(body);
        serde_json::from_slice(&plaintext).unwrap()
    }
}

/// An agent saves an item in its user inbox and announces it, as its
/// `user_inbox_add` tool does.
async fn file_item(hub: &Fixture, agent: &str, id: &str) {
    hub.add_inbox_item(agent, id);
    agent_bus(hub, agent)
        .publisher()
        .publish(
            topics::UserInbox,
            UserInboxAddedEvent {
                item_id: id.to_string(),
            },
        )
        .await
        .unwrap();
}

/// A task sent to `laptop` whose unreachable streak is eleven minutes old.
fn outbound_task(unreachable_notified: bool) -> crate::a2a::TrackedTask {
    let now = Utc::now();
    crate::a2a::TrackedTask {
        sender_address: "main".to_string(),
        agent: "laptop".to_string(),
        task_id: "t1".to_string(),
        context_id: "c1".to_string(),
        state: "working".to_string(),
        last_status_text: None,
        hop_count: 0,
        created_at: now,
        updated_at: now,
        first_unreachable_at: Some(now - chrono::Duration::minutes(11)),
        unreachable_notified,
        notified_this_turn: false,
        stopped_by_user: false,
    }
}

async fn announce_outbound(hub: &Fixture, agent: &str, task: crate::a2a::TrackedTask) {
    agent_bus(hub, agent)
        .publisher()
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            OutboundA2aTaskEvent { task },
        )
        .await
        .unwrap();
}

/// Open the hub WebSocket, which presence is reported on.
async fn open_hub_socket(hub: &Fixture) -> HubSocketStream {
    tokio_tungstenite::connect_async(format!("ws://{}/api/hub/ws", hub.addr))
        .await
        .unwrap()
        .0
}

async fn report_presence(socket: &mut HubSocketStream, device_id: &str, active: bool) {
    socket
        .send(WsMessage::text(
            json!({ "type": "presence", "device_id": device_id, "active": active }).to_string(),
        ))
        .await
        .unwrap();
}

/// Wait until the hub's book says whether `device_id` is present.
async fn until_present(hub: &Fixture, device_id: &str, present: bool) {
    wait::until("the hub to record the device's presence", || async {
        (hub.services
            .push
            .presence()
            .active_devices()
            .contains(device_id)
            == present)
            .then_some(())
    })
    .await;
}

// ─── inbox_item ───────────────────────────────────────────────────────

#[tokio::test]
async fn an_item_an_agent_files_is_one_push_whose_badge_is_every_agents_unread_count() {
    let hub = Fixture::new(&["scout", "atlas"], "").await;
    mount_script(hub.mock("scout"), "filed it", |role, content| {
        is_user_message_with(role, content, "file a note").then(|| {
            (
                "user_inbox_add",
                json!({ "title": "Heads up", "body": "look at **this**" }),
            )
        })
    })
    .await;
    hub.host.start("scout").await.unwrap();
    // A stopped agent's unread items count toward the badge too.
    hub.add_inbox_item("atlas", "20260930_heron");
    let mut phone = Phone::register(&hub, "Phone", all_on()).await;

    hub.chat("scout", "please file a note").await;
    phone.until_pushes(1).await;

    let payload = phone.first_payload().await;
    let tag = str_at(&payload, "tag").to_string();
    let item_id = tag.strip_prefix("inbox:scout:").expect("an inbox tag");
    assert_eq!(
        payload,
        json!({
            "v": 1,
            "event": "inbox_item",
            "agent": "scout",
            "title": "Heads up",
            "body": "From scout: look at this",
            "target": format!("/inbox?item=scout:{item_id}"),
            "tag": tag,
            "badge": 2,
        })
    );
    // The turn that filed it ended with the web client connected, and the
    // file change beside the announcement is not another item.
    assert_eq!(
        phone.pushes_after_quiet().await,
        1,
        "one item is one push, and the reply the client saw is none"
    );
}

#[tokio::test]
async fn each_device_hears_only_the_events_its_preferences_turn_on() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let wants = Phone::register(&hub, "Wants", all_on()).await;
    let declines = Phone::register(
        &hub,
        "Declines",
        PushPreferencesPatch {
            inbox_item: Some(false),
            ..all_on()
        },
    )
    .await;

    file_item(&hub, "scout", "20260930_pelican").await;
    wants.until_pushes(1).await;
    announce_outbound(&hub, "scout", outbound_task(true)).await;
    declines.until_pushes(1).await;

    assert_eq!(
        wants.pushes_after_quiet().await,
        2,
        "it wants the item and the unreachable remote"
    );
    assert_eq!(
        declines.pushes().await,
        1,
        "with inbox items off it hears only the unreachable remote"
    );
}

// ─── agent_failed ─────────────────────────────────────────────────────

#[tokio::test]
async fn an_agent_that_cant_start_is_one_push_and_nothing_in_its_inbox() {
    let hub = Fixture::new(&["scout"], "").await;
    std::fs::write(
        hub.root.path().join("scout/config/config.toml"),
        "this is not = valid toml [",
    )
    .unwrap();
    let mut phone = Phone::register(&hub, "Phone", PushPreferencesPatch::default()).await;

    hub.host.start("scout").await.unwrap_err();
    phone.until_pushes(1).await;

    let payload = phone.first_payload().await;
    assert_eq!(str_at(&payload, "event"), "agent_failed");
    assert_eq!(str_at(&payload, "agent"), "scout");
    assert_eq!(str_at(&payload, "title"), "scout couldn't start");
    assert_eq!(
        str_at(&payload, "body"),
        "Its settings need fixing before it can run."
    );
    assert_eq!(str_at(&payload, "tag"), "failed:scout");
    assert_eq!(str_at(&payload, "target"), "/agent/scout");
    assert_eq!(
        phone.pushes_after_quiet().await,
        1,
        "the failure is the only push, with no inbox item to announce beside it"
    );
    assert!(
        !hub.root.path().join("scout/inbox").exists(),
        "a first-start failure creates no inbox for the user to find a note in"
    );
}

#[tokio::test]
async fn an_agent_that_crashes_while_running_says_it_stopped_unexpectedly() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut phone = Phone::register(&hub, "Phone", PushPreferencesPatch::default()).await;

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/agents/scout/ws", hub.addr))
            .await
            .unwrap();
    ws.send(WsMessage::text(
        json!({ "type": "server_command", "name": "panic_for_test" }).to_string(),
    ))
    .await
    .unwrap();
    phone.until_pushes(1).await;

    let payload = phone.first_payload().await;
    assert_eq!(str_at(&payload, "title"), "scout stopped unexpectedly");
    assert_eq!(
        str_at(&payload, "body"),
        "It hit an internal error. Open Residuum to restart it."
    );
    assert_eq!(str_at(&payload, "tag"), "failed:scout");
}

// ─── outbound_unreachable ─────────────────────────────────────────────

#[tokio::test]
async fn an_unreachable_streak_is_one_push_when_it_passes_the_threshold() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut phone = Phone::register(&hub, "Phone", all_on()).await;

    // The streak starts and passes the threshold, and then the tracker
    // announces the same streak once more.
    announce_outbound(&hub, "scout", outbound_task(false)).await;
    announce_outbound(&hub, "scout", outbound_task(true)).await;
    phone.until_pushes(1).await;
    announce_outbound(&hub, "scout", outbound_task(true)).await;

    assert_eq!(phone.pushes_after_quiet().await, 1, "once per streak");
    let payload = phone.first_payload().await;
    assert_eq!(str_at(&payload, "event"), "outbound_unreachable");
    assert_eq!(str_at(&payload, "title"), "scout can't reach laptop");
    assert!(
        str_at(&payload, "body").starts_with("A task has been waiting since "),
        "{payload}"
    );
    assert_eq!(str_at(&payload, "tag"), "outbound:scout:t1");
    assert_eq!(str_at(&payload, "target"), "/agent/scout/activity");
}

// ─── reply_while_away ─────────────────────────────────────────────────

#[tokio::test]
async fn a_reply_is_pushed_once_when_no_client_was_connected_and_never_for_a_background_turn() {
    use crate::hub::team::{TeamLink, parse_team_address};

    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut phone = Phone::register(&hub, "Phone", all_on()).await;

    // A reply the connected client saw.
    assert_eq!(hub.chat("scout", "hello").await, "scout here");

    // A reply the client left before: a slow model, and the client closes
    // its socket while the turn runs.
    hub.mock("scout").reset().await;
    mount_reply(
        hub.mock("scout"),
        "**Away** reply",
        Duration::from_millis(600),
    )
    .await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/agents/scout/ws", hub.addr))
            .await
            .unwrap();
    ws.send(WsMessage::text(
        json!({ "type": "send_message", "id": "m1", "content": "ping" }).to_string(),
    ))
    .await
    .unwrap();
    wait::until("scout to be busy", || async {
        hub.activity_of("scout").busy.then_some(())
    })
    .await;
    ws.close(None).await.unwrap();
    drop(ws);
    phone.until_pushes(1).await;

    // A teammate's message starts a background turn that still replies.
    let mut changes = hub.host.agent_changes().subscribe();
    let link = TeamLink::new("guest", Arc::clone(&hub.services.team_router));
    let target = parse_team_address("agent:scout").unwrap().unwrap();
    let main =
        crate::bus::SessionAddress::from(crate::background::registry::MAIN_ADDRESS.to_string());
    link.send(&main, &target, "status check".to_string(), 0)
        .await
        .unwrap();
    loop {
        let change = wait::guarded("the background turn to end", changes.recv())
            .await
            .unwrap();
        if matches!(change.kind, crate::hub::agent_watch::AgentChangeKind::TurnEnded(ref turn)
            if turn.visibility == crate::memory::types::Visibility::Background)
        {
            break;
        }
    }

    assert_eq!(
        phone.pushes_after_quiet().await,
        1,
        "only the reply nobody saw is a push"
    );
    let payload = phone.first_payload().await;
    assert_eq!(str_at(&payload, "event"), "reply_while_away");
    assert_eq!(str_at(&payload, "title"), "scout replied");
    assert_eq!(str_at(&payload, "body"), "Away reply");
    assert_eq!(str_at(&payload, "tag"), "reply:scout");
    assert_eq!(str_at(&payload, "target"), "/agent/scout");
}

// ─── Presence ─────────────────────────────────────────────────────────

#[tokio::test]
async fn a_device_in_front_of_its_user_hears_nothing_until_it_isnt() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut phone = Phone::register(&hub, "Phone", PushPreferencesPatch::default()).await;
    let other = Phone::register(&hub, "Other", PushPreferencesPatch::default()).await;
    let mut socket = open_hub_socket(&hub).await;

    // The window is visible and focused.
    report_presence(&mut socket, &phone.id, true).await;
    until_present(&hub, &phone.id, true).await;
    file_item(&hub, "scout", "20260930_first").await;
    other.until_pushes(1).await;
    assert_eq!(
        phone.pushes_after_quiet().await,
        0,
        "the device in front of its user is skipped, and the other one is not"
    );

    // The window loses focus.
    report_presence(&mut socket, &phone.id, false).await;
    until_present(&hub, &phone.id, false).await;
    file_item(&hub, "scout", "20260930_second").await;
    phone.until_pushes(1).await;

    // It is active again, so the next item skips it.
    report_presence(&mut socket, &phone.id, true).await;
    until_present(&hub, &phone.id, true).await;
    file_item(&hub, "scout", "20260930_third").await;
    other.until_pushes(3).await;
    assert_eq!(
        phone.pushes_after_quiet().await,
        1,
        "present again, so the third item is skipped"
    );

    // Its socket closes without another word.
    socket.close(None).await.unwrap();
    drop(socket);
    until_present(&hub, &phone.id, false).await;
    file_item(&hub, "scout", "20260930_fourth").await;
    phone.until_pushes(2).await;

    let payload = phone.first_payload().await;
    assert_eq!(
        str_at(&payload, "tag"),
        "inbox:scout:20260930_second",
        "the first push after the focus was lost"
    );
}

#[tokio::test]
async fn presence_that_cant_be_read_is_refused_like_any_other_message() {
    let hub = Fixture::new(&["scout"], "").await;
    let mut socket = open_hub_socket(&hub).await;
    frame_where(&mut socket, "agents_snapshot", |_| true).await;

    socket
        .send(WsMessage::text(
            json!({ "type": "presence", "device_id": "phone" }).to_string(),
        ))
        .await
        .unwrap();

    let notice = frame_where(&mut socket, "notice", |_| true).await;
    assert_eq!(str_at(&notice, "level"), "warn");
    assert!(hub.services.push.presence().active_devices().is_empty());
}

// ─── Delivery notices ─────────────────────────────────────────────────

#[tokio::test]
async fn a_device_that_stops_receiving_is_told_to_the_user() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let phone = Phone::register(&hub, "Work phone", PushPreferencesPatch::default()).await;
    phone.server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&phone.server)
        .await;

    file_item(&hub, "scout", "20260930_first").await;

    let notice = wait::until("the notice about the removed device", || async {
        hub.push_notices.lock().unwrap().first().cloned()
    })
    .await;
    assert!(
        notice.contains("Work phone no longer receives notifications"),
        "{notice}"
    );
    assert!(
        hub.services.push.devices().await.unwrap().is_empty(),
        "the device was removed"
    );
}
