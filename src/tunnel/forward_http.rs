//! HTTP request forwarding to the local residuum instance.

use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tracing::{debug, warn};

use super::protocol::TunnelFrame;

/// Hop-by-hop headers that must not be forwarded between proxy and backend.
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "transfer-encoding",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
];

/// Returns `true` if the given header name is a hop-by-hop header.
#[must_use]
fn is_hop_by_hop(name: &str) -> bool {
    let lower = name.to_lowercase();
    HOP_BY_HOP_HEADERS.iter().any(|h| *h == lower)
}

/// Forward an HTTP request to the local residuum instance and return the
/// response as a [`TunnelFrame::HttpResponse`].
///
/// The request body is expected to be base64-encoded (if present). The response
/// body is base64-encoded before returning.
///
/// On any error (connection refused, timeout, etc.) a 502 response is returned
/// instead of propagating the error.
///
/// # Errors
///
/// This function does not return errors directly; failures are encoded as 502
/// HTTP responses in the returned `TunnelFrame`.
pub(super) async fn forward(
    client: &reqwest::Client,
    port: u16,
    request_id: String,
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Option<String>,
) -> TunnelFrame {
    let url = format!("http://localhost:{port}{path}");
    debug!(request_id, method, url, "forwarding HTTP request to local");

    let http_method = match method.to_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        "HEAD" => reqwest::Method::HEAD,
        "OPTIONS" => reqwest::Method::OPTIONS,
        other => {
            warn!(request_id, method = other, "unsupported HTTP method");
            return error_response(request_id, 502, &format!("unsupported method: {other}"));
        }
    };

    // Decode the base64-encoded body if present.
    let decoded_body = match body {
        Some(ref b64) => match STANDARD.decode(b64) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                warn!(request_id, error = %e, "failed to decode request body");
                return error_response(request_id, 502, &format!("base64 decode error: {e}"));
            }
        },
        None => None,
    };

    // Build the request.
    let mut req = client.request(http_method, &url);

    for (name, value) in &headers {
        if !is_hop_by_hop(name) {
            req = req.header(name, value);
        }
    }

    if let Some(bytes) = decoded_body {
        req = req.body(bytes);
    }

    // Send the request.
    let response = match req.send().await {
        Ok(resp) => resp,
        Err(e) => {
            warn!(request_id, error = %e, "failed to forward request to local");
            return error_response(request_id, 502, &format!("upstream error: {e}"));
        }
    };

    let status = response.status().as_u16();

    // Collect response headers, filtering hop-by-hop.
    let mut response_headers = HashMap::new();
    for (name, value) in response.headers() {
        if !is_hop_by_hop(name.as_str())
            && let Ok(v) = value.to_str()
        {
            response_headers.insert(name.to_string(), v.to_string());
        }
    }

    // Read and base64-encode the response body.
    let response_body = match response.bytes().await {
        Ok(bytes) => {
            if bytes.is_empty() {
                None
            } else {
                Some(STANDARD.encode(&bytes))
            }
        }
        Err(e) => {
            warn!(request_id, error = %e, "failed to read response body");
            return error_response(request_id, 502, &format!("failed to read response: {e}"));
        }
    };

    debug!(request_id, status, "forwarded HTTP request successfully");

    TunnelFrame::HttpResponse {
        request_id,
        status,
        headers: response_headers,
        body: response_body,
    }
}

/// Build a 502-style error response frame.
#[must_use]
fn error_response(request_id: String, status: u16, message: &str) -> TunnelFrame {
    TunnelFrame::HttpResponse {
        request_id,
        status,
        headers: HashMap::new(),
        body: Some(STANDARD.encode(message.as_bytes())),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_detection() {
        assert!(is_hop_by_hop("Connection"), "Connection is hop-by-hop");
        assert!(
            is_hop_by_hop("transfer-encoding"),
            "transfer-encoding is hop-by-hop"
        );
        assert!(is_hop_by_hop("Keep-Alive"), "Keep-Alive is hop-by-hop");
        assert!(
            !is_hop_by_hop("content-type"),
            "content-type is not hop-by-hop"
        );
        assert!(
            !is_hop_by_hop("authorization"),
            "authorization is not hop-by-hop"
        );
    }

    #[test]
    fn error_response_encodes_body() {
        let frame = error_response("req-1".to_string(), 502, "test error");
        assert!(
            matches!(frame, TunnelFrame::HttpResponse { .. }),
            "expected HttpResponse variant"
        );
        if let TunnelFrame::HttpResponse {
            request_id,
            status,
            body,
            ..
        } = frame
        {
            assert!(request_id == "req-1", "request_id should match");
            assert!(status == 502, "status should be 502");
            let decoded = STANDARD.decode(body.unwrap()).unwrap();
            let text = String::from_utf8(decoded).unwrap();
            assert!(text == "test error", "body should contain error message");
        }
    }
}
