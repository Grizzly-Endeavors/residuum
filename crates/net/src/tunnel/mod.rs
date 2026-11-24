mod connection;
pub(crate) mod v2;

pub(crate) use connection::start_tunnel;
pub use v2::frames::AgentInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelStatus {
    Disconnected,
    Connecting,
    /// The relay refused this install's token (HTTP 401 or 403 on
    /// registration). The tunnel keeps trying at the longest backoff, so a
    /// token the relay accepts again reconnects without a restart.
    Refused,
    Connected {
        user_id: String,
        origin: Option<String>,
        workbench_origin: Option<String>,
        instance: Option<String>,
        instance_origin: Option<String>,
    },
}
