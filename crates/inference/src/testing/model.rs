//! A model that answers through a [`Gate`], over HTTP or in process.
//!
//! [`GatedModel`] is a small HTTP server an agent's provider config points
//! at. It waits at its gate, then forwards the call to a wiremock server
//! behind it, so a test keeps wiremock's scripted replies and request log.
//! [`GatedProvider`] is the same for code that takes an [`InferenceProvider`]
//! directly. Either way, a test holds a model call open by closing the gate,
//! instead of slowing the reply down.
//!
//! Other crates' tests (and the root's, through `test-support`) can use
//! [`GatedModel::in_front_of`], [`GatedModel::uri`], [`GatedModel::gate`] and
//! [`GatedProvider::new`]. Only inference's own tests can use
//! `GatedModel::replying`, which answers with a fixed reply.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tokio::task::JoinHandle;

use crate::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message,
    ToolDefinition,
};
use residuum_core::testing::gate::{Gate, GateEntry};

/// What the server does with a call once the gate lets it through.
enum Answer {
    /// Forward it to this base URL and return what comes back.
    Forward {
        upstream: String,
        client: reqwest::Client,
    },
    /// Answer with this text as an OpenAI-shaped chat completion.
    #[cfg(test)]
    Reply(String),
}

/// The state each call is answered from.
struct Shared {
    entry: GateEntry,
    answer: Answer,
}

/// A model server whose answers wait at a gate. Dropping it stops the server.
pub struct GatedModel {
    uri: String,
    gate: Gate,
    task: JoinHandle<()>,
}

impl GatedModel {
    /// Serve in front of `upstream` (a wiremock server's URI), forwarding every
    /// call to it. The gate starts open, so calls pass straight through until
    /// a test closes it.
    pub async fn in_front_of(upstream: &str) -> Self {
        Self::start(
            Gate::open("model"),
            Answer::Forward {
                upstream: upstream.trim_end_matches('/').to_string(),
                client: reqwest::Client::new(),
            },
        )
        .await
    }

    /// Answer every call with `text`. The gate starts closed: no call is
    /// answered until the test releases it.
    #[cfg(test)]
    pub(crate) async fn replying(text: &str) -> Self {
        Self::start(Gate::closed("model"), Answer::Reply(text.to_string())).await
    }

    #[expect(
        clippy::unwrap_used,
        reason = "test-support helper: a loopback listener that can't bind or serve fails the test that started it"
    )]
    async fn start(gate: Gate, answer: Answer) -> Self {
        let shared = Arc::new(Shared {
            entry: gate.entry(),
            answer,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback(answer_call).with_state(shared);
        let task = residuum_core::util::spawn_in_span(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { uri, gate, task }
    }

    /// The base URL to point a provider at.
    #[must_use]
    pub fn uri(&self) -> String {
        self.uri.clone()
    }

    /// The gate every call waits at.
    #[must_use]
    pub fn gate(&self) -> &Gate {
        &self.gate
    }
}

impl Drop for GatedModel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn answer_call(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let target = uri
        .path_and_query()
        .map_or_else(|| uri.path().to_string(), ToString::to_string);
    shared.entry.pass().await;
    match &shared.answer {
        #[cfg(test)]
        Answer::Reply(text) => axum::Json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": text } }]
        }))
        .into_response(),
        Answer::Forward { upstream, client } => {
            forward(
                client,
                &format!("{upstream}{target}"),
                method,
                headers,
                body,
            )
            .await
        }
    }
}

/// Send the call on to the upstream server and hand back its answer. An
/// upstream that can't be reached is a 502 naming why, so a broken proxy
/// shows in the test's failure rather than as a missing reply.
async fn forward(
    client: &reqwest::Client,
    url: &str,
    method: Method,
    mut headers: HeaderMap,
    body: Bytes,
) -> Response {
    for hop in ["host", "content-length", "connection", "transfer-encoding"] {
        headers.remove(hop);
    }
    let sent = client
        .request(method, url)
        .headers(headers)
        .body(body)
        .send()
        .await;
    let upstream = match sent {
        Ok(response) => response,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("upstream {url}: {e}")).into_response(),
    };
    let status = upstream.status();
    let mut answer_headers = upstream.headers().clone();
    for hop in ["content-length", "connection", "transfer-encoding"] {
        answer_headers.remove(hop);
    }
    match upstream.bytes().await {
        Ok(bytes) => (status, answer_headers, Body::from(bytes)).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("upstream {url} body: {e}")).into_response(),
    }
}

/// An in-process provider whose every completion waits at a gate, then
/// answers with a fixed reply.
pub struct GatedProvider {
    entry: GateEntry,
    reply: String,
}

impl GatedProvider {
    /// A provider whose completions wait at `gate`, then answer `reply`.
    #[must_use]
    pub fn new(gate: &Gate, reply: &str) -> Self {
        Self {
            entry: gate.entry(),
            reply: reply.to_string(),
        }
    }
}

#[async_trait]
impl InferenceProvider for GatedProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        self.entry.pass().await;
        Ok(InferenceResponse::new(self.reply.clone(), vec![]))
    }

    fn model_name(&self) -> &'static str {
        "gated"
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use residuum_core::testing::wait;

    async fn post(uri: &str, body: &str) -> (u16, String) {
        let response = reqwest::Client::new()
            .post(format!("{uri}/chat/completions?model=m"))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }

    #[tokio::test]
    async fn in_front_of_forwards_calls_and_holds_them_while_closed() {
        let upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(201).set_body_string("from upstream"))
            .mount(&upstream)
            .await;
        let model = GatedModel::in_front_of(&upstream.uri()).await;

        assert_eq!(
            post(&model.uri(), "{}").await,
            (201, "from upstream".to_string())
        );

        model.gate().close();
        let uri = model.uri();
        let held =
            residuum_core::util::spawn_in_span(async move { post(&uri, r#"{"n":2}"#).await });
        model.gate().until_held(1).await;
        assert_eq!(upstream.received_requests().await.unwrap().len(), 1);
        model.gate().release(1);
        let (status, _) = wait::guarded("the held call's answer", held).await.unwrap();
        assert_eq!(status, 201);

        let received = upstream.received_requests().await.unwrap();
        assert_eq!(received.len(), 2);
        let held_call = received.last().unwrap();
        assert_eq!(held_call.method.to_string(), "POST");
        assert_eq!(held_call.url.path(), "/chat/completions");
        assert_eq!(held_call.url.query(), Some("model=m"));
        assert_eq!(held_call.body, br#"{"n":2}"#.to_vec());
    }

    #[tokio::test]
    async fn an_unreachable_upstream_is_a_bad_gateway_naming_it() {
        // A bound socket that never listens refuses connections, and it keeps
        // its port out of every other test's reach until this test ends. A
        // released port could be handed to another test's proxy in between.
        let reserved = tokio::net::TcpSocket::new_v4().unwrap();
        reserved.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let gone = format!("http://{}", reserved.local_addr().unwrap());
        let model = GatedModel::in_front_of(&gone).await;
        let (status, body) = post(&model.uri(), "{}").await;
        assert_eq!(status, 502);
        assert!(body.contains(&gone), "{body}");
        drop(reserved);
    }

    #[tokio::test]
    async fn replying_answers_only_once_released() {
        let model = GatedModel::replying("hello").await;
        let uri = model.uri();
        let held = residuum_core::util::spawn_in_span(async move { post(&uri, "{}").await });
        model.gate().until_held(1).await;
        model.gate().release(1);
        let (status, body) = wait::guarded("the reply", held).await.unwrap();
        assert_eq!(status, 200);
        assert!(body.contains("hello"), "{body}");
    }

    #[tokio::test]
    async fn the_provider_completes_once_released() {
        let gate = Gate::closed("provider");
        let provider = Arc::new(GatedProvider::new(&gate, "done"));
        let running = Arc::clone(&provider);
        let call = residuum_core::util::spawn_in_span(async move {
            running
                .complete(&[], &[], &CompletionOptions::default())
                .await
        });
        gate.until_held(1).await;
        assert!(!call.is_finished(), "a closed gate holds the completion");
        gate.release(1);
        let response = wait::guarded("the completion", call)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.content, "done");
        assert_eq!(provider.model_name(), "gated");
    }
}
