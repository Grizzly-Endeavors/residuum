//! Memory pipeline helpers: observation, reflection, and persistence.
//!
//! Extraction (the LLM call) is [`Observer::extract`]'s job; persistence —
//! episode id allocation, the observation log, indexing, embedding, and the
//! reflector check — is the [`MemoryMergeWriter`]'s, shared with every
//! session run's own completion pipeline so episode numbering and log
//! appends never race. This module's job is the main agent's side of that
//! flow: loading/clearing `recent_messages.json`, saving the recent-context
//! narrative (session merges never touch it), and reloading the agent's
//! context after a merge.

use std::sync::Arc;

use super::helpers::{publish_error, publish_notice};
use crate::agent::Agent;
use crate::bus::Publisher;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::{ObserveAction, Observer};
use crate::memory::recent_context::{RecentContext, save_recent_context};
use crate::memory::recent_messages::{
    append_recent_messages, load_recent_messages, remove_observed_recent_messages,
};
use crate::memory::types::{SourceTag, Visibility};
use crate::workspace::layout::WorkspaceLayout;

/// Persist new messages and check whether observation thresholds are met.
///
/// Appends messages to the recent messages file and returns the appropriate
/// `ObserveAction` based on current token levels. `turn_id` is the
/// correlation id of the turn that produced these messages, or `None` for a
/// non-turn persist (e.g. a message with no matching turn).
pub(super) async fn persist_and_check_thresholds(
    new_messages: &[crate::inference::Message],
    visibility: Visibility,
    observer: &Observer,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
    turn_id: Option<&str>,
) -> ObserveAction {
    if new_messages.is_empty() {
        return ObserveAction::None;
    }

    if let Err(e) = append_recent_messages(
        &layout.recent_messages_json(),
        new_messages,
        visibility,
        tz,
        turn_id,
    )
    .await
    {
        tracing::warn!(error = %e, "failed to persist recent messages");
        return ObserveAction::None;
    }

    let recent = match load_recent_messages(&layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent messages");
            return ObserveAction::None;
        }
    };

    observer.check_thresholds(&recent)
}

/// Subsystem handles for the main agent's observation flow. Owned/`Arc`'d
/// rather than borrowed so the same value can be handed to the background
/// post-turn worker (see `crate::gateway::post_turn`), which needs `'static`
/// data to spawn with — cheap to construct fresh per call, the same way a
/// borrow used to be.
#[derive(Clone)]
pub(crate) struct MemorySubsystems {
    pub observer: Arc<Observer>,
    pub merge_writer: Arc<MemoryMergeWriter>,
    pub layout: WorkspaceLayout,
    pub tz: chrono_tz::Tz,
    /// For the once-per-streak user notices on an automatic observer/reflector
    /// failure or recovery — see [`execute_observation`].
    pub publisher: Publisher,
}

/// Run one observation cycle: extract, merge, and persist the file-only
/// outcomes (recent-context narrative, clearing observed messages).
///
/// Returns whether the caller should follow up with
/// [`apply_observation_reload`] — this function never touches `Agent`
/// itself, which is what makes it safe to run off the event loop (see
/// `crate::gateway::post_turn::ObserveWorker`); the reload it may call for
/// has to happen back on the main loop instead.
///
/// This is the *automatic* trigger (a threshold crossing) — it backs off
/// after a failure rather than re-attempting, and re-spending an LLM call,
/// on every later crossing while recent messages keep accumulating
/// unobserved; see [`Observer::automatic_failure_tracker`]. A manually
/// forced observe ([`run_forced_observe`]) always attempts regardless.
///
/// `shutdown` cancelled while the extraction call is in flight drops the
/// cycle with nothing written; once extraction returns, the merge and the
/// removal of observed messages run to completion.
#[tracing::instrument(skip_all)]
pub(crate) async fn execute_observation(
    mem: &MemorySubsystems,
    shutdown: &tokio_util::sync::CancellationToken,
) -> bool {
    use crate::util::{NoticeAction, RetryGate};

    let recent = match load_recent_messages(&mem.layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent messages for observation");
            return false;
        }
    };

    if recent.is_empty() {
        return false;
    }

    let tracker = mem.observer.automatic_failure_tracker();
    if tracker.gate() == RetryGate::Skip {
        tracing::debug!("observer is backing off after a recent failure, skipping this attempt");
        return false;
    }

    let extraction = tokio::select! {
        biased;
        () = shutdown.cancelled() => {
            tracing::debug!("shutting down, dropping in-flight observation before anything was written");
            return false;
        }
        extraction = mem.observer.extract(&recent, &mem.layout) => extraction,
    };
    let extraction = match extraction {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "observer failed");
            if tracker.record_failure() == NoticeAction::FailureStarted {
                publish_notice(
                    &mem.publisher,
                    "[memory] the observer couldn't process recent messages; it will keep retrying \
                     with a backing-off delay. Recent messages are safe and will be observed once \
                     it recovers."
                        .to_string(),
                )
                .await;
            }
            return false;
        }
    };

    match mem
        .merge_writer
        .merge(extraction, SourceTag::main(), mem.tz)
        .await
    {
        Ok(outcome) => {
            tracing::info!(episode_id = %outcome.id, "observer extracted episode");
            if tracker.record_success() == NoticeAction::Recovered {
                publish_notice(
                    &mem.publisher,
                    "[memory] the observer has recovered".to_string(),
                )
                .await;
            }
            persist_observation_outcome(mem, &outcome, recent.len()).await;
            if outcome.reflected {
                tracing::info!(episode_id = %outcome.id, "reflection triggered");
            }
            if let Some(notice) = outcome.reflector_notice {
                publish_reflector_notice(&mem.publisher, notice).await;
            }
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to merge observation");
            if tracker.record_failure() == NoticeAction::FailureStarted {
                publish_notice(
                    &mem.publisher,
                    "[memory] the observer extracted new observations but couldn't save them; it \
                     will keep retrying with a backing-off delay."
                        .to_string(),
                )
                .await;
            }
            false
        }
    }
}

/// Tell the user about an automatic reflector failure/recovery, in the same
/// once-per-streak shape as the observer's own notices above.
async fn publish_reflector_notice(publisher: &Publisher, notice: crate::util::NoticeAction) {
    use crate::util::NoticeAction;
    match notice {
        NoticeAction::FailureStarted => {
            publish_notice(
                publisher,
                "[memory] the reflector couldn't compress the observation log; it will keep \
                 retrying with a backing-off delay. Nothing is lost — observations just won't be \
                 compressed until it recovers."
                    .to_string(),
            )
            .await;
        }
        NoticeAction::Recovered => {
            publish_notice(
                publisher,
                "[memory] the reflector has recovered".to_string(),
            )
            .await;
        }
        NoticeAction::None => {}
    }
}

/// Persist a successful merge's file-only side effects: save the
/// recent-context narrative and remove the `observed` messages the cycle
/// loaded (and only those — a turn that ended while the cycle ran appended
/// messages it never saw, and they wait for the next cycle).
/// No `Agent` access needed — see [`execute_observation`]'s own doc for why
/// that matters. Only the main agent's own observations replace the
/// recent-context narrative — session merges never touch it (see the
/// design's "Memory model").
async fn persist_observation_outcome(
    mem: &MemorySubsystems,
    outcome: &crate::memory::merge_writer::MergeOutcome,
    observed: usize,
) {
    if let Some(narrative) = &outcome.narrative {
        let ctx = RecentContext {
            narrative: narrative.clone(),
            created_at: crate::time::now_local(mem.observer.timezone()),
            episode_id: outcome.id.clone(),
        };
        if let Err(e) = save_recent_context(&mem.layout.recent_context_json(), &ctx).await {
            tracing::warn!(error = %e, "failed to save recent context");
        }
    }

    if let Err(e) =
        remove_observed_recent_messages(&mem.layout.recent_messages_json(), observed).await
    {
        tracing::warn!(error = %e, observed, "failed to remove observed recent messages");
    }
}

/// The `Agent`-touching tail of a successful observation: rotate its
/// in-memory history and reload the observations/recent-context views it
/// assembles prompts from.
///
/// Always applied on the main loop — from a manually forced observe
/// immediately, or from the background worker's result once a cycle
/// finishes (see `crate::gateway::post_turn`) — never from inside the
/// background task itself, since `Agent` isn't shared off the event loop.
/// This is the "one step stale" window the owner accepted: a turn that
/// starts before this runs still sees the pre-observation context.
pub(crate) async fn apply_observation_reload(agent: &mut Agent, layout: &WorkspaceLayout) {
    agent.rotate_messages_after_observation();
    if let Err(e) = agent.reload_observations(layout).await {
        tracing::warn!(error = %e, "failed to reload observations");
    }
    if let Err(e) = agent.reload_recent_context(layout).await {
        tracing::warn!(error = %e, "failed to reload recent context");
    }
}

/// Force an observation cycle regardless of token threshold.
///
/// Loads recent messages, extracts and merges, removes the observed messages,
/// and publishes a notice.
#[tracing::instrument(skip_all)]
pub(super) async fn run_forced_observe(
    mem: &MemorySubsystems,
    agent: &mut Agent,
    publisher: &Publisher,
) {
    let recent = match load_recent_messages(&mem.layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "forced observe failed to load recent messages");
            publish_error(
                publisher,
                "Couldn't summarize older messages. Try again.".to_string(),
                Some(format!("{e:#}")),
            )
            .await;
            return;
        }
    };

    if recent.is_empty() {
        publish_notice(
            publisher,
            "There are no recent messages to summarize.".to_string(),
        )
        .await;
        return;
    }

    // A manual force always attempts regardless of the automatic tracker's
    // backoff state, but still reports its outcome to it — a working manual
    // retry should un-stick a stuck automatic backoff just as readily as a
    // later automatic success would.
    let extraction = match mem.observer.extract(&recent, &mem.layout).await {
        Ok(e) => {
            mem.observer.automatic_failure_tracker().record_success();
            e
        }
        Err(e) => {
            mem.observer.automatic_failure_tracker().record_failure();
            tracing::warn!(error = %e, "forced observe failed");
            publish_error(
                publisher,
                "Couldn't summarize older messages. Try again.".to_string(),
                Some(format!("{e:#}")),
            )
            .await;
            return;
        }
    };

    let outcome = match mem
        .merge_writer
        .merge(extraction, SourceTag::main(), mem.tz)
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, "forced observe failed to merge");
            publish_error(
                publisher,
                "Couldn't summarize older messages. Try again.".to_string(),
                Some(format!("{e:#}")),
            )
            .await;
            return;
        }
    };

    persist_observation_outcome(mem, &outcome, recent.len()).await;
    apply_observation_reload(agent, &mem.layout).await;

    let suffix = if outcome.reflected {
        " and condensed the memory log"
    } else {
        ""
    };
    let count = recent.len();
    let notice = format!(
        "Summarized {count} older message{}{suffix}.",
        if count == 1 { "" } else { "s" }
    );
    publish_notice(publisher, notice).await;
}

/// Force a reflection cycle regardless of observation log size.
///
/// Runs the reflector, reloads observations into the agent, and publishes a notice.
#[tracing::instrument(skip_all)]
pub(super) async fn run_forced_reflect(
    merge_writer: &MemoryMergeWriter,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
    publisher: &Publisher,
) {
    // Same reasoning as `run_forced_observe`: this bypasses the automatic
    // reflector tracker's backoff gate, but still reports its outcome to it.
    match merge_writer.force_reflect().await {
        Ok(compressed) => {
            merge_writer.reflector_failure_tracker().record_success();
            if let Err(e) = agent.reload_observations(layout).await {
                tracing::warn!(error = %e, "failed to reload observations after forced reflect");
            }
            publish_notice(
                publisher,
                format!(
                    "Condensed the memory log into {} observations.",
                    compressed.observations.len()
                ),
            )
            .await;
        }
        Err(e) => {
            merge_writer.reflector_failure_tracker().record_failure();
            tracing::warn!(error = %e, "forced reflect failed");
            publish_error(
                publisher,
                "Couldn't condense the memory log. Try again.".to_string(),
                Some(format!("{e:#}")),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentConfig, HopCounter};
    use crate::bus::{ErrorEvent, NoticeEvent, NotifyName, SYSTEM_CHANNEL, spawn_broker, topics};
    use crate::inference::CompletionOptions;
    use crate::memory::recent_messages::append_recent_messages;
    use crate::memory::reflector::{Reflector, ReflectorConfig};
    use crate::memory::search::MemoryIndex;
    use crate::testing::wait;

    const TEST_TZ: chrono_tz::Tz = chrono_tz::UTC;

    fn test_agent() -> Agent {
        Agent::new(
            Box::new(crate::inference::providers::null::NullProvider),
            crate::tools::ToolRegistry::new(),
            crate::mcp::McpRegistry::new_shared(),
            crate::workspace::identity::IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: TEST_TZ,
                layout: None,
            },
            HopCounter::new(0),
        )
    }

    fn always_failing_observer() -> Observer {
        // The observer's threshold config is irrelevant here — the test
        // calls `execute_observation` directly rather than going through
        // `check_thresholds` — only the `NullProvider`'s guaranteed error
        // matters.
        Observer::new(
            Box::new(crate::inference::providers::null::NullProvider),
            crate::memory::observer::ObserverConfig::default(),
        )
    }

    fn merge_writer(layout: &WorkspaceLayout) -> Arc<MemoryMergeWriter> {
        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let reflector = Reflector::new(
            Box::new(crate::inference::providers::null::NullProvider),
            ReflectorConfig {
                threshold_tokens: usize::MAX,
                ..ReflectorConfig::default()
            },
        );
        Arc::new(MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            None,
            None,
        ))
    }

    #[tokio::test]
    async fn automatic_observer_failure_notifies_once_then_backs_off() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        append_recent_messages(
            &layout.recent_messages_json(),
            &[crate::inference::Message::user("hello")],
            Visibility::User,
            TEST_TZ,
            None,
        )
        .await
        .unwrap();

        let observer = always_failing_observer();
        let mw = merge_writer(&layout);
        let handle = spawn_broker();
        let mut notices = handle
            .subscribe::<_, NoticeEvent>(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let publisher = handle.publisher();

        let mem = MemorySubsystems {
            observer: Arc::new(observer),
            merge_writer: mw,
            layout: layout.clone(),
            tz: TEST_TZ,
            publisher,
        };

        // First attempt: extract fails (NullProvider always errors) — a new
        // failure streak, so the user is told once.
        assert!(
            !execute_observation(&mem, &tokio_util::sync::CancellationToken::new()).await,
            "a failed cycle must not report a reload"
        );
        let first_notice =
            wait::next_event("the first observer failure notice", &mut notices).await;
        assert!(
            first_notice.message.contains("observer"),
            "got: {}",
            first_notice.message
        );

        // Recent messages must still be there — a failed observation never
        // clears the buffer, so a later success can still observe them.
        let still_pending = load_recent_messages(&layout.recent_messages_json())
            .await
            .unwrap();
        assert_eq!(
            still_pending.len(),
            1,
            "a failed observation must not discard unobserved messages"
        );

        // Second attempt, immediately after: backing off, so no repeat
        // attempt and no repeat notice.
        assert!(
            !execute_observation(&mem, &tokio_util::sync::CancellationToken::new()).await,
            "backed off, so nothing should have run"
        );
        wait::bus_barrier(&handle).await;
        assert!(
            notices.drain().is_empty(),
            "must not renotify while backing off from the same failure streak"
        );
    }

    #[tokio::test]
    async fn apply_observation_reload_does_not_panic_with_no_prior_state() {
        // The main-loop-applied tail of a successful observation cycle (see
        // `crate::gateway::post_turn`) — this only needs to run cleanly
        // against a freshly created workspace with nothing on disk yet, the
        // same as a brand new gateway's first observation.
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        let mut agent = test_agent();

        apply_observation_reload(&mut agent, &layout).await;
    }

    #[tokio::test]
    async fn persist_and_check_thresholds_sets_turn_id_from_the_turn_correlation_id() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        let observer = always_failing_observer();

        persist_and_check_thresholds(
            &[crate::inference::Message::user("hello")],
            Visibility::User,
            &observer,
            &layout,
            TEST_TZ,
            Some("turn-abc"),
        )
        .await;

        let recent = load_recent_messages(&layout.recent_messages_json())
            .await
            .unwrap();
        assert_eq!(
            recent.first().and_then(|m| m.turn_id.clone()),
            Some("turn-abc".to_string()),
            "persisted message should carry the turn's correlation id"
        );
    }

    #[tokio::test]
    async fn persist_and_check_thresholds_leaves_turn_id_none_when_not_given() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        let observer = always_failing_observer();

        persist_and_check_thresholds(
            &[crate::inference::Message::user("hello")],
            Visibility::Background,
            &observer,
            &layout,
            TEST_TZ,
            None,
        )
        .await;

        let recent = load_recent_messages(&layout.recent_messages_json())
            .await
            .unwrap();
        assert_eq!(
            recent.first().and_then(|m| m.turn_id.clone()),
            None,
            "a persist with no turn id should leave the field unset"
        );
    }

    async fn subscribe_notices(
        handle: &crate::bus::BusHandle,
    ) -> crate::bus::Subscriber<NoticeEvent> {
        handle
            .subscribe::<_, NoticeEvent>(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap()
    }

    async fn subscribe_errors(
        handle: &crate::bus::BusHandle,
    ) -> crate::bus::Subscriber<ErrorEvent> {
        handle
            .subscribe::<_, ErrorEvent>(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn run_forced_observe_with_no_recent_messages_reads_as_plain_language() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let handle = spawn_broker();
        let mut notices = subscribe_notices(&handle).await;
        let publisher = handle.publisher();
        let mem = MemorySubsystems {
            observer: Arc::new(always_failing_observer()),
            merge_writer: merge_writer(&layout),
            layout: layout.clone(),
            tz: TEST_TZ,
            publisher: publisher.clone(),
        };
        let mut agent = test_agent();

        run_forced_observe(&mem, &mut agent, &publisher).await;

        let notice = wait::next_event("the published notice", &mut notices).await;
        assert_eq!(notice.message, "There are no recent messages to summarize.");
    }

    #[tokio::test]
    async fn run_forced_observe_failure_reports_plain_message_with_cause_in_details() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        append_recent_messages(
            &layout.recent_messages_json(),
            &[crate::inference::Message::user("hello")],
            Visibility::User,
            TEST_TZ,
            None,
        )
        .await
        .unwrap();

        let handle = spawn_broker();
        let mut errors = subscribe_errors(&handle).await;
        let publisher = handle.publisher();
        let mem = MemorySubsystems {
            observer: Arc::new(always_failing_observer()),
            merge_writer: merge_writer(&layout),
            layout: layout.clone(),
            tz: TEST_TZ,
            publisher: publisher.clone(),
        };
        let mut agent = test_agent();

        run_forced_observe(&mem, &mut agent, &publisher).await;

        let error = wait::next_event("the published error", &mut errors).await;
        assert_eq!(
            error.message,
            "Couldn't summarize older messages. Try again."
        );
        assert!(
            error.details.is_some_and(|d| d.contains("null provider")),
            "the technical cause should be in details, not the message"
        );
    }

    #[tokio::test]
    async fn run_forced_observe_success_notice_reads_as_plain_language() {
        const EXTRACT_RESPONSE: &str = r#"{
            "observations": [
                {"content": "an observation", "timestamp": "2026-02-21T14:30", "visibility": "user"}
            ],
            "narrative": ""
        }"#;
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        append_recent_messages(
            &layout.recent_messages_json(),
            &[
                crate::inference::Message::user("hello"),
                crate::inference::Message::user("world"),
            ],
            Visibility::User,
            TEST_TZ,
            None,
        )
        .await
        .unwrap();

        let observer = Arc::new(Observer::new(
            Box::new(crate::memory::test_helpers::MockMemoryProvider::new(
                EXTRACT_RESPONSE,
            )),
            crate::memory::observer::ObserverConfig::default(),
        ));
        let handle = spawn_broker();
        let mut notices = subscribe_notices(&handle).await;
        let publisher = handle.publisher();
        let mem = MemorySubsystems {
            observer,
            merge_writer: merge_writer(&layout),
            layout: layout.clone(),
            tz: TEST_TZ,
            publisher: publisher.clone(),
        };
        let mut agent = test_agent();

        run_forced_observe(&mem, &mut agent, &publisher).await;

        let notice = wait::next_event("the published notice", &mut notices).await;
        assert_eq!(notice.message, "Summarized 2 older messages.");
    }

    async fn seed_observation_log(layout: &WorkspaceLayout) {
        let mut log = crate::memory::types::ObservationLog::new();
        log.observations.push(crate::memory::types::Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            source_episodes: Some("ep-001".to_string()),
            visibility: Visibility::User,
            content: "an observation".to_string(),
            source: SourceTag::main(),
        });
        crate::memory::log_store::save_observation_log(&layout.observations_json(), &log)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_forced_reflect_failure_reports_plain_message_with_cause_in_details() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        seed_observation_log(&layout).await;

        let mw = merge_writer(&layout);
        let handle = spawn_broker();
        let mut errors = subscribe_errors(&handle).await;
        let publisher = handle.publisher();
        let mut agent = test_agent();

        run_forced_reflect(&mw, &layout, &mut agent, &publisher).await;

        let error = wait::next_event("the published error", &mut errors).await;
        assert_eq!(
            error.message,
            "Couldn't condense the memory log. Try again."
        );
        assert!(
            error.details.is_some_and(|d| d.contains("null provider")),
            "the technical cause should be in details, not the message"
        );
    }

    #[tokio::test]
    async fn run_forced_reflect_success_notice_reads_as_plain_language() {
        const COMPRESSED_RESPONSE: &str = r#"{"observations": [{"content": "compressed", "timestamp": "2026-02-21T14:30", "visibility": "user"}]}"#;
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        seed_observation_log(&layout).await;

        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let reflector = Reflector::new(
            Box::new(crate::memory::test_helpers::MockMemoryProvider::new(
                COMPRESSED_RESPONSE,
            )),
            ReflectorConfig::default(),
        );
        let mw = MemoryMergeWriter::new(reflector, layout.clone(), search_index, None, None);
        let handle = spawn_broker();
        let mut notices = subscribe_notices(&handle).await;
        let publisher = handle.publisher();
        let mut agent = test_agent();

        run_forced_reflect(&mw, &layout, &mut agent, &publisher).await;

        let notice = wait::next_event("the published notice", &mut notices).await;
        assert_eq!(
            notice.message,
            "Condensed the memory log into 1 observations."
        );
    }
}
