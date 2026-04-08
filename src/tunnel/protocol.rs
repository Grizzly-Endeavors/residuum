//! Tunnel protocol frame types.
//!
//! Shared between the relay server and tunnel client. Bodies are base64-encoded
//! strings; headers are flattened `HashMap<String, String>`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A single frame exchanged over the tunnel WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TunnelFrame {
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
#[expect(clippy::panic, reason = "test code uses panic for clarity")]
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
        {
            let json = serde_json::to_string(&TunnelFrame::Ping).unwrap();
            assert!(matches!(
                serde_json::from_str::<TunnelFrame>(&json).unwrap(),
                TunnelFrame::Ping
            ));
        }
        {
            let json = serde_json::to_string(&TunnelFrame::Pong).unwrap();
            assert!(matches!(
                serde_json::from_str::<TunnelFrame>(&json).unwrap(),
                TunnelFrame::Pong
            ));
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
        if let TunnelFrame::HttpRequest {
            request_id,
            method,
            path,
            headers: parsed_headers,
            body,
        } = parsed
        {
            assert_eq!(request_id, "req-1");
            assert_eq!(method, "POST");
            assert_eq!(path, "/api/test");
            assert_eq!(
                parsed_headers.get("content-type").map(String::as_str),
                Some("application/json")
            );
            assert_eq!(body.as_deref(), Some("eyJrZXkiOiJ2YWx1ZSJ9"));
        } else {
            panic!("expected HttpRequest variant");
        }
    }

    #[test]
    fn round_trip_http_response() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/plain".to_string());
        let frame = TunnelFrame::HttpResponse {
            request_id: "req-1".to_string(),
            status: 200,
            headers,
            body: Some("aGVsbG8=".to_string()),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let parsed: TunnelFrame = serde_json::from_str(&json).unwrap();
        if let TunnelFrame::HttpResponse {
            request_id,
            status,
            headers: parsed_headers,
            body,
        } = parsed
        {
            assert_eq!(request_id, "req-1");
            assert_eq!(status, 200);
            assert_eq!(
                parsed_headers.get("content-type").map(String::as_str),
                Some("text/plain")
            );
            assert_eq!(body.as_deref(), Some("aGVsbG8="));
        } else {
            panic!("expected HttpResponse variant");
        }
    }

    #[test]
    fn round_trip_ws_open() {
        let json = serde_json::to_string(&TunnelFrame::WsOpen {
            channel_id: "ch-1".to_string(),
            path: "/ws".to_string(),
            headers: HashMap::new(),
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::WsOpen { channel_id, path, .. } if channel_id == "ch-1" && path == "/ws"),
            "WsOpen should round-trip with correct fields"
        );
    }

    #[test]
    fn round_trip_ws_open_result_success() {
        let json = serde_json::to_string(&TunnelFrame::WsOpenResult {
            channel_id: "ch-1".to_string(),
            success: true,
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::WsOpenResult { channel_id, success } if channel_id == "ch-1" && success),
            "WsOpenResult success=true should round-trip"
        );
    }

    #[test]
    fn round_trip_ws_open_result_failure() {
        let json = serde_json::to_string(&TunnelFrame::WsOpenResult {
            channel_id: "ch-1".to_string(),
            success: false,
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::WsOpenResult { channel_id, success } if channel_id == "ch-1" && !success),
            "WsOpenResult success=false should round-trip"
        );
    }

    #[test]
    fn round_trip_ws_message() {
        let json = serde_json::to_string(&TunnelFrame::WsMessage {
            channel_id: "ch-1".to_string(),
            data: "hello".to_string(),
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::WsMessage { channel_id, data } if channel_id == "ch-1" && data == "hello"),
            "WsMessage should round-trip with correct fields"
        );
    }

    #[test]
    fn round_trip_ws_close() {
        let json = serde_json::to_string(&TunnelFrame::WsClose {
            channel_id: "ch-1".to_string(),
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::WsClose { channel_id } if channel_id == "ch-1"),
            "WsClose should round-trip with correct fields"
        );
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

    #[test]
    fn deserialize_unknown_type_returns_error() {
        let json = r#"{"type":"unknown_frame"}"#;
        assert!(
            serde_json::from_str::<TunnelFrame>(json).is_err(),
            "unknown frame type should return an error"
        );
    }

    #[test]
    fn deserialize_malformed_json_returns_error() {
        let json = "{malformed json}";
        assert!(
            serde_json::from_str::<TunnelFrame>(json).is_err(),
            "malformed JSON should return an error"
        );
    }
}
