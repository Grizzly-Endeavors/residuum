//! Integration tests for MCP client/registry with a real MCP server process.
//!
//! These tests require an MCP server binary to be available. They are marked
//! `#[ignore]` so they don't run in normal CI. Run them explicitly with:
//!
//! ```sh
//! cargo test --test mcp_integration -- --ignored
//! ```
//!
//! The tests use `npx @anthropic/mcp-echo-server` or a simple script-based
//! approach. If npx is unavailable, the tests skip gracefully.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod mcp_integration {
    use std::collections::HashMap;

    use ironclaw::mcp::{McpRegistry, McpStatus};
    use ironclaw::projects::types::McpServerEntry;

    fn echo_server_entry() -> McpServerEntry {
        McpServerEntry {
            name: "echo".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-everything".to_string(),
            ],
            env: HashMap::new(),
        }
    }

    #[tokio::test]
    #[ignore = "requires npx and @modelcontextprotocol/server-everything"]
    async fn connect_list_tools_and_disconnect() {
        let entry = echo_server_entry();
        let mut registry = McpRegistry::new();

        let report = registry.reconcile_and_connect(&[entry]).await;
        assert_eq!(
            report.failures.len(),
            0,
            "should connect without failures: {:?}",
            report.failures
        );
        assert_eq!(report.started, 1, "should start one server");

        // Verify server is running
        let servers = registry.servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers.first().unwrap().status, McpStatus::Running);

        // Should have discovered tools
        let tool_defs = registry.tool_definitions();
        assert!(
            !tool_defs.is_empty(),
            "server-everything should advertise at least one tool"
        );

        // Tool names should be non-empty strings
        for td in &tool_defs {
            assert!(!td.name.is_empty(), "tool name should not be empty");
        }

        // Disconnect
        let disconnected = registry.disconnect_all().await;
        assert_eq!(disconnected, vec!["echo"]);
        assert!(registry.servers().is_empty());
    }

    #[tokio::test]
    #[ignore = "requires npx and @modelcontextprotocol/server-everything"]
    async fn call_tool_on_live_server() {
        let entry = echo_server_entry();
        let mut registry = McpRegistry::new();

        let report = registry.reconcile_and_connect(&[entry]).await;
        assert_eq!(
            report.failures.len(),
            0,
            "should connect: {:?}",
            report.failures
        );

        // The "echo" tool from server-everything returns what you send
        let tool_defs = registry.tool_definitions();
        let has_echo = tool_defs.iter().any(|t| t.name == "echo");
        assert!(
            has_echo,
            "server-everything should have an 'echo' tool, found: {:?}",
            tool_defs.iter().map(|t| &t.name).collect::<Vec<_>>()
        );

        let result = registry
            .call_tool("echo", serde_json::json!({"message": "hello ironclaw"}))
            .await;
        assert!(result.is_ok(), "echo tool call should succeed: {result:?}");
        let tool_result = result.unwrap();
        assert!(!tool_result.is_error, "echo should not be an error");
        assert!(
            !tool_result.output.is_empty(),
            "echo should return non-empty output"
        );

        registry.disconnect_all().await;
    }

    #[tokio::test]
    async fn connect_nonexistent_server_fails_gracefully() {
        let entry = McpServerEntry {
            name: "nonexistent".to_string(),
            command: "/definitely/not/a/real/binary".to_string(),
            args: vec![],
            env: HashMap::new(),
        };

        let mut registry = McpRegistry::new();
        let report = registry.reconcile_and_connect(&[entry]).await;

        assert_eq!(report.started, 0, "nothing should start");
        assert_eq!(report.failures.len(), 1, "should have one failure");
        assert_eq!(report.failures.first().unwrap().0, "nonexistent");

        let servers = registry.servers();
        assert_eq!(servers.len(), 1);
        assert!(
            matches!(&servers.first().unwrap().status, McpStatus::Failed(_)),
            "should be marked failed"
        );
    }

    #[tokio::test]
    async fn call_tool_not_found_returns_error() {
        let registry = McpRegistry::new();
        let result = registry
            .call_tool("nonexistent_tool", serde_json::json!({}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn disconnect_all_on_empty_is_noop() {
        let mut registry = McpRegistry::new();
        let disconnected = registry.disconnect_all().await;
        assert!(disconnected.is_empty());
    }

    #[tokio::test]
    async fn reconcile_and_connect_mixed_success_and_failure() {
        let good_but_missing = McpServerEntry {
            name: "bad-server".to_string(),
            command: "/no/such/binary".to_string(),
            args: vec![],
            env: HashMap::new(),
        };
        let also_missing = McpServerEntry {
            name: "also-bad".to_string(),
            command: "/also/missing".to_string(),
            args: vec![],
            env: HashMap::new(),
        };

        let mut registry = McpRegistry::new();
        let report = registry
            .reconcile_and_connect(&[good_but_missing, also_missing])
            .await;

        assert_eq!(report.started, 0);
        assert_eq!(report.failures.len(), 2);

        let failed_names: Vec<&str> = report.failures.iter().map(|(n, _)| n.as_str()).collect();
        assert!(failed_names.contains(&"bad-server"));
        assert!(failed_names.contains(&"also-bad"));
    }
}
