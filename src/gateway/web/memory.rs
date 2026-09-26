//! Memory search HTTP API: `GET /api/memory/search`, the same hybrid search
//! the agent's `memory_search` tool runs, exposed to workbench artifacts.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::memory::search::{HybridSearcher, SearchFilters, validate_date_filter};
use crate::memory::types::DocSource;

/// Fewest results a caller may ask for.
const MIN_LIMIT: usize = 1;

/// Results per page when the request doesn't say.
const DEFAULT_LIMIT: usize = 10;

/// Shared state for the memory search API.
#[derive(Clone)]
pub(crate) struct MemoryApiState {
    pub hybrid_searcher: Arc<HybridSearcher>,
}

/// Build the memory search API router.
pub(crate) fn memory_api_router(state: MemoryApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/memory/search", axum::routing::get(api_memory_search))
        .with_state(state)
}

/// Query parameters for `GET /api/memory/search`.
#[derive(Debug, Deserialize)]
pub(super) struct MemorySearchQuery {
    /// The search query. Blank answers `400`.
    q: String,
    /// Results to return, floored at 1 (default 10). No upper cap.
    #[serde(default)]
    limit: Option<usize>,
    /// `"observations"`, `"episodes"`, or `"wiki"`. Omit to search all three.
    #[serde(default)]
    source: Option<String>,
    /// Inclusive lower date bound (`YYYY-MM-DD`).
    #[serde(default)]
    date_from: Option<String>,
    /// Inclusive upper date bound (`YYYY-MM-DD`).
    #[serde(default)]
    date_to: Option<String>,
    /// Overrides the configured relevance threshold for this search only.
    #[serde(default)]
    min_score: Option<f64>,
}

/// One search result, in the shape the workbench API returns it.
#[derive(Debug, Serialize, PartialEq)]
pub(super) struct MemorySearchResultItem {
    pub id: String,
    pub source: String,
    pub episode_id: String,
    pub date: String,
    pub line_start: Option<usize>,
    pub line_end: Option<usize>,
    pub snippet: String,
    pub score: f32,
}

/// Response body for `GET /api/memory/search`.
#[derive(Debug, Serialize, PartialEq)]
pub(super) struct MemorySearchResponse {
    pub results: Vec<MemorySearchResultItem>,
    /// Whether vector search contributed to these results.
    pub semantic: bool,
    /// How many results scored below the relevance threshold and were
    /// dropped; retry with a lower `min_score` to see them.
    pub below_threshold: usize,
}

/// `GET /api/memory/search` — run the same hybrid BM25 + vector search the
/// `memory_search` tool uses.
///
/// # Errors
/// `400` for a blank `q`, an unrecognized `source`, or a malformed
/// `date_from`/`date_to`. `500` if the search itself fails.
pub(super) async fn api_memory_search(
    Query(query): Query<MemorySearchQuery>,
    State(state): State<MemoryApiState>,
) -> Result<Json<MemorySearchResponse>, (StatusCode, String)> {
    if query.q.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "q must not be blank".to_string()));
    }

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).max(MIN_LIMIT);

    let source = match query.source.as_deref() {
        Some(s) => Some(DocSource::from_query_str(s).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("unknown source '{s}': expected 'observations', 'episodes', or 'wiki'"),
            )
        })?),
        None => None,
    };

    if let Some(ref from) = query.date_from {
        validate_date_filter(from).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    }
    if let Some(ref to) = query.date_to {
        validate_date_filter(to).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    }

    let filters = SearchFilters {
        source,
        date_from: query.date_from,
        date_to: query.date_to,
        episode_ids: None,
    };

    let outcome = state
        .hybrid_searcher
        .search(&query.q, limit, &filters, query.min_score)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, query = %query.q, "memory search failed via http");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "memory search failed".to_string(),
            )
        })?;

    let semantic = state.hybrid_searcher.has_vector();
    let results = outcome
        .results
        .into_iter()
        .map(|r| MemorySearchResultItem {
            id: r.id,
            source: r.source_type.as_str().to_string(),
            episode_id: r.episode_id,
            date: r.date,
            line_start: r.line_start,
            line_end: r.line_end,
            snippet: r.snippet,
            score: r.score,
        })
        .collect();

    Ok(Json(MemorySearchResponse {
        results,
        semantic,
        below_threshold: outcome.below_threshold,
    }))
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;
    use crate::config::SearchConfig;
    use crate::memory::search::MemoryIndex;
    use crate::memory::types::{Observation, Visibility};

    fn make_state() -> (tempfile::TempDir, MemoryApiState) {
        let dir = tempfile::tempdir().unwrap();
        let index = MemoryIndex::open_or_create(&dir.path().join(".index")).unwrap();

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
        (
            dir,
            MemoryApiState {
                hybrid_searcher: Arc::new(searcher),
            },
        )
    }

    #[tokio::test]
    async fn blank_query_is_rejected() {
        let (_dir, state) = make_state();
        let err = api_memory_search(
            Query(MemorySearchQuery {
                q: "   ".to_string(),
                limit: None,
                source: None,
                date_from: None,
                date_to: None,
                min_score: None,
            }),
            State(state),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_source_is_rejected() {
        let (_dir, state) = make_state();
        let err = api_memory_search(
            Query(MemorySearchQuery {
                q: "rust".to_string(),
                limit: None,
                source: Some("everything".to_string()),
                date_from: None,
                date_to: None,
                min_score: None,
            }),
            State(state),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn malformed_date_is_rejected() {
        let (_dir, state) = make_state();
        let err = api_memory_search(
            Query(MemorySearchQuery {
                q: "rust".to_string(),
                limit: None,
                source: None,
                date_from: Some("02-19-2026".to_string()),
                date_to: None,
                min_score: None,
            }),
            State(state),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn limit_is_clamped_to_range() {
        let (_dir, state) = make_state();
        // A limit of 0 (below the floor) should be clamped up to 1 rather
        // than rejected or passed straight through to the searcher.
        let response = api_memory_search(
            Query(MemorySearchQuery {
                q: "rust".to_string(),
                limit: Some(0),
                source: None,
                date_from: None,
                date_to: None,
                min_score: None,
            }),
            State(state),
        )
        .await
        .unwrap();
        assert!(response.0.results.len() <= 1);
    }

    #[tokio::test]
    async fn limit_above_the_old_fifty_ceiling_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let index = MemoryIndex::open_or_create(&dir.path().join(".index")).unwrap();
        let obs: Vec<Observation> = (0..60)
            .map(|i| Observation {
                timestamp: chrono::Utc::now().naive_utc(),
                source_episodes: Some("ep-001".to_string()),
                visibility: Visibility::User,
                content: format!("rust memory safety topic number {i}"),
                source: crate::memory::types::SourceTag::main(),
            })
            .collect();
        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();
        let searcher = HybridSearcher::new(Arc::new(index), None, None, SearchConfig::default());
        let state = MemoryApiState {
            hybrid_searcher: Arc::new(searcher),
        };

        let response = api_memory_search(
            Query(MemorySearchQuery {
                q: "rust memory".to_string(),
                limit: Some(55),
                source: None,
                date_from: None,
                date_to: None,
                min_score: None,
            }),
            State(state),
        )
        .await
        .unwrap();

        assert_eq!(
            response.0.results.len(),
            55,
            "a limit above the old 50-result ceiling should be honoured"
        );
    }

    #[tokio::test]
    async fn min_score_override_surfaces_a_filtered_result_and_reports_the_count() {
        let dir = tempfile::tempdir().unwrap();
        let index = MemoryIndex::open_or_create(&dir.path().join(".index")).unwrap();
        // A single-result search always normalizes to score 1.0, so it can
        // never itself be filtered; two observations with a relevance gap
        // let the weaker one fall below a high threshold.
        let strong = Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            source_episodes: Some("ep-001".to_string()),
            visibility: Visibility::User,
            content: "rust ownership rust ownership rust ownership model".to_string(),
            source: crate::memory::types::SourceTag::main(),
        };
        let weak = Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            source_episodes: Some("ep-001".to_string()),
            visibility: Visibility::User,
            content: "a passing, incidental mention of rust".to_string(),
            source: crate::memory::types::SourceTag::main(),
        };
        index
            .index_observations("ep-001", "2026-02-19", &[strong, weak])
            .unwrap();
        let cfg = SearchConfig {
            min_score: 0.9,
            ..SearchConfig::default()
        };
        let searcher = HybridSearcher::new(Arc::new(index), None, None, cfg);
        let state = MemoryApiState {
            hybrid_searcher: Arc::new(searcher),
        };

        let strict = api_memory_search(
            Query(MemorySearchQuery {
                q: "rust ownership".to_string(),
                limit: None,
                source: None,
                date_from: None,
                date_to: None,
                min_score: None,
            }),
            State(state.clone()),
        )
        .await
        .unwrap();
        assert_eq!(strict.0.results.len(), 1);
        assert_eq!(strict.0.below_threshold, 1);

        let relaxed = api_memory_search(
            Query(MemorySearchQuery {
                q: "rust ownership".to_string(),
                limit: None,
                source: None,
                date_from: None,
                date_to: None,
                min_score: Some(0.0),
            }),
            State(state),
        )
        .await
        .unwrap();
        assert_eq!(
            relaxed.0.results.len(),
            2,
            "overriding min_score should surface the weaker match too"
        );
    }

    #[tokio::test]
    async fn maps_source_filter_and_reports_results() {
        let (_dir, state) = make_state();
        let response = api_memory_search(
            Query(MemorySearchQuery {
                q: "rust memory".to_string(),
                limit: None,
                source: Some("observations".to_string()),
                date_from: None,
                date_to: None,
                min_score: None,
            }),
            State(state),
        )
        .await
        .unwrap();

        assert_eq!(response.0.results.len(), 1);
        assert_eq!(response.0.results[0].source, "observation");
        assert_eq!(response.0.results[0].episode_id, "ep-001");
        assert!(!response.0.semantic, "no embedding provider was configured");
    }

    #[tokio::test]
    async fn source_filter_excludes_other_sources() {
        let (_dir, state) = make_state();
        let response = api_memory_search(
            Query(MemorySearchQuery {
                q: "rust memory".to_string(),
                limit: None,
                source: Some("wiki".to_string()),
                date_from: None,
                date_to: None,
                min_score: None,
            }),
            State(state),
        )
        .await
        .unwrap();

        assert!(response.0.results.is_empty());
    }
}
