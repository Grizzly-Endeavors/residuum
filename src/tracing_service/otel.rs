//! OTEL span conversion and exporter construction.
//!
//! Converts in-memory `CompletedSpan` structs from the span buffer into
//! OpenTelemetry `SpanData` for export via OTLP HTTP/protobuf.

use std::borrow::Cow;
use std::time::SystemTime;

use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::trace::{SpanData, SpanEvents, SpanLinks};

use crate::config::OtelEndpoint;
use crate::util::telemetry::CompletedSpan;

/// Service name reported in OTEL traces.
const SERVICE_NAME: &str = "residuum";

/// Build an OTLP HTTP/protobuf span exporter for the given endpoint.
///
/// # Errors
/// Returns an error if the exporter cannot be constructed.
pub(super) fn build_exporter(
    endpoint: &OtelEndpoint,
) -> Result<opentelemetry_otlp::SpanExporter, opentelemetry_otlp::ExporterBuildError> {
    use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};

    let mut headers = std::collections::HashMap::new();
    for (k, v) in &endpoint.headers {
        headers.insert(k.clone(), v.clone());
    }

    opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(&endpoint.url)
        .with_headers(headers)
        .build()
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
    use std::time::Duration;

    fn make_test_span() -> CompletedSpan {
        CompletedSpan {
            span_id: 42,
            parent_id: Some(1),
            name: "test_operation".to_string(),
            target: "residuum::test".to_string(),
            level: tracing::Level::INFO,
            start: SystemTime::now(),
            duration: Duration::from_millis(100),
            fields: vec![
                ("key1".to_string(), "value1".to_string()),
                ("key2".to_string(), "value2".to_string()),
            ],
            events: vec![crate::util::telemetry::SpanEvent {
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
        assert!(result.is_empty());
    }

    #[test]
    fn rand_trace_id_is_nonzero() {
        let id = rand_trace_id();
        assert_ne!(id, [0_u8; 16]);
    }
}
