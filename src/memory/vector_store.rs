//! Vector similarity search using `SQLite` + sqlite-vec.
//!
//! Stores observation and chunk embeddings in `vec0` virtual tables with
//! denormalized metadata columns for efficient filtered search.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use zerocopy::IntoBytes;

use crate::error::ResiduumError;
use crate::memory::types::{IndexChunk, Observation};

/// A vector search result from either the observation or chunk table.
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    /// Document identifier (obs or chunk ID).
    pub id: String,
    /// Source type: `"observation"` or `"chunk"`.
    pub source_type: String,
    /// Parent episode identifier.
    pub episode_id: String,
    /// Date string (YYYY-MM-DD).
    pub date: String,
    /// Project context tag.
    pub context: String,
    /// Snippet of content.
    pub content: String,
    /// Line range start (chunks only).
    pub line_start: Option<usize>,
    /// Line range end (chunks only).
    pub line_end: Option<usize>,
    /// Cosine distance from query vector (lower = more similar).
    pub distance: f64,
}

/// Filters for narrowing vector search results.
#[derive(Debug, Clone, Default)]
pub struct VectorSearchFilters {
    /// Filter results on or after this date (YYYY-MM-DD, inclusive).
    pub date_from: Option<String>,
    /// Filter results on or before this date (YYYY-MM-DD, inclusive).
    pub date_to: Option<String>,
    /// Filter by project context (exact match).
    pub project_context: Option<String>,
}

/// `SQLite` + sqlite-vec backed vector store for memory embeddings.
pub struct VectorStore {
    conn: Mutex<Connection>,
    dim: usize,
}

/// Register the sqlite-vec extension globally. Must be called once before
/// opening any connections.
#[expect(
    unsafe_code,
    reason = "sqlite-vec C FFI registration — no safe wrapper exists"
)]
fn register_sqlite_vec_extension() {
    // Safety: sqlite3_vec_init is a valid sqlite3 extension entry point
    // provided by the sqlite-vec crate. transmute converts it to the function
    // pointer type expected by sqlite3_auto_extension.
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> i32,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )));
    }
}

impl VectorStore {
    /// Open or create a vector store at the given database path.
    ///
    /// Registers the sqlite-vec extension, opens the database in WAL mode,
    /// and creates the virtual tables if they don't exist.
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened or schema creation fails.
    pub fn open_or_create(db_path: &Path, dim: usize) -> Result<Self, ResiduumError> {
        register_sqlite_vec_extension();

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ResiduumError::Memory(format!(
                    "failed to create vector store directory at {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let conn = Connection::open(db_path).map_err(|e| {
            ResiduumError::Memory(format!(
                "failed to open vector store at {}: {e}",
                db_path.display()
            ))
        })?;

        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| {
                ResiduumError::Memory(format!("failed to set WAL mode for vector store: {e}"))
            })?;

        create_tables(&conn, dim)?;

        Ok(Self {
            conn: Mutex::new(conn),
            dim,
        })
    }

    /// Embedding dimension this store was created with.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Insert observation embeddings for a single episode.
    ///
    /// Each observation gets a doc ID of `"{episode_id}-o{index}"`.
    ///
    /// # Errors
    /// Returns an error if the insert fails or embedding dimensions don't match.
    pub fn insert_observations(
        &self,
        episode_id: &str,
        date: &str,
        observations: &[Observation],
        embeddings: &[Vec<f32>],
    ) -> Result<Vec<String>, ResiduumError> {
        if observations.is_empty() {
            return Ok(Vec::new());
        }
        if observations.len() != embeddings.len() {
            return Err(ResiduumError::Memory(format!(
                "observation count ({}) does not match embedding count ({})",
                observations.len(),
                embeddings.len()
            )));
        }

        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare_cached(
                "INSERT INTO obs_vectors(obs_id, episode_id, date, context, content, embedding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| ResiduumError::Memory(format!("failed to prepare obs insert: {e}")))?;

        let mut doc_ids = Vec::with_capacity(observations.len());
        for (i, (obs, emb)) in observations.iter().zip(embeddings.iter()).enumerate() {
            self.check_dim(emb)?;
            let doc_id = format!("{episode_id}-o{i}");
            match stmt.execute(rusqlite::params![
                doc_id,
                episode_id,
                date,
                obs.project_context,
                obs.content,
                emb.as_bytes(),
            ]) {
                Ok(_) => {}
                Err(ref e) if is_unique_violation(e) => {
                    tracing::debug!(doc_id, "observation vector already exists, skipping");
                }
                Err(e) => {
                    return Err(ResiduumError::Memory(format!(
                        "failed to insert observation vector {doc_id}: {e}"
                    )));
                }
            }
            doc_ids.push(doc_id);
        }

        Ok(doc_ids)
    }

    /// Insert chunk embeddings.
    ///
    /// # Errors
    /// Returns an error if the insert fails or embedding dimensions don't match.
    pub fn insert_chunks(
        &self,
        chunks: &[IndexChunk],
        embeddings: &[Vec<f32>],
    ) -> Result<Vec<String>, ResiduumError> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        if chunks.len() != embeddings.len() {
            return Err(ResiduumError::Memory(format!(
                "chunk count ({}) does not match embedding count ({})",
                chunks.len(),
                embeddings.len()
            )));
        }

        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare_cached(
                "INSERT INTO chunk_vectors(chunk_id, episode_id, date, context, content,
                 line_start, line_end, embedding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(|e| ResiduumError::Memory(format!("failed to prepare chunk insert: {e}")))?;

        let mut doc_ids = Vec::with_capacity(chunks.len());
        for (chunk, emb) in chunks.iter().zip(embeddings.iter()) {
            self.check_dim(emb)?;
            let line_start_i64 = i64::try_from(chunk.line_start).unwrap_or(i64::MAX);
            let line_end_i64 = i64::try_from(chunk.line_end).unwrap_or(i64::MAX);
            match stmt.execute(rusqlite::params![
                chunk.chunk_id,
                chunk.episode_id,
                chunk.date,
                chunk.context,
                chunk.content,
                line_start_i64,
                line_end_i64,
                emb.as_bytes(),
            ]) {
                Ok(_) => {}
                Err(ref e) if is_unique_violation(e) => {
                    tracing::debug!(
                        chunk_id = chunk.chunk_id,
                        "chunk vector already exists, skipping"
                    );
                }
                Err(e) => {
                    return Err(ResiduumError::Memory(format!(
                        "failed to insert chunk vector {}: {e}",
                        chunk.chunk_id
                    )));
                }
            }
            doc_ids.push(chunk.chunk_id.clone());
        }

        Ok(doc_ids)
    }

    /// Search for similar vectors across both tables.
    ///
    /// Returns results from both observation and chunk tables, sorted by distance.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        filters: &VectorSearchFilters,
    ) -> Result<Vec<VectorSearchResult>, ResiduumError> {
        self.check_dim(query_embedding)?;
        let conn = self.lock_conn()?;

        let mut results = Vec::new();

        // Search observations
        let obs_results = search_obs_table(&conn, query_embedding, limit, filters)?;
        results.extend(obs_results);

        // Search chunks
        let chunk_results = search_chunk_table(&conn, query_embedding, limit, filters)?;
        results.extend(chunk_results);

        // Sort by distance ascending (most similar first)
        results.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }

    /// Delete documents by their IDs from both tables.
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub fn delete_by_doc_ids(&self, ids: &[String]) -> Result<(), ResiduumError> {
        if ids.is_empty() {
            return Ok(());
        }

        let conn = self.lock_conn()?;
        for id in ids {
            // Try both tables — a given ID only exists in one
            conn.execute("DELETE FROM obs_vectors WHERE obs_id = ?1", [id])
                .map_err(|e| {
                    ResiduumError::Memory(format!("failed to delete from obs_vectors: {e}"))
                })?;
            conn.execute("DELETE FROM chunk_vectors WHERE chunk_id = ?1", [id])
                .map_err(|e| {
                    ResiduumError::Memory(format!("failed to delete from chunk_vectors: {e}"))
                })?;
        }

        Ok(())
    }

    /// Check if an observation vector exists by its doc ID.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn has_observation(&self, obs_id: &str) -> Result<bool, ResiduumError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare_cached("SELECT 1 FROM obs_vectors WHERE obs_id = ?1 LIMIT 1")
            .map_err(|e| {
                ResiduumError::Memory(format!("failed to prepare obs existence check: {e}"))
            })?;
        let exists = stmt
            .exists(rusqlite::params![obs_id])
            .map_err(|e| ResiduumError::Memory(format!("obs existence check failed: {e}")))?;
        Ok(exists)
    }

    /// Check if a chunk vector exists by its doc ID.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn has_chunk(&self, chunk_id: &str) -> Result<bool, ResiduumError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare_cached("SELECT 1 FROM chunk_vectors WHERE chunk_id = ?1 LIMIT 1")
            .map_err(|e| {
                ResiduumError::Memory(format!("failed to prepare chunk existence check: {e}"))
            })?;
        let exists = stmt
            .exists(rusqlite::params![chunk_id])
            .map_err(|e| ResiduumError::Memory(format!("chunk existence check failed: {e}")))?;
        Ok(exists)
    }

    /// Drop and recreate both tables, clearing all vector data.
    ///
    /// # Errors
    /// Returns an error if the tables cannot be recreated.
    pub fn clear(&self) -> Result<(), ResiduumError> {
        let conn = self.lock_conn()?;
        conn.execute_batch("DROP TABLE IF EXISTS obs_vectors; DROP TABLE IF EXISTS chunk_vectors;")
            .map_err(|e| ResiduumError::Memory(format!("failed to drop vector tables: {e}")))?;
        create_tables(&conn, self.dim)?;
        Ok(())
    }

    /// Lock the connection mutex.
    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, ResiduumError> {
        self.conn
            .lock()
            .map_err(|e| ResiduumError::Memory(format!("vector store lock poisoned: {e}")))
    }

    /// Validate that an embedding has the expected dimension.
    fn check_dim(&self, embedding: &[f32]) -> Result<(), ResiduumError> {
        if embedding.len() != self.dim {
            return Err(ResiduumError::Memory(format!(
                "embedding dimension mismatch: expected {}, got {}",
                self.dim,
                embedding.len()
            )));
        }
        Ok(())
    }
}

/// Check if a rusqlite error is a UNIQUE constraint violation.
///
/// Handles both standard `SQLite` constraint errors and vec0 virtual table
/// errors which report UNIQUE violations via extended error codes.
fn is_unique_violation(err: &rusqlite::Error) -> bool {
    // vec0 virtual tables use ConstraintViolation with a "UNIQUE constraint" message
    if let rusqlite::Error::SqliteFailure(ffi_err, msg) = err {
        return ffi_err.code == rusqlite::ffi::ErrorCode::ConstraintViolation
            || msg
                .as_deref()
                .is_some_and(|m| m.contains("UNIQUE constraint"));
    }
    false
}

/// Create the vec0 virtual tables if they don't exist.
fn create_tables(conn: &Connection, dim: usize) -> Result<(), ResiduumError> {
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS obs_vectors USING vec0(
            obs_id TEXT PRIMARY KEY,
            episode_id TEXT,
            date TEXT,
            context TEXT,
            +content TEXT,
            embedding FLOAT[{dim}] DISTANCE_METRIC=cosine
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vectors USING vec0(
            chunk_id TEXT PRIMARY KEY,
            episode_id TEXT,
            date TEXT,
            context TEXT,
            +content TEXT,
            +line_start INTEGER,
            +line_end INTEGER,
            embedding FLOAT[{dim}] DISTANCE_METRIC=cosine
        );"
    ))
    .map_err(|e| ResiduumError::Memory(format!("failed to create vector tables: {e}")))?;
    Ok(())
}

/// Search the `obs_vectors` table.
fn search_obs_table(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
    filters: &VectorSearchFilters,
) -> Result<Vec<VectorSearchResult>, ResiduumError> {
    let (where_clause, params) = build_filter_clauses(filters);

    let sql = format!(
        "SELECT obs_id, episode_id, date, context, content, distance
         FROM obs_vectors
         WHERE embedding MATCH ?1
           AND k = ?2
           {where_clause}
         ORDER BY distance"
    );

    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

    let conn_ref = conn;
    let mut stmt = conn_ref
        .prepare_cached(&sql)
        .map_err(|e| ResiduumError::Memory(format!("failed to prepare obs search: {e}")))?;

    // Build parameter list: embedding bytes, limit, then filter values
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(query_embedding.as_bytes().to_vec()));
    all_params.push(Box::new(limit_i64));
    for p in params {
        all_params.push(Box::new(p));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| &**p).collect();

    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(VectorSearchResult {
                id: row.get(0)?,
                source_type: "observation".to_string(),
                episode_id: row.get(1)?,
                date: row.get(2)?,
                context: row.get(3)?,
                content: row.get(4)?,
                line_start: None,
                line_end: None,
                distance: row.get(5)?,
            })
        })
        .map_err(|e| ResiduumError::Memory(format!("obs vector search failed: {e}")))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(
            row.map_err(|e| ResiduumError::Memory(format!("failed to read obs search row: {e}")))?,
        );
    }
    Ok(results)
}

/// Search the `chunk_vectors` table.
fn search_chunk_table(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
    filters: &VectorSearchFilters,
) -> Result<Vec<VectorSearchResult>, ResiduumError> {
    let (where_clause, params) = build_filter_clauses(filters);

    let sql = format!(
        "SELECT chunk_id, episode_id, date, context, content, line_start, line_end, distance
         FROM chunk_vectors
         WHERE embedding MATCH ?1
           AND k = ?2
           {where_clause}
         ORDER BY distance"
    );

    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

    let mut stmt = conn
        .prepare_cached(&sql)
        .map_err(|e| ResiduumError::Memory(format!("failed to prepare chunk search: {e}")))?;

    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(query_embedding.as_bytes().to_vec()));
    all_params.push(Box::new(limit_i64));
    for p in params {
        all_params.push(Box::new(p));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| &**p).collect();

    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            let line_start: Option<i64> = row.get(5)?;
            let line_end: Option<i64> = row.get(6)?;
            Ok(VectorSearchResult {
                id: row.get(0)?,
                source_type: "chunk".to_string(),
                episode_id: row.get(1)?,
                date: row.get(2)?,
                context: row.get(3)?,
                content: row.get(4)?,
                line_start: line_start.and_then(|v| usize::try_from(v).ok()),
                line_end: line_end.and_then(|v| usize::try_from(v).ok()),
                distance: row.get(7)?,
            })
        })
        .map_err(|e| ResiduumError::Memory(format!("chunk vector search failed: {e}")))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(
            row.map_err(|e| {
                ResiduumError::Memory(format!("failed to read chunk search row: {e}"))
            })?,
        );
    }
    Ok(results)
}

/// Build SQL WHERE clause fragments and parameter values from filters.
///
/// Returns `(clause_string, params)` where `clause_string` contains SQL fragments
/// like `AND date >= ?3 AND date <= ?4` and `params` are the corresponding values.
/// Parameter numbering starts at `?3` because `?1` is the embedding and `?2` is k.
fn build_filter_clauses(filters: &VectorSearchFilters) -> (String, Vec<String>) {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    let mut idx = 3_u32; // ?1 = embedding, ?2 = k

    if let Some(ref from) = filters.date_from {
        clauses.push(format!("AND date >= ?{idx}"));
        params.push(from.clone());
        idx += 1;
    }
    if let Some(ref to) = filters.date_to {
        clauses.push(format!("AND date <= ?{idx}"));
        params.push(to.clone());
        idx += 1;
    }
    if let Some(ref ctx) = filters.project_context {
        clauses.push(format!("AND context = ?{idx}"));
        params.push(ctx.clone());
    }

    (clauses.join(" "), params)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes into known-length slices"
)]
mod tests {
    use super::*;
    use crate::memory::types::Visibility;

    const TEST_DIM: usize = 4;

    fn sample_embedding(seed: f32) -> Vec<f32> {
        vec![seed, seed + 0.1, seed + 0.2, seed + 0.3]
    }

    fn sample_observation(text: &str) -> Observation {
        Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "residuum".to_string(),
            source_episodes: vec!["ep-001".to_string()],
            visibility: Visibility::User,
            content: text.to_string(),
        }
    }

    fn create_test_store() -> (tempfile::TempDir, VectorStore) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vectors.db");
        let store = VectorStore::open_or_create(&db_path, TEST_DIM).unwrap();
        (dir, store)
    }

    #[test]
    fn open_or_create_succeeds() {
        let (_dir, store) = create_test_store();
        assert_eq!(store.dim(), TEST_DIM, "dimension should match");
    }

    #[test]
    fn insert_and_search_observations() {
        let (_dir, store) = create_test_store();

        let obs = vec![
            sample_observation("rust memory safety"),
            sample_observation("observer token threshold"),
        ];
        let embeddings = vec![sample_embedding(0.1), sample_embedding(0.5)];

        let ids = store
            .insert_observations("ep-001", "2026-02-19", &obs, &embeddings)
            .unwrap();
        assert_eq!(ids.len(), 2, "should insert 2 observations");
        assert_eq!(ids[0], "ep-001-o0");
        assert_eq!(ids[1], "ep-001-o1");

        // Search for something similar to the first embedding
        let query = sample_embedding(0.1);
        let results = store
            .search(&query, 5, &VectorSearchFilters::default())
            .unwrap();
        assert!(!results.is_empty(), "should find results");
        assert_eq!(results[0].source_type, "observation");
        assert_eq!(results[0].episode_id, "ep-001");
    }

    #[test]
    fn insert_and_search_chunks() {
        let (_dir, store) = create_test_store();

        let chunks = vec![IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 2,
            line_end: 3,
            content: "user: hello\nassistant: hi there".to_string(),
        }];
        let embeddings = vec![sample_embedding(0.3)];

        let ids = store.insert_chunks(&chunks, &embeddings).unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "ep-001-c0");

        let query = sample_embedding(0.3);
        let results = store
            .search(&query, 5, &VectorSearchFilters::default())
            .unwrap();
        assert!(!results.is_empty(), "should find chunk");
        assert_eq!(results[0].source_type, "chunk");
        assert_eq!(results[0].line_start, Some(2));
        assert_eq!(results[0].line_end, Some(3));
    }

    #[test]
    fn delete_by_doc_ids() {
        let (_dir, store) = create_test_store();

        let obs = vec![sample_observation("test content")];
        let embeddings = vec![sample_embedding(0.2)];
        let ids = store
            .insert_observations("ep-001", "2026-02-19", &obs, &embeddings)
            .unwrap();

        let query = sample_embedding(0.2);
        let before = store
            .search(&query, 5, &VectorSearchFilters::default())
            .unwrap();
        assert!(!before.is_empty(), "should find before delete");

        store.delete_by_doc_ids(&ids).unwrap();

        let after = store
            .search(&query, 5, &VectorSearchFilters::default())
            .unwrap();
        assert!(after.is_empty(), "should be empty after delete");
    }

    #[test]
    fn clear_removes_all() {
        let (_dir, store) = create_test_store();

        let obs = vec![sample_observation("test")];
        let embeddings = vec![sample_embedding(0.1)];
        store
            .insert_observations("ep-001", "2026-02-19", &obs, &embeddings)
            .unwrap();

        store.clear().unwrap();

        let query = sample_embedding(0.1);
        let results = store
            .search(&query, 5, &VectorSearchFilters::default())
            .unwrap();
        assert!(results.is_empty(), "should be empty after clear");
    }

    #[test]
    fn empty_search_returns_empty() {
        let (_dir, store) = create_test_store();

        let query = sample_embedding(0.5);
        let results = store
            .search(&query, 5, &VectorSearchFilters::default())
            .unwrap();
        assert!(
            results.is_empty(),
            "empty store should return empty results"
        );
    }

    #[test]
    fn dimension_mismatch_rejected() {
        let (_dir, store) = create_test_store();

        let obs = vec![sample_observation("test")];
        let bad_embedding = vec![vec![0.1, 0.2]]; // wrong dimension
        let result = store.insert_observations("ep-001", "2026-02-19", &obs, &bad_embedding);
        assert!(result.is_err(), "wrong dimension should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("dimension mismatch"),
            "error should mention dimension: {err}"
        );
    }

    #[test]
    fn search_with_date_filter() {
        let (_dir, store) = create_test_store();

        let obs1 = vec![sample_observation("early data")];
        let emb1 = vec![sample_embedding(0.1)];
        store
            .insert_observations("ep-001", "2026-02-10", &obs1, &emb1)
            .unwrap();

        let obs2 = vec![sample_observation("later data")];
        let emb2 = vec![sample_embedding(0.15)];
        store
            .insert_observations("ep-002", "2026-02-20", &obs2, &emb2)
            .unwrap();

        let query = sample_embedding(0.1);
        let results = store
            .search(
                &query,
                5,
                &VectorSearchFilters {
                    date_from: Some("2026-02-15".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        assert!(
            results.iter().all(|r| r.date.as_str() >= "2026-02-15"),
            "all results should be on or after filter date"
        );
    }

    #[test]
    fn search_with_context_filter() {
        let (_dir, store) = create_test_store();

        let obs1 = vec![sample_observation("residuum data")];
        let emb1 = vec![sample_embedding(0.1)];
        store
            .insert_observations("ep-001", "2026-02-19", &obs1, &emb1)
            .unwrap();

        let obs2 = vec![Observation {
            project_context: "devops".to_string(),
            ..sample_observation("devops data")
        }];
        let emb2 = vec![sample_embedding(0.15)];
        store
            .insert_observations("ep-002", "2026-02-19", &obs2, &emb2)
            .unwrap();

        let query = sample_embedding(0.1);
        let results = store
            .search(
                &query,
                5,
                &VectorSearchFilters {
                    project_context: Some("residuum".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        assert!(
            results.iter().all(|r| r.context == "residuum"),
            "all results should have residuum context"
        );
    }

    #[test]
    fn count_mismatch_rejected() {
        let (_dir, store) = create_test_store();

        let obs = vec![sample_observation("one"), sample_observation("two")];
        let embeddings = vec![sample_embedding(0.1)]; // only 1 embedding for 2 obs
        let result = store.insert_observations("ep-001", "2026-02-19", &obs, &embeddings);
        assert!(result.is_err(), "count mismatch should be rejected");
    }

    #[test]
    fn insert_empty_observations_ok() {
        let (_dir, store) = create_test_store();
        let ids = store
            .insert_observations("ep-001", "2026-02-19", &[], &[])
            .unwrap();
        assert!(ids.is_empty(), "empty insert should return empty");
    }

    #[test]
    fn insert_empty_chunks_ok() {
        let (_dir, store) = create_test_store();
        let ids = store.insert_chunks(&[], &[]).unwrap();
        assert!(ids.is_empty(), "empty insert should return empty");
    }

    #[test]
    fn delete_empty_ids_ok() {
        let (_dir, store) = create_test_store();
        store.delete_by_doc_ids(&[]).unwrap();
    }

    #[test]
    fn has_observation_returns_false_when_missing() {
        let (_dir, store) = create_test_store();
        assert!(!store.has_observation("nonexistent").unwrap());
    }

    #[test]
    fn has_observation_returns_true_when_present() {
        let (_dir, store) = create_test_store();
        let obs = vec![sample_observation("test content")];
        let embeddings = vec![sample_embedding(0.1)];
        store
            .insert_observations("ep-001", "2026-02-19", &obs, &embeddings)
            .unwrap();
        assert!(store.has_observation("ep-001-o0").unwrap());
    }

    #[test]
    fn has_chunk_returns_false_when_missing() {
        let (_dir, store) = create_test_store();
        assert!(!store.has_chunk("nonexistent").unwrap());
    }

    #[test]
    fn has_chunk_returns_true_when_present() {
        let (_dir, store) = create_test_store();
        let chunks = vec![IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 1,
            line_end: 2,
            content: "test content".to_string(),
        }];
        let embeddings = vec![sample_embedding(0.3)];
        store.insert_chunks(&chunks, &embeddings).unwrap();
        assert!(store.has_chunk("ep-001-c0").unwrap());
    }

    #[test]
    fn duplicate_insert_observations_is_idempotent() {
        let (_dir, store) = create_test_store();
        let obs = vec![sample_observation("test content")];
        let embeddings = vec![sample_embedding(0.1)];

        let ids1 = store
            .insert_observations("ep-001", "2026-02-19", &obs, &embeddings)
            .unwrap();
        // Second insert with same IDs should succeed (INSERT OR IGNORE)
        let ids2 = store
            .insert_observations("ep-001", "2026-02-19", &obs, &embeddings)
            .unwrap();

        assert_eq!(ids1, ids2, "should return same doc IDs");

        // Should still only have one result when searching
        let query = sample_embedding(0.1);
        let results = store
            .search(&query, 10, &VectorSearchFilters::default())
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "duplicate insert should not create extra rows"
        );
    }

    #[test]
    fn duplicate_insert_chunks_is_idempotent() {
        let (_dir, store) = create_test_store();
        let chunks = vec![IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 1,
            line_end: 2,
            content: "test content".to_string(),
        }];
        let embeddings = vec![sample_embedding(0.3)];

        let ids1 = store.insert_chunks(&chunks, &embeddings).unwrap();
        // Second insert with same IDs should succeed (INSERT OR IGNORE)
        let ids2 = store.insert_chunks(&chunks, &embeddings).unwrap();

        assert_eq!(ids1, ids2, "should return same doc IDs");

        let query = sample_embedding(0.3);
        let results = store
            .search(&query, 10, &VectorSearchFilters::default())
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "duplicate insert should not create extra rows"
        );
    }
}
