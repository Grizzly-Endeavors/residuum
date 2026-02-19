//! Agent runtime: context assembly, tool loop, and session management.

pub mod context;
pub mod session;

use crate::channels::cli::CliChannel;
use crate::error::IronclawError;
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse, Role};
use crate::tools::ToolRegistry;
use crate::workspace::identity::IdentityFiles;

use self::context::assemble_system_prompt;
use self::session::Session;

/// Maximum number of tool-call iterations before the agent stops.
const MAX_TOOL_ITERATIONS: usize = 50;

/// The agent runtime that processes user messages through the model.
pub struct Agent {
    provider: Box<dyn ModelProvider>,
    tools: ToolRegistry,
    identity: IdentityFiles,
    session: Session,
    options: CompletionOptions,
    observations: Option<String>,
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

    /// Process a user message through the model, executing tool calls as needed.
    ///
    /// Returns the final text response from the model after all tool iterations.
    ///
    /// # Errors
    /// Returns `IronclawError` if the model call fails or tool execution errors
    /// are unrecoverable.
    pub async fn process_message(
        &mut self,
        user_input: &str,
        cli: &CliChannel,
    ) -> Result<String, IronclawError> {
        // Push user message to session
        self.session.push(Message {
            role: Role::User,
            content: user_input.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });

        let tool_definitions = self.tools.definitions();

        for iteration in 0..MAX_TOOL_ITERATIONS {
            // Assemble full message list: system prompt + session history
            let messages = assemble_system_prompt(
                &self.identity,
                &self.tools,
                &self.session,
                self.observations.as_deref(),
            );

            // Call the model
            let response = self
                .provider
                .complete(&messages, &tool_definitions, &self.options)
                .await
                .map_err(IronclawError::Model)?;

            if response.is_complete() || response.tool_calls.is_empty() {
                // Final text response
                self.session.push(Message {
                    role: Role::Assistant,
                    content: response.content.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                });

                log_usage(&response);
                return Ok(response.content);
            }

            // Tool call loop
            tracing::info!(
                iteration,
                tool_count = response.tool_calls.len(),
                "processing tool calls"
            );

            // Push assistant message with tool calls
            self.session.push(Message {
                role: Role::Assistant,
                content: response.content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
            });

            // Execute each tool call and push results
            for tool_call in &response.tool_calls {
                cli.show_tool_call(&tool_call.name, &tool_call.arguments);

                let result = self
                    .tools
                    .execute(&tool_call.name, tool_call.arguments.clone())
                    .await;

                let (output, is_error) = match result {
                    Ok(r) => (r.output, r.is_error),
                    Err(e) => (e.to_string(), true),
                };

                cli.show_tool_result(&tool_call.name, &output, is_error);

                self.session.push(Message {
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

    fn make_cli() -> CliChannel {
        CliChannel::new("test-agent").unwrap()
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

        let cli = make_cli();
        let result = agent.process_message("hi", &cli).await.unwrap();
        assert_eq!(result, "hello there", "should return model text");
    }

    #[tokio::test]
    async fn tool_loop_then_text() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults();

        let provider = MockProvider::new(vec![
            // First response: tool call
            ModelResponse::new(
                String::new(),
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "echo test"}),
                }],
            ),
            // Second response: text (after seeing tool result)
            ModelResponse::new("the result was: test".to_string(), vec![]),
        ]);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            IdentityFiles::default(),
            CompletionOptions::default(),
        );

        let cli = make_cli();
        let result = agent.process_message("run echo test", &cli).await.unwrap();
        assert_eq!(
            result, "the result was: test",
            "should return final text after tool loop"
        );
    }

    #[tokio::test]
    async fn max_iterations_guard() {
        // Provider always returns tool calls, never text
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

        let cli = make_cli();
        let result = agent.process_message("loop forever", &cli).await;
        assert!(result.is_err(), "should error after max iterations");
    }
}
