//! Channels the hub, the gateway, the interfaces and the web routes use to
//! ask a running agent to reload, to run a command or to stop its turn.

use tokio::sync::mpsc;

/// Describes what kind of configuration reload was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadSignal {
    /// Agent config reload (the agent's own `config.toml`/`providers.toml`
    /// changed).
    Agent,
    /// Hub config reload (`hub/config.toml` changed).
    Hub,
    /// Workspace-level reload (`mcp.json`, `channels.toml`, or
    /// `agent-card.json` changed).
    Workspace,
}

/// Sending half of the reload queue. Every signal sent is delivered, so two
/// files changing close together each get their own reload.
pub type ReloadSender = mpsc::UnboundedSender<ReloadSignal>;

/// Receiving half of the reload queue, consumed by the event loop.
pub type ReloadReceiver = mpsc::UnboundedReceiver<ReloadSignal>;

/// A named command dispatched from any client channel to the server event loop.
pub struct ServerCommand {
    /// Command name (e.g. "observe", "reflect", "context").
    pub name: String,
    /// Optional argument text.
    pub args: Option<String>,
    /// Optional oneshot sender for routing a response back to the sender only.
    ///
    /// `Ok` carries a successful command's reply text (e.g. "context"); the
    /// WS layer intentionally ignores it since the outcome is already
    /// reflected via the normal bus broadcast. `Err` carries a rejection
    /// reason (e.g. an unknown command name) and is routed to the sender as
    /// an error, never broadcast to other clients.
    pub reply_tx: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
}

/// A request to stop the currently running main-agent turn.
///
/// Delivered outside the `ServerCommand`/`command_tx` pipeline because that
/// pipeline is only drained *between* turns (the event loop blocks on
/// `handle_inbound_message` for the whole turn) — a stop needs to reach a
/// turn while it is running, so it travels its own channel that the active
/// turn's select loop watches directly.
pub struct StopRequest {
    /// Correlation id of the turn to stop, when the sender knows it (e.g. a
    /// WebSocket client stopping the turn it's watching). `None` means "stop
    /// whichever turn is currently running" — used by chat commands, which
    /// don't track turn ids and only ever have at most one turn to stop.
    pub reply_to: Option<String>,
    /// Reports whether a running turn actually matched and was signalled to
    /// stop, so the caller can tell a successful stop from "nothing was
    /// running" without guessing from a timeout.
    pub result_tx: Option<tokio::sync::oneshot::Sender<bool>>,
}
