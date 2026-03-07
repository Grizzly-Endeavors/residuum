//! Tunnel protocol frame types.
//!
//! Shared between the relay server and tunnel client. Bodies are base64-encoded
//! strings; headers are flattened `HashMap<String, String>`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A single frame exchanged over the tunnel WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TunnelFrame {
    /// Sent by the relay after a successful tunnel registration.
    Connected {
        user_id: String,
        keepalive_interval_secs: u64,
    },
    /// Keepalive ping (relay → client).
    Ping,
    /// Keepalive pong (client → relay).
    Pong,
    /// Proxied HTTP request (relay → client).
    HttpRequest {
        request_id: String,
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Option<String>,
    },
    /// Proxied HTTP response (client → relay).
    HttpResponse {
        request_id: String,
        status: u16,
        headers: HashMap<String, String>,
        body: Option<String>,
    },
    /// Open a WebSocket channel through the tunnel (relay → client).
    WsOpen {
        channel_id: String,
        path: String,
        headers: HashMap<String, String>,
    },
    /// Result of a WebSocket open attempt (client → relay).
    WsOpenResult { channel_id: String, success: bool },
    /// A WebSocket message forwarded through the tunnel.
    WsMessage { channel_id: String, data: String },
    /// Close a WebSocket channel.
    WsClose { channel_id: String },
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn round_trip_connected() {
        let frame = TunnelFrame::Connected {
            user_id: "bear".to_string(),
            keepalive_interval_secs: 30,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let parsed: TunnelFrame = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(parsed, TunnelFrame::Connected { user_id, keepalive_interval_secs } if user_id == "bear" && keepalive_interval_secs == 30),
            "connected frame should round-trip"
        );
    }

    #[test]
    fn round_trip_ping_pong() {
        for frame in [TunnelFrame::Ping, TunnelFrame::Pong] {
            let json = serde_json::to_string(&frame).unwrap();
            let _: TunnelFrame = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn round_trip_http_request() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        let frame = TunnelFrame::HttpRequest {
            request_id: "req-1".to_string(),
            method: "POST".to_string(),
            path: "/api/test".to_string(),
            headers,
            body: Some("eyJrZXkiOiJ2YWx1ZSJ9".to_string()),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let parsed: TunnelFrame = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(parsed, TunnelFrame::HttpRequest { request_id, .. } if request_id == "req-1"),
            "http request should round-trip"
        );
    }

    #[test]
    fn round_trip_http_response() {
        let frame = TunnelFrame::HttpResponse {
            request_id: "req-1".to_string(),
            status: 200,
            headers: HashMap::new(),
            body: None,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let parsed: TunnelFrame = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(parsed, TunnelFrame::HttpResponse { status: 200, .. }),
            "http response should round-trip"
        );
    }

    #[test]
    fn round_trip_ws_frames() {
        let frames = vec![
            TunnelFrame::WsOpen {
                channel_id: "ch-1".to_string(),
                path: "/ws".to_string(),
                headers: HashMap::new(),
            },
            TunnelFrame::WsOpenResult {
                channel_id: "ch-1".to_string(),
                success: true,
            },
            TunnelFrame::WsMessage {
                channel_id: "ch-1".to_string(),
                data: "hello".to_string(),
            },
            TunnelFrame::WsClose {
                channel_id: "ch-1".to_string(),
            },
        ];
        for frame in frames {
            let json = serde_json::to_string(&frame).unwrap();
            let _: TunnelFrame = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn deserialize_tagged_format() {
        let json = r#"{"type":"ping"}"#;
        let frame: TunnelFrame = serde_json::from_str(json).unwrap();
        assert!(
            matches!(frame, TunnelFrame::Ping),
            "tagged format should deserialize"
        );
    }
}
