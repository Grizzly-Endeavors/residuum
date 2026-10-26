//! Fireworks AI, an OpenAI-compatible host with a few quirks of its own.
//!
//! Requests and responses use the shared wire format in [`super::openai`];
//! this module builds the [`OpenAiDialect`] that carries what differs: cache
//! hits reported in response headers rather than the `usage` body, no hosted
//! web search tool, and routing by an affinity key.

use super::openai::OpenAiDialect;

/// Response header carrying the prompt tokens served from cache.
const CACHED_PROMPT_TOKENS_HEADER: &str = "fireworks-cached-prompt-tokens";

/// Request header that pins requests to a replica holding a warm cache.
const SESSION_AFFINITY_HEADER: &str = "x-session-affinity";

/// The dialect Fireworks speaks, with `session_affinity` as the value of the
/// `x-session-affinity` header when there is one.
pub(crate) fn dialect(session_affinity: Option<String>) -> OpenAiDialect {
    OpenAiDialect {
        request_headers: session_affinity
            .map(|affinity| (SESSION_AFFINITY_HEADER, affinity))
            .into_iter()
            .collect(),
        cached_tokens_from_headers: Some(cached_prompt_tokens),
        hosted_web_search: false,
        always_replays_reasoning: true,
    }
}

/// Parse the Fireworks cached-prompt-token header, logging a malformed value.
fn cached_prompt_tokens(headers: &reqwest::header::HeaderMap) -> Option<u32> {
    let raw = headers.get(CACHED_PROMPT_TOKENS_HEADER)?;
    let parsed = raw.to_str().ok().and_then(|v| v.trim().parse::<u32>().ok());
    if parsed.is_none() {
        tracing::warn!(
            header = CACHED_PROMPT_TOKENS_HEADER,
            value = ?raw,
            "fireworks returned an unparseable cached-token header; cache stats unavailable for this call"
        );
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpClientConfig, SharedHttpClient};
    use crate::providers::openai::OpenAiClient;
    use crate::test_support::{RecordingSink, ScriptedServer, Step};
    use crate::{
        CompletionOptions, InferenceProvider, Message, ProviderApi, ThinkingBlock, ThinkingOrigin,
        ToolCall,
    };
    use residuum_config::RetryConfig;
    use serde_json::{Value, json};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(url: impl Into<String>, model: &str, api_key: &str) -> OpenAiClient {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        OpenAiClient::with_http_client_and_api_key(
            http,
            url,
            model,
            api_key,
            RetryConfig::no_retry(),
        )
    }

    fn make_fireworks_client(url: impl Into<String>, affinity: Option<&str>) -> OpenAiClient {
        make_client(url, "accounts/fireworks/models/test", "fw-key")
            .with_dialect(dialect(affinity.map(String::from)))
    }

    fn fireworks_usage_body(cached_in_body: Option<u32>) -> Value {
        let usage = match cached_in_body {
            Some(cached) => json!({
                "prompt_tokens": 1000,
                "completion_tokens": 10,
                "prompt_tokens_details": {"cached_tokens": cached}
            }),
            None => json!({"prompt_tokens": 1000, "completion_tokens": 10}),
        };
        json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}}],
            "usage": usage
        })
    }

    #[tokio::test]
    async fn complete_passes_through_a_malformed_tool_call_name_verbatim() {
        // Fireworks (and vLLM, upstream) have shipped tool-call parsers for
        // GLM models that fail to strip the model's own `<arg_key>`/
        // `<arg_value>` tool-call template out of a structured response,
        // leaking it into `function.name` instead of a clean `write_file`.
        // This client has no name/markup parsing of its own — it's a thin
        // OpenAI-compatible wire decoder — so it must pass the field through
        // exactly as the API returned it. Downstream (`agent::turn`) is
        // responsible for recognizing the shape is bogus and failing
        // legibly instead of chasing it through the tool registries.
        let mock_server = MockServer::start().await;
        let malformed_name = "write_file\tcontent</arg_key><arg_value># Hello";

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_glm_1",
                            "type": "function",
                            "function": {
                                "name": malformed_name,
                                "arguments": "{}"
                            }
                        }]
                    }
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let messages = vec![Message::user("write a file")];

        let response = client
            .complete(&messages, &[], &CompletionOptions::default())
            .await
            .unwrap();

        assert_eq!(
            response.tool_calls.first().map(|t| t.name.as_str()),
            Some(malformed_name),
            "the client must not attempt to repair or hand-parse markup out of the tool name; \
             that decision belongs to the caller, not the wire-format decoder"
        );
    }

    #[tokio::test]
    async fn fireworks_sends_session_affinity_header() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("x-session-affinity", "residuum-main-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fireworks_usage_body(None)))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), Some("residuum-main-abc"));
        client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn fireworks_reads_cached_tokens_from_header_when_body_lacks_them() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("fireworks-cached-prompt-tokens", "768")
                    .set_body_json(fireworks_usage_body(None)),
            )
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let response = client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(
            response.usage.and_then(|u| u.cache_read_tokens),
            Some(768),
            "header cache count should fill the gap"
        );
    }

    #[tokio::test]
    async fn fireworks_prefers_body_cached_tokens_over_header() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("fireworks-cached-prompt-tokens", "1")
                    .set_body_json(fireworks_usage_body(Some(512))),
            )
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let response = client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(
            response.usage.and_then(|u| u.cache_read_tokens),
            Some(512),
            "body cache count is authoritative when present"
        );
    }

    #[tokio::test]
    async fn fireworks_malformed_cache_header_leaves_stats_empty() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("fireworks-cached-prompt-tokens", "lots")
                    .set_body_json(fireworks_usage_body(None)),
            )
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let response = client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 1000, "usage still parses");
        assert_eq!(
            usage.cache_read_tokens, None,
            "bad header is not guessed at"
        );
    }

    #[tokio::test]
    async fn openai_dialect_ignores_fireworks_cache_header() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("fireworks-cached-prompt-tokens", "768")
                    .set_body_json(fireworks_usage_body(None)),
            )
            .mount(&mock_server)
            .await;

        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let client = OpenAiClient::with_http_client(
            http,
            mock_server.uri(),
            "gpt-4",
            RetryConfig::no_retry(),
        );
        let response = client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(
            response.usage.and_then(|u| u.cache_read_tokens),
            None,
            "only the fireworks dialect trusts fireworks headers"
        );
    }

    #[tokio::test]
    async fn fireworks_never_sends_hosted_web_search_tool() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fireworks_usage_body(None)))
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let options = CompletionOptions {
            web_search: Some(crate::WebSearchNativeConfig::default()),
            ..CompletionOptions::default()
        };
        client
            .complete(&[Message::user("hi")], &[], &options)
            .await
            .unwrap();

        let requests = mock_server.received_requests().await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert!(
            body.get("tools").is_none(),
            "fireworks rejects web_search_preview, so no tools should be sent: {body}"
        );
    }

    #[tokio::test]
    async fn fireworks_cache_header_is_used_for_streamed_usage() {
        let stream = [
            format!(
                "data: {}\n\n",
                json!({"choices": [{"index": 0, "delta": {"content": "x"}, "finish_reason": null}]})
            ),
            format!(
                "data: {}\n\n",
                json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
            ),
            format!(
                "data: {}\n\n",
                json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 2}})
            ),
            "data: [DONE]\n\n".to_string(),
        ]
        .concat();
        let script = vec![
            Step::Write(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                  fireworks-cached-prompt-tokens: 7\r\ntransfer-encoding: chunked\r\n\
                  connection: close\r\n\r\n"
                    .to_vec(),
            ),
            Step::chunk(stream),
            Step::end(),
        ];
        let server = ScriptedServer::start(vec![script]).await;
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let client =
            OpenAiClient::with_http_client(http, server.uri(), "m", RetryConfig::no_retry())
                .with_dialect(dialect(None));
        let sink = RecordingSink::default();
        let result = client
            .complete_streaming(
                &[Message::user("hi")],
                &[],
                &CompletionOptions::default(),
                &sink,
            )
            .await;
        assert_eq!(
            result.unwrap().usage.unwrap().cache_read_tokens,
            Some(7),
            "cache hits come from the header when the body lacks them"
        );
    }

    #[tokio::test]
    async fn fireworks_gets_reasoning_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fireworks_usage_body(None)))
            .mount(&server)
            .await;

        let origin = ThinkingOrigin::new(ProviderApi::OpenAiCompatible, "m");
        let mut current = Message::assistant(
            "",
            Some(vec![ToolCall {
                id: "c1".to_string(),
                name: "exec".to_string(),
                arguments: json!({}),
                server: None,
            }]),
        );
        current.thinking = vec![ThinkingBlock::text("current reasoning").with_origin(&origin)];
        let messages = [Message::user("two"), current, Message::tool("r1", "c1")];

        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let client =
            OpenAiClient::with_http_client(http, server.uri(), "m", RetryConfig::no_retry())
                .with_dialect(dialect(None));
        client
            .complete(&messages, &[], &CompletionOptions::default())
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert_eq!(
            body.pointer("/messages/1/reasoning_content"),
            Some(&json!("current reasoning")),
            "Fireworks understands reasoning_content: {body}"
        );
    }
}
