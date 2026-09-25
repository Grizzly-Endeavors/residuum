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
    append_recent_messages, clear_recent_messages, load_recent_messages,
};
use crate::memory::types::{SourceTag, Visibility};
use crate::workspace::layout::WorkspaceLayout;

/// Persist new messages and check whether observation thresholds are met.
///
/// Appends messages to the recent messages file and returns the appropriate
/// `ObserveAction` based on current token levels.
pub(super) async fn persist_and_check_thresholds(
    new_messages: &[crate::inference::Message],
    visibility: Visibility,
    observer: &Observer,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
) -> ObserveAction {
    if new_messages.is_empty() {
        return ObserveAction::None;
    }

    if let Err(e) =
        append_recent_messages(&layout.recent_messages_json(), new_messages, visibility, tz).await
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

/// Subsystem references for the main agent's observation flow.
pub(super) struct MemorySubsystems<'a> {
    pub observer: &'a Observer,
    pub merge_writer: &'a Arc<MemoryMergeWriter>,
    pub layout: &'a WorkspaceLayout,
    pub tz: chrono_tz::Tz,
    /// For the once-per-streak user notices on an automatic observer/reflector
    /// failure or recovery — see [`execute_observation`].
    pub publisher: &'a Publisher,
}

/// Execute an observation cycle: extract, merge, clear file, rotate messages, reload.
///
/// This is the *automatic* trigger (a threshold crossing) — it backs off
/// after a failure rather than re-attempting, and re-spending an LLM call,
/// on every later crossing while recent messages keep accumulating
/// unobserved; see [`Observer::automatic_failure_tracker`]. A manually
/// forced observe ([`run_forced_observe`]) always attempts regardless.
#[tracing::instrument(skip_all)]
pub(super) async fn execute_observation(mem: &MemorySubsystems<'_>, agent: &mut Agent) {
    use crate::util::{NoticeAction, RetryGate};

    let recent = match load_recent_messages(&mem.layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent messages for observation");
            return;
        }
    };

    if recent.is_empty() {
        return;
    }

    let tracker = mem.observer.automatic_failure_tracker();
    if tracker.gate() == RetryGate::Skip {
        tracing::debug!("observer is backing off after a recent failure, skipping this attempt");
        return;
    }

    let extraction = match mem.observer.extract(&recent, mem.layout).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "observer failed");
            if tracker.record_failure() == NoticeAction::FailureStarted {
                publish_notice(
                    mem.publisher,
                    "[memory] the observer couldn't process recent messages; it will keep retrying \
                     with a backing-off delay. Recent messages are safe and will be observed once \
                     it recovers."
                        .to_string(),
                )
                .await;
            }
            return;
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
                    mem.publisher,
                    "[memory] the observer has recovered".to_string(),
                )
                .await;
            }
            apply_observation_outcome(mem, agent, &outcome).await;
            if outcome.reflected {
                tracing::info!(episode_id = %outcome.id, "reflection triggered");
            }
            if let Some(notice) = outcome.reflector_notice {
                publish_reflector_notice(mem.publisher, notice).await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to merge observation");
            if tracker.record_failure() == NoticeAction::FailureStarted {
                publish_notice(
                    mem.publisher,
                    "[memory] the observer extracted new observations but couldn't save them; it \
                     will keep retrying with a backing-off delay."
                        .to_string(),
                )
                .await;
            }
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

/// Apply post-merge steps for the main agent: save the recent-context
/// narrative, clear recent messages, rotate the agent's own history, and
/// reload its observation/recent-context views. Only the main agent's own
/// observations replace the recent-context narrative — session merges never
/// touch it (see the design's "Memory model").
async fn apply_observation_outcome(
    mem: &MemorySubsystems<'_>,
    agent: &mut Agent,
    outcome: &crate::memory::merge_writer::MergeOutcome,
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

    if let Err(e) = clear_recent_messages(&mem.layout.recent_messages_json()).await {
        tracing::warn!(error = %e, "failed to clear recent messages");
    }
    agent.rotate_messages_after_observation();

    if let Err(e) = agent.reload_observations(mem.layout).await {
        tracing::warn!(error = %e, "failed to reload observations");
    }
    if let Err(e) = agent.reload_recent_context(mem.layout).await {
        tracing::warn!(error = %e, "failed to reload recent context");
    }
}

/// Force an observation cycle regardless of token threshold.
///
/// Loads recent messages, extracts and merges, clears recent messages, and
/// publishes a notice.
#[tracing::instrument(skip_all)]
pub(super) async fn run_forced_observe(
    mem: &MemorySubsystems<'_>,
    agent: &mut Agent,
    publisher: &Publisher,
) {
    let recent = match load_recent_messages(&mem.layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "forced observe failed to load recent messages");
            publish_error(publisher, format!("observe failed: {e}")).await;
            return;
        }
    };

    if recent.is_empty() {
        publish_notice(
            publisher,
            "[memory] observe: no recent messages".to_string(),
        )
        .await;
        return;
    }

    // A manual force always attempts regardless of the automatic tracker's
    // backoff state, but still reports its outcome to it — a working manual
    // retry should un-stick a stuck automatic backoff just as readily as a
    // later automatic success would.
    let extraction = match mem.observer.extract(&recent, mem.layout).await {
        Ok(e) => {
            mem.observer.automatic_failure_tracker().record_success();
            e
        }
        Err(e) => {
            mem.observer.automatic_failure_tracker().record_failure();
            tracing::warn!(error = %e, "forced observe failed");
            publish_error(publisher, format!("observe failed: {e}")).await;
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
            publish_error(publisher, format!("observe failed: {e}")).await;
            return;
        }
    };

    apply_observation_outcome(mem, agent, &outcome).await;

    let suffix = if outcome.reflected {
        "; reflection triggered"
    } else {
        ""
    };
    let notice = format!(
        "[memory] observed: {} ({} observations){suffix}",
        outcome.id,
        outcome.observations.len()
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
                    "[memory] reflected: {} observations",
                    compressed.observations.len()
                ),
            )
            .await;
        }
        Err(e) => {
            merge_writer.reflector_failure_tracker().record_failure();
            tracing::warn!(error = %e, "forced reflect failed");
            publish_error(publisher, format!("reflect failed: {e}")).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentConfig, HopCounter};
    use crate::bus::{NoticeEvent, NotifyName, SYSTEM_CHANNEL, spawn_broker, topics};
    use crate::inference::CompletionOptions;
    use crate::memory::recent_messages::append_recent_messages;
    use crate::memory::reflector::{Reflector, ReflectorConfig};
    use crate::memory::search::MemoryIndex;

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
        let mut agent = test_agent();

        let mem = MemorySubsystems {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            tz: TEST_TZ,
            publisher: &publisher,
        };

        // First attempt: extract fails (NullProvider always errors) — a new
        // failure streak, so the user is told once.
        execute_observation(&mem, &mut agent).await;
        let first_notice =
            tokio::time::timeout(std::time::Duration::from_millis(200), notices.recv())
                .await
                .expect("a notice should have been published")
                .expect("subscriber should still be open")
                .expect("event should deserialize");
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
        execute_observation(&mem, &mut agent).await;
        let second =
            tokio::time::timeout(std::time::Duration::from_millis(100), notices.recv()).await;
        assert!(
            second.is_err(),
            "must not renotify while backing off from the same failure streak"
        );
    }
}
