//! Memory search tool for querying past observations and interaction chunks.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult};
use crate::memory::search::{MemoryIndex, SearchFilters};
use crate::models::ToolDefinition;

/// Tool that searches the memory index using BM25 full-text search.
pub struct MemorySearchTool {
    index: Arc<MemoryIndex>,
}

impl MemorySearchTool {
    /// Create a new memory search tool with the given shared index.
    #[must_use]
    pub fn new(index: Arc<MemoryIndex>) -> Self {
        Self { index }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &'static str {
        "memory_search"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory_search".to_string(),
            description: "Search past conversation observations and interaction chunks using \
                          BM25 full-text search. Returns matching results with relevance scores \
                          and snippets. Supports filtering by source type, date range, project \
                          context, and episode IDs."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (supports AND, OR, phrase queries with quotes)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5)"
                    },
                    "source": {
                        "type": "string",
                        "description": "Filter by source type: 'observation' or 'chunk'",
                        "enum": ["observation", "chunk"]
                    },
                    "date_from": {
                        "type": "string",
                        "description": "Filter results on or after this date (YYYY-MM-DD, inclusive)"
                    },
                    "date_to": {
                        "type": "string",
                        "description": "Filter results on or before this date (YYYY-MM-DD, inclusive)"
                    },
                    "project_context": {
                        "type": "string",
                        "description": "Filter by project context (exact match)"
                    },
                    "episode_ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to results from these episode IDs"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'query' argument".to_string())
            })?;

        if query.trim().is_empty() {
            return Ok(ToolResult::error("query cannot be empty"));
        }

        let limit = match arguments.get("limit").and_then(Value::as_u64) {
            Some(l) => usize::try_from(l.min(20)).unwrap_or_default().max(1),
            None => 5,
        };

        let filters = SearchFilters {
            source: arguments
                .get("source")
                .and_then(Value::as_str)
                .map(String::from),
            date_from: arguments
                .get("date_from")
                .and_then(Value::as_str)
                .map(String::from),
            date_to: arguments
                .get("date_to")
                .and_then(Value::as_str)
                .map(String::from),
            project_context: arguments
                .get("project_context")
                .and_then(Value::as_str)
                .map(String::from),
            episode_ids: arguments.get("episode_ids").and_then(|v| {
                v.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
            }),
        };

        match self.index.search(query, limit, &filters) {
            Ok(results) if results.is_empty() => Ok(ToolResult::success("no results found")),
            Ok(results) => {
                let formatted: Vec<String> = results
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let line_info = match (r.line_start, r.line_end) {
                            (Some(s), Some(e)) => format!(" | lines {s}-{e}"),
                            _ => String::new(),
                        };
                        format!(
                            "{}. [{}] {} | {} | {}{} (score: {:.2})\n   {}",
                            i + 1,
                            r.source_type,
                            r.id,
                            r.date,
                            r.context,
                            line_info,
                            r.score,
                            r.snippet
                        )
                    })
                    .collect();

                Ok(ToolResult::success(format!(
                    "Found {} result(s):\n\n{}",
                    results.len(),
                    formatted.join("\n\n")
                )))
            }
            Err(e) => Ok(ToolResult::error(format!("search failed: {e}"))),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::types::Observation;
    use crate::memory::types::Visibility;

    fn create_test_tool() -> (tempfile::TempDir, MemorySearchTool) {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();

        let obs = vec![Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "ironclaw".to_string(),
            source_episodes: vec!["ep-001".to_string()],
            visibility: Visibility::User,
            content: "rust memory safety and ownership model".to_string(),
        }];
        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let tool = MemorySearchTool::new(Arc::new(index));
        (dir, tool)
    }

    #[tokio::test]
    async fn search_tool_success() {
        let (_dir, tool) = create_test_tool();
        let result = tool
            .execute(serde_json::json!({"query": "rust memory"}))
            .await
            .unwrap();

        assert!(!result.is_error, "search should succeed");
        assert!(result.output.contains("result"), "should report results");
        assert!(
            result.output.contains("[observation]"),
            "should include source type"
        );
    }

    #[tokio::test]
    async fn search_tool_no_results() {
        let (_dir, tool) = create_test_tool();
        let result = tool
            .execute(serde_json::json!({"query": "nonexistent xyz"}))
            .await
            .unwrap();

        assert!(!result.is_error, "no results is not an error");
        assert!(
            result.output.contains("no results"),
            "should report no results"
        );
    }

    #[tokio::test]
    async fn search_tool_missing_query() {
        let (_dir, tool) = create_test_tool();
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing query should be ToolError");
    }

    #[tokio::test]
    async fn search_tool_empty_query() {
        let (_dir, tool) = create_test_tool();
        let result = tool
            .execute(serde_json::json!({"query": "  "}))
            .await
            .unwrap();
        assert!(result.is_error, "empty query should be error result");
    }

    #[test]
    fn search_tool_definition() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();
        let tool = MemorySearchTool::new(Arc::new(index));

        assert_eq!(tool.name(), "memory_search", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "memory_search", "definition name should match");
    }

    #[tokio::test]
    async fn search_tool_with_source_filter() {
        let (_dir, tool) = create_test_tool();
        let result = tool
            .execute(serde_json::json!({
                "query": "rust memory",
                "source": "observation"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "filtered search should succeed");
        assert!(
            result.output.contains("[observation]"),
            "should return observations"
        );
    }

    #[tokio::test]
    async fn search_tool_with_date_filter() {
        let (_dir, tool) = create_test_tool();
        let result = tool
            .execute(serde_json::json!({
                "query": "rust memory",
                "date_from": "2026-02-01",
                "date_to": "2026-02-28"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "date filtered search should succeed");
    }
}
