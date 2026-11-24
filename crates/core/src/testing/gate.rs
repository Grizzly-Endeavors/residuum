use std::sync::Arc;

use tokio::sync::watch;

#[derive(Debug, Clone, Default)]
struct GateState {
    arrived: usize,
    passed: usize,
    released: usize,
    open: bool,
}

pub struct Gate {
    state: Arc<watch::Sender<GateState>>,
    name: String,
}

#[derive(Clone)]
pub struct GateEntry {
    state: Arc<watch::Sender<GateState>>,
}

impl Gate {
    #[must_use]
    pub fn closed(name: &str) -> Self {
        Self::with_open(name, false)
    }

    #[must_use]
    pub fn open(name: &str) -> Self {
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

    #[must_use]
    pub fn entry(&self) -> GateEntry {
        GateEntry {
            state: Arc::clone(&self.state),
        }
    }

    pub fn release(&self, n: usize) {
        self.state.send_modify(|state| state.released += n);
    }

    pub fn open_all(&self) {
        self.state.send_modify(|state| state.open = true);
    }

    pub fn close(&self) {
        self.state.send_modify(|state| {
            state.open = false;
            state.released = state.released.max(state.arrived);
        });
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        let state = self.state.borrow();
        state.arrived - state.passed
    }

    #[must_use]
    pub fn arrived(&self) -> usize {
        self.state.borrow().arrived
    }

    pub async fn until_held(&self, n: usize) {
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
    #[expect(
        clippy::panic,
        reason = "test-support helper: the gate state outlives every entry, so the wait cannot fail"
    )]
    pub async fn pass(&self) {
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
