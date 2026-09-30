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

/// A loopback TCP port that is free right now and is handed out only once per
/// test process, with room after it for a component that scans upward.
///
/// The starting slot depends on the process id so separate test processes
/// running side by side (nextest) begin at different points in the range.
///
/// # Panics
/// Panics if no port in the reserved range can be bound.
#[must_use]
pub(crate) fn free_port() -> u16 {
    let slots = RANGE_LEN / STRIDE;
    let pid_slot = u16::try_from(std::process::id() % u32::from(slots)).unwrap_or(0);
    for _ in 0..slots {
        let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
        let port = RANGE_START + (pid_slot.wrapping_add(slot) % slots) * STRIDE;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!(
        "no free loopback port in {RANGE_START}..{}",
        RANGE_START + RANGE_LEN
    );
}
