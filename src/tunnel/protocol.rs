//! Tunnel protocol frame types.
//!
//! Shared between the relay server and tunnel client. Bodies are base64-encoded
//! strings; headers are flattened `HashMap<String, String>`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Which local listener a proxied request or socket open is for. Frames with
/// no surface go to the main gateway listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Surface {
    /// The workbench artifacts listener, for `{user}.workbench.<relay>` hosts.
    /// Both `HttpRequest` and `WsOpen` frames carry it; a socket open on this
    /// surface reaches the hub's API sockets through that listener.
    Workbench,
    /// The A2A listener, for `{relay}/a2a/{instance}/{agent}/*` routes. Always answered in
    /// streamed form (`HttpResponseStart`/`Chunk`/`End`), never a buffered
    /// `HttpResponse`. It serves no sockets, so a `WsOpen` on this surface is
    /// refused.
    #[serde(rename = "a2a")]
    A2a,
    /// One agent's Teams listener, for `{origin}/teams/{instance}/{agent}`.
    /// The relay forwards only `POST` of a Bot Framework activity, and the hub
    /// delivers it to that agent's listener as `POST /api/teams/messages`.
    /// Answered with a buffered `HttpResponse`. It serves no sockets.
    Teams,
}

/// One agent this hub advertises to the relay in a
/// [`TunnelFrame::AgentsUpdate`] frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentInfo {
    /// The agent's name: its identity and its A2A path segment.
    pub name: String,
    /// A human-readable label for the agent, shown in the relay's A2A
    /// directory.
    pub display_name: String,
    /// Whether the agent answers A2A requests right now.
    pub a2a_enabled: bool,
    /// Whether the agent is gated by caller key or sibling attestation,
    /// rather than open to anyone.
    pub a2a_private: bool,
    /// Whether the agent has a complete Teams configuration. The relay accepts
    /// Teams messages for it while this is set, including while the agent is
    /// stopped: the hub answers those with a retry until the listener is back.
    /// Absent on a hub that predates Teams-through-the-relay, which the relay
    /// reads as not configured.
    #[serde(default)]
    pub teams_configured: bool,
}

/// A single frame exchanged over the tunnel WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TunnelFrame {
    /// Sent by the relay after a successful tunnel registration.
    Connected {
        user_id: String,
        keepalive_interval_secs: u64,
        /// Public origin of this user's web UI through the relay.
        #[serde(default)]
        origin: Option<String>,
        /// Public origin of this user's workbench artifacts through the relay.
        #[serde(default)]
        workbench_origin: Option<String>,
        /// This instance's slug, as the relay knows it. Absent on older relays.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance: Option<String>,
        /// Sibling credential minted for this connection (`rsa_` + 32
        /// alphanumeric characters), valid only while this tunnel is
        /// connected. Absent on older relays and never logged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        a2a_token: Option<String>,
    },
    /// The hub's full agent list (client → relay). Sent after `Connected` and
    /// again whenever the list changes. The relay replaces its stored list
    /// for this instance, so resending an unchanged list is harmless.
    AgentsUpdate { agents: Vec<AgentInfo> },
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface: Option<Surface>,
        /// The agent an A2A-surface request is for, set by the relay on
        /// `/a2a/{instance}/{agent}` requests. Absent otherwise.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// Proxied HTTP response (client → relay).
    HttpResponse {
        request_id: String,
        status: u16,
        headers: HashMap<String, String>,
        body: Option<String>,
    },
    /// Start of a streamed HTTP response for an A2A-surface request (client →
    /// relay). Followed by zero or more `HttpResponseChunk`, then exactly one
    /// `HttpResponseEnd`.
    HttpResponseStart {
        request_id: String,
        status: u16,
        headers: HashMap<String, String>,
    },
    /// A chunk of a streamed HTTP response body, at most 64 KiB decoded
    /// (client → relay).
    HttpResponseChunk { request_id: String, data: String },
    /// End of a streamed HTTP response (client → relay). `error` is set when
    /// the body stream failed partway through; the response up to that point
    /// still stands.
    HttpResponseEnd {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Cancel an in-flight A2A-surface request because the public client
    /// disconnected (relay → client).
    HttpCancel { request_id: String },
    /// Open a WebSocket channel through the tunnel (relay → client).
    WsOpen {
        channel_id: String,
        path: String,
        headers: HashMap<String, String>,
        /// The listener the socket connects to. Absent means the main gateway
        /// listener, which is also what a relay that predates the field gets.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface: Option<Surface>,
    },
    /// Result of a WebSocket open attempt (client → relay).
    WsOpenResult {
        channel_id: String,
        success: bool,
        /// Why a failed open failed, in plain language the relay can pass on
        /// to the browser. Absent on success.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A WebSocket message forwarded through the tunnel.
    WsMessage { channel_id: String, data: String },
    /// Close a WebSocket channel.
    WsClose { channel_id: String },
}

impl TunnelFrame {
    /// A short, stable name for this frame's variant, for logging.
    #[must_use]
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Self::Connected { .. } => "connected",
            Self::AgentsUpdate { .. } => "agents_update",
            Self::Ping => "ping",
            Self::Pong => "pong",
            Self::HttpRequest { .. } => "http_request",
            Self::HttpResponse { .. } => "http_response",
            Self::HttpResponseStart { .. } => "http_response_start",
            Self::HttpResponseChunk { .. } => "http_response_chunk",
            Self::HttpResponseEnd { .. } => "http_response_end",
            Self::HttpCancel { .. } => "http_cancel",
            Self::WsOpen { .. } => "ws_open",
            Self::WsOpenResult { .. } => "ws_open_result",
            Self::WsMessage { .. } => "ws_message",
            Self::WsClose { .. } => "ws_close",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_connected() {
        let frame = TunnelFrame::Connected {
            user_id: "bear".to_string(),
            keepalive_interval_secs: 30,
            origin: None,
            workbench_origin: None,
            instance: Some("laptop".to_string()),
            a2a_token: Some("rsa_abc123".to_string()),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let parsed: TunnelFrame = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(parsed, TunnelFrame::Connected { user_id, keepalive_interval_secs, instance, a2a_token, .. } if user_id == "bear" && keepalive_interval_secs == 30 && instance.as_deref() == Some("laptop") && a2a_token.as_deref() == Some("rsa_abc123")),
            "connected frame should round-trip"
        );
    }

    #[test]
    fn connected_without_instance_or_a2a_token_defaults_to_none() {
        // Compatibility: an older relay that doesn't know about A2A omits
        // both fields entirely.
        let json = r#"{"type":"connected","user_id":"bear","keepalive_interval_secs":30}"#;
        let parsed: TunnelFrame = serde_json::from_str(json).unwrap();
        assert!(
            matches!(
                parsed,
                TunnelFrame::Connected {
                    instance: None,
                    a2a_token: None,
                    origin: None,
                    workbench_origin: None,
                    ..
                }
            ),
            "missing optional fields should deserialize as None"
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
    fn relay_frames_parse_with_and_without_new_fields() {
        let old = r#"{"type":"http_request","request_id":"r","method":"GET","path":"/","headers":{},"body":null}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(old).unwrap(),
            TunnelFrame::HttpRequest { surface: None, .. }
        ));
        let workbench = r#"{"type":"http_request","request_id":"r","method":"GET","path":"/chart/","headers":{},"body":null,"surface":"workbench"}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(workbench).unwrap(),
            TunnelFrame::HttpRequest {
                surface: Some(Surface::Workbench),
                ..
            }
        ));
        let a2a = r#"{"type":"http_request","request_id":"r","method":"GET","path":"/.well-known/agent-card.json","headers":{},"body":null,"surface":"a2a","agent":"scout"}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(a2a).unwrap(),
            TunnelFrame::HttpRequest {
                surface: Some(Surface::A2a),
                ..
            }
        ));
        let connected = r#"{"type":"connected","user_id":"bear","keepalive_interval_secs":30,"instance":"laptop","a2a_token":"rsa_abc123","origin":"https://bear.agent-residuum.com","workbench_origin":"https://bear.workbench.agent-residuum.com"}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(connected).unwrap(),
            TunnelFrame::Connected {
                workbench_origin: Some(_),
                instance: Some(_),
                a2a_token: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn agents_update_serializes_to_the_relays_exact_json() {
        let frame = TunnelFrame::AgentsUpdate {
            agents: vec![
                AgentInfo {
                    name: "scout".to_string(),
                    display_name: "Scout".to_string(),
                    a2a_enabled: true,
                    a2a_private: false,
                    teams_configured: false,
                },
                AgentInfo {
                    name: "archivist".to_string(),
                    display_name: "Archivist".to_string(),
                    a2a_enabled: false,
                    a2a_private: true,
                    teams_configured: true,
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            serde_json::json!({
                "type": "agents_update",
                "agents": [
                    {"name": "scout", "display_name": "Scout", "a2a_enabled": true, "a2a_private": false, "teams_configured": false},
                    {"name": "archivist", "display_name": "Archivist", "a2a_enabled": false, "a2a_private": true, "teams_configured": true},
                ],
            })
        );
        let parsed: TunnelFrame =
            serde_json::from_str(&serde_json::to_string(&frame).unwrap()).unwrap();
        let TunnelFrame::AgentsUpdate { agents } = parsed else {
            panic!("expected AgentsUpdate, got {parsed:?}");
        };
        assert_eq!(agents.len(), 2);
        assert_eq!(agents.first().map(|a| a.name.as_str()), Some("scout"));
        assert_eq!(agents.first().map(|a| a.teams_configured), Some(false));
        assert_eq!(agents.get(1).map(|a| a.teams_configured), Some(true));
    }

    #[test]
    fn an_agent_list_without_teams_configured_reads_as_not_configured() {
        let json = r#"{"type":"agents_update","agents":[{"name":"scout","display_name":"Scout","a2a_enabled":true,"a2a_private":false}]}"#;
        let TunnelFrame::AgentsUpdate { agents } = serde_json::from_str(json).unwrap() else {
            panic!("expected AgentsUpdate");
        };
        assert_eq!(agents.first().map(|a| a.teams_configured), Some(false));
    }

    #[test]
    fn an_empty_agents_update_serializes_as_an_empty_list() {
        let json = serde_json::to_string(&TunnelFrame::AgentsUpdate { agents: vec![] }).unwrap();
        assert_eq!(json, r#"{"type":"agents_update","agents":[]}"#);
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(&json).unwrap(),
            TunnelFrame::AgentsUpdate { agents } if agents.is_empty()
        ));
    }

    #[test]
    fn http_request_agent_field_is_optional_on_the_wire() {
        let with = r#"{"type":"http_request","request_id":"r","method":"GET","path":"/.well-known/agent-card.json","headers":{},"body":null,"surface":"a2a","agent":"scout"}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(with).unwrap(),
            TunnelFrame::HttpRequest { agent: Some(agent), surface: Some(Surface::A2a), .. } if agent == "scout"
        ));
        let without =
            r#"{"type":"http_request","request_id":"r","method":"GET","path":"/","headers":{}}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(without).unwrap(),
            TunnelFrame::HttpRequest { agent: None, .. }
        ));
        let frame = TunnelFrame::HttpRequest {
            request_id: "r".to_string(),
            method: "GET".to_string(),
            path: "/".to_string(),
            headers: HashMap::new(),
            body: None,
            surface: None,
            agent: None,
        };
        assert!(
            !serde_json::to_string(&frame).unwrap().contains("agent"),
            "an absent agent must not serialize"
        );
    }

    #[test]
    fn surface_a2a_serializes_as_a2a() {
        assert_eq!(
            serde_json::to_string(&Surface::A2a).unwrap(),
            "\"a2a\"",
            "the a2a capability and surface tag must be the literal string \"a2a\""
        );
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
            surface: None,
            agent: None,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let parsed: TunnelFrame = serde_json::from_str(&json).unwrap();
        if let TunnelFrame::HttpRequest {
            request_id,
            method,
            path,
            headers: parsed_headers,
            body,
            ..
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
    fn round_trip_http_response_start() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/event-stream".to_string());
        let frame = TunnelFrame::HttpResponseStart {
            request_id: "req-1".to_string(),
            status: 200,
            headers,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let parsed: TunnelFrame = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(parsed, TunnelFrame::HttpResponseStart { request_id, status: 200, headers: parsed_headers } if request_id == "req-1" && parsed_headers.get("content-type").map(String::as_str) == Some("text/event-stream")),
            "HttpResponseStart should round-trip"
        );
    }

    #[test]
    fn round_trip_http_response_chunk() {
        let json = serde_json::to_string(&TunnelFrame::HttpResponseChunk {
            request_id: "req-1".to_string(),
            data: "aGVsbG8=".to_string(),
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::HttpResponseChunk { request_id, data } if request_id == "req-1" && data == "aGVsbG8="),
            "HttpResponseChunk should round-trip"
        );
    }

    #[test]
    fn round_trip_http_response_end_without_error() {
        let json = serde_json::to_string(&TunnelFrame::HttpResponseEnd {
            request_id: "req-1".to_string(),
            error: None,
        })
        .unwrap();
        assert!(
            !json.contains("error"),
            "a successful end should not serialize the error field"
        );
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(&json).unwrap(),
            TunnelFrame::HttpResponseEnd { error: None, .. }
        ));
    }

    #[test]
    fn http_response_end_without_error_field_defaults_to_none() {
        let json = r#"{"type":"http_response_end","request_id":"req-1"}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(json).unwrap(),
            TunnelFrame::HttpResponseEnd { error: None, .. }
        ));
    }

    #[test]
    fn round_trip_http_response_end_with_error() {
        let json = serde_json::to_string(&TunnelFrame::HttpResponseEnd {
            request_id: "req-1".to_string(),
            error: Some("upstream closed the connection".to_string()),
        })
        .unwrap();
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(&json).unwrap(),
            TunnelFrame::HttpResponseEnd { error: Some(_), .. }
        ));
    }

    #[test]
    fn round_trip_http_cancel() {
        let json = serde_json::to_string(&TunnelFrame::HttpCancel {
            request_id: "req-1".to_string(),
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::HttpCancel { request_id } if request_id == "req-1"),
            "HttpCancel should round-trip"
        );
    }

    #[test]
    fn round_trip_ws_open() {
        let json = serde_json::to_string(&TunnelFrame::WsOpen {
            channel_id: "ch-1".to_string(),
            path: "/ws".to_string(),
            headers: HashMap::new(),
            surface: None,
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::WsOpen { channel_id, path, surface: None, .. } if channel_id == "ch-1" && path == "/ws"),
            "WsOpen should round-trip with correct fields"
        );
    }

    #[test]
    fn ws_open_without_a_surface_deserializes_to_none_and_does_not_serialize_one() {
        // Compatibility: a relay that predates the field sends no `surface`,
        // and its socket opens must keep going to the main listener.
        let old = r#"{"type":"ws_open","channel_id":"ch-1","path":"/api/hub/ws","headers":{}}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(old).unwrap(),
            TunnelFrame::WsOpen { surface: None, .. }
        ));
        let frame = TunnelFrame::WsOpen {
            channel_id: "ch-1".to_string(),
            path: "/api/hub/ws".to_string(),
            headers: HashMap::new(),
            surface: None,
        };
        assert!(
            !serde_json::to_string(&frame).unwrap().contains("surface"),
            "an absent surface must not serialize"
        );
    }

    #[test]
    fn ws_open_carries_the_workbench_and_a2a_surfaces() {
        let workbench = r#"{"type":"ws_open","channel_id":"ch-1","path":"/api/hub/ws","headers":{},"surface":"workbench"}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(workbench).unwrap(),
            TunnelFrame::WsOpen {
                surface: Some(Surface::Workbench),
                ..
            }
        ));
        let a2a =
            r#"{"type":"ws_open","channel_id":"ch-1","path":"/","headers":{},"surface":"a2a"}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(a2a).unwrap(),
            TunnelFrame::WsOpen {
                surface: Some(Surface::A2a),
                ..
            }
        ));
        let frame = TunnelFrame::WsOpen {
            channel_id: "ch-1".to_string(),
            path: "/api/hub/ws".to_string(),
            headers: HashMap::new(),
            surface: Some(Surface::Workbench),
        };
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            serde_json::json!({
                "type": "ws_open",
                "channel_id": "ch-1",
                "path": "/api/hub/ws",
                "headers": {},
                "surface": "workbench",
            })
        );
    }

    #[test]
    fn round_trip_ws_open_result_success() {
        let json = serde_json::to_string(&TunnelFrame::WsOpenResult {
            channel_id: "ch-1".to_string(),
            success: true,
            reason: None,
        })
        .unwrap();
        assert_eq!(
            json, r#"{"type":"ws_open_result","channel_id":"ch-1","success":true}"#,
            "a successful open serializes with no reason field"
        );
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::WsOpenResult { channel_id, success, reason: None } if channel_id == "ch-1" && success),
            "WsOpenResult success=true should round-trip"
        );
    }

    #[test]
    fn round_trip_ws_open_result_failure() {
        let json = serde_json::to_string(&TunnelFrame::WsOpenResult {
            channel_id: "ch-1".to_string(),
            success: false,
            reason: Some("the listener isn't running".to_string()),
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_str::<TunnelFrame>(&json).unwrap(), TunnelFrame::WsOpenResult { channel_id, success, reason: Some(reason) } if channel_id == "ch-1" && !success && reason == "the listener isn't running"),
            "WsOpenResult success=false should round-trip with its reason"
        );
        let without_reason = r#"{"type":"ws_open_result","channel_id":"ch-1","success":false}"#;
        assert!(matches!(
            serde_json::from_str::<TunnelFrame>(without_reason).unwrap(),
            TunnelFrame::WsOpenResult { reason: None, .. }
        ));
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
    fn type_name_matches_the_serialized_tag() {
        assert_eq!(TunnelFrame::Ping.type_name(), "ping");
        assert_eq!(
            TunnelFrame::AgentsUpdate { agents: vec![] }.type_name(),
            "agents_update"
        );
        assert_eq!(
            TunnelFrame::HttpCancel {
                request_id: "r".to_string(),
            }
            .type_name(),
            "http_cancel"
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
