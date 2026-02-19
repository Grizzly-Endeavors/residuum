//! Agent runtime: context assembly, tool loop, and session management.

pub mod context;
pub mod session;

use crate::channels::TurnDisplay;
use crate::error::IronclawError;
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse, Role};
use crate::tools::ToolRegistry;
use crate::workspace::identity::IdentityFiles;

use self::context::assemble_system_prompt;
use self::session::Session;

/// Maximum number of tool-call iterations before the agent stops.
const MAX_TOOL_ITERATIONS: usize = 50;

/// Result of a background system turn (pulse or cron).
pub struct SystemTurnResult {
    /// The assistant's final text response.
    pub response: String,
    /// All messages from the ephemeral session (user prompt + assistant response + tool calls).
    ///
    /// Feed these into `run_memory_pipeline()` so background turns contribute to memory.
    pub messages: Vec<Message>,
}

/// The agent runtime that processes user messages through the model.
pub struct Agent {
    provider: Box<dyn ModelProvider>,
    tools: ToolRegistry,
    identity: IdentityFiles,
    session: Session,
    options: CompletionOptions,
    observations: Option<String>,
    /// System event texts queued from cron jobs; injected at the start of the next user turn.
    pending_system_events: Vec<String>,
}

impl Agent {
    /// Create a new agent with the given components.
    #[must_use]
    pub fn new(
        provider: Box<dyn ModelProvider>,
        tools: ToolRegistry,
        identity: IdentityFiles,
        options: CompletionOptions,
    ) -> Self {
        Self {
            provider,
            tools,
            identity,
            session: Session::new(),
            options,
            observations: None,
            pending_system_events: Vec::new(),
        }
    }

    /// Reload observations from the observation log file.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read.
    pub async fn reload_observations(
        &mut self,
        layout: &crate::workspace::layout::WorkspaceLayout,
    ) -> Result<(), IronclawError> {
        let path = layout.observations_json();
        match tokio::fs::read_to_string(&path).await {
            Ok(content) if !content.trim().is_empty() => {
                self.observations = Some(content);
            }
            Ok(_) => {
                self.observations = None;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.observations = None;
            }
            Err(e) => {
                return Err(IronclawError::Memory(format!(
                    "failed to read observations at {}: {e}",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    /// Queue a system event to be injected at the start of the next user turn.
    ///
    /// Used by the cron executor for `SessionTarget::Main` jobs.
    pub fn queue_system_event(&mut self, text: String) {
        self.pending_system_events.push(text);
    }

    /// Process a user message through the model, executing tool calls as needed.
    ///
    /// Any queued system events are prepended to the user message so the agent
    /// sees pending alerts in context. Returns the final text response from the
    /// model after all tool iterations.
    ///
    /// # Errors
    /// Returns `IronclawError` if the model call fails or tool execution errors
    /// are unrecoverable.
    pub async fn process_message(
        &mut self,
        user_input: &str,
        display: &dyn TurnDisplay,
    ) -> Result<String, IronclawError> {
        // Build effective input: prepend any pending system events
        let effective_input = if self.pending_system_events.is_empty() {
            user_input.to_string()
        } else {
            let events: Vec<String> = self.pending_system_events.drain(..).collect();
            let events_text = events.join("\n\n");
            format!("[System Alerts — please review]\n\n{events_text}\n\n---\n\n{user_input}")
        };

        self.session.push(Message {
            role: Role::User,
            content: effective_input,
            tool_calls: None,
            tool_call_id: None,
        });

        execute_turn(
            &*self.provider,
            &self.tools,
            &self.identity,
            &self.options,
            self.observations.as_deref(),
            &mut self.session,
            display,
        )
        .await
    }

    /// Run an ephemeral agent turn for background pulse or cron tasks.
    ///
    /// Creates a temporary session that is not added to the main conversation.
    /// Returns both the response text and the ephemeral messages so the caller
    /// can feed them into `run_memory_pipeline()`.
    ///
    /// # Errors
    /// Returns `IronclawError` if the model call fails.
    pub async fn run_system_turn(
        &self,
        prompt: &str,
        display: &dyn TurnDisplay,
    ) -> Result<SystemTurnResult, IronclawError> {
        let mut session = Session::new();
        session.push(Message {
            role: Role::User,
            content: prompt.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });

        let response = execute_turn(
            &*self.provider,
            &self.tools,
            &self.identity,
            &self.options,
            self.observations.as_deref(),
            &mut session,
            display,
        )
        .await?;

        Ok(SystemTurnResult {
            response,
            messages: session.messages().to_vec(),
        })
    }

    /// Get the current session message count.
    #[must_use]
    pub fn message_count(&self) -> usize {
        self.session.len()
    }

    /// Get messages added to the session since the given index.
    #[must_use]
    pub fn messages_since(&self, idx: usize) -> &[Message] {
        self.session.messages_since(idx)
    }
}

/// Execute the tool loop against the given session.
///
/// Calls the provider repeatedly until it returns a text response (no tool calls),
/// executing any requested tools in between. Updates `session` in place.
async fn execute_turn(
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    identity: &IdentityFiles,
    options: &CompletionOptions,
    observations: Option<&str>,
    session: &mut Session,
    display: &dyn TurnDisplay,
) -> Result<String, IronclawError> {
    let tool_definitions = tools.definitions();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        let messages = assemble_system_prompt(identity, tools, session, observations);

        let response = provider
            .complete(&messages, &tool_definitions, options)
            .await
            .map_err(IronclawError::Model)?;

        if response.is_complete() || response.tool_calls.is_empty() {
            session.push(Message {
                role: Role::Assistant,
                content: response.content.clone(),
                tool_calls: None,
                tool_call_id: None,
            });

            log_usage(&response);
            return Ok(response.content);
        }

        tracing::info!(
            iteration,
            tool_count = response.tool_calls.len(),
            "processing tool calls"
        );

        session.push(Message {
            role: Role::Assistant,
            content: response.content.clone(),
            tool_calls: Some(response.tool_calls.clone()),
            tool_call_id: None,
        });

        for tool_call in &response.tool_calls {
            display.show_tool_call(&tool_call.name, &tool_call.arguments);

            let result = tools
                .execute(&tool_call.name, tool_call.arguments.clone())
                .await;

            let (output, is_error) = match result {
                Ok(r) => (r.output, r.is_error),
                Err(e) => (e.to_string(), true),
            };

            display.show_tool_result(&tool_call.name, &output, is_error);

            session.push(Message {
                role: Role::Tool,
                content: output,
                tool_calls: None,
                tool_call_id: Some(tool_call.id.clone()),
            });
        }

        log_usage(&response);
    }

    Err(IronclawError::Other(anyhow::anyhow!(
        "agent exceeded maximum tool iterations ({MAX_TOOL_ITERATIONS})"
    )))
}

/// Log token usage from a model response at info level.
fn log_usage(response: &ModelResponse) {
    if let Some(usage) = response.usage {
        tracing::info!(
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            "token usage"
        );
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::channels::null::NullDisplay;
    use crate::models::{ModelError, ToolCall, ToolDefinition};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock provider that returns pre-configured responses in sequence.
    struct MockProvider {
        responses: Vec<ModelResponse>,
        call_count: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(responses: Vec<ModelResponse>) -> Self {
            Self {
                responses,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.responses
                .get(idx)
                .cloned()
                .ok_or_else(|| ModelError::Api("no more mock responses".to_string()))
        }

        fn model_name(&self) -> &'static str {
            "mock-model"
        }
    }

    #[tokio::test]
    async fn single_text_response() {
        let provider =
            MockProvider::new(vec![ModelResponse::new("hello there".to_string(), vec![])]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            IdentityFiles::default(),
            CompletionOptions::default(),
        );

        let display = NullDisplay;
        let result = agent.process_message("hi", &display).await.unwrap();
        assert_eq!(result, "hello there", "should return model text");
    }

    #[tokio::test]
    async fn tool_loop_then_text() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults();

        let provider = MockProvider::new(vec![
            ModelResponse::new(
                String::new(),
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "echo test"}),
                }],
            ),
            ModelResponse::new("the result was: test".to_string(), vec![]),
        ]);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            IdentityFiles::default(),
            CompletionOptions::default(),
        );

        let display = NullDisplay;
        let result = agent
            .process_message("run echo test", &display)
            .await
            .unwrap();
        assert_eq!(
            result, "the result was: test",
            "should return final text after tool loop"
        );
    }

    #[tokio::test]
    async fn max_iterations_guard() {
        let responses: Vec<ModelResponse> = (0..=MAX_TOOL_ITERATIONS)
            .map(|i| {
                ModelResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: format!("call_{i}"),
                        name: "exec".to_string(),
                        arguments: serde_json::json!({"command": "echo loop"}),
                    }],
                )
            })
            .collect();

        let mut registry = ToolRegistry::new();
        registry.register_defaults();

        let provider = MockProvider::new(responses);
        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            IdentityFiles::default(),
            CompletionOptions::default(),
        );

        let display = NullDisplay;
        let result = agent.process_message("loop forever", &display).await;
        assert!(result.is_err(), "should error after max iterations");
    }

    #[tokio::test]
    async fn run_system_turn_ephemeral() {
        let provider =
            MockProvider::new(vec![ModelResponse::new("HEARTBEAT_OK".to_string(), vec![])]);

        let agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            IdentityFiles::default(),
            CompletionOptions::default(),
        );

        let display = NullDisplay;
        let result = agent
            .run_system_turn("check status", &display)
            .await
            .unwrap();
        assert_eq!(result.response, "HEARTBEAT_OK", "response should match");
        assert!(
            !result.messages.is_empty(),
            "should have ephemeral messages"
        );
        assert_eq!(agent.message_count(), 0, "main session should be untouched");
    }

    #[tokio::test]
    async fn pending_system_events_prepended() {
        let provider =
            MockProvider::new(vec![ModelResponse::new("acknowledged".to_string(), vec![])]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            IdentityFiles::default(),
            CompletionOptions::default(),
        );

        agent.queue_system_event("email arrived from boss".to_string());
        let display = NullDisplay;
        agent.process_message("what's up?", &display).await.unwrap();

        // Queue should be drained
        let msgs = agent.messages_since(0);
        assert_eq!(msgs.len(), 2, "should have user + assistant messages");
        assert!(
            msgs.first()
                .is_some_and(|m| m.content.contains("email arrived from boss")),
            "system event should be in user message"
        );
    }
}
