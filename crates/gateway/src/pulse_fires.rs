use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use residuum_background::registry::SessionRegistry;
use residuum_bus::session::{SessionCategory, SessionInfo};
use residuum_bus::{
    NotifyName, Publisher, PulseFireChange, PulseFireEvent, PulseFireOutcome, PulseFirePhase,
    PulseFireRun, PulseOverlap, SYSTEM_CHANNEL, SessionAddress, topics,
};

#[derive(Default)]
pub struct PulseFires {
    fires: Mutex<HashMap<String, FireEntry>>,
    next_seq: Mutex<u64>,
    announce: tokio::sync::Mutex<()>,
    agent_stop: CancellationToken,
}

struct FireEntry {
    pulse: String,
    seq: u64,
    started_at: DateTime<Utc>,
    phase: PulseFirePhase,
    address: Option<SessionAddress>,
    run_id: Option<String>,
    overlap: Option<PulseOverlap>,
    stop: CancellationToken,
}

impl FireEntry {
    fn run(&self, fire_id: &str, registry: &SessionRegistry) -> PulseFireRun {
        let run_id = self.run_id.clone().or_else(|| {
            self.address
                .as_ref()
                .and_then(|address| registry.get(address))
                .map(|info| info.run_id)
        });
        PulseFireRun {
            fire_id: Some(fire_id.to_string()),
            phase: self.phase,
            started_at: self.started_at,
            address: self.address.clone(),
            run_id,
            overlap: self.overlap.clone(),
        }
    }
}

fn untracked_sessions(
    fires: &HashMap<String, FireEntry>,
    registry: &SessionRegistry,
    pulse: &str,
) -> Vec<SessionInfo> {
    let source_label = format!("pulse:{pulse}");
    registry
        .list_live()
        .into_iter()
        .filter(|info| {
            info.category == SessionCategory::Scheduled
                && info.source_label == source_label
                && !fires
                    .values()
                    .any(|fire| fire.address.as_ref() == Some(&info.address))
        })
        .collect()
}

fn session_run(info: SessionInfo) -> PulseFireRun {
    PulseFireRun {
        fire_id: None,
        phase: PulseFirePhase::Session,
        started_at: info.started_at,
        address: Some(info.address),
        run_id: Some(info.run_id),
        overlap: info.overlap,
    }
}

impl PulseFires {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, FireEntry>> {
        self.fires.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn stop_all(&self) {
        self.agent_stop.cancel();
    }

    pub(crate) fn begin(
        self: &Arc<Self>,
        pulse: &str,
        phase: PulseFirePhase,
        overlap: Option<PulseOverlap>,
        publisher: Publisher,
        registry: Arc<SessionRegistry>,
    ) -> PulseFire {
        let id = format!("fire-{}", uuid::Uuid::new_v4().simple());
        let stop = self.agent_stop.child_token();
        let seq = {
            let mut next = self.next_seq.lock().unwrap_or_else(PoisonError::into_inner);
            *next += 1;
            *next
        };
        self.lock().insert(
            id.clone(),
            FireEntry {
                pulse: pulse.to_string(),
                seq,
                started_at: Utc::now(),
                phase,
                address: None,
                run_id: None,
                overlap,
                stop: stop.clone(),
            },
        );
        PulseFire {
            fires: Arc::clone(self),
            id,
            pulse: pulse.to_string(),
            stop,
            publisher,
            registry,
            ended: false,
        }
    }

    pub(crate) fn stop(&self, pulse: &str, fire_id: &str) -> bool {
        let fires = self.lock();
        let Some(fire) = fires.get(fire_id).filter(|fire| fire.pulse == pulse) else {
            return false;
        };
        fire.stop.cancel();
        true
    }

    pub(crate) fn current_run(
        &self,
        pulse: &str,
        registry: &SessionRegistry,
    ) -> Option<PulseFireRun> {
        let fires = self.lock();
        let newest = fires
            .iter()
            .filter(|(_, fire)| fire.pulse == pulse)
            .max_by_key(|(_, fire)| fire.seq);
        if let Some((id, fire)) = newest {
            return Some(fire.run(id, registry));
        }
        untracked_sessions(&fires, registry, pulse)
            .pop()
            .map(session_run)
    }

    pub(crate) fn overlap(&self, pulse: &str, registry: &SessionRegistry) -> Option<PulseOverlap> {
        let fires = self.lock();
        let oldest_fire = fires
            .iter()
            .filter(|(_, fire)| fire.pulse == pulse)
            .min_by_key(|(_, fire)| fire.seq)
            .map(|(id, fire)| fire.run(id, registry));
        let oldest_session = untracked_sessions(&fires, registry, pulse)
            .into_iter()
            .next()
            .map(session_run);
        let previous = match (oldest_fire, oldest_session) {
            (Some(fire), Some(session)) if session.started_at < fire.started_at => session,
            (Some(fire), _) => fire,
            (None, session) => session?,
        };
        Some(PulseOverlap {
            previous_run_id: previous.run_id,
            previous_fire_id: previous.fire_id,
            previous_started_at: previous.started_at,
        })
    }

    fn update(&self, fire_id: &str, change: impl FnOnce(&mut FireEntry)) {
        if let Some(fire) = self.lock().get_mut(fire_id) {
            change(fire);
        }
    }

    fn remove(&self, fire_id: &str) {
        self.lock().remove(fire_id);
    }
}

async fn announce(
    fires: &PulseFires,
    publisher: &Publisher,
    registry: &SessionRegistry,
    pulse: &str,
    fire_id: &str,
    change: PulseFireChange,
) {
    let _in_order = fires.announce.lock().await;
    let event = PulseFireEvent {
        pulse: pulse.to_string(),
        fire_id: fire_id.to_string(),
        change,
        current_run: fires.current_run(pulse, registry),
    };
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            event,
        )
        .await
    {
        tracing::warn!(pulse = %pulse, fire = %fire_id, error = %e, "failed to publish a pulse fire's change");
    }
}

pub(crate) struct PulseFire {
    fires: Arc<PulseFires>,
    id: String,
    pulse: String,
    stop: CancellationToken,
    publisher: Publisher,
    registry: Arc<SessionRegistry>,
    ended: bool,
}

impl PulseFire {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn pulse(&self) -> &str {
        &self.pulse
    }

    pub(crate) fn stop_token(&self) -> &CancellationToken {
        &self.stop
    }

    pub(crate) fn agent_stopping(&self) -> bool {
        self.fires.agent_stop.is_cancelled()
    }

    pub(crate) fn request_session(&self, address: SessionAddress) {
        self.fires.update(&self.id, |fire| {
            fire.phase = PulseFirePhase::Session;
            fire.address = Some(address);
        });
    }

    pub(crate) fn session_registered(&self, run_id: String) {
        self.fires
            .update(&self.id, |fire| fire.run_id = Some(run_id));
    }

    fn leave_tracker(&self) {
        self.fires.remove(&self.id);
        self.registry.fire_stops().withdraw(&self.id);
    }

    pub(crate) async fn announce(&self, change: PulseFireChange) {
        announce(
            &self.fires,
            &self.publisher,
            &self.registry,
            &self.pulse,
            &self.id,
            change,
        )
        .await;
    }

    pub(crate) async fn finish(
        mut self,
        outcome: PulseFireOutcome,
        session_run_id: Option<String>,
    ) {
        self.leave_tracker();
        self.ended = true;
        self.announce(PulseFireChange::Finished {
            outcome,
            session_run_id,
        })
        .await;
    }

    pub(crate) async fn end(mut self) {
        self.leave_tracker();
        self.ended = true;
        self.announce(PulseFireChange::Ended).await;
    }
}

impl Drop for PulseFire {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        self.leave_tracker();
        tracing::warn!(
            pulse = %self.pulse,
            fire = %self.id,
            "a pulse fire's task ended before the fire finished; it no longer shows as running"
        );
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let fires = Arc::clone(&self.fires);
        let publisher = self.publisher.clone();
        let registry = Arc::clone(&self.registry);
        let pulse = std::mem::take(&mut self.pulse);
        let id = std::mem::take(&mut self.id);
        runtime.spawn(async move {
            announce(
                &fires,
                &publisher,
                &registry,
                &pulse,
                &id,
                PulseFireChange::Ended,
            )
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_bus::session::{MAIN_DEPTH, SessionState};
    use residuum_bus::testing::next_event;
    use residuum_bus::{EventTrigger, PulseFireStatus, Subscriber};

    fn live_session(pulse: &str, address: &str, run_id: &str) -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from(address),
            run_id: run_id.to_string(),
            category: SessionCategory::Scheduled,
            trigger: EventTrigger::Pulse,
            source_label: format!("pulse:{pulse}"),
            state: SessionState::Running,
            spawner: None,
            depth: MAIN_DEPTH + 1,
            purpose: "check".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::SMALL,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    struct Fixture {
        fires: Arc<PulseFires>,
        registry: Arc<SessionRegistry>,
        bus: residuum_bus::BusHandle,
    }

    fn fixture() -> Fixture {
        Fixture {
            fires: Arc::new(PulseFires::default()),
            registry: Arc::new(SessionRegistry::new()),
            bus: residuum_bus::spawn_broker(),
        }
    }

    impl Fixture {
        fn begin(&self, pulse: &str, phase: PulseFirePhase) -> PulseFire {
            let overlap = self.fires.overlap(pulse, &self.registry);
            self.fires.begin(
                pulse,
                phase,
                overlap,
                self.bus.publisher(),
                Arc::clone(&self.registry),
            )
        }

        async fn events(&self) -> Subscriber<PulseFireEvent> {
            self.bus
                .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
                .await
                .unwrap()
        }
    }

    fn ok_outcome() -> PulseFireOutcome {
        PulseFireOutcome {
            status: PulseFireStatus::Ok,
            at: Utc::now(),
            error: None,
        }
    }

    #[tokio::test]
    async fn a_fire_shows_as_the_current_run_through_its_phases_until_it_finishes() {
        let f = fixture();
        let mut events = f.events().await;
        let fire = f.begin("watch", PulseFirePhase::Script);

        let scripting = f.fires.current_run("watch", &f.registry).unwrap();
        assert_eq!(scripting.fire_id.as_deref(), Some(fire.id()));
        assert_eq!(scripting.phase, PulseFirePhase::Script);
        assert_eq!((scripting.address, scripting.run_id), (None, None));
        assert_eq!(f.fires.current_run("other", &f.registry), None);

        fire.announce(PulseFireChange::Started).await;
        let started = next_event("the start", &mut events).await;
        assert_eq!(started.change, PulseFireChange::Started);
        assert_eq!(started.pulse, "watch");
        assert_eq!(
            started.current_run.map(|current| current.phase),
            Some(PulseFirePhase::Script)
        );

        let address = SessionAddress::from("scheduled-watch-0001");
        fire.request_session(address.clone());
        let requested = f.fires.current_run("watch", &f.registry).unwrap();
        assert_eq!(requested.phase, PulseFirePhase::Session);
        assert_eq!(requested.address.as_ref(), Some(&address));
        assert_eq!(requested.run_id, None, "the session hasn't registered yet");

        f.registry
            .register(
                live_session("watch", address.as_ref(), "run-7"),
                CancellationToken::new(),
            )
            .unwrap();
        let registered = f.fires.current_run("watch", &f.registry).unwrap();
        assert_eq!(
            registered.run_id.as_deref(),
            Some("run-7"),
            "the registry's run id shows before the fire hears of it"
        );
        assert_eq!(
            registered.fire_id.as_deref(),
            Some(fire.id()),
            "the fire's own session is not a second, untracked run"
        );

        fire.finish(ok_outcome(), Some("run-7".to_string())).await;
        let finished = next_event("the finish", &mut events).await;
        assert!(matches!(
            finished.change,
            PulseFireChange::Finished {
                session_run_id: Some(_),
                ..
            }
        ));
        assert_eq!(
            finished.current_run.and_then(|current| current.fire_id),
            None,
            "only the untracked live session is left"
        );
    }

    #[tokio::test]
    async fn a_dropped_fire_leaves_no_stale_entry_and_says_it_ended() {
        let f = fixture();
        let mut events = f.events().await;
        let fire = f.begin("watch", PulseFirePhase::Script);
        let id = fire.id().to_string();

        drop(fire);

        assert_eq!(f.fires.current_run("watch", &f.registry), None);
        assert!(
            !f.fires.stop("watch", &id),
            "a dropped fire can't be stopped"
        );
        let ended = next_event("the end", &mut events).await;
        assert_eq!(ended.fire_id, id);
        assert_eq!(ended.change, PulseFireChange::Ended);
        assert_eq!(ended.current_run, None);
    }

    #[tokio::test]
    async fn a_fire_that_panics_leaves_no_stale_entry() {
        let f = fixture();
        let fire = f.begin("watch", PulseFirePhase::Script);
        let task = tokio::spawn(async move {
            let _fire = fire;
            panic!("a bug in the fire's task");
        });
        assert!(task.await.is_err(), "the task panicked");
        assert_eq!(f.fires.current_run("watch", &f.registry), None);
    }

    #[tokio::test]
    async fn the_newest_fire_is_the_current_run_and_carries_the_overlap_with_the_oldest() {
        let f = fixture();
        let first = f.begin("watch", PulseFirePhase::Script);
        let second = f.begin("watch", PulseFirePhase::Script);

        let newest = f.fires.current_run("watch", &f.registry).unwrap();
        assert_eq!(newest.fire_id.as_deref(), Some(second.id()));
        let overlap = newest.overlap.expect("the second fire overlaps the first");
        assert_eq!(overlap.previous_fire_id.as_deref(), Some(first.id()));
        assert_eq!(
            overlap.previous_run_id, None,
            "the first fire is still in its scripts"
        );

        second.finish(ok_outcome(), None).await;
        let older = f.fires.current_run("watch", &f.registry).unwrap();
        assert_eq!(
            older.fire_id.as_deref(),
            Some(first.id()),
            "the older fire still running is the current run again"
        );
        assert_eq!(older.overlap, None);
        first.finish(ok_outcome(), None).await;
        assert_eq!(f.fires.overlap("watch", &f.registry), None);
    }

    #[tokio::test]
    async fn an_untracked_live_session_of_the_pulse_is_its_current_run_and_overlap() {
        let f = fixture();
        f.registry
            .register(
                live_session("watch", "scheduled-watch-00aa", "run-resumed"),
                CancellationToken::new(),
            )
            .unwrap();

        let run = f.fires.current_run("watch", &f.registry).unwrap();
        assert_eq!(run.fire_id, None);
        assert_eq!(run.run_id.as_deref(), Some("run-resumed"));

        let overlap = f.fires.overlap("watch", &f.registry).unwrap();
        assert_eq!(overlap.previous_run_id.as_deref(), Some("run-resumed"));
        assert_eq!(overlap.previous_fire_id, None);
    }

    #[tokio::test]
    async fn stop_cancels_only_the_named_fire_of_the_named_pulse() {
        let f = fixture();
        let first = f.begin("watch", PulseFirePhase::Script);
        let second = f.begin("watch", PulseFirePhase::Script);

        assert!(!f.fires.stop("other", first.id()), "the pulse must match");
        assert!(!f.fires.stop("watch", "fire-unknown"));
        assert!(f.fires.stop("watch", first.id()));

        assert!(first.stop_token().is_cancelled());
        assert!(!first.agent_stopping());
        assert!(!second.stop_token().is_cancelled());

        f.fires.stop_all();
        assert!(second.stop_token().is_cancelled());
        assert!(second.agent_stopping());
    }
}
