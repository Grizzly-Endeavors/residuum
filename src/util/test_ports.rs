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

static NEXT_SLOT: AtomicU16 = AtomicU16::new(0);

/// A loopback port picked the way [`free_port`] picks one, held open until
/// dropped.
///
/// The per-process starting slot in [`reserve_port`] only offsets two
/// processes' sequences through the same ring of slots; given enough calls
/// they still walk onto the same port. Probing a port and releasing it right
/// away, as [`free_port`] does, leaves a gap before the real component binds
/// it in which another process's probe can take it, which then fails that
/// process's real bind with "address already in use". Holding the listener
/// returned here keeps the port out of every other process's reach until the
/// caller drops it, which it should do immediately before the real component
/// binds the same port number.
pub(crate) struct ReservedPort {
    port: u16,
    /// Never read; held only so dropping this value frees the port.
    _listener: std::net::TcpListener,
}

impl ReservedPort {
    /// The reserved port number. Still reserved until this value is dropped.
    #[must_use]
    pub(crate) fn port(&self) -> u16 {
        self.port
    }
}

/// Probe the reserved loopback range for a port that is free right now, the
/// shared walk behind both [`free_port`] and [`reserve_port`].
fn probe_free_port() -> (u16, std::net::TcpListener) {
    let slots = RANGE_LEN / STRIDE;
    let pid_slot = u16::try_from(std::process::id() % u32::from(slots)).unwrap_or(0);
    for _ in 0..slots {
        let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
        let port = RANGE_START + (pid_slot.wrapping_add(slot) % slots) * STRIDE;
        if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) {
            return (port, listener);
        }
    }
    panic!(
        "no free loopback port in {RANGE_START}..{}",
        RANGE_START + RANGE_LEN
    );
}

/// A loopback TCP port that is free right now and is handed out only once per
/// test process, with room after it for a component that scans upward.
///
/// The starting slot depends on the process id so separate test processes
/// running side by side (nextest) begin at different points in the range.
/// The port is free the instant this returns, but nothing stops another
/// process from taking it before a component that binds it later gets there;
/// a test whose component binds straight away can usually get away with
/// this, but one with any real work in between should reserve with
/// [`reserve_port`] instead and drop the reservation immediately before that
/// component binds.
///
/// # Panics
/// Panics if no port in the reserved range can be bound.
#[must_use]
pub(crate) fn free_port() -> u16 {
    probe_free_port().0
}

/// Reserve a loopback port the same way [`free_port`] does, but keep holding
/// it open so the caller can do other setup — writing config files, starting
/// unrelated mock servers — without another test process taking the port out
/// from under the real component. Drop the returned value immediately before
/// that component binds the port.
///
/// # Panics
/// Panics if no port in the reserved range can be bound.
#[must_use]
pub(crate) fn reserve_port() -> ReservedPort {
    let (port, listener) = probe_free_port();
    ReservedPort {
        port,
        _listener: listener,
    }
}
