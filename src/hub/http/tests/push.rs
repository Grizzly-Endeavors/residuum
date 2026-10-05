//! Tests for `/api/hub/push/...`, against a push service in the harness's hub
//! directory and a fake push endpoint.

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::hub::push::{PushDevice, PutPushDeviceRequest, WebPushSubscription};

/// A well-formed subscription body for `endpoint`. The key is the hub's own
/// public key, which is a valid P-256 point.
async fn subscription(h: &Harness, endpoint: &str) -> Value {
    let (_status, key) = h.call(Method::GET, "/api/hub/push/key", None).await;
    json!({
        "endpoint": endpoint,
        "expirationTime": null,
        "keys": {
            "p256dh": key["public_key"],
            "auth": "AAAAAAAAAAAAAAAAAAAAAA",
        },
    })
}

async fn register(h: &Harness, endpoint: &str, label: &str) -> Value {
    let body = json!({ "subscription": subscription(h, endpoint).await, "label": label });
    h.expect(
        Method::PUT,
        "/api/hub/push/devices",
        Some(body),
        StatusCode::OK,
    )
    .await
}

#[tokio::test]
async fn the_key_route_returns_the_base64url_public_key_and_creates_the_key_file() {
    let h = Harness::new();
    let key_file = h.root.path().join("hub/push-vapid.key");
    assert!(!key_file.exists());

    let reply = h.get_expect("/api/hub/push/key", StatusCode::OK).await;

    let public_key = reply["public_key"].as_str().unwrap();
    // 65 bytes of base64url without padding is 87 characters.
    assert_eq!(public_key.len(), 87, "{public_key}");
    assert!(
        public_key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "{public_key}"
    );
    assert!(key_file.exists());
    let again = h.get_expect("/api/hub/push/key", StatusCode::OK).await;
    assert_eq!(again["public_key"], reply["public_key"]);
}

#[tokio::test]
async fn an_empty_hub_lists_no_devices() {
    let h = Harness::new();
    let reply = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(reply, json!({ "devices": [] }));
}

#[tokio::test]
async fn put_registers_a_device_with_the_default_preferences() {
    let h = Harness::new();

    let reply = register(&h, "https://push.example.com/send/abc", "Laptop").await;

    let device = &reply["device"];
    assert_eq!(device["label"], "Laptop");
    assert_eq!(device["last_success_at"], Value::Null);
    assert_eq!(device["last_failure"], Value::Null);
    assert_eq!(
        device["preferences"],
        json!({
            "inbox_item": true,
            "agent_failed": true,
            "outbound_unreachable": false,
            "reply_while_away": false,
        })
    );
    assert!(device["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(device["created_at"].as_str().unwrap().ends_with('Z'));
    let listed = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(listed["devices"], json!([device]));
    assert!(
        !listed.to_string().contains("push.example.com"),
        "the endpoint is never returned"
    );
}

#[tokio::test]
async fn put_is_idempotent_for_one_endpoint() {
    let h = Harness::new();
    let first = register(&h, "https://push.example.com/send/abc", "Laptop").await;
    let second = register(&h, "https://push.example.com/send/abc", "Laptop").await;
    assert_eq!(first["device"]["id"], second["device"]["id"]);

    let renamed = h
        .expect(
            Method::PUT,
            "/api/hub/push/devices",
            Some(json!({
                "subscription": subscription(&h, "https://push.example.com/send/abc").await,
                "label": "Work laptop",
                "preferences": { "reply_while_away": true },
            })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(renamed["device"]["id"], first["device"]["id"]);
    assert_eq!(renamed["device"]["label"], "Work laptop");
    assert_eq!(renamed["device"]["preferences"]["reply_while_away"], true);
    assert_eq!(renamed["device"]["preferences"]["inbox_item"], true);

    let one_device = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(one_device["devices"].as_array().unwrap().len(), 1);

    register(&h, "https://push.example.com/send/other", "Phone").await;
    let two_devices = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(two_devices["devices"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn put_with_a_previous_endpoint_updates_the_device_that_had_it() {
    let h = Harness::new();
    let first = register(&h, "https://push.example.com/send/old", "Laptop").await;

    let rotated = h
        .expect(
            Method::PUT,
            "/api/hub/push/devices",
            Some(json!({
                "subscription": subscription(&h, "https://push.example.com/send/new").await,
                "previous_endpoint": "https://push.example.com/send/old",
            })),
            StatusCode::OK,
        )
        .await;

    assert_eq!(
        rotated["device"]["id"], first["device"]["id"],
        "the rotation keeps the device's id"
    );
    assert_eq!(rotated["device"]["label"], "Laptop", "and its label");
    let listed = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(
        listed["devices"].as_array().unwrap().len(),
        1,
        "no second device is left behind"
    );
}

#[tokio::test]
async fn put_refuses_a_subscription_it_cannot_use() {
    let h = Harness::new();
    let good = subscription(&h, "https://push.example.com/send/abc").await;
    let with = |change: &dyn Fn(&mut Value)| {
        let mut sub = good.clone();
        change(&mut sub);
        json!({ "subscription": sub, "label": "x" })
    };

    for (what, body) in [
        (
            "an http endpoint",
            with(&|s| s["endpoint"] = json!("http://push.example.com/a")),
        ),
        (
            "an endpoint that isn't an address",
            with(&|s| s["endpoint"] = json!("not a url")),
        ),
        (
            "a public key that isn't a point",
            with(&|s| s["keys"]["p256dh"] = json!("AAAA")),
        ),
        (
            "a short auth secret",
            with(&|s| s["keys"]["auth"] = json!("AAAA")),
        ),
        ("no subscription", json!({ "label": "x" })),
        ("a blank label", {
            let mut body = with(&|_| {});
            body["label"] = json!("  ");
            body
        }),
    ] {
        let reply = h
            .expect(
                Method::PUT,
                "/api/hub/push/devices",
                Some(body),
                StatusCode::BAD_REQUEST,
            )
            .await;
        assert!(reply["error"].is_string(), "{what}: {reply}");
    }
    let listed = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(listed["devices"], json!([]));
}

#[tokio::test]
async fn patch_changes_the_label_and_preferences() {
    let h = Harness::new();
    let id = register(&h, "https://push.example.com/send/abc", "Laptop").await["device"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let reply = h
        .expect(
            Method::PATCH,
            &format!("/api/hub/push/devices/{id}"),
            Some(json!({ "label": "Desk", "preferences": { "agent_failed": false } })),
            StatusCode::OK,
        )
        .await;

    assert_eq!(reply["device"]["id"], id);
    assert_eq!(reply["device"]["label"], "Desk");
    assert_eq!(reply["device"]["preferences"]["agent_failed"], false);
    assert_eq!(reply["device"]["preferences"]["inbox_item"], true);
    let listed = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(listed["devices"], json!([reply["device"]]));
}

#[tokio::test]
async fn patch_answers_404_for_an_unknown_device_and_400_for_nothing_to_change() {
    let h = Harness::new();
    let id = register(&h, "https://push.example.com/send/abc", "Laptop").await["device"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let unknown = h
        .expect(
            Method::PATCH,
            "/api/hub/push/devices/nope",
            Some(json!({ "label": "x" })),
            StatusCode::NOT_FOUND,
        )
        .await;
    assert!(unknown["error"].as_str().unwrap().contains("nope"));
    for body in [json!({}), json!({ "label": " " }), json!("text")] {
        let reply = h
            .expect(
                Method::PATCH,
                &format!("/api/hub/push/devices/{id}"),
                Some(body),
                StatusCode::BAD_REQUEST,
            )
            .await;
        assert!(reply["error"].is_string());
    }
}

#[tokio::test]
async fn delete_answers_204_and_then_404() {
    let h = Harness::new();
    let id = register(&h, "https://push.example.com/send/abc", "Laptop").await["device"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, _headers, body) = h
        .send(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/hub/push/devices/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty());
    let listed = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(listed["devices"], json!([]));

    h.expect(
        Method::DELETE,
        &format!("/api/hub/push/devices/{id}"),
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
}

/// Register a device whose push endpoint is `server`, which the route's own
/// validation refuses (it is plain http), through the service instead.
async fn register_local(h: &Harness, server: &MockServer) -> PushDevice {
    let key = crate::hub::push::PushService::public_key(&h.push)
        .await
        .unwrap();
    h.push
        .upsert_device(PutPushDeviceRequest {
            subscription: WebPushSubscription {
                endpoint: format!("{}/push/laptop", server.uri()),
                keys: crate::hub::push::WebPushSubscriptionKeys {
                    p256dh: key,
                    auth: "AAAAAAAAAAAAAAAAAAAAAA".to_string(),
                },
            },
            label: Some("Laptop".to_string()),
            preferences: None,
            previous_endpoint: None,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn the_test_route_reports_a_delivered_notification() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    let h = Harness::new();
    let device = register_local(&h, &server).await;

    let reply = h
        .post_expect(
            &format!("/api/hub/push/devices/{}/test", device.id),
            StatusCode::OK,
        )
        .await;

    assert_eq!(reply, json!({ "delivered": true, "error": null }));
    let listed = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert!(listed["devices"][0]["last_success_at"].is_string());
}

#[tokio::test]
async fn the_test_route_reports_a_refusal_in_plain_words() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let h = Harness::new();
    let device = register_local(&h, &server).await;

    let reply = h
        .post_expect(
            &format!("/api/hub/push/devices/{}/test", device.id),
            StatusCode::OK,
        )
        .await;

    assert_eq!(reply["delivered"], false);
    assert!(reply["error"].as_str().unwrap().contains("403"), "{reply}");
    let listed = h.get_expect("/api/hub/push/devices", StatusCode::OK).await;
    assert_eq!(listed["devices"][0]["last_failure"]["status"], 403);
}

#[tokio::test]
async fn the_test_route_answers_404_for_an_unknown_device() {
    let h = Harness::new();
    let reply = h
        .post_expect("/api/hub/push/devices/nope/test", StatusCode::NOT_FOUND)
        .await;
    assert!(reply["error"].as_str().unwrap().contains("nope"));
}

#[tokio::test]
async fn a_damaged_devices_file_answers_500_with_a_message_and_is_left_alone() {
    let h = Harness::new();
    let file = h.root.path().join("hub/push-devices.json");
    std::fs::write(&file, "garbage").unwrap();

    let reply = h
        .get_expect("/api/hub/push/devices", StatusCode::INTERNAL_SERVER_ERROR)
        .await;

    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("push-devices.json"),
        "{reply}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "garbage");
}

#[tokio::test]
async fn the_push_routes_stay_open_over_the_tunnel() {
    // A phone on the relay manages its own notifications, so unlike shutdown
    // and disconnect these routes accept tunnelled requests.
    let h = Harness::new();
    for (method, uri) in [
        (Method::GET, "/api/hub/push/key"),
        (Method::GET, "/api/hub/push/devices"),
    ] {
        let status = h.status(through_the_tunnel(method, uri)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
    let status = h
        .status(through_the_tunnel(
            Method::DELETE,
            "/api/hub/push/devices/nope",
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "reached the route");
}

#[tokio::test]
async fn the_cross_site_guard_refuses_writes_to_the_push_routes() {
    let h = Harness::new();
    for (method, uri) in [
        (Method::PUT, "/api/hub/push/devices"),
        (Method::PATCH, "/api/hub/push/devices/any"),
        (Method::DELETE, "/api/hub/push/devices/any"),
        (Method::POST, "/api/hub/push/devices/any/test"),
    ] {
        let status = h.status(cross_site(method, uri)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
    assert!(!h.root.path().join("hub/push-devices.json").exists());
}
