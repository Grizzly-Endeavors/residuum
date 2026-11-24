use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct FireStops {
    fires: Mutex<HashMap<String, OfferedStop>>,
}

struct OfferedStop {
    stop: CancellationToken,
    claimed: bool,
}

impl FireStops {
    pub fn offer(&self, fire_id: &str, stop: CancellationToken) {
        self.lock().insert(
            fire_id.to_string(),
            OfferedStop {
                stop,
                claimed: false,
            },
        );
    }

    pub fn claim(&self, fire_id: &str) -> Option<CancellationToken> {
        let mut fires = self.lock();
        let offered = fires.get_mut(fire_id)?;
        if offered.stop.is_cancelled() {
            return None;
        }
        offered.claimed = true;
        Some(offered.stop.child_token())
    }

    pub fn withdraw(&self, fire_id: &str) -> bool {
        self.lock()
            .remove(fire_id)
            .is_some_and(|offered| offered.claimed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, OfferedStop>> {
        self.fires.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claimed_session_stops_with_its_fire() {
        let stops = FireStops::default();
        let fire = CancellationToken::new();
        stops.offer("fire-1", fire.clone());

        let session = stops.claim("fire-1").expect("a live fire's session forks");
        assert!(!session.is_cancelled());
        fire.cancel();
        assert!(
            session.is_cancelled(),
            "the fire's stop reaches its session"
        );
        assert!(stops.withdraw("fire-1"), "the session was claimed");
    }

    #[test]
    fn a_fire_stopped_before_the_claim_forks_nothing() {
        let stops = FireStops::default();
        let fire = CancellationToken::new();
        stops.offer("fire-1", fire.clone());
        fire.cancel();

        assert!(stops.claim("fire-1").is_none());
        assert!(!stops.withdraw("fire-1"), "no session was claimed");
    }

    #[test]
    fn a_withdrawn_offer_forks_nothing() {
        let stops = FireStops::default();
        stops.offer("fire-1", CancellationToken::new());

        assert!(!stops.withdraw("fire-1"));
        assert!(stops.claim("fire-1").is_none());
        assert!(stops.claim("fire-unknown").is_none());
        assert!(!stops.withdraw("fire-unknown"));
    }
}
