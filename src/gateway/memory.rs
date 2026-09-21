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
}

/// Execute an observation cycle: extract, merge, clear file, rotate messages, reload.
#[tracing::instrument(skip_all)]
pub(super) async fn execute_observation(mem: &MemorySubsystems<'_>, agent: &mut Agent) {
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

    let extraction = match mem.observer.extract(&recent, mem.layout).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "observer failed");
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
            apply_observation_outcome(mem, agent, &outcome).await;
            if outcome.reflected {
                tracing::info!(episode_id = %outcome.id, "reflection triggered");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to merge observation");
        }
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

    let extraction = match mem.observer.extract(&recent, mem.layout).await {
        Ok(e) => e,
        Err(e) => {
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
    match merge_writer.force_reflect().await {
        Ok(compressed) => {
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
            tracing::warn!(error = %e, "forced reflect failed");
            publish_error(publisher, format!("reflect failed: {e}")).await;
        }
    }
}
