//! The join endpoints on the instance host (`/_sibling/join/...`), served
//! in-process. They authenticate their callers themselves, by signature and
//! by the join id only the requester knows, so there is no device gate.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::header::{self, HeaderValue};
use axum::http::{Extensions, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::Utc;
use serde_json::{Value, json};

use super::host::{HostError, JoinHost};
use super::protocol::{NONCE_PATH, REQUEST_PATH};
use crate::pairing::remote::RemoteTransport;
use crate::remote_access::engine_limit::PeerRateLimiter;

/// Join requests a peer address may make per minute.
const PER_PEER_PER_MINUTE: f64 = 10.0;
/// The largest request body: a signed request is a few hundred bytes.
const MAX_BODY: usize = 16 * 1024;

#[derive(Clone)]
struct RouteState {
    host: Arc<JoinHost>,
    limiter: Arc<Mutex<PeerRateLimiter>>,
}

/// The router for the join endpoints.
pub(crate) fn router(host: Arc<JoinHost>) -> Router {
    let state = RouteState {
        host,
        limiter: Arc::new(Mutex::new(PeerRateLimiter::with_rate(
            PER_PEER_PER_MINUTE,
            PER_PEER_PER_MINUTE,
            Instant::now(),
        ))),
    };
    Router::new()
        .route(NONCE_PATH, get(nonce))
        .route(REQUEST_PATH, axum::routing::post(submit))
        .route("/_sibling/join/{join_id}", get(poll))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit))
        .with_state(state)
}

fn peer_of(extensions: &Extensions) -> String {
    extensions
        .get::<RemoteTransport>()
        .and_then(|transport| transport.peer_ip.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

async fn rate_limit(State(state): State<RouteState>, req: Request, next: Next) -> Response {
    let peer = peer_of(req.extensions());
    let verdict = state
        .limiter
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .check(&peer, Instant::now());
    match verdict {
        Ok(()) => no_store(next.run(req).await),
        Err(wait) => {
            tracing::warn!(peer, "rate limited a sibling join request");
            let mut response = failure(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many join requests from your address. Wait a minute and try again.",
            );
            if let Ok(value) = HeaderValue::from_str(&wait_seconds(wait)) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response
        }
    }
}

fn wait_seconds(wait: Duration) -> String {
    wait.as_secs().saturating_add(1).to_string()
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn host_failure(error: &HostError) -> Response {
    let status = match error {
        HostError::NotReady => StatusCode::SERVICE_UNAVAILABLE,
        HostError::TooManyPending => StatusCode::TOO_MANY_REQUESTS,
        HostError::Invalid(_) | HostError::UnknownNonce => StatusCode::BAD_REQUEST,
    };
    failure(status, &error.to_string())
}

async fn nonce(State(state): State<RouteState>) -> Response {
    match state.host.issue_nonce(Utc::now()) {
        Ok(reply) => Json(reply).into_response(),
        Err(e) => host_failure(&e),
    }
}

async fn submit(State(state): State<RouteState>, Json(body): Json<Value>) -> Response {
    match state.host.submit(&body, Utc::now()) {
        Ok(reply) => (StatusCode::ACCEPTED, Json(reply)).into_response(),
        Err(e) => {
            if matches!(e, HostError::Invalid(_)) {
                tracing::warn!(error = %e, "refused a sibling join request");
            }
            host_failure(&e)
        }
    }
}

async fn poll(State(state): State<RouteState>, Path(join_id): Path<String>) -> Response {
    match state.host.poll(&join_id, Utc::now()) {
        Some(reply) => Json(reply).into_response(),
        None => failure(
            StatusCode::NOT_FOUND,
            "That join request has expired or never existed. Start the join again.",
        ),
    }
}
