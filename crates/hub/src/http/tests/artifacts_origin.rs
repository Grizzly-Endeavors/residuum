use super::*;
use residuum_net::workbench::ArtifactsOrigin;

const BLOCKED: [&str; 8] = [
    "/api/hub/shutdown",
    "/api/hub/stop-all",
    "/api/hub/update/check",
    "/api/hub/update/apply",
    "/api/hub/update/restart",
    "/api/hub/config/complete-setup",
    "/api/hub/cloud/connect",
    "/api/hub/cloud/disconnect",
];

fn through_the_artifacts_origin(mut request: Request<Body>) -> Request<Body> {
    request.extensions_mut().insert(ArtifactsOrigin);
    request
}

fn request(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn every_blocked_route_is_refused_through_the_artifacts_origin() {
    let mut h = Harness::new();
    for uri in BLOCKED {
        let exists = h.status(request(Method::GET, uri)).await;
        assert_eq!(exists, StatusCode::METHOD_NOT_ALLOWED, "{uri}");

        for method in [Method::POST, Method::GET] {
            let (status, body) = h
                .send_body(through_the_artifacts_origin(request(method.clone(), uri)))
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
            let body: Value = serde_json::from_slice(&body).unwrap();
            let message = body["error"].as_str().unwrap();
            assert!(message.contains("Residuum app"), "{uri}: {message}");
        }
    }
    assert!(h.directory.calls().is_empty(), "no agent was stopped");
    assert!(h.shutdown_rx.try_recv().is_err(), "no shutdown was sent");
}

#[tokio::test]
async fn the_blocked_routes_work_without_the_marker() {
    let mut h = Harness::new();

    let shutdown = h.post_expect("/api/hub/shutdown", StatusCode::OK).await;
    assert_eq!(shutdown, json!({ "shutting_down": true }));
    assert!(h.shutdown_rx.try_recv().is_ok());

    h.post_expect("/api/hub/stop-all", StatusCode::OK).await;
    assert_eq!(h.directory.calls(), ["stop scout"]);

    let restart = h
        .post_expect("/api/hub/update/restart", StatusCode::OK)
        .await;
    assert_eq!(restart, json!({ "restarting": true }));

    h.post_expect("/api/hub/update/apply", StatusCode::BAD_REQUEST)
        .await;

    let setup = h
        .expect(
            Method::POST,
            "/api/hub/config/complete-setup",
            Some(json!({
                "hub_config": "timezone = \"UTC\"\n",
                "agent_name": "Team",
                "config": "",
                "providers": "",
            })),
            StatusCode::BAD_REQUEST,
        )
        .await;
    assert_eq!(setup["valid"], false);

    let sign_in = h
        .expect(
            Method::POST,
            "/api/hub/cloud/connect",
            Some(json!({})),
            StatusCode::OK,
        )
        .await;
    assert!(sign_in["url"].as_str().unwrap().contains("state="));

    h.post_expect("/api/hub/cloud/disconnect", StatusCode::OK)
        .await;
}

#[tokio::test]
async fn a_page_on_the_artifacts_origin_cannot_start_a_cloud_sign_in() {
    let mut h = Harness::new();
    let connect = Request::builder()
        .method(Method::POST)
        .uri("/api/hub/cloud/connect")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let (status, body) = h.send_body(through_the_artifacts_origin(connect)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body.get("url").is_none(), "{body}");

    let (_, headers, _) = h
        .send(request(
            Method::GET,
            "/cloud/callback?token=rst_from_a_page&state=AAAAAAAAAAAAAAAAAAAAAA",
        ))
        .await;
    assert_eq!(
        headers["location"],
        "/home?settings=_all/cloud&cloud=not-started-here"
    );
    assert!(h.reload_rx.try_recv().is_err());
}

#[tokio::test]
async fn a_header_imitating_the_marker_changes_nothing() {
    let h = Harness::new();
    let forged = Request::builder()
        .method(Method::POST)
        .uri("/api/hub/stop-all")
        .header("x-residuum-artifacts-origin", "1")
        .header("x-residuum-artifact", "chart")
        .header("origin", "http://localhost:7702")
        .header("host", "localhost:7702")
        .header("sec-fetch-site", "same-origin")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        h.status(forged).await,
        StatusCode::OK,
        "a client can't mark its own request, so the gateway serves it as it always has"
    );
    assert_eq!(h.directory.calls(), ["stop scout"]);
}

#[tokio::test]
async fn everything_else_is_allowed_through_the_artifacts_origin() {
    let h = Harness::new();
    let allowed = [
        (Method::GET, "/api/hub/agents"),
        (Method::GET, "/api/hub/status"),
        (Method::GET, "/api/hub/update/status"),
        (Method::GET, "/api/agents/scout/status"),
        (Method::GET, "/api/team/workspace/files"),
        (Method::POST, "/api/hub/agents/scout/stop"),
        (Method::POST, "/api/hub/agents/quiet/start"),
        (Method::DELETE, "/api/hub/agents/quiet"),
    ];
    for (method, uri) in allowed {
        let status = h
            .status(through_the_artifacts_origin(request(method.clone(), uri)))
            .await;
        assert_eq!(status, StatusCode::OK, "{method} {uri}");
    }
}
