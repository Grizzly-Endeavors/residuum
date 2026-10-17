//! The whole hub started through its runtime, with its agents started by the
//! gateway's launcher: the servers on their ports, the team event log, config
//! reloads, onboarding and team-file reloads, each checked against running
//! agents' `/api/status` and A2A cards.
//!
//! covers: residuum-hub, residuum-gateway

use std::sync::Arc;

use tokio::task::JoinHandle;
use wiremock::MockServer;

use crate::config::HubConfig;
use crate::hub::directory::AgentDirectory as _;
use crate::hub::host::AgentHost;
use crate::hub::runtime::{HubExit, HubInputs, HubRuntime};
use crate::hub::test_support::{mount_reply, write_agent};
use crate::hub::types::{HubEvent, NoticeLevel};
use residuum_core::testing::ports::reserve_port;

/// What the hub said about one config reload.
struct Reload {
    ok: bool,
    changed: bool,
    message: Option<String>,
    /// The notices sent before the `hub_config_reloaded` frame. The frame
    /// is the reload's report, so a notice here came from something else.
    notices: Vec<String>,
}

/// A hub started on temp directories with two agents, running on free
/// ports, with the model server all its agents talk to.
struct RunningHub {
    root: tempfile::TempDir,
    gateway_port: u16,
    a2a_port: u16,
    events: tokio::sync::broadcast::Receiver<HubEvent>,
    host: Arc<AgentHost>,
    push: Arc<crate::hub::push::PushService>,
    exit: JoinHandle<HubExit>,
    http: reqwest::Client,
    model: MockServer,
}

/// The value at the JSON pointer `path` of `value`, `null` when there is none.
fn at<'a>(value: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    value.pointer(path).unwrap_or(&serde_json::Value::Null)
}

/// The next frame of a hub socket, parsed.
async fn next_json(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> serde_json::Value {
    use futures_util::StreamExt as _;

    let message = residuum_core::testing::wait::guarded("a hub socket frame", socket.next())
        .await
        .expect("a frame arrives")
        .unwrap();
    serde_json::from_str(&message.into_text().unwrap()).unwrap()
}

impl RunningHub {
    async fn start() -> Self {
        Self::start_with(&["atlas", "scout"]).await
    }

    /// A hub over the agents `names`, which may be none.
    async fn start_with(names: &[&str]) -> Self {
        Self::start_after(names, None).await
    }

    /// A hub over the agents `names` that starts as if its live config
    /// had failed to load for `fallback_problem`.
    async fn start_after(names: &[&str], fallback_problem: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let (gateway_reservation, a2a_reservation) = (reserve_port(), reserve_port());
        let (gateway_port, a2a_port) = (gateway_reservation.port(), a2a_reservation.port());
        let hub_dir = root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(
            hub_dir.join("config.toml"),
            format!(
                "timezone = \"UTC\"\n[gateway]\nport = {gateway_port}\n[a2a]\nport = {a2a_port}\n"
            ),
        )
        .unwrap();
        let model = MockServer::start().await;
        mount_reply(&model, "hello").await;
        for name in names {
            write_agent(root.path(), name, &model.uri());
        }
        let hub_config = HubConfig::load_at(&hub_dir).unwrap();
        // The hub serves on the sockets the reservations hold, so no other
        // process can take either port before the hub starts.
        let gateway = gateway_reservation.into_tokio_listener();
        let a2a = a2a_reservation.into_tokio_listener();
        let inputs = HubInputs {
            root: root.path().to_path_buf(),
            build: residuum_core::testing::HARNESS_BUILD,
            launcher: Arc::new(crate::gateway::GatewayLauncher::new()),
            assets: crate::hub::test_support::fixture_assets,
            on_ready: Box::new(|dir| {
                std::fs::write(crate::config::HubPaths::new(dir).ready_file(), "ready").unwrap();
            }),
            log_filter: None,
        };
        let runtime = HubRuntime::start(
            inputs,
            hub_config,
            fallback_problem.map(str::to_string),
            gateway,
            Some(a2a),
        )
        .await
        .unwrap();
        let events = runtime.host.subscribe();
        let host = Arc::clone(&runtime.host);
        let push = Arc::clone(&runtime.services.push);
        Self {
            root,
            gateway_port,
            a2a_port,
            events,
            host,
            push,
            exit: crate::util::spawn_in_span(runtime.run(false)),
            http: reqwest::Client::new(),
            model,
        }
    }

    async fn status_of(&self, port: u16, path: &str) -> Option<u16> {
        self.http
            .get(format!("http://127.0.0.1:{port}{path}"))
            .send()
            .await
            .ok()
            .map(|response| response.status().as_u16())
    }

    async fn eventually_status(&self, port: u16, path: &str, expected: Option<u16>) {
        residuum_core::testing::wait::until(
            format!("{path} on port {port} to answer {expected:?}"),
            || async { (self.status_of(port, path).await == expected).then_some(()) },
        )
        .await;
    }

    /// The boot id of whatever hub answers on `port`, if one does.
    async fn boot_id_on(&self, port: u16) -> Option<String> {
        let page: serde_json::Value = self
            .http
            .get(format!("http://127.0.0.1:{port}/api/hub/events?limit=1"))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        page.pointer("/boot_id")?.as_str().map(str::to_string)
    }

    /// The hub's team event log over HTTP: its boot id and every entry,
    /// oldest first.
    async fn team_events(&self) -> (String, Vec<serde_json::Value>) {
        let page: serde_json::Value = self
            .http
            .get(format!(
                "http://127.0.0.1:{}/api/hub/events?limit=200",
                self.gateway_port
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut events = at(&page, "/events").as_array().unwrap().clone();
        events.reverse();
        (at(&page, "/boot_id").as_str().unwrap().to_string(), events)
    }

    /// Wait until the team event log holds an entry of `kind` about
    /// `agent`, and return it.
    async fn team_event(&self, kind: &str, agent: &str) -> serde_json::Value {
        residuum_core::testing::wait::until(format!("a {kind} entry about {agent}"), || async {
            let (_boot, events) = self.team_events().await;
            events
                .into_iter()
                .find(|event| at(event, "/kind") == kind && at(event, "/agent") == agent)
        })
        .await
    }

    fn hub_config_path(&self) -> std::path::PathBuf {
        self.root.path().join("hub").join("config.toml")
    }

    /// Wait for the hub to announce a finished config reload, and return
    /// its frame with the notices that came before it.
    async fn next_reload(&mut self) -> Reload {
        residuum_core::testing::wait::guarded("the hub to announce the reload", async {
            let mut notices = Vec::new();
            loop {
                match self.events.recv().await.unwrap() {
                    HubEvent::Notice { message, .. } => notices.push(message),
                    HubEvent::HubConfigReloaded {
                        ok,
                        changed,
                        message,
                    } => {
                        return Reload {
                            ok,
                            changed,
                            message,
                            notices,
                        };
                    }
                    HubEvent::AgentState { .. }
                    | HubEvent::AgentStopping { .. }
                    | HubEvent::AgentCreated { .. }
                    | HubEvent::AgentRestored { .. }
                    | HubEvent::AgentDeleted { .. }
                    | HubEvent::AgentActivity { .. } => {}
                }
            }
        })
        .await
    }

    async fn shut_down(self) {
        let response = self
            .http
            .post(format!(
                "http://127.0.0.1:{}/api/hub/shutdown",
                self.gateway_port
            ))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let exit = residuum_core::testing::wait::guarded(
            "the hub to stop after a shutdown request",
            self.exit,
        )
        .await
        .unwrap();
        assert!(matches!(exit, HubExit::Shutdown));
    }
}

#[tokio::test]
async fn the_hub_serves_its_agents_on_one_port_and_stops_them_on_shutdown() {
    let hub = RunningHub::start().await;

    for name in ["atlas", "scout"] {
        hub.eventually_status(
            hub.gateway_port,
            &format!("/api/agents/{name}/status"),
            Some(200),
        )
        .await;
        hub.eventually_status(
            hub.a2a_port,
            &format!("/agents/{name}/.well-known/agent-card.json"),
            Some(200),
        )
        .await;
    }
    assert!(
        hub.root.path().join("hub").join("residuum.ready").is_file(),
        "the readiness marker is written once the agents are up"
    );
    let gateway_port = hub.gateway_port;
    let http = hub.http.clone();

    hub.shut_down().await;

    assert!(
        http.get(format!(
            "http://127.0.0.1:{gateway_port}/api/agents/scout/status"
        ))
        .send()
        .await
        .is_err(),
        "the server is gone"
    );
}

#[tokio::test]
async fn the_hub_logs_its_start_and_its_agents_and_sends_new_entries_under_its_boot_id() {
    let hub = RunningHub::start().await;
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/api/hub/ws", hub.gateway_port))
            .await
            .unwrap();
    let boot = next_json(&mut socket).await;
    assert_eq!(at(&boot, "/type"), "hub_boot");

    for name in ["atlas", "scout"] {
        hub.team_event("agent_started", name).await;
    }
    let (boot_id, events) = hub.team_events().await;
    assert_eq!(
        at(&boot, "/boot_id"),
        boot_id.as_str(),
        "the endpoint and hub_boot agree"
    );
    let first = events.first().unwrap();
    assert_eq!(at(first, "/kind"), "hub_started");
    assert_eq!(at(first, "/id"), 1, "the hub's start begins the log");
    assert_eq!(at(first, "/summary"), "Residuum started");

    let response = hub
        .http
        .post(format!(
            "http://127.0.0.1:{}/api/hub/agents/scout/stop",
            hub.gateway_port
        ))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{response:?}");
    let frame = loop {
        let frame = next_json(&mut socket).await;
        if at(&frame, "/type") == "team_event" && at(&frame, "/event/kind") == "agent_stopped" {
            break frame;
        }
    };
    assert_eq!(
        at(&frame, "/boot_id"),
        at(&boot, "/boot_id"),
        "the frame agrees too"
    );
    assert_eq!(at(&frame, "/event/agent"), "scout");
    assert_eq!(at(&frame, "/event/summary"), "scout stopped");

    hub.shut_down().await;
}

#[tokio::test]
async fn a_notice_the_hub_publishes_while_starting_is_in_its_log() {
    let hub = RunningHub::start_after(&["scout"], Some("the hub config has a stray bracket")).await;

    let (_boot, events) = hub.team_events().await;

    let notice = events
        .iter()
        .find(|event| at(event, "/kind") == "hub_notice")
        .expect("the startup notice is logged");
    assert_eq!(at(notice, "/level"), "error");
    assert!(
        at(notice, "/summary")
            .as_str()
            .unwrap()
            .contains("the hub config has a stray bracket"),
        "{notice}"
    );
    assert_eq!(at(notice, "/agent"), &serde_json::Value::Null);
    assert_eq!(at(notice, "/target"), &serde_json::Value::Null);
    hub.shut_down().await;
}

#[tokio::test]
async fn changing_the_gateway_port_rebinds_the_server_and_keeps_the_agents_running() {
    let hub = RunningHub::start().await;
    hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
        .await;
    let (boot_id, _) = hub.team_events().await;
    let new_port = residuum_core::testing::ports::unprobed_port_after(hub.gateway_port);

    std::fs::write(
        hub.hub_config_path(),
        format!(
            "timezone = \"UTC\"\n[gateway]\nport = {new_port}\n[a2a]\nport = {}\n",
            hub.a2a_port
        ),
    )
    .unwrap();

    hub.eventually_status(new_port, "/api/agents/scout/status", Some(200))
        .await;
    hub.eventually_status(new_port, "/api/agents/atlas/status", Some(200))
        .await;
    // Another test process may bind the old port once this hub lets go of
    // it, so the check is that this hub no longer answers there, not that
    // nothing does.
    residuum_core::testing::wait::until(
        format!("this hub to stop answering on port {}", hub.gateway_port),
        || async {
            (hub.boot_id_on(hub.gateway_port).await.as_ref() != Some(&boot_id)).then_some(())
        },
    )
    .await;
    let mut hub = hub;
    hub.gateway_port = new_port;
    hub.shut_down().await;
}

#[tokio::test]
async fn moving_the_a2a_listener_onto_a_taken_port_warns_the_user() {
    let mut hub = RunningHub::start().await;
    hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
        .await;
    // The reservation holds the port, so the listener's bind fails.
    let taken = reserve_port();
    let taken_port = taken.port();
    std::fs::write(
        hub.hub_config_path(),
        format!(
            "timezone = \"UTC\"\n[gateway]\nport = {}\n[a2a]\nport = {taken_port}\n",
            hub.gateway_port
        ),
    )
    .unwrap();

    let notice = residuum_core::testing::wait::next_matching(
        "the hub to warn that the A2A listener couldn't start",
        &mut hub.events,
        |event| {
            matches!(
                event,
                HubEvent::Notice {
                    level: NoticeLevel::Warn,
                    message,
                    ..
                } if message.contains(&format!("start on port {taken_port}"))
            )
        },
    )
    .await;
    let HubEvent::Notice { message, .. } = notice else {
        unreachable!("the predicate only matches notices");
    };
    assert_eq!(
        message,
        format!(
            "The agent-to-agent listener couldn't start on port {taken_port}: the port is already in use. Choose another port in settings or stop the program using it."
        )
    );
    drop(taken);
    hub.shut_down().await;
}

#[tokio::test]
async fn a_broken_hub_config_reload_keeps_the_hub_running_and_says_so() {
    let mut hub = RunningHub::start().await;
    hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
        .await;

    std::fs::write(hub.hub_config_path(), "not valid toml [[[").unwrap();

    let reload = hub.next_reload().await;
    assert!(
        !reload.ok && !reload.changed,
        "a failed reload changes nothing"
    );
    let message = reload.message.expect("a failed reload says why");
    assert!(message.starts_with("hub config reload failed"), "{message}");
    assert!(message.contains("keeping current hub config"), "{message}");
    assert!(
        reload.notices.is_empty(),
        "the frame is the only report: {:?}",
        reload.notices
    );
    hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
        .await;
    hub.shut_down().await;
}

#[tokio::test]
async fn a_hub_config_reload_that_changes_a_setting_says_what_changed() {
    let mut hub = RunningHub::start().await;
    hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
        .await;

    let config = std::fs::read_to_string(hub.hub_config_path()).unwrap();
    std::fs::write(
        hub.hub_config_path(),
        config.replace("timezone = \"UTC\"", "timezone = \"America/Chicago\""),
    )
    .unwrap();

    let reload = hub.next_reload().await;
    assert!(reload.ok && reload.changed);
    assert_eq!(
        reload.message.as_deref(),
        Some("hub configuration reloaded: timezone")
    );
    assert!(
        reload.notices.is_empty(),
        "the frame is the only report: {:?}",
        reload.notices
    );
    hub.shut_down().await;
}

#[tokio::test]
async fn a_hub_config_reload_applies_a_changed_push_contact() {
    let mut hub = RunningHub::start().await;
    assert_eq!(hub.push.subject(), crate::hub::push::DEFAULT_CONTACT);

    let config = std::fs::read_to_string(hub.hub_config_path()).unwrap();
    std::fs::write(
        hub.hub_config_path(),
        format!("{config}\n[push]\ncontact = \"mailto:bear@example.com\"\n"),
    )
    .unwrap();
    let set = hub.next_reload().await;
    assert_eq!(
        set.message.as_deref(),
        Some("hub configuration reloaded: push")
    );
    assert_eq!(hub.push.subject(), "mailto:bear@example.com");

    std::fs::write(hub.hub_config_path(), config).unwrap();
    let cleared = hub.next_reload().await;
    assert_eq!(
        cleared.message.as_deref(),
        Some("hub configuration reloaded: push")
    );
    assert_eq!(hub.push.subject(), crate::hub::push::DEFAULT_CONTACT);
    hub.shut_down().await;
}

#[tokio::test]
async fn a_hub_config_reload_that_finds_nothing_new_says_so_in_its_frame() {
    let mut hub = RunningHub::start().await;
    hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
        .await;

    // A comment changes the file and not the settings in it.
    let config = std::fs::read_to_string(hub.hub_config_path()).unwrap();
    std::fs::write(hub.hub_config_path(), format!("# a note to self\n{config}")).unwrap();

    let reload = hub.next_reload().await;
    assert!(reload.ok, "the file loaded");
    assert!(!reload.changed, "and it matches what was running");
    assert_eq!(reload.message, None);
    assert!(reload.notices.is_empty(), "{:?}", reload.notices);
    hub.shut_down().await;
}

/// The body onboarding posts to create the first agent `name`, talking to
/// the hub's mock model.
fn setup_body(hub: &RunningHub, name: &str) -> serde_json::Value {
    serde_json::json!({
        "hub_config": format!(
            "timezone = \"UTC\"\n[gateway]\nport = {}\n[a2a]\nport = {}\n",
            hub.gateway_port, hub.a2a_port
        ),
        "agent_name": name,
        "user_name": "Sam",
        "config": "",
        "providers": format!(
            "[providers]\nmock = {{ type = \"openai\", api_key = \"test-key\", url = \"{}\" }}\n\n[models]\nmain = \"mock/test-model\"\n",
            hub.model.uri()
        ),
    })
}

#[tokio::test]
async fn onboarding_a_hub_without_agents_starts_the_first_agent_without_a_restart() {
    let hub = RunningHub::start_with(&[]).await;
    hub.eventually_status(hub.gateway_port, "/api/hub/agents", Some(200))
        .await;
    assert_eq!(
        hub.status_of(hub.gateway_port, "/api/agents/scout/status")
            .await,
        Some(404),
        "no agent exists before onboarding"
    );
    let complete_setup = format!(
        "http://127.0.0.1:{}/api/hub/config/complete-setup",
        hub.gateway_port
    );

    let response = hub
        .http
        .post(&complete_setup)
        .json(&setup_body(&hub, "scout"))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success(), "{response:?}");
    hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
        .await;
    hub.eventually_status(
        hub.a2a_port,
        "/agents/scout/.well-known/agent-card.json",
        Some(200),
    )
    .await;
    let again = hub
        .http
        .post(&complete_setup)
        .json(&setup_body(&hub, "atlas"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        again.status().as_u16(),
        409,
        "onboarding only creates the first agent"
    );
    hub.shut_down().await;
}

#[tokio::test]
async fn editing_a_team_identity_file_reloads_every_running_agent() {
    let hub = RunningHub::start().await;
    for name in ["atlas", "scout"] {
        hub.eventually_status(
            hub.gateway_port,
            &format!("/api/agents/{name}/status"),
            Some(200),
        )
        .await;
    }
    let reloads_before = ["atlas", "scout"].map(|name| hub.host.reloads_finished(name).unwrap());

    let response = hub
        .http
        .put(format!(
            "http://127.0.0.1:{}/api/team/workspace/file",
            hub.gateway_port
        ))
        .json(&serde_json::json!({ "path": "USER.md", "content": "Sam prefers short replies." }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success(), "{response:?}");
    for (name, before) in ["atlas", "scout"].into_iter().zip(reloads_before) {
        residuum_core::testing::wait::until(
            format!("{name} to reload after the team USER.md changed"),
            || async { (hub.host.reloads_finished(name).unwrap() > before).then_some(()) },
        )
        .await;
    }
    hub.shut_down().await;
}
