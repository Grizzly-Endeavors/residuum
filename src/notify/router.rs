//! LLM-powered notification router: subscribes to `TopicId::BackgroundResult`
//! and makes content-aware routing decisions using ALERTS.md as policy.
//!
//! Two-layer routing:
//! - **Layer 1 (programmatic)**: heartbeat-ok → discard; agent-spawned → relay to main
//! - **Layer 2 (LLM)**: everything else → call small model with ALERTS.md policy

use std::path::PathBuf;

use tokio::task::JoinHandle;

use crate::background::spawn_context::SpawnContext;
use crate::bus::{
    AgentResultEvent, BusEvent, BusHandle, EndpointRegistry, EventTrigger, HeartbeatStatus,
    NotificationEvent, Publisher, TopicId,
};
use crate::config::BackgroundModelTier;
use crate::models::factory::build_provider_chain;
use crate::models::{CompletionOptions, Message, ModelProvider, ResponseFormat};

/// Spawn the LLM notification router as a bus subscriber.
///
/// Subscribes to `TopicId::BackgroundResult` and routes each `AgentResultEvent`
/// through two layers: programmatic rules first, then LLM-based routing for
/// everything else.
///
/// Returns `None` if subscription fails.
pub(crate) async fn spawn_notification_router(
    bus_handle: &BusHandle,
    spawn_context: &SpawnContext,
    endpoint_registry: EndpointRegistry,
    publisher: Publisher,
    alerts_path: PathBuf,
) -> Option<JoinHandle<()>> {
    let subscriber = match bus_handle.subscribe(TopicId::BackgroundResult).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe to background:result topic");
            return None;
        }
    };

    // Build a small-tier provider for LLM routing decisions
    let specs = spawn_context.background_config.models.resolve_tier(
        &BackgroundModelTier::Small,
        &spawn_context.main_provider_specs,
    );

    let provider = match build_provider_chain(
        &specs,
        spawn_context.max_tokens,
        spawn_context.http_client.clone(),
        spawn_context.retry_config.clone(),
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "failed to build provider for notification router, falling back to inbox-only routing");
            // Fall back to a simple inbox router if we can't build the LLM provider
            return Some(tokio::spawn(fallback_router_loop(subscriber, publisher)));
        }
    };

    let router = NotificationRouter {
        provider,
        endpoint_registry,
        publisher,
        alerts_path,
    };

    Some(tokio::spawn(router_loop(subscriber, router)))
}

/// The notification router state.
struct NotificationRouter {
    provider: Box<dyn ModelProvider>,
    endpoint_registry: EndpointRegistry,
    publisher: Publisher,
    alerts_path: PathBuf,
}

/// Main loop: receive agent results and route them.
async fn router_loop(mut subscriber: crate::bus::Subscriber, router: NotificationRouter) {
    while let Some(event) = subscriber.recv().await {
        if let BusEvent::AgentResult(agent_result) = event {
            route_agent_result(&agent_result, &router).await;
        }
    }
    tracing::info!("notification router shutting down");
}

/// Fallback loop when LLM provider is unavailable: routes everything to inbox.
async fn fallback_router_loop(mut subscriber: crate::bus::Subscriber, publisher: Publisher) {
    while let Some(event) = subscriber.recv().await {
        if let BusEvent::AgentResult(agent_result) = event {
            if agent_result.heartbeat_status == HeartbeatStatus::Ok {
                tracing::info!(source_label = %agent_result.source_label, "pulse check: HEARTBEAT_OK");
                continue;
            }

            if matches!(agent_result.source, EventTrigger::Agent) {
                publish_to_agent_main(&agent_result, &publisher).await;
                continue;
            }

            publish_to_inbox(&agent_result, &publisher).await;
        }
    }
    tracing::info!("fallback notification router shutting down");
}

/// Route a single `AgentResultEvent` through the two-layer system.
async fn route_agent_result(event: &AgentResultEvent, router: &NotificationRouter) {
    // Layer 1: Heartbeat-ok → silent discard
    if event.heartbeat_status == HeartbeatStatus::Ok {
        tracing::info!(source_label = %event.source_label, "pulse check: HEARTBEAT_OK");
        return;
    }

    // Layer 1: Agent-spawned results → relay to main agent
    if matches!(event.source, EventTrigger::Agent) {
        publish_to_agent_main(event, &router.publisher).await;
        return;
    }

    // Layer 2: LLM-based routing
    let targets = llm_route(event, router).await;
    publish_to_targets(event, &targets, &router.publisher).await;
}

/// Call the LLM to decide routing targets based on ALERTS.md policy.
async fn llm_route(event: &AgentResultEvent, router: &NotificationRouter) -> Vec<String> {
    // Load ALERTS.md (reload each time for live policy updates)
    let alerts_content = match tokio::fs::read_to_string(&router.alerts_path).await {
        Ok(content) => content,
        Err(e) => {
            tracing::debug!(error = %e, "failed to read ALERTS.md, using empty policy");
            String::new()
        }
    };

    // Enumerate available notification endpoints
    let notify_endpoints = router.endpoint_registry.notify();
    let endpoint_names: Vec<&str> = notify_endpoints.iter().map(|e| e.id.as_ref()).collect();
    let mut available_targets: Vec<&str> = vec!["inbox"];
    available_targets.extend(endpoint_names.iter());

    // Build the routing prompt
    let prompt = build_routing_prompt(event, &available_targets, &alerts_content);

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "targets": {
                "type": "array",
                "items": { "type": "string" },
                "description": "notification channel names to route this result to"
            }
        },
        "required": ["targets"],
        "additionalProperties": false
    });

    let options = CompletionOptions {
        max_tokens: Some(256),
        response_format: ResponseFormat::JsonSchema {
            name: "routing_decision".to_string(),
            schema,
        },
        ..CompletionOptions::default()
    };

    let messages = vec![Message::user(prompt)];

    match router.provider.complete(&messages, &[], &options).await {
        Ok(response) => parse_routing_response(&response.content, &available_targets),
        Err(e) => {
            tracing::warn!(
                error = %e,
                source_label = %event.source_label,
                "LLM routing failed, falling back to inbox"
            );
            vec!["inbox".to_string()]
        }
    }
}

/// Build the LLM prompt for routing decisions.
fn build_routing_prompt(
    event: &AgentResultEvent,
    available_targets: &[&str],
    alerts_content: &str,
) -> String {
    let source_display = event.source.display_label();
    let status = match &event.status {
        crate::bus::AgentResultStatus::Completed => "completed",
        crate::bus::AgentResultStatus::Cancelled => "cancelled",
        crate::bus::AgentResultStatus::Failed { .. } => "failed",
    };

    let targets_list = available_targets.join(", ");

    format!(
        "You are a notification routing system. Decide where to deliver a background task result.\n\n\
         ## Result\n\
         - Source: {source_display}\n\
         - Label: {label}\n\
         - Preset: {preset}\n\
         - Status: {status}\n\
         - Summary: {summary}\n\n\
         ## Available Targets\n\
         {targets_list}\n\n\
         ## Routing Policy (ALERTS.md)\n\
         {alerts}\n\n\
         Based on the result content and the routing policy, respond with a JSON object \
         containing a \"targets\" array of channel names to deliver this result to. \
         Only use channel names from the available targets list.",
        label = event.source_label,
        preset = event.agent_preset.as_ref(),
        summary = event.summary,
        alerts = if alerts_content.is_empty() {
            "No policy configured. Default: route to inbox."
        } else {
            &alerts_content
        },
    )
}

/// Parse the LLM response and validate targets against known endpoints.
fn parse_routing_response(response: &str, valid_targets: &[&str]) -> Vec<String> {
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(response);

    let targets = match parsed {
        Ok(val) => val
            .get("targets")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse LLM routing response, falling back to inbox");
            return vec!["inbox".to_string()];
        }
    };

    // Filter to only valid targets
    let validated: Vec<String> = targets
        .into_iter()
        .filter(|t| valid_targets.contains(&t.as_str()))
        .collect();

    if validated.is_empty() {
        vec!["inbox".to_string()]
    } else {
        validated
    }
}

/// Publish a result as a `BusEvent::Message` to `TopicId::AgentMain`.
async fn publish_to_agent_main(event: &AgentResultEvent, publisher: &Publisher) {
    // Build a BackgroundResult-like structure to format the message
    let content = format_agent_result_message(event);

    let msg_event = crate::bus::MessageEvent {
        id: format!("bg-result-{}", event.task_id),
        content,
        origin: crate::interfaces::types::MessageOrigin {
            endpoint: "background".to_string(),
            sender_name: "background-task".to_string(),
            sender_id: event.task_id.clone(),
        },
        timestamp: event.timestamp,
        images: vec![],
    };

    if let Err(e) = publisher
        .publish(TopicId::AgentMain, BusEvent::Message(msg_event))
        .await
    {
        tracing::warn!(
            task_id = %event.task_id,
            error = %e,
            "failed to publish background result to agent:main"
        );
    }
}

/// Format an `AgentResultEvent` into a human-readable message for the main agent.
fn format_agent_result_message(event: &AgentResultEvent) -> String {
    let source_kind = event.source.as_str();
    let status = match &event.status {
        crate::bus::AgentResultStatus::Completed => "completed".to_string(),
        crate::bus::AgentResultStatus::Cancelled => "cancelled".to_string(),
        crate::bus::AgentResultStatus::Failed { error } => format!("failed: {error}"),
    };

    let mut parts = vec![format!(
        "[Background Task Result]\nTask: {} ({})\nSource: {}\nStatus: {}",
        event.source_label, event.task_id, source_kind, status
    )];

    if !event.summary.is_empty() {
        parts.push(format!("Output:\n{}", event.summary));
    }

    if let Some(path) = &event.transcript_path {
        parts.push(format!("Transcript: {}", path.display()));
    }

    parts.join("\n")
}

/// Publish a notification to the inbox.
async fn publish_to_inbox(event: &AgentResultEvent, publisher: &Publisher) {
    publish_to_targets(event, &["inbox".to_string()], publisher).await;
}

/// Publish notifications to the specified targets.
async fn publish_to_targets(event: &AgentResultEvent, targets: &[String], publisher: &Publisher) {
    let notification = NotificationEvent {
        title: event.source_label.clone(),
        content: event.summary.clone(),
        source: event.source.clone(),
        timestamp: event.timestamp,
    };

    for target in targets {
        let topic = if target == "inbox" {
            TopicId::Inbox
        } else {
            TopicId::Notify(crate::bus::NotifyName::from(target.as_str()))
        };
        if let Err(e) = publisher
            .publish(topic.clone(), BusEvent::Notification(notification.clone()))
            .await
        {
            tracing::warn!(
                topic = %topic,
                source_label = %event.source_label,
                error = %e,
                "failed to publish notification to bus"
            );
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::bus::{AgentResultStatus, PresetName};
    use chrono::NaiveDate;

    fn sample_timestamp() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 14)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn sample_event() -> AgentResultEvent {
        AgentResultEvent {
            task_id: "t1".into(),
            source_label: "pulse:email_check".into(),
            agent_preset: PresetName::from("general-purpose"),
            source: EventTrigger::Pulse,
            heartbeat_status: HeartbeatStatus::Substantive,
            status: AgentResultStatus::Completed,
            summary: "3 new emails found".into(),
            transcript_path: None,
            timestamp: sample_timestamp(),
        }
    }

    #[test]
    fn parse_routing_response_valid() {
        let response = r#"{"targets": ["inbox", "ntfy_phone"]}"#;
        let valid = vec!["inbox", "ntfy_phone", "ntfy_desktop"];
        let result = parse_routing_response(response, &valid);
        assert_eq!(result, vec!["inbox", "ntfy_phone"]);
    }

    #[test]
    fn parse_routing_response_filters_unknown() {
        let response = r#"{"targets": ["inbox", "unknown_channel"]}"#;
        let valid = vec!["inbox", "ntfy_phone"];
        let result = parse_routing_response(response, &valid);
        assert_eq!(result, vec!["inbox"]);
    }

    #[test]
    fn parse_routing_response_empty_falls_back_to_inbox() {
        let response = r#"{"targets": []}"#;
        let valid = vec!["inbox", "ntfy_phone"];
        let result = parse_routing_response(response, &valid);
        assert_eq!(result, vec!["inbox"]);
    }

    #[test]
    fn parse_routing_response_invalid_json_falls_back() {
        let response = "not json";
        let valid = vec!["inbox"];
        let result = parse_routing_response(response, &valid);
        assert_eq!(result, vec!["inbox"]);
    }

    #[test]
    fn parse_routing_response_all_unknown_falls_back() {
        let response = r#"{"targets": ["fake1", "fake2"]}"#;
        let valid = vec!["inbox"];
        let result = parse_routing_response(response, &valid);
        assert_eq!(result, vec!["inbox"]);
    }

    #[test]
    fn build_routing_prompt_contains_event_details() {
        let event = sample_event();
        let targets = vec!["inbox", "ntfy_phone"];
        let alerts = "Route errors to ntfy.";

        let prompt = build_routing_prompt(&event, &targets, alerts);
        assert!(prompt.contains("pulse:email_check"));
        assert!(prompt.contains("3 new emails found"));
        assert!(prompt.contains("inbox, ntfy_phone"));
        assert!(prompt.contains("Route errors to ntfy."));
        assert!(prompt.contains("completed"));
    }

    #[test]
    fn build_routing_prompt_empty_alerts() {
        let event = sample_event();
        let targets = vec!["inbox"];

        let prompt = build_routing_prompt(&event, &targets, "");
        assert!(prompt.contains("No policy configured"));
    }

    #[test]
    fn format_agent_result_message_completed() {
        let event = sample_event();
        let msg = format_agent_result_message(&event);
        assert!(msg.contains("[Background Task Result]"));
        assert!(msg.contains("pulse:email_check"));
        assert!(msg.contains("completed"));
        assert!(msg.contains("3 new emails found"));
    }

    #[test]
    fn format_agent_result_message_failed() {
        let mut event = sample_event();
        event.status = AgentResultStatus::Failed {
            error: "connection refused".into(),
        };
        event.summary = String::new();

        let msg = format_agent_result_message(&event);
        assert!(msg.contains("failed: connection refused"));
        assert!(
            !msg.contains("Error: connection refused"),
            "error should not be duplicated in a separate Error: line"
        );
    }
}
