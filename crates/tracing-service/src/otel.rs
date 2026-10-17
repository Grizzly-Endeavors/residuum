//! OTEL span conversion and exporter construction.
//!
//! Converts in-memory `CompletedSpan` structs from the span buffer into
//! OpenTelemetry `SpanData` for export via OTLP HTTP/protobuf, and builds the
//! exporter that sends them.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use opentelemetry_sdk::trace::{SpanData, SpanEvents, SpanLinks};

use residuum_config::OtelEndpoint;
use residuum_core::util::telemetry::CompletedSpan;

/// Service name reported in OTEL traces.
const SERVICE_NAME: &str = "residuum";

/// An OTLP span exporter together with the transport log of its HTTP client.
pub(super) struct OtlpExporter {
    pub(super) exporter: opentelemetry_otlp::SpanExporter,
    pub(super) transport: TransportLog,
}

/// Build an OTLP HTTP/protobuf span exporter for the given endpoint.
///
/// The exporter sends through [`DiagnosedClient`], so a failed export can
/// name the transport error behind it (see [`TransportLog::describe_failure`]).
///
/// # Errors
/// Returns an error if the HTTP client or the exporter cannot be constructed.
pub(super) fn build_exporter(
    endpoint: &OtelEndpoint,
) -> Result<OtlpExporter, opentelemetry_otlp::ExporterBuildError> {
    use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};

    let mut headers = std::collections::HashMap::new();
    for (k, v) in &endpoint.headers {
        headers.insert(k.clone(), v.clone());
    }

    // The exporter applies its timeout only to its own built-in client, so a
    // supplied client carries the same default itself.
    let inner = reqwest::Client::builder()
        .timeout(opentelemetry_otlp::OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT)
        .build()
        .map_err(|e| {
            opentelemetry_otlp::ExporterBuildError::InternalFailure(format!(
                "failed to build the HTTP client for {}: {e}",
                endpoint.url
            ))
        })?;
    let transport = TransportLog::default();
    let client = DiagnosedClient {
        inner,
        transport: transport.clone(),
    };

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_http_client(client)
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(&endpoint.url)
        .with_headers(headers)
        .build()?;
    Ok(OtlpExporter {
        exporter,
        transport,
    })
}

/// What an exporter's HTTP client saw: how many requests it sent and the
/// transport error of the last one, if it failed.
///
/// The OTLP exporter reports a transport failure only as "network error", and
/// logs the cause at debug level under its own target, which the log filter
/// drops. Keeping the cause here lets the caller show it to the user and log it.
#[derive(Debug, Clone, Default)]
pub(super) struct TransportLog(Arc<Mutex<TransportRecord>>);

#[derive(Debug, Default)]
struct TransportRecord {
    attempts: usize,
    last_error: Option<String>,
}

impl TransportLog {
    fn record(&self, url: &str, error: Option<&HttpError>) {
        let cause = error.map(|e| error_chain(e.as_ref()));
        if let Some(cause) = &cause {
            tracing::debug!(url, error = %cause, "otlp export request failed");
        }
        let mut record = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        record.attempts += 1;
        record.last_error = cause;
    }

    /// Number of HTTP requests the exporter has sent, retries included.
    pub(super) fn attempts(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .attempts
    }

    /// The exporter's error, followed by the transport error of the last
    /// request when that request failed before getting a response.
    pub(super) fn describe_failure(&self, error: &impl std::fmt::Display) -> String {
        let record = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match &record.last_error {
            Some(cause) => format!("{error} ({cause})"),
            None => error.to_string(),
        }
    }
}

/// An error's message followed by the messages of its sources, skipping a
/// source whose message the chain already contains.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        let text = inner.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = inner.source();
    }
    message
}

/// The OTLP exporter's HTTP client: reqwest, recording each request's outcome
/// in a [`TransportLog`].
#[derive(Debug)]
struct DiagnosedClient {
    inner: reqwest::Client,
    transport: TransportLog,
}

#[async_trait::async_trait]
impl HttpClient for DiagnosedClient {
    async fn send_bytes(&self, request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        let url = request.uri().to_string();
        let result = self.inner.send_bytes(request).await;
        self.transport.record(&url, result.as_ref().err());
        result
    }
}

/// Encode a batch of OTEL `SpanData` into the OTLP protobuf wire format
/// (`ExportTraceServiceRequest`) for embedding in a bug-report payload.
pub(super) fn encode_otlp_protobuf(spans: Vec<SpanData>) -> Vec<u8> {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::transform::common::tonic::ResourceAttributesWithSchema;
    use opentelemetry_proto::transform::trace::tonic::group_spans_by_resource_and_scope;
    use opentelemetry_sdk::Resource;
    use prost::Message;

    let resource = Resource::builder().with_service_name(SERVICE_NAME).build();
    let resource: ResourceAttributesWithSchema = (&resource).into();
    let resource_spans = group_spans_by_resource_and_scope(spans, &resource);
    let request = ExportTraceServiceRequest { resource_spans };
    request.encode_to_vec()
}

/// Convert a batch of `CompletedSpan` into OTEL `SpanData`.
pub(super) fn convert_spans(spans: &[CompletedSpan]) -> Vec<SpanData> {
    // Generate a single trace ID for the batch (all spans in one trace dump
    // get grouped together for coherent viewing in trace UIs).
    let trace_id = TraceId::from_bytes(rand_trace_id());
    let scope = InstrumentationScope::builder(SERVICE_NAME)
        .with_version(env!("CARGO_PKG_VERSION"))
        .build();

    spans
        .iter()
        .map(|s| convert_one(s, trace_id, &scope))
        .collect()
}

/// Convert a single `CompletedSpan` to OTEL `SpanData`.
fn convert_one(span: &CompletedSpan, trace_id: TraceId, scope: &InstrumentationScope) -> SpanData {
    let span_id = SpanId::from_bytes(span.span_id.to_be_bytes());
    let parent_span_id = span
        .parent_id
        .map_or(SpanId::INVALID, |id| SpanId::from_bytes(id.to_be_bytes()));

    let span_context = SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::SAMPLED,
        false,
        TraceState::default(),
    );

    let mut attributes = Vec::with_capacity(span.fields.len() + 2);
    attributes.push(KeyValue::new("target", span.target.clone()));
    attributes.push(KeyValue::new("level", span.level.to_string()));
    for (key, value) in &span.fields {
        attributes.push(KeyValue::new(key.clone(), value.clone()));
    }

    let events: Vec<opentelemetry::trace::Event> = span
        .events
        .iter()
        .map(|e| {
            let mut event_attrs: Vec<KeyValue> = e
                .fields
                .iter()
                .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
                .collect();
            if !e.message.is_empty() {
                event_attrs.push(KeyValue::new("message", e.message.clone()));
            }
            opentelemetry::trace::Event::new(
                Cow::Owned(e.level.to_string()),
                e.timestamp,
                event_attrs,
                0,
            )
        })
        .collect();

    let end_time = span
        .start
        .checked_add(span.duration)
        .unwrap_or(SystemTime::now());

    SpanData {
        span_context,
        parent_span_id,
        parent_span_is_remote: false,
        span_kind: SpanKind::Internal,
        name: Cow::Owned(span.name.clone()),
        start_time: span.start,
        end_time,
        attributes,
        dropped_attributes_count: 0,
        events: {
            let mut se = SpanEvents::default();
            se.events = events;
            se
        },
        links: SpanLinks::default(),
        status: Status::Ok,
        instrumentation_scope: scope.clone(),
    }
}

/// Generate a random 16-byte trace ID.
fn rand_trace_id() -> [u8; 16] {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    let h1 = hasher.finish();

    let mut hasher2 = DefaultHasher::new();
    h1.hash(&mut hasher2);
    // Mix in some entropy from the thread ID
    std::thread::current().id().hash(&mut hasher2);
    let h2 = hasher2.finish();

    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&h1.to_be_bytes());
    bytes[8..].copy_from_slice(&h2.to_be_bytes());
    bytes
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes known-length slices"
)]
mod tests {
    use super::*;
    use opentelemetry_sdk::trace::SpanExporter as _;
    use std::time::Duration;

    fn make_test_span() -> CompletedSpan {
        CompletedSpan {
            span_id: 42,
            parent_id: Some(1),
            name: "test_operation".to_string(),
            target: "residuum_tracing_service::test".to_string(),
            level: tracing::Level::INFO,
            start: SystemTime::now(),
            duration: Duration::from_millis(100),
            fields: vec![
                ("key1".to_string(), "value1".to_string()),
                ("key2".to_string(), "value2".to_string()),
            ],
            events: vec![residuum_core::util::telemetry::SpanEvent {
                timestamp: SystemTime::now(),
                level: tracing::Level::WARN,
                message: "something happened".to_string(),
                fields: vec![("detail".to_string(), "more info".to_string())],
            }],
        }
    }

    #[test]
    fn convert_spans_produces_correct_count() {
        let spans = vec![make_test_span(), make_test_span()];
        let result = convert_spans(&spans);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn convert_preserves_span_name() {
        let spans = vec![make_test_span()];
        let result = convert_spans(&spans);
        assert_eq!(result[0].name, "test_operation");
    }

    #[test]
    fn convert_preserves_parent_relationship() {
        let spans = vec![make_test_span()];
        let result = convert_spans(&spans);
        assert_ne!(result[0].parent_span_id, SpanId::INVALID);
    }

    #[test]
    fn convert_includes_fields_as_attributes() {
        let spans = vec![make_test_span()];
        let result = convert_spans(&spans);
        let attr_keys: Vec<&str> = result[0]
            .attributes
            .iter()
            .map(|kv| kv.key.as_str())
            .collect();
        assert!(attr_keys.contains(&"key1"));
        assert!(attr_keys.contains(&"key2"));
        assert!(attr_keys.contains(&"target"));
        assert!(attr_keys.contains(&"level"));
    }

    #[test]
    fn convert_includes_events() {
        let spans = vec![make_test_span()];
        let result = convert_spans(&spans);
        assert_eq!(result[0].events.events.len(), 1);
    }

    #[test]
    fn convert_empty_spans_returns_empty() {
        let result = convert_spans(&[]);
        assert!(result.is_empty(), "expected {result:?} to be empty");
    }

    #[test]
    fn rand_trace_id_is_nonzero() {
        let id = rand_trace_id();
        assert_ne!(id, [0_u8; 16]);
    }

    fn endpoint(port: u16) -> OtelEndpoint {
        OtelEndpoint {
            url: format!("http://127.0.0.1:{port}/v1/traces"),
            name: None,
            headers: std::collections::HashMap::new(),
        }
    }

    /// Answer every request on `listener` with `401 Unauthorized`, reading
    /// each request in full first so the client sees the response, not a reset.
    async fn serve_unauthorized(listener: tokio::net::TcpListener) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut buf = [0_u8; 4096];
            while let Ok(n) = stream.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                let Some(head_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&request[..head_end]).to_lowercase();
                let body_len = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= head_end + 4 + body_len {
                    break;
                }
            }
            let response =
                b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
            drop(stream.write_all(response).await);
            drop(stream.shutdown().await);
        }
    }

    #[tokio::test]
    async fn refused_connection_names_the_transport_cause_after_retrying() {
        // Bound but not listening, so every connection is refused.
        let port = residuum_core::testing::ports::reserve_port();
        let otlp = build_exporter(&endpoint(port.port())).unwrap();

        let err = otlp.exporter.export(Vec::new()).await.unwrap_err();
        let described = otlp.transport.describe_failure(&err);

        assert!(
            described.starts_with(&err.to_string()) && described.contains("error sending request"),
            "expected the exporter's error followed by the transport cause, got {described:?}"
        );
        assert!(
            otlp.transport.attempts() > 1,
            "a refused connection is retried, got {} attempt(s)",
            otlp.transport.attempts()
        );
    }

    #[tokio::test]
    async fn status_error_has_no_transport_cause_and_is_not_retried() {
        let listener = residuum_core::testing::ports::reserve_port().into_tokio_listener();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(serve_unauthorized(listener));
        let otlp = build_exporter(&endpoint(port)).unwrap();

        let err = otlp.exporter.export(Vec::new()).await.unwrap_err();
        let described = otlp.transport.describe_failure(&err);
        server.abort();

        assert_eq!(described, err.to_string());
        assert!(described.contains("401"), "got {described:?}");
        assert_eq!(otlp.transport.attempts(), 1);
    }

    #[test]
    fn error_chain_joins_sources_without_repeating_them() {
        #[derive(Debug)]
        struct Outer {
            message: &'static str,
            source: std::io::Error,
        }
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.message)
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.source)
            }
        }
        let refused = || std::io::Error::other("connection refused");

        let err = Outer {
            message: "error sending request",
            source: refused(),
        };
        assert_eq!(
            error_chain(&err),
            "error sending request: connection refused"
        );

        let repeated = Outer {
            message: "tcp connect error: connection refused",
            source: refused(),
        };
        assert_eq!(
            error_chain(&repeated),
            "tcp connect error: connection refused"
        );
    }
}
