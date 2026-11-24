use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use residuum_bus::interrupt::Interrupt;
use residuum_bus::session::{MAIN_ADDRESS, SessionInfo, SessionState, slugify};
use residuum_bus::{ConversationTarget, EventTrigger, SessionAddress, SkillName};
use residuum_config::ModelSelector;

pub const INTERRUPT_CHANNEL_CAPACITY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverOutcome {
    Delivered,
    Completing,
    Full,
    NotLive,
}

#[derive(Debug, Clone)]
pub struct RegisterError {
    pub address: SessionAddress,
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "address {} is already registered to a live run",
            self.address
        )
    }
}

impl std::error::Error for RegisterError {}

struct SessionEntry {
    info: SessionInfo,
    stop_token: CancellationToken,
    interrupt_tx: mpsc::Sender<Interrupt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumePoint {
    pub previous_run_id: String,
    pub trigger: EventTrigger,
    pub source_label: String,
    pub agent_skill: Option<SkillName>,
    #[serde(alias = "model_tier")]
    pub model: ModelSelector,
    pub spawner: Option<SessionAddress>,
    pub depth: u32,
    pub conversation_target: Option<ConversationTarget>,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct SessionRegistry {
    sessions: Mutex<HashMap<SessionAddress, SessionEntry>>,
    resume_points: Mutex<HashMap<SessionAddress, ResumePoint>>,
    persist_path: Option<PathBuf>,
    persist_lock: tokio::sync::Mutex<()>,
    cleared: Notify,
    fire_stops: crate::fire_stops::FireStops,
}

impl SessionRegistry {
    #[must_use]
    pub fn fire_stops(&self) -> &crate::fire_stops::FireStops {
        &self.fire_stops
    }

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub async fn load(persist_path: PathBuf) -> Self {
        let resume_points = load_resume_points(&persist_path).await;
        Self {
            resume_points: Mutex::new(resume_points),
            persist_path: Some(persist_path),
            ..Self::default()
        }
    }

    pub fn register(
        &self,
        info: SessionInfo,
        stop_token: CancellationToken,
    ) -> Result<mpsc::Receiver<Interrupt>, RegisterError> {
        let (interrupt_tx, interrupt_rx) = mpsc::channel(INTERRUPT_CHANNEL_CAPACITY);
        let address = info.address.clone();
        let mut guard = self.lock();
        if guard.contains_key(&address) {
            return Err(RegisterError { address });
        }
        guard.insert(
            address,
            SessionEntry {
                info,
                stop_token,
                interrupt_tx,
            },
        );
        Ok(interrupt_rx)
    }

    pub fn deliver(&self, address: &SessionAddress, message: Interrupt) -> DeliverOutcome {
        let guard = self.lock();
        let Some(entry) = guard.get(address) else {
            return DeliverOutcome::NotLive;
        };
        if matches!(
            entry.info.state,
            SessionState::Completing | SessionState::Completed
        ) {
            return DeliverOutcome::Completing;
        }
        match entry.interrupt_tx.try_send(message) {
            Ok(()) => DeliverOutcome::Delivered,
            Err(e) => {
                tracing::warn!(address = %address, error = %e, "session interrupt channel full, refusing to deliver agent message");
                DeliverOutcome::Full
            }
        }
    }

    pub async fn wait_until_clear(&self, address: &SessionAddress) {
        const WARN_AFTER: Duration = Duration::from_secs(30);

        let start = std::time::Instant::now();
        let mut warned = false;
        loop {
            let notified = self.cleared.notified();
            if self.get(address).is_none() {
                return;
            }
            if tokio::time::timeout(WARN_AFTER, notified).await.is_err() && !warned {
                tracing::warn!(
                    address = %address,
                    waited_secs = start.elapsed().as_secs(),
                    "still waiting for a completing session to leave the registry"
                );
                warned = true;
            }
        }
    }

    pub(crate) async fn wait_until_empty(&self) {
        loop {
            let notified = self.cleared.notified();
            if self.lock().is_empty() {
                return;
            }
            notified.await;
        }
    }

    pub async fn record_resume_point(&self, address: &SessionAddress, mut point: ResumePoint) {
        point.recorded_at = Utc::now();
        let _persist_guard = self.persist_lock.lock().await;
        let snapshot = {
            let mut guard = self
                .resume_points
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.insert(address.clone(), point);
            self.persist_path.as_ref().map(|_| guard.clone())
        };
        let (Some(path), Some(snapshot)) = (self.persist_path.as_ref(), snapshot) else {
            return;
        };
        if let Err(e) = persist_resume_points(path, &snapshot).await {
            tracing::warn!(path = %path.display(), error = %e, "failed to persist resume points to disk");
        }
    }

    #[must_use]
    pub fn resume_point(&self, address: &SessionAddress) -> Option<ResumePoint> {
        self.resume_points
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(address)
            .cloned()
    }

    pub fn set_state(&self, address: &SessionAddress, state: SessionState) {
        let mut guard = self.lock();
        if let Some(entry) = guard.get_mut(address) {
            entry.info.state = state;
        }
    }

    pub fn accumulate_usage(
        &self,
        address: &SessionAddress,
        usage: Option<residuum_inference::Usage>,
    ) -> Option<residuum_bus::usage::SessionUsageTotals> {
        let mut guard = self.lock();
        let entry = guard.get_mut(address)?;
        entry.info.usage.accumulate(usage);
        Some(entry.info.usage)
    }

    pub fn accumulate_tool_calls(
        &self,
        address: &SessionAddress,
        n: u32,
    ) -> Option<residuum_bus::usage::SessionUsageTotals> {
        let mut guard = self.lock();
        let entry = guard.get_mut(address)?;
        entry.info.usage.record_tool_calls(n);
        Some(entry.info.usage)
    }

    pub fn stop(&self, address: &SessionAddress) -> bool {
        let guard = self.lock();
        let Some(entry) = guard.get(address) else {
            return false;
        };
        if matches!(
            entry.info.state,
            SessionState::Completing | SessionState::Completed
        ) {
            return false;
        }
        entry.stop_token.cancel();
        true
    }

    pub fn stop_if_running(&self, address: &SessionAddress) -> bool {
        let guard = self.lock();
        let Some(entry) = guard.get(address) else {
            return false;
        };
        if entry.info.state != SessionState::Running {
            return false;
        }
        entry.stop_token.cancel();
        true
    }

    pub fn stop_all(&self) -> usize {
        let guard = self.lock();
        let mut signalled = 0;
        for entry in guard.values() {
            if matches!(
                entry.info.state,
                SessionState::Completing | SessionState::Completed
            ) {
                continue;
            }
            entry.stop_token.cancel();
            signalled += 1;
        }
        signalled
    }

    pub fn remove(&self, address: &SessionAddress, run_id: &str) -> Option<SessionInfo> {
        let removed = {
            let mut guard = self.lock();
            if guard.get(address).is_none_or(|e| e.info.run_id != run_id) {
                return None;
            }
            guard.remove(address).map(|entry| entry.info)
        };
        self.cleared.notify_waiters();
        removed
    }

    #[must_use]
    pub fn get_by_run_id(&self, run_id: &str) -> Option<SessionInfo> {
        self.lock()
            .values()
            .find(|entry| entry.info.run_id == run_id)
            .map(|entry| entry.info.clone())
    }

    #[must_use]
    pub fn get(&self, address: &SessionAddress) -> Option<SessionInfo> {
        self.lock().get(address).map(|entry| entry.info.clone())
    }

    #[must_use]
    pub fn list_live(&self) -> Vec<SessionInfo> {
        let guard = self.lock();
        let mut sessions: Vec<SessionInfo> = guard.values().map(|e| e.info.clone()).collect();
        sessions.sort_by_key(|s| s.started_at);
        sessions
    }

    #[must_use]
    pub fn subagent_snapshot(&self) -> Vec<residuum_tracing_service::Subagent> {
        self.list_live()
            .into_iter()
            .map(|info| residuum_tracing_service::Subagent {
                name: info.address.to_string(),
                status: format!("{} ({})", info.state, info.source_label),
            })
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SessionAddress, SessionEntry>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub struct SessionUsageSink<'a> {
    registry: &'a SessionRegistry,
    address: SessionAddress,
    run_id: String,
    component: residuum_inference::UsageComponent,
    recorder: residuum_inference::UsageRecorder,
}

impl<'a> SessionUsageSink<'a> {
    #[must_use]
    pub fn new(
        registry: &'a SessionRegistry,
        info: &SessionInfo,
        layout: &residuum_config::WorkspaceLayout,
    ) -> Self {
        Self {
            registry,
            address: info.address.clone(),
            run_id: info.run_id.clone(),
            component: residuum_inference::UsageComponent::Session {
                kind: info.trigger.as_str().to_string(),
            },
            recorder: residuum_inference::UsageRecorder::new(layout),
        }
    }
}

#[async_trait::async_trait]
impl residuum_agent::usage::UsageSink for SessionUsageSink<'_> {
    async fn record_call(
        &self,
        provider: &dyn residuum_inference::InferenceProvider,
        result: &Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>,
    ) {
        self.recorder
            .record_call(&self.component, Some(&self.run_id), provider, result)
            .await;
    }

    async fn accumulate(
        &self,
        usage: Option<residuum_inference::Usage>,
    ) -> residuum_bus::usage::SessionUsageTotals {
        self.registry
            .accumulate_usage(&self.address, usage)
            .unwrap_or_default()
    }

    async fn record_tool_calls(&self, n: u32) -> residuum_bus::usage::SessionUsageTotals {
        self.registry
            .accumulate_tool_calls(&self.address, n)
            .unwrap_or_default()
    }
}

async fn load_resume_points(path: &Path) -> HashMap<SessionAddress, ResumePoint> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to read persisted resume points, starting with none"
            );
            return HashMap::new();
        }
    };
    if contents.trim().is_empty() {
        return HashMap::new();
    }
    match serde_json::from_str(&contents) {
        Ok(points) => points,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to parse persisted resume points, starting with none"
            );
            HashMap::new()
        }
    }
}

async fn persist_resume_points(
    path: &Path,
    points: &HashMap<SessionAddress, ResumePoint>,
) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(points).context("failed to serialize resume points")?;
    residuum_core::util::fs::atomic_write(path, &json).await
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StopSessionError {
    #[error("The main agent can't be stopped as a session.")]
    Main,
    #[error("{address} isn't running, so there's nothing to stop.")]
    NotLive { address: SessionAddress },
}

pub fn stop_named_session(
    registry: &SessionRegistry,
    address: &str,
) -> Result<(), StopSessionError> {
    if address == MAIN_ADDRESS {
        return Err(StopSessionError::Main);
    }
    let address = SessionAddress::from(address);
    if registry.stop(&address) {
        tracing::info!(address = %address, "session stop requested");
        Ok(())
    } else {
        Err(StopSessionError::NotLive { address })
    }
}

#[must_use]
pub fn conversation_session_address(endpoint: &str, conversation_id: &str) -> SessionAddress {
    let mut input = endpoint.as_bytes().to_vec();
    input.push(0);
    input.extend_from_slice(conversation_id.as_bytes());
    let hash = fnv1a64(&input);
    SessionAddress::from(format!("external-{}-{hash:016x}", slugify(endpoint)))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_bus::session::SessionCategory;

    fn sample_info(address: &str) -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from(address),
            run_id: "run-1".to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Running,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research the thing".to_string(),
            agent_skill: None,
            model: ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    #[test]
    fn conversation_session_address_is_stable_for_the_same_conversation() {
        let a = conversation_session_address("discord", "12345");
        let b = conversation_session_address("discord", "12345");
        assert_eq!(
            a, b,
            "the same conversation must always resolve to the same address"
        );
    }

    #[test]
    fn conversation_session_address_differs_by_conversation_id() {
        let a = conversation_session_address("discord", "12345");
        let b = conversation_session_address("discord", "67890");
        assert_ne!(a, b);
    }

    #[test]
    fn conversation_session_address_differs_by_endpoint() {
        let a = conversation_session_address("discord", "12345");
        let b = conversation_session_address("telegram", "12345");
        assert_ne!(
            a, b,
            "the same conversation id on two different interfaces must not collide"
        );
    }

    #[test]
    fn conversation_session_address_is_url_and_filename_safe() {
        let addr = conversation_session_address("teams", "19:abc@thread.tacv2");
        let s = addr.as_ref();
        assert!(
            s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "address must contain only URL/filename-safe characters, got {s}"
        );
    }

    #[test]
    fn conversation_session_address_is_prefixed_by_endpoint() {
        let addr = conversation_session_address("discord", "12345");
        assert!(addr.as_ref().starts_with("external-discord-"));
    }

    #[test]
    fn register_and_get_round_trips() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0001");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let got = registry.get(&info.address).unwrap();
        assert_eq!(got.address, info.address);
        assert_eq!(got.state, SessionState::Running);
    }

    #[test]
    fn register_refuses_a_second_run_at_an_address_already_live() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-cas0001");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let mut other = sample_info("spawned-researcher-cas0001");
        other.run_id = "run-2".to_string();
        let err = registry
            .register(other, CancellationToken::new())
            .expect_err("a second registration at a live address must be refused");
        assert_eq!(err.address, info.address);

        assert_eq!(
            registry.get(&info.address).unwrap().run_id,
            info.run_id,
            "a refused registration must not clobber the live entry it lost to"
        );
    }

    #[test]
    fn register_succeeds_again_once_the_address_has_been_removed() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-cas0002");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        registry.remove(&info.address, &info.run_id);

        let mut resumed = sample_info("spawned-researcher-cas0002");
        resumed.run_id = "run-resumed".to_string();
        assert!(
            registry.register(resumed, CancellationToken::new()).is_ok(),
            "a legitimate resume, registering after the old run cleared, must still succeed"
        );
    }

    #[test]
    fn set_state_updates_existing_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0002");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        registry.set_state(&info.address, SessionState::Idle);
        assert_eq!(
            registry.get(&info.address).unwrap().state,
            SessionState::Idle
        );
    }

    #[test]
    fn set_state_on_unknown_address_is_a_noop() {
        let registry = SessionRegistry::new();
        registry.set_state(&SessionAddress::from("nope"), SessionState::Idle);
        assert!(registry.get(&SessionAddress::from("nope")).is_none());
    }

    fn usage(input: u32, output: u32) -> residuum_inference::Usage {
        residuum_inference::Usage {
            uncached_input_tokens: Some(input),
            output_tokens: Some(output),
            ..Default::default()
        }
    }

    #[test]
    fn accumulate_usage_folds_into_the_running_session_entry() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0010");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let first = registry
            .accumulate_usage(&info.address, Some(usage(100, 20)))
            .expect("a live session should accumulate");
        assert_eq!(first.input_tokens, 100);
        assert_eq!(first.output_tokens, 20);

        let second = registry
            .accumulate_usage(&info.address, Some(usage(50, 10)))
            .expect("a live session should still accumulate");
        assert_eq!(
            second.input_tokens, 150,
            "totals must accumulate across calls"
        );
        assert_eq!(second.output_tokens, 30);

        assert_eq!(
            registry.get(&info.address).unwrap().usage,
            second,
            "the registry entry's own usage field must reflect the latest totals"
        );
    }

    #[test]
    fn accumulate_usage_on_unknown_address_returns_none() {
        let registry = SessionRegistry::new();
        assert!(
            registry
                .accumulate_usage(&SessionAddress::from("ghost"), Some(usage(10, 5)))
                .is_none()
        );
    }

    #[test]
    fn accumulate_tool_calls_folds_into_the_running_session_entry() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0012");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let first = registry
            .accumulate_tool_calls(&info.address, 3)
            .expect("a live session should accumulate");
        assert_eq!(first.tool_calls, 3);

        let second = registry
            .accumulate_tool_calls(&info.address, 2)
            .expect("a live session should still accumulate");
        assert_eq!(
            second.tool_calls, 5,
            "tool calls must accumulate across batches"
        );

        assert_eq!(
            registry.get(&info.address).unwrap().usage,
            second,
            "the registry entry's own usage field must reflect the latest totals"
        );
    }

    #[test]
    fn accumulate_tool_calls_on_unknown_address_returns_none() {
        let registry = SessionRegistry::new();
        assert!(
            registry
                .accumulate_tool_calls(&SessionAddress::from("ghost"), 1)
                .is_none()
        );
    }

    #[tokio::test]
    async fn session_usage_sink_accumulates_through_the_registry() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0011");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let sink = SessionUsageSink::new(
            &registry,
            &info,
            &residuum_config::WorkspaceLayout::new(dir.path()),
        );
        let totals =
            residuum_agent::usage::UsageSink::accumulate(&sink, Some(usage(200, 40))).await;
        assert_eq!(totals.input_tokens, 200);
        assert_eq!(registry.get(&info.address).unwrap().usage.output_tokens, 40);
    }

    #[tokio::test]
    async fn session_usage_sink_records_tool_calls_through_the_registry() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0013");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let sink = SessionUsageSink::new(
            &registry,
            &info,
            &residuum_config::WorkspaceLayout::new(dir.path()),
        );
        let totals = residuum_agent::usage::UsageSink::record_tool_calls(&sink, 4).await;
        assert_eq!(totals.tool_calls, 4);
        assert_eq!(registry.get(&info.address).unwrap().usage.tool_calls, 4);
    }

    #[tokio::test]
    async fn session_usage_sink_for_a_completed_session_returns_default() {
        let registry = SessionRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let sink = SessionUsageSink::new(
            &registry,
            &sample_info("gone"),
            &residuum_config::WorkspaceLayout::new(dir.path()),
        );
        let totals = residuum_agent::usage::UsageSink::accumulate(&sink, Some(usage(10, 5))).await;
        assert_eq!(
            totals,
            residuum_bus::usage::SessionUsageTotals::default(),
            "a session no longer in the registry has nowhere to accumulate; the sink degrades to a no-op"
        );
    }

    struct SessionModel;

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for SessionModel {
        async fn complete(
            &self,
            _messages: &[residuum_inference::Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            Ok(residuum_inference::InferenceResponse::new(
                String::new(),
                vec![],
            ))
        }

        fn model_name(&self) -> &'static str {
            "session-model"
        }
    }

    #[tokio::test]
    async fn session_usage_sink_records_each_call_under_the_runs_trigger_and_id() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("scheduled-pulse-0001");
        info.trigger = EventTrigger::Pulse;
        let dir = tempfile::tempdir().unwrap();
        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        let sink = SessionUsageSink::new(&registry, &info, &layout);
        let mut response = residuum_inference::InferenceResponse::new("ok".to_string(), vec![]);
        response.usage = Some(usage(70, 9));
        residuum_agent::usage::UsageSink::record_call(&sink, &SessionModel, &Ok(response)).await;

        let records = residuum_inference::UsageRecorder::new(&layout)
            .load(DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC, None)
            .await
            .unwrap();
        assert_eq!(records.len(), 1, "one record for the one call");
        let record = records.first().unwrap();
        assert_eq!(
            record.component,
            residuum_inference::UsageComponent::Session {
                kind: "pulse".to_string()
            }
        );
        assert_eq!(record.conversation.as_deref(), Some("run-1"));
        assert_eq!(record.model, "session-model");
        assert_eq!(record.usage, Some(usage(70, 9)));
    }

    #[test]
    fn stop_cancels_token_for_running_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0003");
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone()).unwrap();

        assert!(registry.stop(&info.address));
        assert!(token.is_cancelled());
    }

    #[test]
    fn stop_returns_false_for_unknown_address() {
        let registry = SessionRegistry::new();
        assert!(!registry.stop(&SessionAddress::from("ghost")));
    }

    #[test]
    fn stop_returns_false_for_already_completing_session() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("spawned-researcher-0004");
        info.state = SessionState::Completing;
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert!(!registry.stop(&info.address));
    }

    #[test]
    fn stop_if_running_cancels_token_for_a_running_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0006");
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone()).unwrap();

        assert!(registry.stop_if_running(&info.address));
        assert!(token.is_cancelled());
    }

    #[test]
    fn stop_if_running_leaves_an_idle_session_untouched() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("spawned-researcher-0007");
        info.state = SessionState::Idle;
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone()).unwrap();

        assert!(
            !registry.stop_if_running(&info.address),
            "an idle session has nothing running to stop"
        );
        assert!(
            !token.is_cancelled(),
            "an idle session must not be cancelled by stop_if_running"
        );
        assert_eq!(
            registry.get(&info.address).unwrap().state,
            SessionState::Idle
        );
    }

    #[test]
    fn stop_if_running_returns_false_for_unknown_address() {
        let registry = SessionRegistry::new();
        assert!(!registry.stop_if_running(&SessionAddress::from("ghost")));
    }

    #[test]
    fn stop_if_running_returns_false_for_a_forking_session() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("spawned-researcher-0008");
        info.state = SessionState::Forking;
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone()).unwrap();

        assert!(!registry.stop_if_running(&info.address));
        assert!(!token.is_cancelled());
    }

    #[test]
    fn stop_all_cancels_every_live_session_but_skips_completing() {
        let registry = SessionRegistry::new();
        let running = sample_info("spawned-a-0001");
        let mut idle = sample_info("spawned-b-0002");
        idle.state = SessionState::Idle;
        let mut completing = sample_info("spawned-c-0003");
        completing.state = SessionState::Completing;

        let running_token = CancellationToken::new();
        let idle_token = CancellationToken::new();
        let completing_token = CancellationToken::new();
        let _running_rx = registry
            .register(running.clone(), running_token.clone())
            .unwrap();
        let _idle_rx = registry.register(idle.clone(), idle_token.clone()).unwrap();
        let _completing_rx = registry
            .register(completing.clone(), completing_token.clone())
            .unwrap();

        let signalled = registry.stop_all();

        assert_eq!(
            signalled, 2,
            "only the running and idle sessions should be signalled"
        );
        assert!(running_token.is_cancelled());
        assert!(idle_token.is_cancelled());
        assert!(
            !completing_token.is_cancelled(),
            "a session already completing should be left alone"
        );
    }

    #[test]
    fn stop_all_on_empty_registry_returns_zero() {
        let registry = SessionRegistry::new();
        assert_eq!(registry.stop_all(), 0);
    }

    #[test]
    fn remove_takes_session_out_of_live_list() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0005");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert_eq!(registry.list_live().len(), 1);
        let removed = registry.remove(&info.address, &info.run_id).unwrap();
        assert_eq!(removed.address, info.address);
        assert!(registry.list_live().is_empty());
    }

    #[test]
    fn remove_is_a_noop_for_a_stale_run_id() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0005b");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert!(registry.remove(&info.address, "run-stale").is_none());
        assert_eq!(
            registry.list_live().len(),
            1,
            "the live entry must survive a removal attempt naming the wrong run id"
        );
    }

    #[test]
    fn list_live_orders_by_start_time() {
        let registry = SessionRegistry::new();
        let mut earlier = sample_info("spawned-a-0001");
        earlier.started_at = Utc::now() - chrono::Duration::seconds(60);
        let later = sample_info("spawned-b-0002");

        let _later_rx = registry
            .register(later.clone(), CancellationToken::new())
            .unwrap();
        let _earlier_rx = registry
            .register(earlier.clone(), CancellationToken::new())
            .unwrap();

        let mut live = registry.list_live().into_iter();
        assert_eq!(live.next().unwrap().address, earlier.address);
        assert_eq!(live.next().unwrap().address, later.address);
        assert!(live.next().is_none());
    }

    #[test]
    fn subagent_snapshot_reflects_live_sessions() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0006");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let snapshot = registry.subagent_snapshot();
        assert_eq!(snapshot.len(), 1);
        let first = snapshot.first().unwrap();
        assert_eq!(first.name, info.address.to_string());
        assert!(first.status.contains("running"));
    }

    fn sample_agent_message() -> Interrupt {
        Interrupt::AgentMessage(residuum_bus::AgentMessageEvent::new(
            SessionAddress::from(MAIN_ADDRESS),
            "main",
            "hello",
            0,
        ))
    }

    #[test]
    fn deliver_sends_to_a_live_session_channel() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0007");
        let mut rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert_eq!(
            registry.deliver(&info.address, sample_agent_message()),
            DeliverOutcome::Delivered
        );
        let received = rx.try_recv().expect("message should be queued");
        assert!(matches!(received, Interrupt::AgentMessage(_)));
    }

    #[test]
    fn deliver_returns_not_live_for_unknown_address() {
        let registry = SessionRegistry::new();
        assert_eq!(
            registry.deliver(&SessionAddress::from("ghost"), sample_agent_message()),
            DeliverOutcome::NotLive
        );
    }

    #[test]
    fn deliver_returns_completing_for_a_session_tearing_down() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("spawned-researcher-0007b");
        info.state = SessionState::Completing;
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert_eq!(
            registry.deliver(&info.address, sample_agent_message()),
            DeliverOutcome::Completing,
            "a completing session must not accept a message into its own run"
        );
    }

    #[test]
    fn deliver_returns_full_when_the_channel_is_saturated() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0007c");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        for _ in 0..INTERRUPT_CHANNEL_CAPACITY {
            assert_eq!(
                registry.deliver(&info.address, sample_agent_message()),
                DeliverOutcome::Delivered
            );
        }
        assert_eq!(
            registry.deliver(&info.address, sample_agent_message()),
            DeliverOutcome::Full,
            "a saturated channel must be distinguishable from an unknown address"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_until_clear_returns_once_the_entry_is_removed() {
        let registry = std::sync::Arc::new(SessionRegistry::new());
        let info = sample_info("spawned-researcher-0007d");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let waiter_registry = std::sync::Arc::clone(&registry);
        let address = info.address.clone();
        let waiter = residuum_core::util::spawn_in_span(async move {
            waiter_registry.wait_until_clear(&address).await;
        });

        residuum_core::testing::clock::elapse(Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "should still be waiting while the entry is live"
        );

        registry.remove(&info.address, &info.run_id);
        residuum_core::testing::wait::guarded(
            "wait_until_clear to return once the entry is removed",
            waiter,
        )
        .await
        .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn wait_until_empty_returns_only_once_the_last_entry_is_removed() {
        let registry = std::sync::Arc::new(SessionRegistry::new());
        let first = sample_info("spawned-researcher-0007e");
        let second = sample_info("spawned-researcher-0007f");
        let _first_rx = registry
            .register(first.clone(), CancellationToken::new())
            .unwrap();
        let _second_rx = registry
            .register(second.clone(), CancellationToken::new())
            .unwrap();

        let waiter_registry = std::sync::Arc::clone(&registry);
        let waiter = residuum_core::util::spawn_in_span(async move {
            waiter_registry.wait_until_empty().await;
        });

        registry.remove(&first.address, &first.run_id);
        residuum_core::testing::clock::elapse(Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "should still be waiting while one entry is live"
        );

        registry.remove(&second.address, &second.run_id);
        residuum_core::testing::wait::guarded(
            "wait_until_empty to return once the last entry is removed",
            waiter,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wait_until_empty_returns_at_once_for_an_empty_registry() {
        let registry = SessionRegistry::new();
        residuum_core::testing::wait::guarded(
            "wait_until_empty on an empty registry to return",
            registry.wait_until_empty(),
        )
        .await;
    }

    #[test]
    fn a_resume_point_saved_with_a_model_tier_key_still_loads() {
        let mut saved = serde_json::to_value(sample_resume_point("run-old")).unwrap();
        let map = saved.as_object_mut().unwrap();
        map.remove("model");
        map.insert("model_tier".to_string(), serde_json::json!("large"));
        let point: ResumePoint = serde_json::from_value(saved).unwrap();
        assert_eq!(point.model, ModelSelector::LARGE);
    }

    fn sample_resume_point(run_id: &str) -> ResumePoint {
        ResumePoint {
            previous_run_id: run_id.to_string(),
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            agent_skill: Some(SkillName::from("researcher")),
            model: ModelSelector::LARGE,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            conversation_target: None,
            recorded_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn resume_point_round_trips() {
        let registry = SessionRegistry::new();
        let address = SessionAddress::from("spawned-researcher-0008");
        let point = sample_resume_point("run-1");
        registry.record_resume_point(&address, point.clone()).await;

        let found = registry
            .resume_point(&address)
            .expect("resume point should be recorded");
        assert_eq!(found.previous_run_id, "run-1");
        assert_eq!(found.spawner, Some(SessionAddress::from(MAIN_ADDRESS)));
        assert_eq!(found.depth, 1);
    }

    #[test]
    fn resume_point_is_none_for_a_session_that_never_ran() {
        let registry = SessionRegistry::new();
        assert!(
            registry
                .resume_point(&SessionAddress::from("never-existed"))
                .is_none()
        );
    }

    #[tokio::test]
    async fn record_resume_point_on_a_registry_built_with_new_never_touches_disk() {
        let registry = SessionRegistry::new();
        let address = SessionAddress::from("spawned-researcher-mem-only");
        registry
            .record_resume_point(&address, sample_resume_point("run-1"))
            .await;
        assert!(registry.resume_point(&address).is_some());
    }

    #[tokio::test]
    async fn record_resume_point_persists_write_through_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");
        let registry = SessionRegistry::load(path.clone()).await;
        let address = SessionAddress::from("spawned-researcher-persisted");

        registry
            .record_resume_point(&address, sample_resume_point("run-1"))
            .await;

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let on_disk: HashMap<SessionAddress, ResumePoint> =
            serde_json::from_str(&contents).unwrap();
        assert_eq!(
            on_disk
                .get(&address)
                .expect("the recorded point should be on disk")
                .previous_run_id,
            "run-1"
        );
    }

    #[tokio::test]
    async fn a_resume_point_survives_a_simulated_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");
        let address = SessionAddress::from("external-discord-restart-test");

        {
            let registry = SessionRegistry::load(path.clone()).await;
            registry
                .record_resume_point(&address, sample_resume_point("run-before-restart"))
                .await;
        }

        let restarted = SessionRegistry::load(path).await;
        let found = restarted
            .resume_point(&address)
            .expect("the resume point must survive the simulated restart");
        assert_eq!(found.previous_run_id, "run-before-restart");
    }

    #[tokio::test]
    async fn concurrent_recordings_all_reach_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");
        let registry = std::sync::Arc::new(SessionRegistry::load(path.clone()).await);

        let recordings = (0..32).map(|i| {
            let registry = std::sync::Arc::clone(&registry);
            residuum_core::util::spawn_in_span(async move {
                let address = SessionAddress::from(format!("external-concurrent-{i}"));
                registry
                    .record_resume_point(&address, sample_resume_point(&format!("run-{i}")))
                    .await;
            })
        });
        for handle in recordings {
            handle.await.unwrap();
        }

        let restarted = SessionRegistry::load(path).await;
        for i in 0..32 {
            let address = SessionAddress::from(format!("external-concurrent-{i}"));
            assert!(
                restarted.resume_point(&address).is_some(),
                "resume point {i} must be on disk after concurrent recordings"
            );
        }
    }

    #[tokio::test]
    async fn load_from_a_missing_file_starts_with_no_resume_points() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let registry = SessionRegistry::load(path).await;
        assert!(
            registry
                .resume_point(&SessionAddress::from("anything"))
                .is_none()
        );
    }

    #[tokio::test]
    async fn load_from_a_corrupt_file_starts_empty_instead_of_blocking_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");
        tokio::fs::write(&path, "not valid json").await.unwrap();

        let registry = SessionRegistry::load(path).await;
        assert!(
            registry
                .resume_point(&SessionAddress::from("anything"))
                .is_none(),
            "a corrupt persisted file must not block startup or panic — it should just \
             start with no resume points, as if the file were empty"
        );
    }

    #[tokio::test]
    async fn load_keeps_entries_regardless_of_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");

        let mut ancient = sample_resume_point("run-ancient");
        ancient.recorded_at = Utc::now() - chrono::Duration::days(400);
        let mut fresh = sample_resume_point("run-fresh");
        fresh.recorded_at = Utc::now() - chrono::Duration::days(1);

        let mut on_disk = HashMap::new();
        on_disk.insert(SessionAddress::from("spawned-ancient"), ancient);
        on_disk.insert(SessionAddress::from("spawned-fresh"), fresh);
        tokio::fs::write(&path, serde_json::to_string(&on_disk).unwrap())
            .await
            .unwrap();

        let registry = SessionRegistry::load(path).await;
        assert!(
            registry
                .resume_point(&SessionAddress::from("spawned-ancient"))
                .is_some(),
            "a resume point is small; there's no reason to drop an old one on load"
        );
        assert!(
            registry
                .resume_point(&SessionAddress::from("spawned-fresh"))
                .is_some()
        );
    }
}
