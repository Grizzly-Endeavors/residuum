//! Idle transition logic.
//!
//! After a configurable period of user inactivity the gateway deactivates
//! active projects and skills, fires the observer, clears the message
//! buffer, and injects a continuity system message.

use std::sync::Arc;

use crate::config::BackgroundModelTier;
use crate::memory::recent_messages::load_recent_messages;
use crate::models::factory::build_provider_chain;
use crate::models::{CompletionOptions, Message};

use super::GatewayRuntime;
use super::memory::execute_observation;

/// Run the full idle transition sequence.
pub(super) async fn execute_idle_transition(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    let timeout_mins = rt.cfg.idle.timeout.as_secs() / 60;
    tracing::info!(timeout_mins, "idle timeout reached, transitioning");

    // 1. Deactivate project (if active), capturing removed skill count
    let (project_name, skill_count_from_project) = deactivate_project_if_active(rt).await;

    // 2. Deactivate remaining explicitly-activated skills
    let extra_skill_count = deactivate_remaining_skills(rt).await;
    let total_skills = skill_count_from_project + extra_skill_count;

    // 3. Fire observer, then clear in-memory message buffer
    execute_observation(
        &rt.observer,
        &rt.reflector,
        &rt.search_index,
        &rt.layout,
        &mut rt.agent,
        rt.vector_store.as_ref(),
        rt.embedding_provider.as_ref(),
    )
    .await;
    *observe_deadline = None;
    rt.agent.clear_messages();

    // 4. Switch notification interface (if configured)
    if let Some(channel_name) = rt.cfg.idle.idle_channel.clone() {
        switch_idle_interface(rt, &channel_name);
    }

    // 5. Inject system message for continuity
    let summary = format_idle_summary(timeout_mins, project_name.as_deref(), total_skills);
    rt.agent.inject_system_message(&summary);
}

/// Switch `last_reply` to the unsolicited handle for the configured idle channel.
fn switch_idle_interface(rt: &mut GatewayRuntime, channel_name: &str) {
    match rt.unsolicited_handles.get(channel_name) {
        Some(handle) => {
            rt.last_reply = Some(Arc::clone(handle));
            tracing::info!(channel = %channel_name, "switched to idle interface");
        }
        None => {
            tracing::warn!(
                channel = %channel_name,
                "idle_channel configured but no message received on that interface yet"
            );
        }
    }
}

/// Deactivate the currently active project, generating an LLM summary log.
///
/// Returns `(project_name, skills_removed_by_rescan)`.
async fn deactivate_project_if_active(rt: &mut GatewayRuntime) -> (Option<String>, usize) {
    let (name, description) = {
        let state = rt.project_state.lock().await;
        let Some(active) = state.active() else {
            return (None, 0);
        };
        (active.name.clone(), active.frontmatter.description.clone())
    };

    // Count skills before rescan so we can measure how many were removed
    let skills_before = rt.skill_state.lock().await.active_skill_names().len();

    // Generate deactivation log via LLM (or fallback)
    let deactivation_log = generate_deactivation_log(rt, &name, &description).await;

    // Deactivate project state
    let now = crate::time::now_local(rt.tz);
    {
        let mut state = rt.project_state.lock().await;
        if let Err(e) = state.deactivate(&deactivation_log, now).await {
            tracing::warn!(project = %name, error = %e, "failed to deactivate project during idle");
        }
    }

    // Clear path policy
    rt.path_policy.write().await.set_active_project(None);

    // Clear tool filter
    rt.agent.clear_tool_filter().await;

    // Deactivate MCP refs
    rt.mcp_registry
        .write()
        .await
        .deactivate_project(&name)
        .await;

    // Rescan skills (removes project-scoped skills)
    if let Err(e) = rt.skill_state.lock().await.rescan(None).await {
        tracing::warn!(error = %e, "failed to rescan skills during idle");
    }

    let skills_after = rt.skill_state.lock().await.active_skill_names().len();
    let skills_removed = skills_before.saturating_sub(skills_after);

    tracing::info!(project = %name, skills_removed, "deactivated project during idle transition");

    (Some(name), skills_removed)
}

/// Generate a deactivation log entry using the LLM.
///
/// Falls back to a plain text entry if the LLM call fails, saving the
/// raw recent messages to a timestamped file for later review.
async fn generate_deactivation_log(
    rt: &GatewayRuntime,
    project_name: &str,
    project_description: &str,
) -> String {
    let timeout_mins = rt.cfg.idle.timeout.as_secs() / 60;

    // Load recent messages for context
    let recent = match load_recent_messages(&rt.layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent messages for idle log");
            return format!(
                "[idle] Auto-deactivated after {timeout_mins}m of inactivity. \
                 Failed to load recent messages for summary."
            );
        }
    };

    // Format messages for the prompt
    let conversation = recent
        .iter()
        .map(|rm| format!("[{}] {}", rm.message.role.as_str(), rm.message.content))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "You are summarizing a work session for the project \"{project_name}\" ({project_description}).\n\
         The user has been inactive for {timeout_mins} minutes and the project is being auto-deactivated.\n\n\
         Recent conversation:\n{conversation}\n\n\
         Write a concise 1-3 sentence session log entry summarizing what was discussed or accomplished. \
         Focus on outcomes and decisions, not the idle timeout itself. \
         Write only the log entry, no preamble."
    );

    // Build a small model provider for the summary
    let specs = rt.spawn_context.background_config.models.resolve_tier(
        &BackgroundModelTier::Small,
        &rt.spawn_context.main_provider_specs,
    );

    let provider = match build_provider_chain(
        &specs,
        rt.spawn_context.max_tokens,
        rt.spawn_context.http_client.clone(),
        rt.spawn_context.retry_config.clone(),
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "failed to build provider for idle summary");
            return build_fallback_log(rt, timeout_mins, &recent).await;
        }
    };

    let messages = vec![Message::user(prompt)];
    let options = CompletionOptions {
        max_tokens: Some(512),
        ..CompletionOptions::default()
    };

    match provider.complete(&messages, &[], &options).await {
        Ok(response) if !response.content.trim().is_empty() => {
            let summary = response.content.trim().to_string();
            format!("[idle] {summary}")
        }
        Ok(_) => {
            tracing::warn!("LLM returned empty idle summary");
            build_fallback_log(rt, timeout_mins, &recent).await
        }
        Err(e) => {
            tracing::warn!(error = %e, "LLM idle summary failed");
            build_fallback_log(rt, timeout_mins, &recent).await
        }
    }
}

/// Build a fallback deactivation log and save raw messages to disk.
async fn build_fallback_log(
    rt: &GatewayRuntime,
    timeout_mins: u64,
    recent: &[crate::memory::recent_messages::RecentMessage],
) -> String {
    let now = crate::time::now_local(rt.tz);
    let month_dir = rt
        .layout
        .root()
        .join("notes")
        .join("log")
        .join(now.format("%Y-%m").to_string());

    if let Err(e) = tokio::fs::create_dir_all(&month_dir).await {
        tracing::warn!(error = %e, "failed to create idle log directory");
        return format!(
            "[idle] Auto-deactivated after {timeout_mins}m of inactivity. \
             LLM summary failed — could not create log directory."
        );
    }

    let filename = format!("idle-raw-{}.json", now.format("%d-%H%M%S"));
    let path = month_dir.join(&filename);

    match serde_json::to_string_pretty(recent) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, &json).await {
                tracing::warn!(error = %e, path = %path.display(), "failed to write idle raw messages");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize idle raw messages");
        }
    }

    let rel_path = format!("notes/log/{}/{filename}", now.format("%Y-%m"));
    format!(
        "[idle] Auto-deactivated after {timeout_mins}m of inactivity. \
         LLM summary failed — raw messages saved to {rel_path}."
    )
}

/// Deactivate all remaining explicitly-activated skills.
async fn deactivate_remaining_skills(rt: &mut GatewayRuntime) -> usize {
    let mut state = rt.skill_state.lock().await;
    let names: Vec<String> = state
        .active_skill_names()
        .into_iter()
        .map(String::from)
        .collect();
    let count = names.len();

    for name in &names {
        if let Err(e) = state.deactivate(name) {
            tracing::warn!(skill = %name, error = %e, "failed to deactivate skill during idle");
        }
    }

    if count > 0 {
        tracing::info!(count, "deactivated remaining skills during idle transition");
    }

    count
}

/// Build the idle summary message injected into the agent context.
fn format_idle_summary(
    timeout_mins: u64,
    project_name: Option<&str>,
    skill_count: usize,
) -> String {
    let mut parts = vec![format!(
        "[Idle] Transitioned to idle after {timeout_mins}m of inactivity."
    )];

    match (project_name, skill_count) {
        (Some(name), 0) => parts.push(format!("Deactivated project \"{name}\".")),
        (Some(name), n) => parts.push(format!(
            "Deactivated project \"{name}\" and {n} skill{}.",
            if n == 1 { "" } else { "s" }
        )),
        (None, n) if n > 0 => parts.push(format!(
            "Deactivated {n} skill{}.",
            if n == 1 { "" } else { "s" }
        )),
        _ => {}
    }

    if project_name.is_some() {
        parts.push("Session log written.".to_string());
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_idle_summary_with_project_and_skills() {
        let result = format_idle_summary(30, Some("aerohive-setup"), 2);
        assert!(result.contains("[Idle]"));
        assert!(result.contains("30m"));
        assert!(result.contains("aerohive-setup"));
        assert!(result.contains("2 skills"));
        assert!(result.contains("Session log written."));
    }

    #[test]
    fn format_idle_summary_no_project() {
        let result = format_idle_summary(15, None, 3);
        assert!(result.contains("[Idle]"));
        assert!(result.contains("15m"));
        assert!(result.contains("3 skills"));
        assert!(!result.contains("Session log written."));
    }

    #[test]
    fn format_idle_summary_nothing_active() {
        let result = format_idle_summary(30, None, 0);
        assert!(result.contains("[Idle]"));
        assert!(result.contains("30m"));
        assert!(!result.contains("Deactivated"));
    }

    #[test]
    fn format_idle_summary_project_no_skills() {
        let result = format_idle_summary(30, Some("my-project"), 0);
        assert!(result.contains("my-project"));
        assert!(!result.contains("skill"));
        assert!(result.contains("Session log written."));
    }

    #[test]
    fn format_idle_summary_single_skill_no_plural() {
        let result = format_idle_summary(30, Some("proj"), 1);
        assert!(result.contains("1 skill."));
        assert!(!result.contains("skills"));
    }
}
