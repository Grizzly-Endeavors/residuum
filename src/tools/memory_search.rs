//! Memory search tool for querying past observations, interaction chunks, and wiki pages.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult};
use crate::inference::ToolDefinition;
use crate::memory::search::{HybridSearcher, SearchFilters};
use crate::memory::types::DocSource;

/// Tool that searches the memory index using hybrid BM25 + vector search.
pub struct MemorySearchTool {
    searcher: Arc<HybridSearcher>,
}

impl MemorySearchTool {
    /// Create a new memory search tool with the given hybrid searcher.
    #[must_use]
    pub fn new(searcher: Arc<HybridSearcher>) -> Self {
        Self { searcher }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &'static str {
        "memory_search"
    }

    fn definition(&self) -> ToolDefinition {
        let desc = if self.searcher.has_vector() {
            "Search past conversation observations, interaction chunks, and knowledge \
             wiki pages using hybrid BM25 + vector similarity search. Returns matching \
             results with relevance scores and snippets; a wiki result's ID is the page \
             path to open with read_file. Supports filtering by source type, date range, \
             and episode IDs."
        } else {
            "Search past conversation observations, interaction chunks, and knowledge \
             wiki pages using BM25 full-text search. Returns matching results with \
             relevance scores and snippets; a wiki result's ID is the page path to open \
             with read_file. Supports filtering by source type, date range, and \
             episode IDs."
        };
        ToolDefinition {
            name: self.name().to_string(),
            description: desc.to_string(),
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
                        "description": "Filter by source type: 'observations', 'episodes', or 'wiki'. Omit to search all three.",
                        "enum": ["observations", "episodes", "wiki"]
                    },
                    "date_from": {
                        "type": "string",
                        "description": "Filter results on or after this date (YYYY-MM-DD, inclusive)"
                    },
                    "date_to": {
                        "type": "string",
                        "description": "Filter results on or before this date (YYYY-MM-DD, inclusive)"
                    },
                    "episode_ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to results from these episode IDs (excludes wiki pages)"
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
            Some(l) => (l.min(20) as usize).max(1),
            None => 5,
        };

        // Map the tool-facing source names onto the internal DocSource vocabulary,
        // shared with the `/api/memory/search` HTTP endpoint. Omitted → None
        // (search every source); an unrecognized value is rejected rather than
        // silently falling back to searching everything.
        let source_filter = match arguments.get("source").and_then(Value::as_str) {
            Some(s) => Some(DocSource::from_query_str(s).ok_or_else(|| {
                ToolError::InvalidArguments(format!(
                    "unknown source '{s}': expected 'observations', 'episodes', or 'wiki'"
                ))
            })?),
            None => None,
        };

        let filters = SearchFilters {
            source: source_filter,
            date_from: arguments
                .get("date_from")
                .and_then(Value::as_str)
                .map(String::from),
            date_to: arguments
                .get("date_to")
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

        match self.searcher.search(query, limit, &filters).await {
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
                            "{}. [{}] {} | {}{} (score: {:.2})\n   {}",
                            i + 1,
                            r.source_type,
                            r.id,
                            r.date,
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
            Err(e) => {
                tracing::error!(error = %e, query = %query, "memory search failed");
                Ok(ToolResult::error(format!("search failed: {e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchConfig;
    use crate::memory::search::MemoryIndex;
    use crate::memory::types::Observation;
    use crate::memory::types::Visibility;

    fn create_test_tool() -> (tempfile::TempDir, MemorySearchTool) {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();

        let obs = vec![Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            source_episodes: Some("ep-001".to_string()),
            visibility: Visibility::User,
            content: "rust memory safety and ownership model".to_string(),
            source: crate::memory::types::SourceTag::main(),
        }];
        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let searcher = HybridSearcher::new(Arc::new(index), None, None, SearchConfig::default());
        let tool = MemorySearchTool::new(Arc::new(searcher));
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
        let searcher = HybridSearcher::new(Arc::new(index), None, None, SearchConfig::default());
        let tool = MemorySearchTool::new(Arc::new(searcher));

        assert_eq!(tool.name(), "memory_search", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "memory_search", "definition name should match");
    }

    #[tokio::test]
    async fn search_tool_with_source_filter() {
        let (_dir, tool) = create_test_tool();
        // Tool accepts "observations" (design-doc value), mapped to internal "observation"
        let result = tool
            .execute(serde_json::json!({
                "query": "rust memory",
                "source": "observations"
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
    async fn search_tool_finds_wiki_pages_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let page = dir.path().join("wiki/homelab/cluster.md");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(
            &page,
            "---\ntype: Machine\ntitle: Homelab cluster\ndescription: Three-node k3s cluster.\n---\nRuns Flux.\n",
        )
        .unwrap();
        let index = MemoryIndex::open_or_create(&dir.path().join(".index")).unwrap();
        let searcher =
            HybridSearcher::new(Arc::new(index), None, None, SearchConfig::default()).with_wiki(
                crate::memory::wiki_index::WikiIndexer::new(dir.path(), dir.path().join("wiki")),
            );
        let tool = MemorySearchTool::new(Arc::new(searcher));

        let result = tool
            .execute(serde_json::json!({"query": "k3s", "source": "wiki"}))
            .await
            .unwrap();

        assert!(!result.is_error, "wiki search should succeed");
        assert!(
            result.output.contains("[wiki] wiki/homelab/cluster.md"),
            "a wiki hit should show its source and page path, got: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn search_tool_rejects_unknown_source() {
        let (_dir, tool) = create_test_tool();
        // An unrecognized source value is a caller mistake — reject it clearly
        // instead of silently searching both sources.
        let result = tool
            .execute(serde_json::json!({
                "query": "rust memory",
                "source": "observation"
            }))
            .await;

        assert!(
            matches!(
                &result,
                Err(ToolError::InvalidArguments(msg))
                    if msg.contains("observation") && msg.contains("expected")
            ),
            "unknown source should be rejected with a clear InvalidArguments error naming the bad value, got {result:?}"
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
        assert!(
            result.output.contains("Found"),
            "search within date range should return results: {}",
            result.output
        );

        // A date range that doesn't include the indexed observation (2026-02-19) should return nothing
        let result_outside = tool
            .execute(serde_json::json!({
                "query": "rust memory",
                "date_from": "2025-01-01",
                "date_to": "2025-12-31"
            }))
            .await
            .unwrap();
        assert!(
            !result_outside.is_error,
            "search outside date range should not error"
        );
        assert!(
            result_outside.output.contains("no results"),
            "search outside date range should return no results: {}",
            result_outside.output
        );
    }
}
