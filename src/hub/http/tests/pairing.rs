//! Device pairing end to end through the hub router: what a remote request
//! needs, each way a browser gets paired, and what a paired one still can't do.

use axum::http::request::Builder;

use super::*;
use crate::workbench::forward::HubApi;

const UI_ORIGIN: &str = "https://bear.agent-residuum.com";
const WORKBENCH_ORIGIN: &str = "https://bear.workbench.agent-residuum.com";
const PEER: &str = "203.0.113.7";

async fn announce(h: &Harness) {
    h.pairing
        .update_identity(crate::pairing::Identity {
            slug: Some("laptop".to_string()),
            ui_origin: Some(UI_ORIGIN.to_string()),
            workbench_origin: Some(WORKBENCH_ORIGIN.to_string()),
        })
        .await
        .unwrap();
}

/// A request as the relay tunnel delivers a browser's: marked with the
/// process's tunnel nonce, with the peer address the relay passes along and the
/// headers a same-origin fetch carries.
fn remote(method: Method, uri: &str) -> Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(TUNNEL_NONCE_HEADER, tunnel_nonce())
        .header("x-real-ip", PEER)
        .header("sec-fetch-site", "same-origin")
}

/// `builder` with header `name` set to `value`, replacing any the builder
/// already carries (`header` would add a second value after the first).
fn set(mut builder: Builder, name: &'static str, value: &str) -> Builder {
    if let Some(headers) = builder.headers_mut() {
        headers.insert(name, axum::http::HeaderValue::from_str(value).unwrap());
    }
    builder
}

fn authed(method: Method, uri: &str, cookie: &str) -> Builder {
    remote(method, uri).header("cookie", cookie)
}

fn empty(builder: Builder) -> Request<Body> {
    builder.body(Body::empty()).unwrap()
}

fn json_request(builder: Builder, payload: &Value) -> Request<Body> {
    builder
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap()
}

/// The `name=value` the response sets as the device cookie.
fn cookie_pair(headers: &HeaderMap) -> String {
    let set_cookie = headers
        .get("set-cookie")
        .expect("the response should set the device cookie")
        .to_str()
        .unwrap();
    set_cookie.split(';').next().unwrap().to_string()
}

async fn json_of(h: &Harness, request: Request<Body>) -> Value {
    let (_, _, bytes) = h.send(request).await;
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn local_pair_link(h: &Harness) -> Value {
    h.call(Method::POST, "/api/hub/remote-access/pair-link", None)
        .await
        .1
}

fn token_of(link: &Value) -> String {
    link["link"]
        .as_str()
        .unwrap()
        .split("#token=")
        .nth(1)
        .unwrap()
        .to_string()
}

fn redeem_request(token: &str, device_name: &str) -> Request<Body> {
    json_request(
        remote(Method::POST, "/api/hub/pairing/redeem"),
        &json!({ "token": token, "device_name": device_name }),
    )
}

/// Pair a first browser the way a person does: a link made locally, redeemed
/// remotely. Returns its `name=value` cookie.
async fn pair_first_device(h: &Harness, device_name: &str) -> String {
    announce(h).await;
    let minted = local_pair_link(h).await;
    let (outcome, headers, _) = h
        .send(redeem_request(&token_of(&minted), device_name))
        .await;
    assert_eq!(outcome, StatusCode::OK, "redeeming the first link");
    cookie_pair(&headers)
}

async fn get_status(h: &Harness, path: &str, cookie: &str) -> StatusCode {
    h.status(empty(authed(Method::GET, path, cookie))).await
}

async fn devices_of(h: &Harness, cookie: &str) -> Value {
    json_of(h, empty(authed(Method::GET, "/api/hub/devices", cookie))).await
}

#[tokio::test]
async fn a_tunneled_request_without_a_credential_is_refused_and_a_local_one_is_not() {
    let h = Harness::new();
    announce(&h).await;
    let refused = json_of(&h, empty(remote(Method::GET, "/api/hub/agents"))).await;
    assert_eq!(refused["code"], "device_required");
    assert_eq!(
        h.status(empty(remote(Method::GET, "/api/hub/agents")))
            .await,
        StatusCode::UNAUTHORIZED
    );

    h.get_expect("/api/hub/agents", StatusCode::OK).await;
}

#[tokio::test]
async fn a_forged_tunnel_marker_does_not_make_a_request_remote() {
    let h = Harness::new();
    let request = Request::get("/api/hub/agents")
        .header(TUNNEL_NONCE_HEADER, "not-the-nonce")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.status(request).await, StatusCode::OK);
}

#[tokio::test]
async fn an_unpaired_navigation_is_redirected_to_the_pairing_page() {
    let h = Harness::new();
    for path in ["/", "/agent/scout", "/team/workbench"] {
        let navigation = set(
            set(remote(Method::GET, path), "sec-fetch-mode", "navigate"),
            "accept",
            "text/html",
        );
        let (outcome, headers, _) = h.send(empty(navigation)).await;
        assert!(outcome.is_redirection(), "{path} gave {outcome}");
        assert_eq!(headers.get("location").unwrap(), "/pair", "{path}");
    }
}

#[tokio::test]
async fn the_pairing_page_and_its_assets_load_before_pairing() {
    let h = Harness::new();
    let page = set(remote(Method::GET, "/pair"), "sec-fetch-mode", "navigate");
    let (outcome, _, bytes) = h.send(empty(page)).await;
    assert_eq!(outcome, StatusCode::OK);
    assert!(
        String::from_utf8_lossy(&bytes).contains("id=\"app\""),
        "the pairing page is the web app's shell"
    );
}

#[tokio::test]
async fn only_the_pairing_surface_answers_before_pairing() {
    let h = Harness::new();
    for (method, path) in [
        (Method::GET, "/api/hub/agents"),
        (Method::GET, "/api/hub/status"),
        (Method::GET, "/api/hub/devices"),
        (Method::POST, "/api/hub/devices/recovery-codes"),
        (Method::GET, "/sw.js"),
        (Method::GET, "/mcp-catalog.json"),
        (Method::POST, "/api/hub/pairing/handoff"),
        (Method::GET, "/webhook/scout/x"),
    ] {
        let outcome = h.status(empty(remote(method.clone(), path))).await;
        assert_eq!(outcome, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn an_unpaired_websocket_upgrade_is_refused() {
    let h = Harness::new();
    let upgrade = remote(Method::GET, "/api/hub/ws")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    assert_eq!(h.status(empty(upgrade)).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_first_device_pairs_with_a_local_link_and_then_works() {
    let h = Harness::new();
    let cookie = pair_first_device(&h, "Laptop").await;
    assert!(
        cookie.starts_with("__Host-residuum_device_laptop="),
        "{cookie}"
    );
    assert_eq!(
        get_status(&h, "/api/hub/agents", &cookie).await,
        StatusCode::OK
    );

    let listing = devices_of(&h, &cookie).await;
    assert_eq!(listing["devices"][0]["name"], "Laptop");
    assert_eq!(listing["devices"][0]["current"], true);
    assert_eq!(listing["remote"], true);
}

#[tokio::test]
async fn the_cookie_is_host_only_http_only_secure_and_lax() {
    let h = Harness::new();
    announce(&h).await;
    let minted = local_pair_link(&h).await;
    let (_, headers, _) = h.send(redeem_request(&token_of(&minted), "Laptop")).await;
    let line = headers.get("set-cookie").unwrap().to_str().unwrap();
    for part in [
        "Max-Age=34560000",
        "Path=/",
        "Secure",
        "HttpOnly",
        "SameSite=Lax",
    ] {
        assert!(line.contains(part), "{part} missing from {line}");
    }
    assert!(!line.contains("Domain"), "{line}");
}

#[tokio::test]
async fn a_pairing_link_needs_the_announced_address() {
    let h = Harness::new();
    let (outcome, reply) = h
        .call(Method::POST, "/api/hub/remote-access/pair-link", None)
        .await;
    assert_eq!(outcome, StatusCode::CONFLICT, "{reply}");
}

#[tokio::test]
async fn a_pairing_link_cannot_be_made_remotely_even_by_a_paired_device() {
    let h = Harness::new();
    let cookie = pair_first_device(&h, "Laptop").await;
    let attempt = authed(Method::POST, "/api/hub/remote-access/pair-link", &cookie);
    assert_eq!(h.status(empty(attempt)).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_first_link_shows_recovery_codes_once() {
    let h = Harness::new();
    announce(&h).await;
    let first = local_pair_link(&h).await;
    assert_eq!(first["recovery_codes"].as_array().unwrap().len(), 10);
    assert!(first["qr_svg"].as_str().unwrap().contains("<svg"));
    assert!(
        first["link"]
            .as_str()
            .unwrap()
            .starts_with("https://bear.agent-residuum.com/pair#token=")
    );
    let second = local_pair_link(&h).await;
    assert!(
        second["recovery_codes"].is_null(),
        "the codes are shown once"
    );
}

#[tokio::test]
async fn a_used_token_fails() {
    let h = Harness::new();
    announce(&h).await;
    let token = token_of(&local_pair_link(&h).await);
    assert_eq!(
        h.status(redeem_request(&token, "Phone")).await,
        StatusCode::OK
    );
    assert_eq!(
        h.status(redeem_request(&token, "Phone")).await,
        StatusCode::BAD_REQUEST,
        "a token works once"
    );
}

#[tokio::test]
async fn an_expired_token_fails() {
    let h = Harness::new();
    announce(&h).await;
    let token = token_of(&local_pair_link(&h).await);
    h.pairing.advance_clock(chrono::Duration::minutes(11));
    assert_eq!(
        h.status(redeem_request(&token, "Phone")).await,
        StatusCode::BAD_REQUEST,
        "a token expires after ten minutes"
    );
}

fn create_pairing_request(builder: Builder, name: &str) -> Request<Body> {
    json_request(builder, &json!({ "device_name": name }))
}

fn poll_request(request_id: &str) -> Request<Body> {
    json_request(
        remote(Method::POST, "/api/hub/pairing/requests/poll"),
        &json!({ "request_id": request_id }),
    )
}

#[tokio::test]
async fn an_additional_device_is_approved_from_a_paired_one() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;

    let created = json_of(
        &h,
        create_pairing_request(remote(Method::POST, "/api/hub/pairing/requests"), "Phone"),
    )
    .await;
    let secret_id = created["request_id"].as_str().unwrap().to_string();
    let code = created["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 6);

    let (_, waiting_headers, waiting_body) = h.send(poll_request(&secret_id)).await;
    assert!(waiting_headers.get("set-cookie").is_none());
    assert_eq!(
        serde_json::from_slice::<Value>(&waiting_body).unwrap()["status"],
        "pending"
    );

    let listing = devices_of(&h, &laptop).await;
    let pending = &listing["pending"][0];
    assert_eq!(
        pending["code"],
        code.as_str(),
        "the approver sees the same code"
    );
    assert_eq!(pending["device_name"], "Phone");
    assert!(
        !listing.to_string().contains(&secret_id),
        "the approving side never learns the secret the waiting browser polls with"
    );

    let approve = format!(
        "/api/hub/devices/pending/{}/approve",
        pending["id"].as_str().unwrap()
    );
    assert_eq!(
        h.status(empty(authed(Method::POST, &approve, &laptop)))
            .await,
        StatusCode::NO_CONTENT
    );

    let (_, approved_headers, approved_body) = h.send(poll_request(&secret_id)).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&approved_body).unwrap()["status"],
        "approved"
    );
    let phone = cookie_pair(&approved_headers);
    assert_eq!(
        get_status(&h, "/api/hub/agents", &phone).await,
        StatusCode::OK,
        "the approved browser is paired"
    );
    assert_eq!(
        json_of(&h, poll_request(&secret_id)).await["status"],
        "expired",
        "the request is spent"
    );
}

#[tokio::test]
async fn a_pairing_request_can_be_approved_from_the_local_ui() {
    let h = Harness::new();
    let created = json_of(
        &h,
        create_pairing_request(remote(Method::POST, "/api/hub/pairing/requests"), "Phone"),
    )
    .await;
    let listing = h.get_expect("/api/hub/devices", StatusCode::OK).await;
    let approve = format!(
        "/api/hub/devices/pending/{}/approve",
        listing["pending"][0]["id"].as_str().unwrap()
    );
    h.post_expect(&approve, StatusCode::NO_CONTENT).await;
    let polled = json_of(&h, poll_request(created["request_id"].as_str().unwrap())).await;
    assert_eq!(polled["status"], "approved");
}

#[tokio::test]
async fn a_refused_request_pairs_nothing() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let created = json_of(
        &h,
        create_pairing_request(
            remote(Method::POST, "/api/hub/pairing/requests"),
            "Stranger",
        ),
    )
    .await;
    let listing = devices_of(&h, &laptop).await;
    let deny = format!(
        "/api/hub/devices/pending/{}/deny",
        listing["pending"][0]["id"].as_str().unwrap()
    );
    h.status(empty(authed(Method::POST, &deny, &laptop))).await;
    let (_, headers, bytes) = h
        .send(poll_request(created["request_id"].as_str().unwrap()))
        .await;
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["status"],
        "denied"
    );
    assert!(headers.get("set-cookie").is_none());
}

#[tokio::test]
async fn an_eleventh_waiting_request_is_refused() {
    let h = Harness::new();
    // Different peers, so the per-address limit is not what refuses.
    for n in 0..10 {
        let from_peer = set(
            remote(Method::POST, "/api/hub/pairing/requests"),
            "x-real-ip",
            &format!("198.51.100.{n}"),
        );
        assert_eq!(
            h.status(create_pairing_request(from_peer, "Phone")).await,
            StatusCode::OK,
            "request {n}"
        );
    }
    let eleventh = set(
        remote(Method::POST, "/api/hub/pairing/requests"),
        "x-real-ip",
        "198.51.100.99",
    );
    assert_eq!(
        h.status(create_pairing_request(eleventh, "Phone")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn recovery_entry_and_request_creation_are_rate_limited_per_address() {
    let h = Harness::new();
    let mut outcomes = Vec::new();
    for _ in 0..11 {
        outcomes.push(
            h.status(json_request(
                remote(Method::POST, "/api/hub/pairing/recovery"),
                &json!({ "code": "AAAA-AAAA-AAAA-AAAA", "device_name": "x" }),
            ))
            .await,
        );
    }
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == StatusCode::BAD_REQUEST)
            .count(),
        10,
        "ten guesses a minute are answered"
    );
    assert_eq!(outcomes.last(), Some(&StatusCode::TOO_MANY_REQUESTS));

    let same_address =
        create_pairing_request(remote(Method::POST, "/api/hub/pairing/requests"), "P");
    assert_eq!(
        h.status(same_address).await,
        StatusCode::TOO_MANY_REQUESTS,
        "the same address shares one count across both operations"
    );
    let other_address = set(
        remote(Method::POST, "/api/hub/pairing/requests"),
        "x-real-ip",
        "198.51.100.1",
    );
    assert_eq!(
        h.status(create_pairing_request(other_address, "P")).await,
        StatusCode::OK,
        "another address is untouched"
    );
}

#[tokio::test]
async fn the_whole_install_is_limited_to_sixty_attempts_a_minute() {
    let h = Harness::new();
    let mut last = StatusCode::OK;
    for n in 0..61 {
        let from_peer = set(
            remote(Method::POST, "/api/hub/pairing/recovery"),
            "x-real-ip",
            &format!("192.0.2.{}", n + 1),
        );
        last = h
            .status(json_request(
                from_peer,
                &json!({ "code": format!("AAAA-AAAA-AAAA-{n:04}"), "device_name": "x" }),
            ))
            .await;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_recovery_code_pairs_a_browser_once() {
    let h = Harness::new();
    announce(&h).await;
    let minted = local_pair_link(&h).await;
    let code = minted["recovery_codes"][0].as_str().unwrap().to_string();
    let attempt = |typed: &str| {
        json_request(
            remote(Method::POST, "/api/hub/pairing/recovery"),
            &json!({ "code": typed, "device_name": "Tablet" }),
        )
    };
    let (outcome, headers, _) = h.send(attempt(&code.to_ascii_lowercase())).await;
    assert_eq!(outcome, StatusCode::OK);
    let tablet = cookie_pair(&headers);
    assert_eq!(
        get_status(&h, "/api/hub/agents", &tablet).await,
        StatusCode::OK
    );
    assert_eq!(
        h.status(attempt(&code)).await,
        StatusCode::BAD_REQUEST,
        "single use"
    );
}

#[tokio::test]
async fn regenerated_recovery_codes_replace_the_old_ones() {
    let h = Harness::new();
    announce(&h).await;
    let minted = local_pair_link(&h).await;
    let old = minted["recovery_codes"][0].as_str().unwrap().to_string();
    let renewed = h
        .call(Method::POST, "/api/hub/devices/recovery-codes", None)
        .await
        .1;
    assert_eq!(renewed["recovery_codes"].as_array().unwrap().len(), 10);
    let with_old = json_request(
        remote(Method::POST, "/api/hub/pairing/recovery"),
        &json!({ "code": old, "device_name": "x" }),
    );
    assert_eq!(h.status(with_old).await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_revoked_device_is_refused() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let listing = devices_of(&h, &laptop).await;
    let id = listing["devices"][0]["id"].as_str().unwrap().to_string();
    let revoke = authed(Method::DELETE, &format!("/api/hub/devices/{id}"), &laptop);
    assert_eq!(h.status(empty(revoke)).await, StatusCode::NO_CONTENT);
    assert_eq!(
        get_status(&h, "/api/hub/agents", &laptop).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_cross_site_write_with_a_valid_credential_is_rejected() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    for site in ["same-site", "cross-site"] {
        let write = set(
            authed(Method::POST, "/api/hub/devices/recovery-codes", &laptop),
            "sec-fetch-site",
            site,
        );
        assert_eq!(
            h.status(empty(write)).await,
            StatusCode::FORBIDDEN,
            "{site}"
        );
    }
    let same_origin = authed(Method::POST, "/api/hub/devices/recovery-codes", &laptop);
    assert_eq!(
        h.status(empty(same_origin)).await,
        StatusCode::OK,
        "the credential is fine for a same-origin write"
    );
}

#[tokio::test]
async fn without_fetch_metadata_the_origin_must_match_and_neither_header_is_a_rejection() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let write = |origin: Option<&str>| {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/hub/devices/recovery-codes")
            .header(TUNNEL_NONCE_HEADER, tunnel_nonce())
            .header("cookie", laptop.as_str());
        if let Some(sent) = origin {
            builder = builder.header("origin", sent);
        }
        empty(builder)
    };
    assert_eq!(h.status(write(Some(UI_ORIGIN))).await, StatusCode::OK);
    assert_eq!(
        h.status(write(Some("https://mallory.agent-residuum.com")))
            .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(h.status(write(None)).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_cross_site_socket_upgrade_is_rejected_even_with_a_credential() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let upgrade = set(
        authed(Method::GET, "/api/hub/ws", &laptop),
        "sec-fetch-site",
        "same-site",
    )
    .header("upgrade", "websocket")
    .header("connection", "upgrade");
    assert_eq!(h.status(empty(upgrade)).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn another_instances_cookie_does_not_pair_this_one() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let secret = laptop.split_once('=').unwrap().1;
    let other = format!("__Host-residuum_device_desktop={secret}");
    assert_eq!(
        get_status(&h, "/api/hub/agents", &other).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_cookie_is_issued_again_once_a_day_of_use() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let (_, first_headers, _) = h
        .send(empty(authed(Method::GET, "/api/hub/agents", &laptop)))
        .await;
    assert!(
        first_headers.get("set-cookie").is_none(),
        "no refresh on the first day"
    );
    h.pairing.advance_clock(chrono::Duration::hours(25));
    let (_, later_headers, _) = h
        .send(empty(authed(Method::GET, "/api/hub/agents", &laptop)))
        .await;
    assert_eq!(
        cookie_pair(&later_headers),
        laptop,
        "the same credential is set again to extend its life"
    );
}

#[tokio::test]
async fn the_state_route_says_whether_a_browser_needs_to_pair() {
    let h = Harness::new();
    let local = h.call(Method::GET, "/api/hub/pairing/state", None).await.1;
    assert_eq!(local, json!({ "remote": false, "paired": true }));
    assert_eq!(
        json_of(&h, empty(remote(Method::GET, "/api/hub/pairing/state"))).await,
        json!({ "remote": true, "paired": false })
    );
    let laptop = pair_first_device(&h, "Laptop").await;
    assert_eq!(
        json_of(
            &h,
            empty(authed(Method::GET, "/api/hub/pairing/state", &laptop))
        )
        .await,
        json!({ "remote": true, "paired": true })
    );
}

#[tokio::test]
async fn a_transport_marker_extension_is_gated_like_the_tunnel() {
    let h = Harness::new();
    let mut request = Request::get("/api/hub/agents").body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(crate::pairing::remote::RemoteTransport {
            peer_ip: Some(PEER.to_string()),
            origin: Some(UI_ORIGIN.to_string()),
        });
    assert_eq!(
        h.status(request).await,
        StatusCode::UNAUTHORIZED,
        "a future transport marks requests through the same gate"
    );
}

// ── The workbench host ───────────────────────────────────────────────

/// The workbench listener's router, serving one artifact `notes`, with the
/// hub router behind its `/api`.
struct Workbench {
    router: Router,
}

impl Workbench {
    fn new(h: &Harness) -> Self {
        let dir = h.root.path().join("team/workbench");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.html"), "<p>hi</p>").unwrap();
        let api = HubApi::new();
        api.bind(h.app.clone());
        Self {
            router: crate::workbench::server::router(dir, api, h.pairing.clone()),
        }
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, HeaderMap) {
        let response = self.router.clone().oneshot(request).await.unwrap();
        (response.status(), response.headers().clone())
    }

    async fn status(&self, request: Request<Body>) -> StatusCode {
        self.send(request).await.0
    }
}

/// Mint a handoff token as the paired UI-host browser `laptop`.
async fn minted_handoff_token(h: &Harness, laptop: &str) -> String {
    let minted = json_of(
        h,
        empty(authed(
            Method::POST,
            "/api/hub/devices/workbench-handoff",
            laptop,
        )),
    )
    .await;
    minted["token"].as_str().unwrap().to_string()
}

fn handoff_request(token: &str) -> Request<Body> {
    json_request(
        remote(Method::POST, "/api/hub/pairing/handoff"),
        &json!({ "token": token }),
    )
}

/// Complete a handoff on the workbench host and return its cookie.
async fn workbench_cookie(h: &Harness, workbench: &Workbench, laptop: &str) -> String {
    let token = minted_handoff_token(h, laptop).await;
    let (outcome, headers) = workbench.send(handoff_request(&token)).await;
    assert_eq!(outcome, StatusCode::OK);
    cookie_pair(&headers)
}

#[tokio::test]
async fn the_workbench_host_refuses_a_browser_without_its_own_credential() {
    let h = Harness::new();
    announce(&h).await;
    let workbench = Workbench::new(&h);
    assert_eq!(
        workbench
            .status(empty(remote(Method::GET, "/notes/")))
            .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        workbench
            .status(empty(remote(Method::GET, "/api/hub/agents")))
            .await,
        StatusCode::UNAUTHORIZED
    );
    // The handoff page is reachable without one, since it is how a browser gets one.
    assert_eq!(
        workbench
            .status(empty(remote(Method::GET, "/_handoff")))
            .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_workbench_navigation_without_a_credential_goes_to_the_artifact_in_the_app() {
    let h = Harness::new();
    announce(&h).await;
    let workbench = Workbench::new(&h);
    let navigation = set(remote(Method::GET, "/notes/"), "sec-fetch-mode", "navigate");
    let (outcome, headers) = workbench.send(empty(navigation)).await;
    assert!(outcome.is_redirection());
    assert_eq!(
        headers.get("location").unwrap(),
        "https://bear.agent-residuum.com/team/workbench/notes"
    );
}

#[tokio::test]
async fn only_a_paired_ui_browser_can_mint_a_handoff_token() {
    let h = Harness::new();
    announce(&h).await;
    assert_eq!(
        h.status(empty(remote(
            Method::POST,
            "/api/hub/devices/workbench-handoff"
        )))
        .await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_handoff_gives_the_workbench_host_its_own_credential() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let workbench = Workbench::new(&h);
    let wb_cookie = workbench_cookie(&h, &workbench, &laptop).await;

    let artifact = empty(remote(Method::GET, "/notes/").header("cookie", wb_cookie.as_str()));
    assert_eq!(workbench.status(artifact).await, StatusCode::OK);
    let api = empty(remote(Method::GET, "/api/hub/agents").header("cookie", wb_cookie.as_str()));
    assert_eq!(
        workbench.status(api).await,
        StatusCode::OK,
        "the API on the workbench host needs only its own credential"
    );
    assert_eq!(
        get_status(&h, "/api/hub/agents", &wb_cookie).await,
        StatusCode::UNAUTHORIZED,
        "the workbench credential is no good on the UI host"
    );
}

#[tokio::test]
async fn a_handoff_token_works_once() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let workbench = Workbench::new(&h);
    let token = minted_handoff_token(&h, &laptop).await;
    assert_eq!(
        workbench.status(handoff_request(&token)).await,
        StatusCode::OK
    );
    assert_eq!(
        workbench.status(handoff_request(&token)).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_handoff_token_expires_after_a_minute() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let workbench = Workbench::new(&h);
    let token = minted_handoff_token(&h, &laptop).await;
    h.pairing.advance_clock(chrono::Duration::seconds(61));
    assert_eq!(
        workbench.status(handoff_request(&token)).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_page_on_the_workbench_host_cannot_manage_devices() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let workbench = Workbench::new(&h);
    let wb_cookie = workbench_cookie(&h, &workbench, &laptop).await;
    let manage = remote(Method::POST, "/api/hub/devices/recovery-codes")
        .header("cookie", wb_cookie.as_str());
    assert_eq!(workbench.status(empty(manage)).await, StatusCode::FORBIDDEN);
    let mint = remote(Method::POST, "/api/hub/remote-access/pair-link")
        .header("cookie", wb_cookie.as_str());
    assert_eq!(workbench.status(empty(mint)).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoking_a_device_ends_its_workbench_access_too() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let workbench = Workbench::new(&h);
    let wb_cookie = workbench_cookie(&h, &workbench, &laptop).await;
    let listing = devices_of(&h, &laptop).await;
    let id = listing["devices"][0]["id"].as_str().unwrap().to_string();
    let revoke = authed(Method::DELETE, &format!("/api/hub/devices/{id}"), &laptop);
    h.status(empty(revoke)).await;
    let artifact = empty(remote(Method::GET, "/notes/").header("cookie", wb_cookie.as_str()));
    assert_eq!(workbench.status(artifact).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_cross_site_write_to_the_workbench_host_is_rejected_with_a_credential() {
    let h = Harness::new();
    let laptop = pair_first_device(&h, "Laptop").await;
    let workbench = Workbench::new(&h);
    let wb_cookie = workbench_cookie(&h, &workbench, &laptop).await;
    let cross = set(
        remote(Method::POST, "/api/hub/hub/anything").header("cookie", wb_cookie.as_str()),
        "sec-fetch-site",
        "same-site",
    );
    assert_eq!(workbench.status(empty(cross)).await, StatusCode::FORBIDDEN);
}
