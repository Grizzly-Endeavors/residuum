//! Holding a call open until the test lets it through.
//!
//! A test that needs to see a state while it lasts (an agent busy with a
//! turn, a session queued behind another) holds the call that ends it at a
//! closed [`Gate`], waits for the call to arrive, looks, then releases it.
//! Nothing depends on how long the call would otherwise have taken.

use std::sync::Arc;

use tokio::sync::watch;

#[derive(Debug, Clone, Default)]
struct GateState {
    /// Calls that reached the gate, ever. A call's ticket is its arrival index.
    arrived: usize,
    /// Calls that went through.
    passed: usize,
    /// Tickets below this go through.
    released: usize,
    /// Whether every call goes through.
    open: bool,
}

/// The test's handle on a gate. Dropping it opens the gate, so calls still
/// held when a test ends never hang its teardown.
pub(crate) struct Gate {
    state: Arc<watch::Sender<GateState>>,
    name: String,
}

/// What a held call awaits; see [`GateEntry::pass`].
#[derive(Clone)]
pub(crate) struct GateEntry {
    state: Arc<watch::Sender<GateState>>,
}

impl Gate {
    /// A gate that holds every call until released.
    pub(crate) fn closed(name: &str) -> Self {
        Self::with_open(name, false)
    }

    /// A gate that lets every call through until closed.
    pub(crate) fn open(name: &str) -> Self {
        Self::with_open(name, true)
    }

    fn with_open(name: &str, open: bool) -> Self {
        let (state, _) = watch::channel(GateState {
            open,
            ..GateState::default()
        });
        Self {
            state: Arc::new(state),
            name: name.to_string(),
        }
    }

    /// What the gated code awaits.
    pub(crate) fn entry(&self) -> GateEntry {
        GateEntry {
            state: Arc::clone(&self.state),
        }
    }

    /// Let the next `n` calls through, in the order they arrive, whether they
    /// have arrived yet or not.
    pub(crate) fn release(&self, n: usize) {
        self.state.send_modify(|state| state.released += n);
    }

    /// Let every call through, held or still to come.
    pub(crate) fn open_all(&self) {
        self.state.send_modify(|state| state.open = true);
    }

    /// Hold calls from now on. Calls already let through stay through.
    pub(crate) fn close(&self) {
        self.state.send_modify(|state| {
            state.open = false;
            state.released = state.released.max(state.arrived);
        });
    }

    /// How many calls are waiting at the gate now.
    pub(crate) fn held(&self) -> usize {
        let state = self.state.borrow();
        state.arrived - state.passed
    }

    /// Wait until `n` calls are waiting at the gate.
    pub(crate) async fn until_held(&self, n: usize) {
        let mut rx = self.state.subscribe();
        crate::testing::wait::watch_until(
            format_args!("{n} call(s) to reach gate {}", self.name),
            &mut rx,
            |state| state.arrived - state.passed >= n,
        )
        .await;
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.state.send_modify(|state| state.open = true);
    }
}

impl GateEntry {
    /// Arrive at the gate and wait to be let through.
    pub(crate) async fn pass(&self) {
        let mut ticket = 0;
        self.state.send_modify(|state| {
            ticket = state.arrived;
            state.arrived += 1;
        });
        let mut rx = self.state.subscribe();
        if rx
            .wait_for(|state| state.open || state.released > ticket)
            .await
            .is_err()
        {
            panic!("a gate's state outlives every entry, so it can't go away under one");
        }
        self.state.send_modify(|state| state.passed += 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::wait;

    #[tokio::test]
    async fn a_closed_gate_holds_calls_until_released_in_arrival_order() {
        let gate = Gate::closed("model");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for call in ["first", "second"] {
            let entry = gate.entry();
            let tx = tx.clone();
            crate::util::spawn_in_span(async move {
                entry.pass().await;
                tx.send(call).unwrap();
            });
            gate.until_held(1).await;
            if call == "first" {
                assert_eq!(gate.held(), 1);
            }
        }
        gate.until_held(2).await;
        assert!(
            wait::drain(&mut rx).is_empty(),
            "nothing passes a closed gate"
        );

        gate.release(1);
        assert_eq!(wait::next("the first call through", &mut rx).await, "first");
        gate.release(1);
        assert_eq!(
            wait::next("the second call through", &mut rx).await,
            "second"
        );
        assert_eq!(gate.held(), 0);
    }

    #[tokio::test]
    async fn an_open_gate_lets_calls_through_until_closed() {
        let gate = Gate::open("model");
        gate.entry().pass().await;

        gate.close();
        let entry = gate.entry();
        let held = crate::util::spawn_in_span(async move { entry.pass().await });
        gate.until_held(1).await;
        gate.open_all();
        wait::guarded("the held call to pass once the gate opens", held)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn dropping_the_gate_lets_held_calls_through() {
        let gate = Gate::closed("model");
        let entry = gate.entry();
        let held = crate::util::spawn_in_span(async move { entry.pass().await });
        gate.until_held(1).await;
        drop(gate);
        wait::guarded("the held call to pass once the gate is dropped", held)
            .await
            .unwrap();
    }
}
