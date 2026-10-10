//! The whole remote access path, end to end: the real manager and engine behind
//! the real tunnel client, a fake relay speaking tunnel v2 with a TCP front
//! door, a fake pin service and (for the issuance tests) a Pebble ACME CA.
//!
//! The Pebble tests need Docker and are `#[ignore]`d; run them with
//! `cargo test --quiet system -- --ignored`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::routing::get;
use chrono::Utc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

use super::engine::EngineRouters;
use super::fake_pin_service::{FakePinService, TEST_GRANT};
use super::manager::{RemoteAccess, RemoteAccessInputs};
use super::pebble_support::PebbleHarness;
use super::slot::RemoteAccessSlot;
use super::status::{RemoteAccessState, RemoteAccessStatus};
use super::tls::CertBundle;
use crate::config::{CloudConfig, RemoteAccessSettings};
use crate::pairing::DevicePairing;
use crate::tunnel::TunnelStatus;
use crate::tunnel::v2::SessionHandler;
use crate::tunnel::v2::frames::V2Frame;
use crate::tunnel::v2::test_relay::{FakeRelay, FakeRelayConfig, V2Mode, client_hello};

const BASE: &str = "relay.test";
const UI: &str = "bear.relay.test";
const WORKBENCH: &str = "bear.workbench.relay.test";
const INSTANCE: &str = "laptop.bear.relay.test";

/// Everything one instance keeps on disk, which survives a restart.
struct Env {
    hub: tempfile::TempDir,
    settings: RemoteAccessSettings,
}

impl Env {
    /// An install whose CA, pin service and resolver are nowhere: whatever
    /// tries to reach them fails at once.
    fn offline() -> Self {
        Self {
            hub: tempfile::tempdir().unwrap(),
            settings: RemoteAccessSettings {
                base_domain: BASE.to_string(),
                acme_directory: "https://127.0.0.1:1/dir".to_string(),
                pin_service_url: "http://127.0.0.1:1".to_string(),
                caa_resolver: "127.0.0.1:1".parse().unwrap(),
                ..RemoteAccessSettings::default()
            },
        }
    }

    /// An install that talks to Pebble and the fake pin service.
    fn pebble(harness: &PebbleHarness, pins: &FakePinService) -> Self {
        Self {
            hub: tempfile::tempdir().unwrap(),
            settings: RemoteAccessSettings {
                base_domain: BASE.to_string(),
                acme_directory: harness.directory_url(),
                acme_root_ca: Some(harness.root_ca_pem_path().to_path_buf()),
                pin_service_url: pins.url().to_string(),
                caa_resolver: harness.dns_addr(),
            },
        }
    }

    fn state_dir(&self) -> std::path::PathBuf {
        self.hub.path().join("remote-access")
    }

    /// Store the identity an earlier enrollment would have.
    fn seed_identity(&self) {
        std::fs::create_dir_all(self.state_dir()).unwrap();
        std::fs::write(
            self.state_dir().join("state.json"),
            r#"{"identity":{"user":"bear","slug":"laptop"}}"#,
        )
        .unwrap();
    }

    /// Store a self-signed certificate for the three names.
    async fn seed_certificate(&self) {
        let names: Vec<String> = [UI, WORKBENCH, INSTANCE].map(String::from).to_vec();
        let key = rcgen::generate_simple_self_signed(names.clone()).unwrap();
        let bundle = CertBundle {
            chain_pem: key.cert.pem(),
            key_pem: key.signing_key.serialize_pem(),
            not_before: Utc::now() - chrono::Duration::days(1),
            not_after: Utc::now() + chrono::Duration::days(60),
            names,
        };
        std::fs::create_dir_all(self.state_dir()).unwrap();
        super::acme::CertStore::new(&self.state_dir(), &self.settings.acme_directory)
            .save(&bundle)
            .await
            .unwrap();
    }
}

/// A running manager behind a tunnel client.
struct Stack {
    remote: RemoteAccess,
    slot: RemoteAccessSlot,
    pairing: DevicePairing,
    notices: Arc<Mutex<Vec<String>>>,
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl Stack {
    fn start(env: &Env, relay: &FakeRelay) -> Self {
        let slot = RemoteAccessSlot::new(env.hub.path());
        let pairing = DevicePairing::open(env.hub.path());
        let (tunnel_status, _status_rx) = watch::channel(TunnelStatus::Disconnected);
        let tunnel_status = Arc::new(tunnel_status);
        let notices = Arc::new(Mutex::new(Vec::new()));
        let (teams_tx, teams_rx) = watch::channel(BTreeMap::new());
        let sink = Arc::clone(&notices);
        let routers = EngineRouters {
            ui: Router::new().route("/hello", get(|| async { "ui-ok" })),
            workbench: Router::new().route("/hello", get(|| async { "workbench-ok" })),
        };
        let remote = RemoteAccess::new(RemoteAccessInputs {
            settings: env.settings.clone(),
            hub_dir: env.hub.path().to_path_buf(),
            routers,
            a2a_port: None,
            teams_ports: teams_rx.clone(),
            pairing: pairing.clone(),
            tunnel_status: Arc::clone(&tunnel_status),
            status: slot.status_sender(),
            notify: Arc::new(move |message| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(message);
            }),
            siblings: slot.sibling_keys(),
            discovery: slot.discovery_sender(),
            sibling_channel: None,
        })
        .unwrap();
        remote.start_background();
        slot.install(remote.clone());
        let cfg = CloudConfig {
            relay_url: relay.ws_url(),
            token: "rst_test".to_string(),
            remote: env.settings.clone(),
        };
        let (shutdown, shutdown_rx) = watch::channel(false);
        let (agents_tx, agents_rx) = watch::channel(Vec::new());
        let handler: Arc<dyn SessionHandler> = Arc::new(remote.clone());
        let task = tokio::spawn(async move {
            crate::tunnel::start_tunnel(cfg, agents_rx, shutdown_rx, tunnel_status, handler).await;
            drop((teams_tx, agents_tx));
        });
        Self {
            remote,
            slot,
            pairing,
            notices,
            shutdown,
            task,
        }
    }

    async fn stop(self) {
        self.shutdown.send_replace(true);
        crate::testing::wait::guarded("the tunnel to stop", self.task)
            .await
            .unwrap();
    }

    fn status(&self) -> RemoteAccessStatus {
        self.slot.status(true)
    }

    async fn wait_for(
        &self,
        what: &str,
        matches: impl Fn(&RemoteAccessStatus) -> bool,
    ) -> RemoteAccessStatus {
        crate::testing::wait::until_reporting(
            what,
            || {
                let status = self.status();
                std::future::ready(matches(&status).then_some(status))
            },
            || format!("last status: {:?}", self.status()),
        )
        .await
    }

    async fn wait_for_state(&self, state: RemoteAccessState) -> RemoteAccessStatus {
        self.wait_for(&format!("{state:?}"), |status| status.state == state)
            .await
    }

    fn notices(&self) -> Vec<String> {
        self.notices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[derive(Debug)]
struct AcceptAnyCertificate;

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, tokio_rustls::rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        tokio_rustls::rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// What a browser sees: the answer and the certificate it was served under.
struct Fetched {
    response: String,
    certificate: Vec<u8>,
}

/// A browser's request to the relay's front door: TLS to `sni`, then HTTP
/// with `host` as the Host header.
async fn fetch(door_port: u16, sni: &str, host: &str, path: &str) -> Fetched {
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let tcp = TcpStream::connect(("127.0.0.1", door_port)).await.unwrap();
    let mut tls = TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from(sni.to_string()).unwrap(), tcp)
        .await
        .expect("the TLS handshake");
    let certificate = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|der| der.as_ref().to_vec())
        .unwrap_or_default();
    tls.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = Vec::new();
    crate::testing::wait::guarded("the answer to arrive", tls.read_to_end(&mut raw))
        .await
        .ok();
    Fetched {
        response: String::from_utf8_lossy(&raw).into_owned(),
        certificate,
    }
}

fn relay_config() -> FakeRelayConfig {
    FakeRelayConfig {
        grant: Ok(TEST_GRANT.to_string()),
        ..FakeRelayConfig::default()
    }
}

fn names_in(certificate: &[u8]) -> Vec<String> {
    let (_, parsed) = x509_parser::parse_x509_certificate(certificate).unwrap();
    let mut names: Vec<String> = parsed
        .subject_alternative_name()
        .unwrap()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|name| {
                    if let x509_parser::extensions::GeneralName::DNSName(dns) = name {
                        Some((*dns).to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

// ── Without a CA ─────────────────────────────────────────────────────

#[tokio::test]
async fn an_enrolled_install_serves_each_host_and_answers_421_for_others() {
    let env = Env::offline();
    env.seed_identity();
    env.seed_certificate().await;
    let relay = FakeRelay::start(relay_config()).await;
    let stack = Stack::start(&env, &relay);
    relay
        .wait_frame(|f| matches!(f, V2Frame::AgentsUpdate { .. }))
        .await;

    let ui = fetch(relay.front_door_port(), UI, UI, "/hello").await;
    assert!(ui.response.starts_with("HTTP/1.1 200"), "{}", ui.response);
    assert!(ui.response.ends_with("ui-ok"), "{}", ui.response);
    let workbench = fetch(relay.front_door_port(), WORKBENCH, WORKBENCH, "/hello").await;
    assert!(
        workbench.response.ends_with("workbench-ok"),
        "{}",
        workbench.response
    );
    assert_eq!(names_in(&ui.certificate), [UI, WORKBENCH, INSTANCE]);

    let unknown = fetch(relay.front_door_port(), UI, "other.relay.test", "/hello").await;
    assert!(
        unknown.response.starts_with("HTTP/1.1 421"),
        "{}",
        unknown.response
    );
    stack.stop().await;
}

#[tokio::test]
async fn a_stream_whose_sni_differs_from_the_announced_host_is_closed() {
    let env = Env::offline();
    env.seed_identity();
    env.seed_certificate().await;
    let relay = FakeRelay::start(relay_config()).await;
    let stack = Stack::start(&env, &relay);
    relay
        .wait_frame(|f| matches!(f, V2Frame::AgentsUpdate { .. }))
        .await;

    let id = relay.open_raw_stream(UI, "203.0.113.5");
    relay.send_stream_data(id, &client_hello(WORKBENCH));
    relay
        .wait_frame(|f| matches!(f, V2Frame::StreamClose { stream_id, .. } if *stream_id == id))
        .await;
    stack.stop().await;
}

#[tokio::test]
async fn a_connected_frame_for_a_different_instance_is_refused() {
    let env = Env::offline();
    env.seed_identity();
    env.seed_certificate().await;
    let relay = FakeRelay::start(FakeRelayConfig {
        instance: "desktop".to_string(),
        hosts: super::types::Hostnames::derive("bear", "desktop", BASE),
        ..relay_config()
    })
    .await;
    let stack = Stack::start(&env, &relay);
    let status = stack.wait_for_state(RemoteAccessState::Refused).await;
    assert!(
        status
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("different identity"),
        "{status:?}"
    );
    assert!(
        stack
            .notices()
            .iter()
            .any(|n| n.contains("different identity"))
    );
    stack.stop().await;
}

#[tokio::test]
async fn announced_hosts_that_differ_from_the_derived_ones_are_refused() {
    let env = Env::offline();
    let mut hosts = super::types::Hostnames::derive("bear", "laptop", BASE);
    hosts.ui = "bear.evil.example".to_string();
    let relay = FakeRelay::start(FakeRelayConfig {
        hosts,
        ..relay_config()
    })
    .await;
    let stack = Stack::start(&env, &relay);
    stack.wait_for_state(RemoteAccessState::Refused).await;
    stack.stop().await;
}

#[tokio::test]
async fn a_relay_without_the_secure_tunnel_leaves_remote_access_down_and_retrying() {
    for code in [404, 426] {
        let env = Env::offline();
        let relay = FakeRelay::start(FakeRelayConfig {
            v2: V2Mode::Respond(code),
            ..relay_config()
        })
        .await;
        let stack = Stack::start(&env, &relay);
        let status = stack.wait_for_state(RemoteAccessState::Error).await;
        assert!(
            status
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("doesn't offer the secure tunnel"),
            "{status:?}"
        );
        stack.stop().await;
    }
}

#[tokio::test]
async fn the_stored_identity_replaces_what_the_relay_announces_for_pairing() {
    let env = Env::offline();
    env.seed_identity();
    env.seed_certificate().await;
    let relay = FakeRelay::start(relay_config()).await;
    let stack = Stack::start(&env, &relay);
    relay
        .wait_frame(|f| matches!(f, V2Frame::AgentsUpdate { .. }))
        .await;
    stack
        .wait_for("the pairing address", |_| {
            stack.pairing.identity().ui_origin.is_some()
        })
        .await;
    let identity = stack.pairing.identity();
    assert_eq!(identity.slug.as_deref(), Some("laptop"));
    assert_eq!(
        identity.ui_origin.as_deref(),
        Some("https://bear.relay.test")
    );
    assert_eq!(
        identity.workbench_origin.as_deref(),
        Some("https://bear.workbench.relay.test")
    );
    let _ = &stack.remote;
    stack.stop().await;
}

// ── With Pebble ──────────────────────────────────────────────────────

async fn pebble_world() -> (Arc<PebbleHarness>, FakePinService, FakeRelay) {
    let harness = Arc::new(PebbleHarness::start().await.expect("Pebble starts"));
    for name in [UI, WORKBENCH, INSTANCE] {
        harness.add_a(name).await.unwrap();
    }
    let pins = FakePinService::start(Some(Arc::clone(&harness))).await;
    harness.release_tls_port();
    let relay = FakeRelay::start(FakeRelayConfig {
        door_port: Some(harness.tls_port()),
        ..relay_config()
    })
    .await;
    (harness, pins, relay)
}

#[tokio::test]
#[ignore = "needs docker: runs Pebble"]
async fn pebble_enrolls_gets_a_certificate_through_the_tunnel_and_alerts_on_an_unknown_pin() {
    let (harness, pins, relay) = pebble_world().await;
    let env = Env::pebble(&harness, &pins);
    let stack = Stack::start(&env, &relay);
    let status = stack.wait_for_state(RemoteAccessState::Ready).await;

    // Enrolled: the pin, the identity, the recovery code to save.
    assert_eq!(pins.operations(), ["enroll"]);
    assert_eq!(pins.pins().len(), 1);
    assert_eq!(status.user.as_deref(), Some("bear"));
    assert!(status.recovery_code_pending);
    assert_eq!(status.recovery_code.as_deref().map(str::len), Some(20));
    relay
        .wait_frame(|f| matches!(f, V2Frame::ChallengeClaim { .. }))
        .await;
    relay
        .wait_frame(|f| matches!(f, V2Frame::ChallengeRelease { .. }))
        .await;

    // The certificate was issued by the CA to the account, and serves.
    let ui = fetch(relay.front_door_port(), UI, UI, "/hello").await;
    assert!(ui.response.ends_with("ui-ok"), "{}", ui.response);
    assert_eq!(names_in(&ui.certificate), [UI, WORKBENCH, INSTANCE]);
    let (_, parsed) = x509_parser::parse_x509_certificate(&ui.certificate).unwrap();
    assert!(
        parsed.issuer().to_string().contains("Pebble"),
        "{}",
        parsed.issuer()
    );
    let unknown = fetch(relay.front_door_port(), UI, "other.relay.test", "/").await;
    assert!(unknown.response.starts_with("HTTP/1.1 421"));

    // Saving the recovery code makes Residuum forget it.
    stack.remote.acknowledge_recovery_code().await.unwrap();
    assert!(!stack.status().recovery_code_pending);

    // A certificate account nobody here approved is reported.
    pins.preload("https://acme.test/acct/stranger", "stranger");
    stack.remote.retry_now();
    let alerted = stack
        .wait_for("the unknown pin", |s| s.pins.iter().any(|p| !p.known))
        .await;
    assert_eq!(alerted.unknown_pins().len(), 1);
    assert!(stack.notices().iter().any(|n| n.contains("stranger")));
    stack.stop().await;
}

#[tokio::test]
#[ignore = "needs docker: runs Pebble"]
async fn pebble_renews_a_due_certificate_through_the_tunnel() {
    let (harness, pins, relay) = pebble_world().await;
    let env = Env::pebble(&harness, &pins);
    let first = Stack::start(&env, &relay);
    first.wait_for_state(RemoteAccessState::Ready).await;
    let before = fetch(relay.front_door_port(), UI, UI, "/hello")
        .await
        .certificate;
    first.stop().await;

    // Age the stored certificate until its renewal is due.
    let store = super::acme::CertStore::new(&env.state_dir(), &env.settings.acme_directory);
    let mut bundle = store.load().expect("a stored certificate");
    bundle.not_before = Utc::now() - chrono::Duration::days(3000);
    store.save(&bundle).await.unwrap();
    assert!(bundle.renew_at() <= Utc::now());

    let second = Stack::start(&env, &relay);
    second
        .wait_for("a renewed certificate", |s| {
            s.certificate.as_ref().is_some_and(|c| {
                chrono::DateTime::parse_from_rfc3339(&c.renews_at).is_ok_and(|at| at > Utc::now())
            })
        })
        .await;
    let after = fetch(relay.front_door_port(), UI, UI, "/hello")
        .await
        .certificate;
    assert_ne!(before, after, "the renewal installed a new certificate");
    assert_eq!(
        pins.operations(),
        ["enroll"],
        "renewal needs no pin service call"
    );
    second.stop().await;
}

#[tokio::test]
#[ignore = "needs docker: runs Pebble"]
async fn pebble_a_second_install_needs_a_join_and_a_recovery_code_resets_the_pins() {
    let (harness, pins, relay) = pebble_world().await;
    let first_env = Env::pebble(&harness, &pins);
    let first = Stack::start(&first_env, &relay);
    let ready = first.wait_for_state(RemoteAccessState::Ready).await;
    let recovery_code = ready.recovery_code.expect("the recovery code");
    first.stop().await;

    // A new install of the same user: pins exist, so it serves nothing.
    let second_env = Env::pebble(&harness, &pins);
    let second = Stack::start(&second_env, &relay);
    second.wait_for_state(RemoteAccessState::NeedsJoin).await;
    assert_eq!(pins.operations(), ["enroll"]);

    // The recovery code takes the address back for this install.
    let wrong = second.remote.reset_pins("AAAAAAAAAAAAAAAAAAAA").await;
    assert!(wrong.is_err(), "a wrong code is refused");
    second.remote.reset_pins(&recovery_code).await.unwrap();
    assert_eq!(pins.operations(), ["enroll", "reset"]);
    second.wait_for_state(RemoteAccessState::Ready).await;
    let fetched = fetch(relay.front_door_port(), UI, UI, "/hello").await;
    assert!(fetched.response.ends_with("ui-ok"), "{}", fetched.response);
    second.stop().await;
}

/// What the install stored about its pending recovery code.
fn stored_recovery_code(env: &Env) -> Option<String> {
    let text = std::fs::read_to_string(env.state_dir().join("state.json")).ok()?;
    let state: serde_json::Value = serde_json::from_str(&text).ok()?;
    state
        .get("pending_recovery_code")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
#[ignore = "needs docker: runs Pebble"]
async fn pebble_an_email_reset_keeps_its_recovery_code_until_the_reset_takes_effect() {
    let (harness, pins, relay) = pebble_world().await;
    let first_env = Env::pebble(&harness, &pins);
    let first = Stack::start(&first_env, &relay);
    first.wait_for_state(RemoteAccessState::Ready).await;
    first.stop().await;

    let second_env = Env::pebble(&harness, &pins);
    let second = Stack::start(&second_env, &relay);
    second.wait_for_state(RemoteAccessState::NeedsJoin).await;

    let masked = second.remote.email_reset().await.unwrap();
    assert_eq!(masked, "b***@example.test");
    assert_eq!(pins.operations(), ["enroll", "email_reset"]);
    let waiting = second
        .wait_for("the pending reset", |s| s.pending_reset.is_some())
        .await;
    let reset = waiting.pending_reset.unwrap();
    assert!(reset.own && !reset.confirmed && !reset.cancellable);
    assert!(
        waiting.recovery_code.is_none() && !waiting.recovery_code_pending,
        "the new code isn't offered before the reset takes effect"
    );
    let kept = stored_recovery_code(&second_env).expect("the new code is stored");

    // A pass through the needs-join path doesn't discard it.
    let checked_before = second.status().checked_at;
    second.remote.retry_now();
    second
        .wait_for("a check after the retry", |status| {
            status.checked_at != checked_before
        })
        .await;
    assert_eq!(second.status().state, RemoteAccessState::NeedsJoin);
    assert_eq!(stored_recovery_code(&second_env).as_ref(), Some(&kept));

    // Confirmed, then the hold ends.
    let effective_at = Utc::now().timestamp() + 3600;
    pins.confirm_email_reset(effective_at);
    pins.apply_email_reset().await;
    second.remote.retry_now();
    let ready = second.wait_for_state(RemoteAccessState::Ready).await;
    assert_eq!(ready.recovery_code.as_deref(), Some(kept.as_str()));
    assert!(ready.recovery_code_pending);
    assert!(ready.pending_reset.is_none());
    assert_eq!(
        pins.recovery_hash().as_deref(),
        Some(super::pins::recovery_code_hash(&kept).as_str())
    );
    assert_eq!(pins.pins().len(), 1);
    second.stop().await;
}

#[tokio::test]
#[ignore = "needs docker: runs Pebble"]
async fn pebble_a_failed_email_reset_request_forgets_the_new_code() {
    let (harness, pins, relay) = pebble_world().await;
    let first_env = Env::pebble(&harness, &pins);
    let first = Stack::start(&first_env, &relay);
    first.wait_for_state(RemoteAccessState::Ready).await;
    first.stop().await;

    let second_env = Env::pebble(&harness, &pins);
    let second = Stack::start(&second_env, &relay);
    second.wait_for_state(RemoteAccessState::NeedsJoin).await;

    pins.fail_next_email_reset(axum::http::StatusCode::BAD_GATEWAY);
    let refused = second.remote.email_reset().await.unwrap_err();
    assert!(
        refused.to_string().contains("couldn't be sent"),
        "{refused}"
    );
    assert!(stored_recovery_code(&second_env).is_none());
    assert!(!second.status().recovery_code_pending);
    assert!(!pins.has_pending_reset());
    assert_eq!(pins.operations(), ["enroll"]);
    second.stop().await;
}

#[tokio::test]
#[ignore = "needs docker: runs Pebble"]
async fn pebble_a_pinned_instance_alerts_on_another_instances_reset_and_cancels_it() {
    let (harness, pins, relay) = pebble_world().await;
    let env = Env::pebble(&harness, &pins);
    let stack = Stack::start(&env, &relay);
    stack.wait_for_state(RemoteAccessState::Ready).await;
    let own_account = pins.pins().first().map(|(uri, _)| uri.clone()).unwrap();

    let effective_at = Utc::now().timestamp() + 86_400;
    pins.preload_pending_reset(
        "https://acme.test/acct/stranger",
        "stranger",
        Some(effective_at),
    );
    stack.remote.retry_now();
    let alerted = stack
        .wait_for("the pending reset", |s| s.pending_reset.is_some())
        .await;
    let reset = alerted.pending_reset.unwrap();
    assert!(reset.cancellable && !reset.own && reset.confirmed);
    assert_eq!(reset.slug, "stranger");
    assert!(
        stack
            .notices()
            .iter()
            .any(|n| n.contains("stranger") && n.contains("UTC")),
        "{:?}",
        stack.notices()
    );

    stack.slot.cancel_reset().await.unwrap();
    assert_eq!(pins.operations(), ["enroll", "cancel_reset"]);
    assert_eq!(pins.cancelled_by().as_deref(), Some(own_account.as_str()));
    assert!(!pins.has_pending_reset());
    assert!(stack.status().pending_reset.is_none());
    stack.stop().await;
}
