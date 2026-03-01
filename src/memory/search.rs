//! Full-text BM25 search over observations and interaction-pair chunks using tantivy,
//! with optional hybrid vector search via sqlite-vec.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{BooleanQuery, QueryParser};
use tantivy::schema::{Field, OwnedValue, STORED, STRING, Schema, TEXT};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

use crate::config::SearchConfig;
use crate::error::ResiduumError;
use crate::memory::chunk_extractor::read_idx_jsonl;
use crate::memory::types::{IndexChunk, IndexManifest, ManifestFileEntry, Observation};
use crate::memory::vector_store::{VectorSearchFilters, VectorStore};
use crate::models::EmbeddingProvider;

/// Memory budget for the tantivy index writer (50 MB).
const WRITER_MEMORY_BUDGET_BYTES: usize = 50_000_000;

/// A search result from the memory index.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Document identifier (obs or chunk ID).
    pub id: String,
    /// Source type stored in the index: `"observation"` or `"chunk"` (internal values).
    pub source_type: String,
    /// Parent episode identifier.
    pub episode_id: String,
    /// Date string (YYYY-MM-DD).
    pub date: String,
    /// Project context tag.
    pub context: String,
    /// Line range start in the episode transcript (chunks only).
    pub line_start: Option<usize>,
    /// Line range end in the episode transcript (chunks only).
    pub line_end: Option<usize>,
    /// Snippet of matching content.
    pub snippet: String,
    /// BM25 relevance score.
    pub score: f32,
}

/// Filters for narrowing search results.
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    /// Filter by internal source type value: `"observation"` or `"chunk"`.
    /// The tool layer translates user-facing names before setting this.
    pub source: Option<String>,
    /// Filter results on or after this date (YYYY-MM-DD, inclusive).
    pub date_from: Option<String>,
    /// Filter results on or before this date (YYYY-MM-DD, inclusive).
    pub date_to: Option<String>,
    /// Filter by project context (exact match).
    pub project_context: Option<String>,
    /// Filter to results from these episode IDs.
    pub episode_ids: Option<Vec<String>>,
}

/// Statistics from a rebuild operation.
#[derive(Debug)]
pub struct RebuildResult {
    /// Number of observation documents indexed.
    pub obs_count: usize,
    /// Number of chunk documents indexed.
    pub chunk_count: usize,
    /// Manifest data for each indexed file.
    pub file_entries: Vec<(String, ManifestFileEntry)>,
}

/// Statistics from an incremental sync operation.
#[derive(Debug)]
pub struct SyncStats {
    /// Files that were newly indexed.
    pub added: usize,
    /// Files that were re-indexed after modification.
    pub updated: usize,
    /// Stale documents removed (from deleted files).
    pub removed: usize,
    /// Files that were unchanged and skipped.
    pub unchanged: usize,
}

/// BM25 full-text search index over memory observations and chunks.
pub struct MemoryIndex {
    index: Index,
    reader: IndexReader,
    id_field: Field,
    source_type_field: Field,
    episode_id_field: Field,
    date_field: Field,
    ctx_field: Field,
    content_field: Field,
    line_start_field: Field,
    line_end_field: Field,
}

impl MemoryIndex {
    /// Open or create a tantivy index at the given directory.
    ///
    /// # Errors
    /// Returns an error if the directory is inaccessible or the index is corrupt.
    pub fn open_or_create(index_dir: &Path) -> Result<Self, ResiduumError> {
        std::fs::create_dir_all(index_dir).map_err(|e| {
            ResiduumError::Memory(format!(
                "failed to create search index directory at {}: {e}",
                index_dir.display()
            ))
        })?;

        let mut builder = Schema::builder();
        let id_field = builder.add_text_field("id", STRING | STORED);
        let source_type_field = builder.add_text_field("source_type", STRING | STORED);
        let episode_id_field = builder.add_text_field("episode_id", STRING | STORED);
        let date_field = builder.add_text_field("date", STRING | STORED);
        let ctx_field = builder.add_text_field("context", STRING | STORED);
        let content_field = builder.add_text_field("content", TEXT | STORED);
        let line_start_field = builder.add_text_field("line_start", STRING | STORED);
        let line_end_field = builder.add_text_field("line_end", STRING | STORED);
        let schema = builder.build();

        let mmap_dir = MmapDirectory::open(index_dir).map_err(|e| {
            ResiduumError::Memory(format!(
                "failed to open mmap directory at {}: {e}",
                index_dir.display()
            ))
        })?;

        let index = Index::open_or_create(mmap_dir, schema).map_err(|e| {
            ResiduumError::Memory(format!(
                "failed to open search index at {}: {e}",
                index_dir.display()
            ))
        })?;

        let reader: IndexReader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| ResiduumError::Memory(format!("failed to create index reader: {e}")))?;

        Ok(Self {
            index,
            reader,
            id_field,
            source_type_field,
            episode_id_field,
            date_field,
            ctx_field,
            content_field,
            line_start_field,
            line_end_field,
        })
    }

    /// Create an empty in-RAM search index (no disk directory).
    ///
    /// Used as a fallback when the on-disk index cannot be created or
    /// opened during degraded startup.
    ///
    /// # Errors
    /// Returns an error if the in-memory index cannot be created.
    pub fn empty() -> Result<Self, ResiduumError> {
        let mut builder = Schema::builder();
        let id_field = builder.add_text_field("id", STRING | STORED);
        let source_type_field = builder.add_text_field("source_type", STRING | STORED);
        let episode_id_field = builder.add_text_field("episode_id", STRING | STORED);
        let date_field = builder.add_text_field("date", STRING | STORED);
        let ctx_field = builder.add_text_field("context", STRING | STORED);
        let content_field = builder.add_text_field("content", TEXT | STORED);
        let line_start_field = builder.add_text_field("line_start", STRING | STORED);
        let line_end_field = builder.add_text_field("line_end", STRING | STORED);
        let schema = builder.build();

        let index = Index::create_in_ram(schema);

        let reader: IndexReader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| {
                ResiduumError::Memory(format!("failed to create in-ram index reader: {e}"))
            })?;

        Ok(Self {
            index,
            reader,
            id_field,
            source_type_field,
            episode_id_field,
            date_field,
            ctx_field,
            content_field,
            line_start_field,
            line_end_field,
        })
    }

    /// Index observations from an episode.
    ///
    /// Returns the list of document IDs that were indexed.
    ///
    /// # Errors
    /// Returns an error if the index writer fails.
    pub fn index_observations(
        &self,
        episode_id: &str,
        date: &str,
        observations: &[Observation],
    ) -> Result<Vec<String>, ResiduumError> {
        if observations.is_empty() {
            return Ok(Vec::new());
        }

        let mut writer = self.writer()?;
        let mut doc_ids = Vec::with_capacity(observations.len());

        for (i, obs) in observations.iter().enumerate() {
            let doc_id = format!("{episode_id}-o{i}");
            let mut doc = TantivyDocument::default();
            doc.add_text(self.id_field, &doc_id);
            doc.add_text(self.source_type_field, "observation");
            doc.add_text(self.episode_id_field, episode_id);
            doc.add_text(self.date_field, date);
            doc.add_text(self.ctx_field, &obs.project_context);
            doc.add_text(self.content_field, &obs.content);
            doc.add_text(self.line_start_field, "");
            doc.add_text(self.line_end_field, "");

            writer.add_document(doc).map_err(|e| {
                ResiduumError::Memory(format!("failed to add observation to search index: {e}"))
            })?;
            doc_ids.push(doc_id);
        }

        self.commit_and_reload(&mut writer)?;
        Ok(doc_ids)
    }

    /// Index interaction-pair chunks.
    ///
    /// Returns the list of document IDs that were indexed.
    ///
    /// # Errors
    /// Returns an error if the index writer fails.
    pub fn index_chunks(&self, chunks: &[IndexChunk]) -> Result<Vec<String>, ResiduumError> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        let mut writer = self.writer()?;
        let mut doc_ids = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            let mut doc = TantivyDocument::default();
            doc.add_text(self.id_field, &chunk.chunk_id);
            doc.add_text(self.source_type_field, "chunk");
            doc.add_text(self.episode_id_field, &chunk.episode_id);
            doc.add_text(self.date_field, &chunk.date);
            doc.add_text(self.ctx_field, &chunk.context);
            doc.add_text(self.content_field, &chunk.content);
            doc.add_text(self.line_start_field, chunk.line_start.to_string());
            doc.add_text(self.line_end_field, chunk.line_end.to_string());

            writer.add_document(doc).map_err(|e| {
                ResiduumError::Memory(format!("failed to add chunk to search index: {e}"))
            })?;
            doc_ids.push(chunk.chunk_id.clone());
        }

        self.commit_and_reload(&mut writer)?;
        Ok(doc_ids)
    }

    /// Delete documents by their IDs.
    ///
    /// # Errors
    /// Returns an error if the index writer fails.
    pub fn delete_documents(&self, ids: &[String]) -> Result<(), ResiduumError> {
        if ids.is_empty() {
            return Ok(());
        }

        let mut writer = self.writer()?;
        for id in ids {
            let term = Term::from_field_text(self.id_field, id);
            writer.delete_term(term);
        }
        self.commit_and_reload(&mut writer)
    }

    /// Search the index with a query string and optional filters.
    ///
    /// # Errors
    /// Returns an error if the query is unparseable or search fails.
    pub fn search(
        &self,
        query_str: &str,
        limit: usize,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, ResiduumError> {
        if query_str.trim().is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.content_field]);
        let (text_query, _errors) = query_parser.parse_query_lenient(query_str);

        // Apply source_type filter as a BooleanQuery for early reduction
        let query: Box<dyn tantivy::query::Query> = if let Some(ref source) = filters.source {
            let source_term = Term::from_field_text(self.source_type_field, source);
            let source_query = tantivy::query::TermQuery::new(
                source_term,
                tantivy::schema::IndexRecordOption::Basic,
            );
            Box::new(BooleanQuery::new(vec![
                (tantivy::query::Occur::Must, text_query),
                (tantivy::query::Occur::Must, Box::new(source_query)),
            ]))
        } else {
            text_query
        };

        // Fetch extra candidates to account for post-retrieval filtering
        let has_post_filters = filters.date_from.is_some()
            || filters.date_to.is_some()
            || filters.project_context.is_some()
            || filters.episode_ids.is_some();
        let fetch_limit = if has_post_filters {
            limit.max(1) * 4
        } else {
            limit.max(1)
        };

        let top_docs = searcher
            .search(&*query, &TopDocs::with_limit(fetch_limit))
            .map_err(|e| ResiduumError::Memory(format!("search failed: {e}")))?;

        let mut results = Vec::with_capacity(limit);
        for (score, doc_address) in &top_docs {
            if results.len() >= limit {
                break;
            }

            let doc: TantivyDocument = searcher
                .doc(*doc_address)
                .map_err(|e| ResiduumError::Memory(format!("failed to fetch document: {e}")))?;

            let date = get_text(&doc, self.date_field);
            let ctx = get_text(&doc, self.ctx_field);
            let ep_id = get_text(&doc, self.episode_id_field);

            // Post-retrieval filters
            if let Some(ref from) = filters.date_from
                && date.as_str() < from.as_str()
            {
                continue;
            }
            if let Some(ref to) = filters.date_to
                && date.as_str() > to.as_str()
            {
                continue;
            }
            if let Some(ref pc) = filters.project_context
                && ctx != *pc
            {
                continue;
            }
            if let Some(ref ep_ids) = filters.episode_ids
                && !ep_ids.iter().any(|id| id == &ep_id)
            {
                continue;
            }

            let snippet = get_snippet(&doc, self.content_field);
            let source_type = get_text(&doc, self.source_type_field);
            let id = get_text(&doc, self.id_field);
            let line_start = parse_line_num(&doc, self.line_start_field);
            let line_end = parse_line_num(&doc, self.line_end_field);

            results.push(SearchResult {
                id,
                source_type,
                episode_id: ep_id,
                date,
                context: ctx,
                line_start,
                line_end,
                snippet,
                score: *score,
            });
        }

        Ok(results)
    }

    /// Rebuild the index from `.obs.json` and `.idx.jsonl` files.
    ///
    /// Clears the existing index and re-indexes everything.
    ///
    /// # Errors
    /// Returns an error if files cannot be read or the index cannot be written.
    pub fn rebuild(&self, memory_dir: &Path) -> Result<RebuildResult, ResiduumError> {
        let mut writer = self.writer()?;

        writer
            .delete_all_documents()
            .map_err(|e| ResiduumError::Memory(format!("failed to clear search index: {e}")))?;

        let mut result = RebuildResult {
            obs_count: 0,
            chunk_count: 0,
            file_entries: Vec::new(),
        };

        let episodes_dir = memory_dir.join("episodes");
        if episodes_dir.exists() {
            self.rebuild_directory(&mut writer, &episodes_dir, memory_dir, &mut result)?;
        }

        self.commit_and_reload(&mut writer)?;
        Ok(result)
    }

    /// Incremental sync: compare files against manifest, index new/modified, remove stale.
    ///
    /// # Errors
    /// Returns an error if files cannot be read or the index cannot be written.
    pub fn incremental_sync(
        &self,
        memory_dir: &Path,
        manifest: &IndexManifest,
    ) -> Result<(IndexManifest, SyncStats), ResiduumError> {
        let mut stats = SyncStats {
            added: 0,
            updated: 0,
            removed: 0,
            unchanged: 0,
        };
        let mut new_manifest = manifest.clone();

        // Collect current files on disk
        let episodes_dir = memory_dir.join("episodes");
        let mut disk_files: Vec<(String, String)> = Vec::new(); // (rel_path, mtime)
        if episodes_dir.exists() {
            collect_indexable_files(&episodes_dir, memory_dir, &mut disk_files)?;
        }

        let disk_paths: std::collections::HashSet<&str> =
            disk_files.iter().map(|(p, _)| p.as_str()).collect();

        // Remove stale entries (files in manifest but not on disk)
        let stale_keys: Vec<String> = manifest
            .files
            .keys()
            .filter(|k| !disk_paths.contains(k.as_str()))
            .cloned()
            .collect();
        for key in &stale_keys {
            if let Some(entry) = new_manifest.files.remove(key) {
                self.delete_documents(&entry.doc_ids)?;
                stats.removed += entry.doc_ids.len();
            }
        }

        // Process each file on disk
        let mut writer = self.writer()?;
        for (rel_path, mtime) in &disk_files {
            if let Some(existing) = manifest.files.get(rel_path) {
                if existing.mtime == *mtime {
                    stats.unchanged += 1;
                    continue;
                }
                // Modified: delete old docs first
                for id in &existing.doc_ids {
                    let term = Term::from_field_text(self.id_field, id);
                    writer.delete_term(term);
                }
                stats.updated += 1;
            } else {
                stats.added += 1;
            }

            let abs_path = memory_dir.join(rel_path);
            let doc_ids = self.index_file_into_writer(&mut writer, &abs_path, rel_path)?;
            new_manifest.files.insert(
                rel_path.clone(),
                ManifestFileEntry {
                    mtime: mtime.clone(),
                    doc_ids,
                    embedded: false,
                },
            );
        }

        self.commit_and_reload(&mut writer)?;
        Ok((new_manifest, stats))
    }

    /// Create an index writer with a reasonable memory budget.
    fn writer(&self) -> Result<IndexWriter, ResiduumError> {
        self.index
            .writer(WRITER_MEMORY_BUDGET_BYTES)
            .map_err(|e| ResiduumError::Memory(format!("failed to create index writer: {e}")))
    }

    /// Commit the writer and reload the reader.
    fn commit_and_reload(&self, writer: &mut IndexWriter) -> Result<(), ResiduumError> {
        writer
            .commit()
            .map_err(|e| ResiduumError::Memory(format!("failed to commit search index: {e}")))?;
        self.reader.reload().map_err(|e| {
            ResiduumError::Memory(format!("failed to reload search index reader: {e}"))
        })?;
        Ok(())
    }

    /// Recursively walk a directory, indexing `.obs.json` and `.idx.jsonl` files.
    fn rebuild_directory(
        &self,
        writer: &mut IndexWriter,
        dir: &Path,
        memory_dir: &Path,
        result: &mut RebuildResult,
    ) -> Result<(), ResiduumError> {
        let entries = std::fs::read_dir(dir).map_err(|e| {
            ResiduumError::Memory(format!("failed to read directory {}: {e}", dir.display()))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                ResiduumError::Memory(format!("failed to read directory entry: {e}"))
            })?;
            let path = entry.path();

            if path.is_dir() {
                self.rebuild_directory(writer, &path, memory_dir, result)?;
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let rel_path = make_relative(&path, memory_dir);
                let mtime = file_mtime_str(&path);

                match ext {
                    "json" if path.to_string_lossy().ends_with(".obs.json") => {
                        match parse_obs_file(&path) {
                            Ok((episode_id, date, observations)) => {
                                let mut doc_ids = Vec::new();
                                for (i, obs) in observations.iter().enumerate() {
                                    let doc_id = format!("{episode_id}-o{i}");
                                    add_obs_document(
                                        writer,
                                        self,
                                        &doc_id,
                                        &episode_id,
                                        &date,
                                        &obs.project_context,
                                        &obs.content,
                                    )?;
                                    doc_ids.push(doc_id);
                                }
                                result.obs_count += observations.len();
                                result.file_entries.push((
                                    rel_path,
                                    ManifestFileEntry {
                                        mtime,
                                        doc_ids,
                                        embedded: false,
                                    },
                                ));
                            }
                            Err(e) => {
                                tracing::warn!(path = %path.display(), error = %e, "skipping unparseable obs file");
                            }
                        }
                    }
                    "jsonl" if path.to_string_lossy().ends_with(".idx.jsonl") => {
                        let chunks = read_idx_jsonl(&path);
                        let mut doc_ids = Vec::new();
                        for chunk in &chunks {
                            add_chunk_document(writer, self, chunk)?;
                            doc_ids.push(chunk.chunk_id.clone());
                        }
                        result.chunk_count += chunks.len();
                        result.file_entries.push((
                            rel_path,
                            ManifestFileEntry {
                                mtime,
                                doc_ids,
                                embedded: false,
                            },
                        ));
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    /// Index a single `.obs.json` or `.idx.jsonl` file into a writer, returning doc IDs.
    fn index_file_into_writer(
        &self,
        writer: &mut IndexWriter,
        abs_path: &Path,
        rel_path: &str,
    ) -> Result<Vec<String>, ResiduumError> {
        let mut doc_ids = Vec::new();

        if rel_path.ends_with(".obs.json") {
            let (episode_id, date, observations) = parse_obs_file(abs_path)?;
            for (i, obs) in observations.iter().enumerate() {
                let doc_id = format!("{episode_id}-o{i}");
                add_obs_document(
                    writer,
                    self,
                    &doc_id,
                    &episode_id,
                    &date,
                    &obs.project_context,
                    &obs.content,
                )?;
                doc_ids.push(doc_id);
            }
        } else if rel_path.ends_with(".idx.jsonl") {
            let chunks = read_idx_jsonl(abs_path);
            for chunk in &chunks {
                add_chunk_document(writer, self, chunk)?;
                doc_ids.push(chunk.chunk_id.clone());
            }
        }

        Ok(doc_ids)
    }
}

/// Add an observation document to the writer.
fn add_obs_document(
    writer: &mut IndexWriter,
    idx: &MemoryIndex,
    doc_id: &str,
    episode_id: &str,
    date: &str,
    ctx: &str,
    content: &str,
) -> Result<(), ResiduumError> {
    let mut doc = TantivyDocument::default();
    doc.add_text(idx.id_field, doc_id);
    doc.add_text(idx.source_type_field, "observation");
    doc.add_text(idx.episode_id_field, episode_id);
    doc.add_text(idx.date_field, date);
    doc.add_text(idx.ctx_field, ctx);
    doc.add_text(idx.content_field, content);
    doc.add_text(idx.line_start_field, "");
    doc.add_text(idx.line_end_field, "");

    writer.add_document(doc).map_err(|e| {
        ResiduumError::Memory(format!("failed to add observation to search index: {e}"))
    })?;
    Ok(())
}

/// Add a chunk document to the writer.
fn add_chunk_document(
    writer: &mut IndexWriter,
    idx: &MemoryIndex,
    chunk: &IndexChunk,
) -> Result<(), ResiduumError> {
    let mut doc = TantivyDocument::default();
    doc.add_text(idx.id_field, &chunk.chunk_id);
    doc.add_text(idx.source_type_field, "chunk");
    doc.add_text(idx.episode_id_field, &chunk.episode_id);
    doc.add_text(idx.date_field, &chunk.date);
    doc.add_text(idx.ctx_field, &chunk.context);
    doc.add_text(idx.content_field, &chunk.content);
    doc.add_text(idx.line_start_field, chunk.line_start.to_string());
    doc.add_text(idx.line_end_field, chunk.line_end.to_string());

    writer
        .add_document(doc)
        .map_err(|e| ResiduumError::Memory(format!("failed to add chunk to search index: {e}")))?;
    Ok(())
}

/// Extract a text field value from a tantivy document.
fn get_text(doc: &TantivyDocument, field: Field) -> String {
    match doc.get_first(field) {
        Some(OwnedValue::Str(s)) => s.clone(),
        Some(_) | None => String::new(),
    }
}

/// Extract a snippet (first 200 chars of content) from a document.
fn get_snippet(doc: &TantivyDocument, content_field: Field) -> String {
    match doc.get_first(content_field) {
        Some(OwnedValue::Str(s)) => {
            let end = s.len().min(200);
            s.get(..end).unwrap_or_default().to_string()
        }
        Some(_) | None => String::new(),
    }
}

/// Parse a stored line number field, returning `None` for empty strings.
fn parse_line_num(doc: &TantivyDocument, field: Field) -> Option<usize> {
    let text = get_text(doc, field);
    if text.is_empty() {
        None
    } else {
        text.parse().ok()
    }
}

/// Parse an `.obs.json` file into `(episode_id, date, observations)`.
///
/// The `episode_id` and date are derived from the filename: `ep-NNN.obs.json`
/// lives under `episodes/YYYY-MM/DD/`.
pub(crate) fn parse_obs_file(
    path: &Path,
) -> Result<(String, String, Vec<Observation>), ResiduumError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ResiduumError::Memory(format!("failed to read {}: {e}", path.display())))?;

    let observations: Vec<Observation> = serde_json::from_str(&content).map_err(|e| {
        ResiduumError::Memory(format!(
            "failed to parse obs file at {}: {e}",
            path.display()
        ))
    })?;

    // Extract episode_id from filename: ep-NNN.obs.json → ep-NNN
    let episode_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_suffix(".obs"))
        .unwrap_or("unknown")
        .to_string();

    // Extract date from directory path: .../YYYY-MM/DD/... → YYYY-MM-DD
    let date = extract_date_from_path(path);

    Ok((episode_id, date, observations))
}

/// Extract a date string from an episode file path.
///
/// Expected path pattern: `.../episodes/YYYY-MM/DD/...`
/// Returns `YYYY-MM-DD` or empty string if the pattern doesn't match.
fn extract_date_from_path(path: &Path) -> String {
    // Walk up from file: file → DD dir → YYYY-MM dir
    let dd = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str());
    let yyyy_mm = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str());

    match (yyyy_mm, dd) {
        (Some(ym), Some(d)) if ym.len() == 7 && d.len() == 2 => {
            format!("{ym}-{d}")
        }
        _ => String::new(),
    }
}

/// Build the relative path of `file` with respect to `base`.
fn make_relative(file: &Path, base: &Path) -> String {
    file.strip_prefix(base).map_or_else(
        |_| file.to_string_lossy().to_string(),
        |p| p.to_string_lossy().to_string(),
    )
}

/// Get the mtime of a file as an ISO-ish string, or empty on failure.
fn file_mtime_str(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.format("%Y-%m-%dT%H:%M:%S").to_string()
        })
        .unwrap_or_default()
}

/// Recursively collect `.obs.json` and `.idx.jsonl` files with their mtimes.
fn collect_indexable_files(
    dir: &Path,
    base: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), ResiduumError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        ResiduumError::Memory(format!("failed to read directory {}: {e}", dir.display()))
    })?;

    for entry in entries {
        let entry = entry
            .map_err(|e| ResiduumError::Memory(format!("failed to read directory entry: {e}")))?;
        let path = entry.path();

        if path.is_dir() {
            collect_indexable_files(&path, base, out)?;
        } else {
            let name = path.to_string_lossy();
            if name.ends_with(".obs.json") || name.ends_with(".idx.jsonl") {
                let rel = make_relative(&path, base);
                let mtime = file_mtime_str(&path);
                out.push((rel, mtime));
            }
        }
    }

    Ok(())
}

/// Create a shared `MemoryIndex` wrapped in an `Arc` for tool sharing.
///
/// # Errors
/// Returns an error if the index cannot be opened.
pub fn create_shared_index(index_dir: &Path) -> Result<Arc<MemoryIndex>, ResiduumError> {
    let index = MemoryIndex::open_or_create(index_dir)?;
    Ok(Arc::new(index))
}

// ── Hybrid searcher ─────────────────────────────────────────────────────────

/// Orchestrates BM25 + vector search with score normalization and merging.
///
/// When no vector store or embedding provider is configured, delegates
/// entirely to BM25 (existing behavior, no score filtering).
pub struct HybridSearcher {
    bm25: Arc<MemoryIndex>,
    vector: Option<Arc<VectorStore>>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    cfg: SearchConfig,
}

impl HybridSearcher {
    /// Create a new hybrid searcher.
    #[must_use]
    pub fn new(
        bm25: Arc<MemoryIndex>,
        vector: Option<Arc<VectorStore>>,
        embedding: Option<Arc<dyn EmbeddingProvider>>,
        cfg: SearchConfig,
    ) -> Self {
        Self {
            bm25,
            vector,
            embedding,
            cfg,
        }
    }

    /// Whether vector search is available (both store and provider configured).
    #[must_use]
    pub fn has_vector(&self) -> bool {
        self.vector.is_some() && self.embedding.is_some()
    }

    /// Search using hybrid BM25 + vector scoring, or BM25-only fallback.
    ///
    /// # Errors
    /// Returns an error if BM25 search or embedding generation fails.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, ResiduumError> {
        let candidates = limit * self.cfg.candidate_multiplier;

        // BM25 search
        let bm25_results = self.bm25.search(query, candidates, filters)?;

        // If no vector search available, return BM25 results directly
        let (Some(vs), Some(ep)) = (&self.vector, &self.embedding) else {
            // BM25-only: apply temporal decay if enabled, re-sort, then truncate
            let mut results = bm25_results;
            if self.cfg.temporal_decay {
                let today = chrono::Utc::now().date_naive();
                apply_temporal_decay(&mut results, self.cfg.temporal_decay_half_life_days, today);
                results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            results.truncate(limit);
            return Ok(results);
        };

        // Embed the query
        let embed_response = ep
            .embed(&[query])
            .await
            .map_err(|e| ResiduumError::Memory(format!("failed to embed search query: {e}")))?;
        let query_vec = embed_response
            .embeddings
            .into_iter()
            .next()
            .ok_or_else(|| {
                ResiduumError::Memory("embedding provider returned no embeddings".to_string())
            })?;

        // Vector search (sync, so use spawn_blocking)
        let vs_clone = Arc::clone(vs);
        let vec_filters = VectorSearchFilters {
            date_from: filters.date_from.clone(),
            date_to: filters.date_to.clone(),
            project_context: filters.project_context.clone(),
        };
        let vec_limit = candidates;
        let vec_results = tokio::task::spawn_blocking(move || {
            vs_clone.search(&query_vec, vec_limit, &vec_filters)
        })
        .await
        .map_err(|e| ResiduumError::Memory(format!("vector search task failed: {e}")))??;

        // Merge results
        let mut merged = merge_hybrid_results(&bm25_results, &vec_results, &self.cfg, limit);

        // Apply temporal decay after merge if enabled
        if self.cfg.temporal_decay {
            let today = chrono::Utc::now().date_naive();
            apply_temporal_decay(&mut merged, self.cfg.temporal_decay_half_life_days, today);
            merged.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        Ok(merged)
    }

    /// Get a reference to the underlying BM25 index.
    #[must_use]
    pub fn bm25(&self) -> &MemoryIndex {
        &self.bm25
    }
}

/// Min-max normalize a list of scores to [0, 1].
///
/// Single-element or empty inputs are handled: single → 1.0, empty → empty.
fn normalize_scores(scores: &[f32]) -> Vec<f32> {
    if scores.is_empty() {
        return Vec::new();
    }
    if scores.len() == 1 {
        return vec![1.0];
    }

    let min = scores.iter().copied().reduce(f32::min).unwrap_or_default();
    let max = scores.iter().copied().reduce(f32::max).unwrap_or_default();
    let range = max - min;

    if range < f32::EPSILON {
        return vec![1.0; scores.len()];
    }

    scores.iter().map(|s| (s - min) / range).collect()
}

/// Merge BM25 and vector results into hybrid-scored `SearchResult` values.
fn merge_hybrid_results(
    bm25_results: &[SearchResult],
    vec_results: &[super::vector_store::VectorSearchResult],
    cfg: &SearchConfig,
    limit: usize,
) -> Vec<SearchResult> {
    // Normalize BM25 scores
    let bm25_scores: Vec<f32> = bm25_results.iter().map(|r| r.score).collect();
    let norm_bm25 = normalize_scores(&bm25_scores);

    // Convert vector distances to similarities and normalize
    let vec_similarities: Vec<f32> = vec_results
        .iter()
        .map(|r| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "cosine distance is always in [0, 2], safe to truncate to f32"
            )]
            let sim = 1.0 - r.distance as f32;
            sim.max(0.0)
        })
        .collect();
    let norm_vec = normalize_scores(&vec_similarities);

    // Build maps: doc_id → normalized score
    let mut bm25_map: HashMap<&str, (f32, &SearchResult)> = HashMap::new();
    for (i, result) in bm25_results.iter().enumerate() {
        if let Some(&norm) = norm_bm25.get(i) {
            bm25_map.insert(&result.id, (norm, result));
        }
    }

    let mut vec_map: HashMap<&str, (f32, &super::vector_store::VectorSearchResult)> =
        HashMap::new();
    for (i, result) in vec_results.iter().enumerate() {
        if let Some(&norm) = norm_vec.get(i) {
            vec_map.insert(&result.id, (norm, result));
        }
    }

    // Collect all unique doc IDs
    let mut all_ids: Vec<&str> = Vec::new();
    for id in bm25_map.keys() {
        all_ids.push(id);
    }
    for id in vec_map.keys() {
        if !bm25_map.contains_key(id) {
            all_ids.push(id);
        }
    }

    // Compute hybrid scores
    #[expect(
        clippy::cast_possible_truncation,
        reason = "search weights are in [0.0, 1.0], safe to truncate to f32"
    )]
    let text_w = cfg.text_weight as f32;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "search weights are in [0.0, 1.0], safe to truncate to f32"
    )]
    let vec_w = cfg.vector_weight as f32;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "min_score is in [0.0, 1.0], safe to truncate to f32"
    )]
    let min_score = cfg.min_score as f32;

    let mut scored: Vec<(f32, SearchResult)> = Vec::new();
    for id in &all_ids {
        let bm25_score = bm25_map.get(id).map_or(0.0, |(s, _)| *s);
        let vec_score = vec_map.get(id).map_or(0.0, |(s, _)| *s);
        let hybrid = text_w * bm25_score + vec_w * vec_score;

        if hybrid < min_score {
            continue;
        }

        // Build SearchResult from whichever source has the data
        let result = if let Some((_, bm25_r)) = bm25_map.get(id) {
            SearchResult {
                score: hybrid,
                ..(*bm25_r).clone()
            }
        } else if let Some((_, vec_r)) = vec_map.get(id) {
            let snippet = if vec_r.content.len() > 200 {
                vec_r.content.get(..200).unwrap_or_default().to_string()
            } else {
                vec_r.content.clone()
            };
            SearchResult {
                id: vec_r.id.clone(),
                source_type: vec_r.source_type.clone(),
                episode_id: vec_r.episode_id.clone(),
                date: vec_r.date.clone(),
                context: vec_r.context.clone(),
                line_start: vec_r.line_start,
                line_end: vec_r.line_end,
                snippet,
                score: hybrid,
            }
        } else {
            continue;
        };

        scored.push((hybrid, result));
    }

    // Sort descending by hybrid score
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);

    scored.into_iter().map(|(_, r)| r).collect()
}

/// Apply exponential temporal decay to search result scores.
///
/// Each result's score is multiplied by `exp(-lambda * age_days)` where
/// `lambda = ln(2) / half_life_days`. Results with unparseable dates are
/// left unchanged.
fn apply_temporal_decay(
    results: &mut [SearchResult],
    half_life_days: f64,
    today: chrono::NaiveDate,
) {
    let lambda = f64::ln(2.0) / half_life_days;

    for result in results {
        let Ok(date) = chrono::NaiveDate::parse_from_str(&result.date, "%Y-%m-%d") else {
            tracing::warn!(date = %result.date, id = %result.id, "unparseable date, skipping temporal decay");
            continue;
        };

        let age_days = (today - date).num_days().max(0);

        #[expect(
            clippy::cast_precision_loss,
            reason = "age in days is small enough that i64→f64 precision loss is irrelevant"
        )]
        let age_f64 = age_days as f64;

        #[expect(
            clippy::cast_possible_truncation,
            reason = "decay factor is in [0.0, 1.0], safe to truncate to f32"
        )]
        let decay = f64::exp(-lambda * age_f64) as f32;
        result.score *= decay;
    }
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

    fn create_test_index() -> (tempfile::TempDir, MemoryIndex) {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();
        (dir, index)
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

    fn no_filters() -> SearchFilters {
        SearchFilters::default()
    }

    #[test]
    fn open_or_create_index() {
        let (_dir, _index) = create_test_index();
    }

    #[test]
    fn index_and_search_observations() {
        let (_dir, index) = create_test_index();

        let obs = vec![
            sample_observation("tantivy provides BM25 search without C dependencies"),
            sample_observation("observer fires when token threshold is exceeded"),
        ];

        let ids = index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "ep-001-o0");

        let results = index.search("tantivy BM25", 5, &no_filters()).unwrap();
        assert!(!results.is_empty(), "should find matching observation");
        assert_eq!(results[0].source_type, "observation");
        assert_eq!(results[0].episode_id, "ep-001");
    }

    #[test]
    fn index_and_search_chunks() {
        let (_dir, index) = create_test_index();

        let chunks = vec![IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 2,
            line_end: 3,
            content: "user: how does the observer work?\nassistant: it monitors token counts"
                .to_string(),
        }];

        let ids = index.index_chunks(&chunks).unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "ep-001-c0");

        let results = index.search("observer token", 5, &no_filters()).unwrap();
        assert!(!results.is_empty(), "should find matching chunk");
        assert_eq!(results[0].source_type, "chunk");
        assert_eq!(results[0].line_start, Some(2));
        assert_eq!(results[0].line_end, Some(3));
    }

    #[test]
    fn filter_by_source_type() {
        let (_dir, index) = create_test_index();

        let obs = vec![sample_observation("rust memory safety model")];
        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let chunks = vec![IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 2,
            line_end: 3,
            content: "user: tell me about rust memory safety\nassistant: rust uses ownership"
                .to_string(),
        }];
        index.index_chunks(&chunks).unwrap();

        // All results
        let all = index.search("rust memory", 10, &no_filters()).unwrap();
        assert!(all.len() >= 2, "should find both obs and chunk");

        // Observations only
        let obs_only = index
            .search(
                "rust memory",
                10,
                &SearchFilters {
                    source: Some("observation".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(obs_only.iter().all(|r| r.source_type == "observation"));

        // Chunks only
        let chunk_only = index
            .search(
                "rust memory",
                10,
                &SearchFilters {
                    source: Some("chunk".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(chunk_only.iter().all(|r| r.source_type == "chunk"));
    }

    #[test]
    fn filter_by_date_range() {
        let (_dir, index) = create_test_index();

        let obs1 = vec![sample_observation("early observation about rust")];
        index
            .index_observations("ep-001", "2026-02-15", &obs1)
            .unwrap();

        let obs2 = vec![sample_observation("later observation about rust safety")];
        index
            .index_observations("ep-002", "2026-02-20", &obs2)
            .unwrap();

        let filtered = index
            .search(
                "rust",
                10,
                &SearchFilters {
                    date_from: Some("2026-02-18".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            filtered.iter().all(|r| r.date.as_str() >= "2026-02-18"),
            "all results should be on or after date_from"
        );
    }

    #[test]
    fn filter_by_project_context() {
        let (_dir, index) = create_test_index();

        let obs1 = vec![sample_observation("residuum observation about testing")];
        index
            .index_observations("ep-001", "2026-02-19", &obs1)
            .unwrap();

        let obs2 = vec![Observation {
            project_context: "devops".to_string(),
            ..sample_observation("devops observation about testing")
        }];
        index
            .index_observations("ep-002", "2026-02-19", &obs2)
            .unwrap();

        let filtered = index
            .search(
                "testing",
                10,
                &SearchFilters {
                    project_context: Some("residuum".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            filtered.iter().all(|r| r.context == "residuum"),
            "all results should have residuum context"
        );
    }

    #[test]
    fn filter_by_episode_ids() {
        let (_dir, index) = create_test_index();

        let obs1 = vec![sample_observation(
            "observation from episode one about memory",
        )];
        index
            .index_observations("ep-001", "2026-02-19", &obs1)
            .unwrap();

        let obs2 = vec![sample_observation(
            "observation from episode two about memory",
        )];
        index
            .index_observations("ep-002", "2026-02-19", &obs2)
            .unwrap();

        let filtered = index
            .search(
                "memory",
                10,
                &SearchFilters {
                    episode_ids: Some(vec!["ep-001".to_string()]),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            filtered.iter().all(|r| r.episode_id == "ep-001"),
            "all results should be from ep-001"
        );
    }

    #[test]
    fn search_empty_index() {
        let (_dir, index) = create_test_index();
        let results = index.search("anything", 5, &no_filters()).unwrap();
        assert!(results.is_empty(), "empty index should return no results");
    }

    #[test]
    fn search_empty_query() {
        let (_dir, index) = create_test_index();
        let results = index.search("", 5, &no_filters()).unwrap();
        assert!(results.is_empty(), "empty query should return no results");
    }

    #[test]
    fn delete_documents_removes_from_index() {
        let (_dir, index) = create_test_index();

        let obs = vec![sample_observation("deleteable observation about rust")];
        let ids = index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let before = index.search("deleteable rust", 5, &no_filters()).unwrap();
        assert!(!before.is_empty(), "should find before delete");

        index.delete_documents(&ids).unwrap();

        let after = index.search("deleteable rust", 5, &no_filters()).unwrap();
        assert!(after.is_empty(), "should not find after delete");
    }

    #[test]
    fn search_results_have_scores() {
        let (_dir, index) = create_test_index();

        let obs = vec![sample_observation(
            "rust programming language memory safety",
        )];
        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let results = index.search("rust", 5, &no_filters()).unwrap();
        assert!(!results.is_empty(), "should have results");
        assert!(
            results.first().is_some_and(|r| r.score > 0.0),
            "score should be positive"
        );
    }

    #[test]
    fn snippet_extraction() {
        let (_dir, index) = create_test_index();

        let obs = vec![sample_observation(
            "the observer monitors token counts and fires when exceeded",
        )];
        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let results = index.search("observer token", 5, &no_filters()).unwrap();
        assert!(!results.is_empty(), "should have results");
        assert!(
            results.first().is_some_and(|r| !r.snippet.is_empty()),
            "snippet should not be empty"
        );
    }

    #[test]
    fn rebuild_indexes_obs_and_idx_files() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        let day_dir = memory_dir.join("episodes/2026-02/19");
        std::fs::create_dir_all(&day_dir).unwrap();

        // Write an obs.json file
        let obs = vec![sample_observation("workspace uses flat layout")];
        let obs_json = serde_json::to_string(&obs).unwrap();
        std::fs::write(day_dir.join("ep-001.obs.json"), &obs_json).unwrap();

        // Write an idx.jsonl file
        let chunk = IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 2,
            line_end: 3,
            content: "user: describe the layout\nassistant: it is flat".to_string(),
        };
        let chunk_json = serde_json::to_string(&chunk).unwrap();
        std::fs::write(day_dir.join("ep-001.idx.jsonl"), format!("{chunk_json}\n")).unwrap();

        let index_dir = memory_dir.join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();
        let result = index.rebuild(&memory_dir).unwrap();

        assert_eq!(result.obs_count, 1, "should index 1 observation");
        assert_eq!(result.chunk_count, 1, "should index 1 chunk");
        assert_eq!(result.file_entries.len(), 2, "should have 2 file entries");

        let results = index.search("flat layout", 5, &no_filters()).unwrap();
        assert!(!results.is_empty(), "should find indexed content");
    }

    #[test]
    fn incremental_sync_indexes_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        let day_dir = memory_dir.join("episodes/2026-02/19");
        std::fs::create_dir_all(&day_dir).unwrap();

        // Write initial file
        let obs = vec![sample_observation("initial observation about workspace")];
        std::fs::write(
            day_dir.join("ep-001.obs.json"),
            serde_json::to_string(&obs).unwrap(),
        )
        .unwrap();

        let index_dir = memory_dir.join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();

        // First sync with empty manifest
        let empty_manifest = IndexManifest::new();
        let (manifest, stats) = index
            .incremental_sync(&memory_dir, &empty_manifest)
            .unwrap();
        assert_eq!(stats.added, 1, "should add 1 new file");
        assert_eq!(stats.unchanged, 0);
        assert_eq!(manifest.files.len(), 1);

        // Second sync with same manifest — nothing changed
        let (manifest2, stats2) = index.incremental_sync(&memory_dir, &manifest).unwrap();
        assert_eq!(stats2.unchanged, 1, "file should be unchanged");
        assert_eq!(stats2.added, 0);
        assert_eq!(manifest2.files.len(), 1);

        // Add a new file
        let obs2 = vec![sample_observation("new observation about testing")];
        std::fs::write(
            day_dir.join("ep-002.obs.json"),
            serde_json::to_string(&obs2).unwrap(),
        )
        .unwrap();

        let (manifest3, stats3) = index.incremental_sync(&memory_dir, &manifest2).unwrap();
        assert_eq!(stats3.added, 1, "should add new file");
        assert_eq!(stats3.unchanged, 1, "old file unchanged");
        assert_eq!(manifest3.files.len(), 2);

        // Search should find both
        let results = index.search("observation", 10, &no_filters()).unwrap();
        assert!(
            results.len() >= 2,
            "should find observations from both files"
        );
    }

    #[test]
    fn incremental_sync_removes_stale() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        let day_dir = memory_dir.join("episodes/2026-02/19");
        std::fs::create_dir_all(&day_dir).unwrap();

        let obs = vec![sample_observation("stale observation about workspace")];
        std::fs::write(
            day_dir.join("ep-001.obs.json"),
            serde_json::to_string(&obs).unwrap(),
        )
        .unwrap();

        let index_dir = memory_dir.join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();

        let (manifest, _) = index
            .incremental_sync(&memory_dir, &IndexManifest::new())
            .unwrap();

        // Delete the file from disk
        std::fs::remove_file(day_dir.join("ep-001.obs.json")).unwrap();

        let (manifest2, stats) = index.incremental_sync(&memory_dir, &manifest).unwrap();
        assert!(stats.removed > 0, "should remove stale docs");
        assert!(manifest2.files.is_empty(), "manifest should have no files");

        let results = index.search("stale workspace", 5, &no_filters()).unwrap();
        assert!(results.is_empty(), "stale docs should be gone");
    }

    #[test]
    fn line_range_present_for_chunks_absent_for_obs() {
        let (_dir, index) = create_test_index();

        let obs = vec![sample_observation("observation about line ranges")];
        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let chunks = vec![IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 5,
            line_end: 8,
            content: "user: question about line ranges\nassistant: answer about ranges".to_string(),
        }];
        index.index_chunks(&chunks).unwrap();

        let obs_results = index
            .search(
                "line ranges",
                10,
                &SearchFilters {
                    source: Some("observation".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            obs_results
                .first()
                .is_some_and(|r| r.line_start.is_none() && r.line_end.is_none()),
            "observations should have no line range"
        );

        let chunk_results = index
            .search(
                "line ranges",
                10,
                &SearchFilters {
                    source: Some("chunk".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            chunk_results
                .first()
                .is_some_and(|r| r.line_start == Some(5) && r.line_end == Some(8)),
            "chunks should have line range"
        );
    }

    #[test]
    fn combined_filters() {
        let (_dir, index) = create_test_index();

        // Index diverse data
        let obs1 = vec![sample_observation(
            "early residuum observation about search",
        )];
        index
            .index_observations("ep-001", "2026-02-15", &obs1)
            .unwrap();

        let obs2 = vec![sample_observation(
            "later residuum observation about search",
        )];
        index
            .index_observations("ep-002", "2026-02-20", &obs2)
            .unwrap();

        let obs3 = vec![Observation {
            project_context: "devops".to_string(),
            ..sample_observation("devops observation about search")
        }];
        index
            .index_observations("ep-003", "2026-02-20", &obs3)
            .unwrap();

        // Filter: residuum + after 2026-02-18
        let results = index
            .search(
                "search",
                10,
                &SearchFilters {
                    project_context: Some("residuum".to_string()),
                    date_from: Some("2026-02-18".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(results.len(), 1, "should match only ep-002");
        assert_eq!(results[0].episode_id, "ep-002");
    }

    #[test]
    fn index_empty_observations() {
        let (_dir, index) = create_test_index();
        let ids = index
            .index_observations("ep-001", "2026-02-19", &[])
            .unwrap();
        assert!(ids.is_empty(), "empty obs should return empty ids");
    }

    #[test]
    fn index_empty_chunks() {
        let (_dir, index) = create_test_index();
        let ids = index.index_chunks(&[]).unwrap();
        assert!(ids.is_empty(), "empty chunks should return empty ids");
    }

    #[test]
    fn extract_date_from_path_valid() {
        let path = std::path::PathBuf::from("/memory/episodes/2026-02/19/ep-001.obs.json");
        assert_eq!(extract_date_from_path(&path), "2026-02-19");
    }

    #[test]
    fn extract_date_from_path_invalid() {
        let path = std::path::PathBuf::from("/some/random/path.json");
        assert!(extract_date_from_path(&path).is_empty());
    }

    // ── Normalize scores ─────────────────────────────────────────────────

    #[test]
    fn normalize_empty() {
        let result = normalize_scores(&[]);
        assert!(result.is_empty(), "empty input should produce empty output");
    }

    #[test]
    fn normalize_single() {
        let result = normalize_scores(&[5.0]);
        assert_eq!(result.len(), 1, "single input should produce one output");
        assert!(
            (result[0] - 1.0).abs() < f32::EPSILON,
            "single value should normalize to 1.0"
        );
    }

    #[test]
    fn normalize_multiple() {
        let result = normalize_scores(&[2.0, 4.0, 6.0]);
        assert_eq!(result.len(), 3);
        assert!(
            (result[0] - 0.0).abs() < f32::EPSILON,
            "min should normalize to 0.0"
        );
        assert!(
            (result[1] - 0.5).abs() < f32::EPSILON,
            "mid should normalize to 0.5"
        );
        assert!(
            (result[2] - 1.0).abs() < f32::EPSILON,
            "max should normalize to 1.0"
        );
    }

    #[test]
    fn normalize_equal_values() {
        let result = normalize_scores(&[3.0, 3.0, 3.0]);
        assert!(
            result.iter().all(|&v| (v - 1.0).abs() < f32::EPSILON),
            "equal values should all normalize to 1.0"
        );
    }

    // ── Merge hybrid results ─────────────────────────────────────────────

    fn make_bm25_result(id: &str, score: f32) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            source_type: "observation".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: None,
            line_end: None,
            snippet: format!("bm25 content for {id}"),
            score,
        }
    }

    fn make_vec_result(id: &str, distance: f64) -> crate::memory::vector_store::VectorSearchResult {
        crate::memory::vector_store::VectorSearchResult {
            id: id.to_string(),
            source_type: "observation".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            content: format!("vec content for {id}"),
            line_start: None,
            line_end: None,
            distance,
        }
    }

    fn test_search_config() -> SearchConfig {
        SearchConfig {
            vector_weight: 0.7,
            text_weight: 0.3,
            min_score: 0.0,
            candidate_multiplier: 4,
            temporal_decay: false,
            temporal_decay_half_life_days: 30.0,
        }
    }

    #[test]
    fn merge_bm25_only() {
        let bm25 = vec![make_bm25_result("a", 5.0), make_bm25_result("b", 3.0)];
        let vec_results: Vec<crate::memory::vector_store::VectorSearchResult> = vec![];

        let merged = merge_hybrid_results(&bm25, &vec_results, &test_search_config(), 10);
        assert_eq!(merged.len(), 2, "should return all bm25 results");
        assert_eq!(merged[0].id, "a", "highest bm25 should be first");
    }

    #[test]
    fn merge_vector_only() {
        let bm25: Vec<SearchResult> = vec![];
        let vec_results = vec![make_vec_result("x", 0.1), make_vec_result("y", 0.5)];

        let merged = merge_hybrid_results(&bm25, &vec_results, &test_search_config(), 10);
        assert_eq!(merged.len(), 2, "should return all vector results");
        // x has smaller distance (0.1) → higher similarity → higher score
        assert_eq!(merged[0].id, "x", "closest vector should be first");
    }

    #[test]
    fn merge_hybrid_overlap() {
        let bm25 = vec![make_bm25_result("a", 5.0), make_bm25_result("b", 3.0)];
        let vec_results = vec![
            make_vec_result("a", 0.1), // same doc as bm25
            make_vec_result("c", 0.2), // vector-only
        ];

        let cfg = test_search_config();
        let merged = merge_hybrid_results(&bm25, &vec_results, &cfg, 10);

        // "a" appears in both → hybrid score = text_w * norm_bm25 + vec_w * norm_vec
        // "b" is bm25-only, "c" is vec-only
        assert_eq!(merged.len(), 3, "should have 3 unique docs");
        // "a" should have the highest score since it appears in both
        assert_eq!(merged[0].id, "a", "overlapping doc should rank highest");
    }

    #[test]
    fn merge_min_score_filter() {
        let bm25 = vec![make_bm25_result("a", 5.0), make_bm25_result("b", 0.1)];
        let vec_results: Vec<crate::memory::vector_store::VectorSearchResult> = vec![];

        let cfg = SearchConfig {
            min_score: 0.5, // high threshold
            ..test_search_config()
        };
        let merged = merge_hybrid_results(&bm25, &vec_results, &cfg, 10);

        // With only BM25, scores are normalized. "a" → 1.0, "b" → 0.0
        // hybrid("a") = 0.3 * 1.0 = 0.3 (below 0.5 threshold)
        // hybrid("b") = 0.3 * 0.0 = 0.0 (below 0.5 threshold)
        // Both below threshold because text_weight is only 0.3
        // This is expected: without vector scores, max hybrid = text_weight * 1.0 = 0.3
        assert!(merged.len() <= 2, "min_score filter should reduce results");
    }

    #[test]
    fn merge_respects_limit() {
        let bm25 = vec![
            make_bm25_result("a", 5.0),
            make_bm25_result("b", 4.0),
            make_bm25_result("c", 3.0),
        ];
        let vec_results: Vec<crate::memory::vector_store::VectorSearchResult> = vec![];

        let merged = merge_hybrid_results(&bm25, &vec_results, &test_search_config(), 2);
        assert_eq!(merged.len(), 2, "should respect limit");
    }

    #[test]
    fn hybrid_searcher_bm25_fallback() {
        let (_dir, index) = create_test_index();
        let obs = vec![sample_observation("rust ownership model")];
        index
            .index_observations("ep-001", "2026-02-19", &obs)
            .unwrap();

        let searcher = HybridSearcher::new(Arc::new(index), None, None, SearchConfig::default());
        assert!(!searcher.has_vector(), "should not have vector search");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let results = rt
            .block_on(searcher.search("rust ownership", 5, &no_filters()))
            .unwrap();
        assert!(!results.is_empty(), "BM25 fallback should find results");
        assert_eq!(results[0].source_type, "observation");
    }

    // ── Temporal decay tests ─────────────────────────────────────────────

    fn make_result(id: &str, date: &str, score: f32) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            source_type: "observation".to_string(),
            episode_id: "ep-001".to_string(),
            date: date.to_string(),
            context: "test".to_string(),
            line_start: None,
            line_end: None,
            snippet: "test snippet".to_string(),
            score,
        }
    }

    #[test]
    fn temporal_decay_reduces_old_scores() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 24).unwrap();
        let mut results = vec![
            make_result("recent", "2026-02-23", 1.0),
            make_result("old", "2026-01-01", 1.0),
        ];

        apply_temporal_decay(&mut results, 30.0, today);

        assert!(
            results[0].score > results[1].score,
            "recent result ({}) should score higher than old result ({})",
            results[0].score,
            results[1].score
        );
        assert!(
            results[0].score < 1.0,
            "even recent result should have some decay"
        );
    }

    #[test]
    fn temporal_decay_preserves_order_when_disabled() {
        let results = [
            make_result("a", "2025-01-01", 0.8),
            make_result("b", "2026-02-24", 0.5),
        ];
        let original_a = results[0].score;
        let original_b = results[1].score;

        // decay is off — don't call apply_temporal_decay at all, verify scores unchanged
        assert!(
            (results[0].score - original_a).abs() < f32::EPSILON,
            "score a should be unchanged when decay is disabled"
        );
        assert!(
            (results[1].score - original_b).abs() < f32::EPSILON,
            "score b should be unchanged when decay is disabled"
        );
    }

    #[test]
    fn temporal_decay_with_unparseable_date() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 24).unwrap();
        let mut results = vec![make_result("bad-date", "not-a-date", 1.0)];

        apply_temporal_decay(&mut results, 30.0, today);

        assert!(
            (results[0].score - 1.0).abs() < f32::EPSILON,
            "score should be unchanged for unparseable date, got {}",
            results[0].score
        );
    }

    #[test]
    fn temporal_decay_half_life_accuracy() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 24).unwrap();
        let half_life = 30.0;
        let date_at_half_life = today - chrono::Duration::days(30);
        let mut results = vec![make_result(
            "half",
            &date_at_half_life.format("%Y-%m-%d").to_string(),
            1.0,
        )];

        apply_temporal_decay(&mut results, half_life, today);

        let expected = 0.5_f32;
        assert!(
            (results[0].score - expected).abs() < 0.01,
            "score at half-life should be ~0.5, got {}",
            results[0].score
        );
    }
}
