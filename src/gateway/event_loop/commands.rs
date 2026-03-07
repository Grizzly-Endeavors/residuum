//! Server command handler in the event loop.

use crate::gateway::types::GatewayRuntime;
use crate::gateway::protocol::ServerMessage;
use crate::gateway::types::ServerCommand;

use super::turns::load_prompt_context_strings;
use crate::gateway::memory::{
    MemorySubsystems, run_forced_observe, run_forced_reflect,
};

/// Dispatch a named server command from any client channel.
pub async fn handle_server_command(
    cmd: ServerCommand,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    match cmd.name.as_str() {
        "observe" => {
            *observe_deadline = None;
            let mem = MemorySubsystems {
                observer: &rt.observer,
                reflector: &rt.reflector,
                search_index: &rt.search_index,
                layout: &rt.layout,
                vector_store: rt.vector_store.as_ref(),
                embedding_provider: rt.embedding_provider.as_ref(),
            };
            run_forced_observe(&mem, &mut rt.agent, &rt.broadcast_tx).await;
        }
        "reflect" => {
            run_forced_reflect(&rt.reflector, &rt.layout, &mut rt.agent, &rt.broadcast_tx).await;
        }
        "context" => {
            let ctx_strings =
                load_prompt_context_strings(&rt.project_state, &rt.skill_state, &rt.layout).await;
            let prompt_ctx = ctx_strings.as_prompt_context();
            let bd = rt.agent.context_breakdown(&prompt_ctx).await;
            let msg = format!(
                "[context]\n  identity:          ~{} tokens\n  observation log:   ~{} tokens\n  subagents index:   ~{} tokens\n  projects index:    ~{} tokens\n  active project:    ~{} tokens\n  skills index:      ~{} tokens\n  active skills:     ~{} tokens\n  system tools:      ~{} tokens\n  mcp tools:         ~{} tokens\n  message history:   ~{} tokens ({} messages)",
                bd.identity_tokens,
                bd.observation_log_tokens,
                bd.subagents_index_tokens,
                bd.projects_index_tokens,
                bd.active_project_tokens,
                bd.skills_index_tokens,
                bd.active_skills_tokens,
                bd.system_tool_tokens,
                bd.mcp_tool_tokens,
                bd.history_tokens,
                bd.history_count,
            );
            if let Some(tx) = cmd.reply_tx {
                tx.send(msg.clone()).ok();
            }
            rt.broadcast_tx
                .send(ServerMessage::Notice { message: msg })
                .ok();
        }
        unknown => {
            rt.broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("unknown server command: {unknown}"),
                })
                .ok();
        }
    }
}
