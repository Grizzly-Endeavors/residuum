//! Server command handler in the event loop.

use crate::types::AgentRuntime;
use residuum_bus::control::ServerCommand;
use residuum_bus::{InlineOutputEvent, NoticeEvent, NotifyName, SYSTEM_CHANNEL, topics};

use super::turns::load_prompt_context_strings;
use crate::memory::MemorySubsystems;

/// Dispatch a named server command from any client channel.
#[tracing::instrument(skip_all, fields(command = %cmd.name))]
pub async fn handle_server_command(
    cmd: ServerCommand,
    rt: &mut AgentRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    match cmd.name.as_str() {
        "observe" => {
            *observe_deadline = None;
            rt.memory_worker
                .force_observe(MemorySubsystems::from_runtime(rt));
        }
        "reflect" => {
            rt.memory_worker
                .force_reflect(MemorySubsystems::from_runtime(rt));
        }
        "context" => {
            let ctx_strings = load_prompt_context_strings(&rt.skill_state).await;
            let prompt_ctx = ctx_strings.as_prompt_context();
            let bd = rt.agent.context_breakdown(&prompt_ctx).await;
            let msg = render_context_breakdown(&bd);
            if let Some(tx) = cmd.reply_tx {
                tx.send(Ok(msg.clone())).ok();
            }
            if let Err(e) = rt
                .publisher
                .publish(
                    topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                    InlineOutputEvent { message: msg },
                )
                .await
            {
                tracing::warn!(error = %e, "failed to publish context inline output");
            }
        }
        #[cfg(any(test, feature = "test-support"))]
        "panic_for_test" => fault_injection::panic_in_event_loop(),
        unknown => {
            tracing::debug!(command = %unknown, "received unknown server command");
            let reason = format!("unknown server command: {unknown}");
            // Route the rejection back to whoever sent it, not to every
            // connected client — an unknown command is specific to one
            // sender's mistake, not system-wide news.
            if let Some(tx) = cmd.reply_tx {
                tx.send(Err(reason)).ok();
            } else if let Err(e) = rt
                .publisher
                .publish(
                    topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                    NoticeEvent::new(reason),
                )
                .await
            {
                tracing::warn!(error = %e, "failed to publish error notice");
            }
        }
    }
}

/// The `/context` reply: the request's estimated total, then each part.
fn render_context_breakdown(bd: &residuum_agent::context::ContextBreakdown) -> String {
    format!(
        "[context] ~{} tokens in the next request\n  \
         identity:        ~{} tokens\n  \
         harness:         ~{} tokens\n  \
         observation log: ~{} tokens\n  \
         skills index:    ~{} tokens\n  \
         active skills:   ~{} tokens\n  \
         system tools:    ~{} tokens\n  \
         mcp tools:       ~{} tokens\n  \
         history:         ~{} tokens ({} messages)\n  \
         status lines:    ~{} tokens\n  \
         images:          ~{} tokens ({} images)\n  \
         reasoning:       ~{} tokens",
        bd.total_tokens,
        bd.identity_tokens,
        bd.harness_tokens,
        bd.memory_pipeline_tokens,
        bd.skills_index_tokens,
        bd.active_skills_tokens,
        bd.system_tool_tokens,
        bd.mcp_tool_tokens,
        bd.history_tokens,
        bd.history_count,
        bd.status_line_tokens,
        bd.image_tokens,
        bd.image_count,
        bd.reasoning_tokens,
    )
}

/// Faults the `hub_host` integration tests (`tests/it/hub_host/`) inject into
/// an agent's event loop to prove the loop's failure stays that agent's own.
/// Those tests turn on this crate's `test-support`, so the command exists only
/// in test builds.
#[cfg(any(test, feature = "test-support"))]
mod fault_injection {
    /// Panic the way an internal bug in the event loop would.
    #[expect(
        clippy::panic,
        reason = "test-support helper: the fault the hub_host tests inject into the event loop"
    )]
    pub(super) fn panic_in_event_loop() -> ! {
        panic!("forced panic in the agent event loop");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_agent::context::ContextBreakdown;

    #[test]
    fn the_context_reply_leads_with_the_total_and_names_every_part() {
        let bd = ContextBreakdown {
            harness_tokens: 900,
            history_tokens: 120,
            history_count: 3,
            status_line_tokens: 25,
            image_count: 2,
            image_tokens: 3200,
            reasoning_tokens: 40,
            total_tokens: 4285,
            ..ContextBreakdown::default()
        };
        let text = render_context_breakdown(&bd);
        assert!(
            text.starts_with("[context] ~4285 tokens in the next request\n"),
            "{text}"
        );
        for line in [
            "  harness:         ~900 tokens\n",
            "  history:         ~120 tokens (3 messages)\n",
            "  status lines:    ~25 tokens\n",
            "  images:          ~3200 tokens (2 images)\n",
            "  reasoning:       ~40 tokens",
        ] {
            assert!(text.contains(line), "{line:?} in {text}");
        }
    }
}
