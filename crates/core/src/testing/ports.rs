//! Loopback ports for tests that must tell a component which port to bind.

use std::sync::atomic::{AtomicU16, Ordering};

/// First port handed out. It sits below every OS's ephemeral range (Windows
/// 49152 and up, Linux 32768 and up), so the outbound connections other
/// parallel tests open cannot occupy a port picked here. A port picked from
/// the ephemeral range and bound later failed on Windows with "Only one usage
/// of each socket address".
const RANGE_START: u16 = 20_000;
const RANGE_LEN: u16 = 10_000;

/// Distance between consecutive ports. The workbench server binds the first
/// free port after its hub's gateway port, scanning upward, so a neighbouring
/// port can be taken by another hub's workbench before that hub's own gateway
/// binds it. Spacing the ports past that scan window keeps hubs running in
/// parallel out of each other's way.
const STRIDE: u16 = 16;

/// Pending-connection queue length once a reservation starts listening.
const LISTEN_BACKLOG: u32 = 1024;

static NEXT_SLOT: AtomicU16 = AtomicU16::new(0);

/// A loopback port held for the lifetime of this value.
///
/// The port is bound but not listening, so connections to it are refused
/// until [`ReservedPort::into_tokio_listener`] starts serving on it. Either
/// way no other socket can bind the port while the reservation lives. A test
/// whose component serves on the port converts the reservation right away; a
/// test that needs nothing to answer on the port keeps the reservation and
/// lets connections be refused.
///
/// The socket deliberately does not set `SO_REUSEADDR`. On Linux two sockets
/// that both set it can bind the same port while neither is listening, so a
/// reuse-enabled reservation would not keep other processes out.
pub struct ReservedPort {
    port: u16,
    /// Bound, not listening. Holding it keeps the port reserved.
    socket: tokio::net::TcpSocket,
}

impl ReservedPort {
    /// The reserved port number. Still reserved until this value is dropped
    /// or converted with [`ReservedPort::into_tokio_listener`].
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Start listening on the reserved port and return the listener. The port
    /// stays bound throughout, so no other process can take it on the way to
    /// the component that serves on it.
    ///
    /// # Panics
    /// Panics outside a tokio runtime, or if the socket can't start listening.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "test-support helper: a reserved port that can't serve fails the test that reserved it"
    )]
    pub fn into_tokio_listener(self) -> tokio::net::TcpListener {
        self.socket
            .listen(LISTEN_BACKLOG)
            .expect("a reserved port can start listening")
    }
}

/// Probe the reserved loopback range for a port that is free right now and
/// hold it, the walk behind [`reserve_port`].
#[expect(
    clippy::panic,
    reason = "test-support helper: a test that can't get a port can't run"
)]
fn probe_free_port() -> ReservedPort {
    let slots = RANGE_LEN / STRIDE;
    let pid_slot = u16::try_from(std::process::id() % u32::from(slots)).unwrap_or(0);
    for _ in 0..slots {
        let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
        let port = RANGE_START + (pid_slot.wrapping_add(slot) % slots) * STRIDE;
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let Ok(socket) = tokio::net::TcpSocket::new_v4() else {
            continue;
        };
        if socket.bind(address).is_ok() {
            return ReservedPort { port, socket };
        }
    }
    panic!(
        "no free loopback port in {RANGE_START}..{}",
        RANGE_START + RANGE_LEN
    );
}

/// A port in the gap that follows `port`, a port handed out here, that no
/// probe in any test process ever picks, for a test that moves a running
/// component to a new port. The component binds the new port only after
/// work the test can't hold a reservation across (a config watcher noticing
/// a change), and another process's probe could take a released reservation
/// in that time. The port sits halfway up the gap, so it stays out of reach
/// of the workbench's upward scan from `port` itself.
#[must_use]
pub fn unprobed_port_after(port: u16) -> u16 {
    port + STRIDE / 2
}

/// Reserve a loopback port that is free right now and keep holding it, so the
/// caller can do other setup (writing config files, starting unrelated mock
/// servers) without another test process taking the port. The starting slot
/// depends on the process id so separate test processes running side by side
/// (nextest) begin at different points in the range. Drop the reservation
/// immediately before a component binds the port, or convert it with
/// [`ReservedPort::into_tokio_listener`] when the test serves on it itself.
///
/// # Panics
/// Panics if no port in the reserved range can be bound.
#[must_use]
pub fn reserve_port() -> ReservedPort {
    probe_free_port()
}
