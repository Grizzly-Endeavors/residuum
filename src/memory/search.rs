//! Full-text BM25 search over episodes and daily logs using tantivy.

use std::path::Path;
use std::sync::Arc;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, OwnedValue, STORED, STRING, Schema, TEXT};
use tantivy::snippet::SnippetGenerator;
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

use crate::error::IronclawError;
use crate::memory::episode_store::EpisodeMeta;
use crate::models::Message;

/// Memory budget for the tantivy index writer (50 MB).
const WRITER_MEMORY_BUDGET_BYTES: usize = 50_000_000;

/// A search result from the memory index.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Path to the source file.
    pub file_path: String,
    /// Snippet of matching content (HTML with `<b>` tags).
    pub snippet: String,
    /// BM25 relevance score.
    pub score: f32,
}

/// BM25 full-text search index over memory files.
pub struct MemoryIndex {
    index: Index,
    reader: IndexReader,
    path_field: Field,
    content_field: Field,
    date_field: Field,
}

impl MemoryIndex {
    /// Open or create a tantivy index at the given directory.
    ///
    /// # Errors
    /// Returns an error if the directory is inaccessible or the index is corrupt.
    pub fn open_or_create(index_dir: &Path) -> Result<Self, IronclawError> {
        std::fs::create_dir_all(index_dir).map_err(|e| {
            IronclawError::Memory(format!(
                "failed to create search index directory at {}: {e}",
                index_dir.display()
            ))
        })?;

        let mut builder = Schema::builder();
        let path_field = builder.add_text_field("path", STRING | STORED);
        let content_field = builder.add_text_field("content", TEXT | STORED);
        let date_field = builder.add_text_field("date", STRING | STORED);
        let schema = builder.build();

        let mmap_dir = MmapDirectory::open(index_dir).map_err(|e| {
            IronclawError::Memory(format!(
                "failed to open mmap directory at {}: {e}",
                index_dir.display()
            ))
        })?;

        let index = Index::open_or_create(mmap_dir, schema).map_err(|e| {
            IronclawError::Memory(format!(
                "failed to open search index at {}: {e}",
                index_dir.display()
            ))
        })?;

        let reader: IndexReader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| IronclawError::Memory(format!("failed to create index reader: {e}")))?;

        Ok(Self {
            index,
            reader,
            path_field,
            content_field,
            date_field,
        })
    }

    /// Rebuild the index by walking all episode JSONL and daily log files.
    ///
    /// Clears the existing index and re-indexes everything.
    ///
    /// # Errors
    /// Returns an error if files cannot be read or the index cannot be written.
    pub fn rebuild(&self, memory_dir: &Path) -> Result<usize, IronclawError> {
        let mut writer = self.writer()?;

        // Clear existing documents
        writer
            .delete_all_documents()
            .map_err(|e| IronclawError::Memory(format!("failed to clear search index: {e}")))?;

        let mut count = 0;

        // Index episode JSONL files
        let episodes_dir = memory_dir.join("episodes");
        if episodes_dir.exists() {
            count += self.index_jsonl_directory(&mut writer, &episodes_dir)?;
        }

        // Index daily log markdown files
        count += self.index_daily_log_dir(&mut writer, &memory_dir.join("daily_log"))?;

        writer
            .commit()
            .map_err(|e| IronclawError::Memory(format!("failed to commit search index: {e}")))?;

        self.reader.reload().map_err(|e| {
            IronclawError::Memory(format!("failed to reload search index reader: {e}"))
        })?;

        Ok(count)
    }

    /// Index a single file by path and content.
    ///
    /// # Errors
    /// Returns an error if the writer cannot add the document.
    pub fn index_file(&self, file_path: &str, content: &str) -> Result<(), IronclawError> {
        let mut writer = self.writer()?;

        let date = extract_daily_log_date(Path::new(file_path));
        add_document(
            &mut writer,
            self.path_field,
            self.content_field,
            self.date_field,
            file_path,
            content,
            &date,
        )?;

        writer
            .commit()
            .map_err(|e| IronclawError::Memory(format!("failed to commit search index: {e}")))?;

        self.reader.reload().map_err(|e| {
            IronclawError::Memory(format!("failed to reload search index reader: {e}"))
        })?;

        Ok(())
    }

    /// Search the index with a query string.
    ///
    /// # Errors
    /// Returns an error if the query is unparseable or search fails.
    pub fn search(
        &self,
        query_str: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, IronclawError> {
        if query_str.trim().is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.content_field]);

        // Use lenient parsing to avoid errors on unusual input
        let (query, _errors) = query_parser.parse_query_lenient(query_str);

        let effective_limit = limit.max(1);

        let snippet_gen = SnippetGenerator::create(&searcher, &*query, self.content_field)
            .map_err(|e| {
                IronclawError::Memory(format!("failed to create snippet generator: {e}"))
            })?;

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(effective_limit))
            .map_err(|e| IronclawError::Memory(format!("search failed: {e}")))?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in &top_docs {
            let doc: TantivyDocument = searcher
                .doc(*doc_address)
                .map_err(|e| IronclawError::Memory(format!("failed to fetch document: {e}")))?;

            let file_path = match doc.get_first(self.path_field) {
                Some(OwnedValue::Str(s)) => s.clone(),
                Some(_) | None => String::new(),
            };

            let snippet = snippet_gen.snippet_from_doc(&doc);
            let snippet_text = if snippet.is_empty() {
                // Fall back to first 200 chars of content
                match doc.get_first(self.content_field) {
                    Some(OwnedValue::Str(s)) => {
                        let end = s.len().min(200);
                        s.get(..end).unwrap_or_default().to_string()
                    }
                    Some(_) | None => String::new(),
                }
            } else {
                snippet.to_html()
            };

            results.push(SearchResult {
                file_path,
                snippet: snippet_text,
                score: *score,
            });
        }

        Ok(results)
    }

    /// Create an index writer with a reasonable memory budget.
    fn writer(&self) -> Result<IndexWriter, IronclawError> {
        self.index
            .writer(WRITER_MEMORY_BUDGET_BYTES)
            .map_err(|e| IronclawError::Memory(format!("failed to create index writer: {e}")))
    }

    /// Recursively index all `.jsonl` episode files in a directory tree.
    ///
    /// Parses each JSONL file and concatenates meta fields with message content.
    /// Warns and skips files that cannot be parsed rather than failing the rebuild.
    fn index_jsonl_directory(
        &self,
        writer: &mut IndexWriter,
        dir: &Path,
    ) -> Result<usize, IronclawError> {
        let mut count = 0;
        let entries = std::fs::read_dir(dir).map_err(|e| {
            IronclawError::Memory(format!("failed to read directory {}: {e}", dir.display()))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                IronclawError::Memory(format!("failed to read directory entry: {e}"))
            })?;
            let path = entry.path();

            if path.is_dir() {
                count += self.index_jsonl_directory(writer, &path)?;
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                match parse_jsonl_for_index(&path) {
                    Ok((date, text)) => {
                        let path_str = path.to_string_lossy();
                        add_document(
                            writer,
                            self.path_field,
                            self.content_field,
                            self.date_field,
                            &path_str,
                            &text,
                            &date,
                        )?;
                        count += 1;
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "skipping unparseable episode file"
                        );
                    }
                }
            }
        }

        Ok(count)
    }

    /// Recursively index all `.md` files in the daily log directory.
    ///
    /// Returns `Ok(0)` if the directory does not exist.
    fn index_daily_log_dir(
        &self,
        writer: &mut IndexWriter,
        daily_log_dir: &Path,
    ) -> Result<usize, IronclawError> {
        if !daily_log_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let Ok(entries) = std::fs::read_dir(daily_log_dir) else {
            return Ok(0);
        };

        for entry in entries {
            let entry = entry.map_err(|e| {
                IronclawError::Memory(format!("failed to read directory entry: {e}"))
            })?;
            let path = entry.path();

            if path.is_dir() {
                count += self.index_daily_log_dir(writer, &path)?;
            } else if path.extension().is_some_and(|ext| ext == "md") {
                let date = extract_daily_log_date(&path);
                match std::fs::read_to_string(&path) {
                    Ok(file_content) => {
                        let path_str = path.to_string_lossy();
                        add_document(
                            writer,
                            self.path_field,
                            self.content_field,
                            self.date_field,
                            &path_str,
                            &file_content,
                            &date,
                        )?;
                        count += 1;
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "skipping unreadable daily log file"
                        );
                    }
                }
            }
        }

        Ok(count)
    }
}

/// Add a document to the index writer.
fn add_document(
    writer: &mut IndexWriter,
    path_field: Field,
    content_field: Field,
    date_field: Field,
    path: &str,
    content: &str,
    date: &str,
) -> Result<(), IronclawError> {
    let mut doc = TantivyDocument::default();
    doc.add_text(path_field, path);
    doc.add_text(content_field, content);
    doc.add_text(date_field, date);

    writer.add_document(doc).map_err(|e| {
        IronclawError::Memory(format!("failed to add document to search index: {e}"))
    })?;

    Ok(())
}

/// Parse a JSONL episode file into a `(date_string, searchable_text)` tuple.
///
/// The first line is the meta object; subsequent lines are messages.
fn parse_jsonl_for_index(path: &Path) -> Result<(String, String), IronclawError> {
    let file_content = std::fs::read_to_string(path)
        .map_err(|e| IronclawError::Memory(format!("failed to read {}: {e}", path.display())))?;

    let mut lines = file_content.lines();
    let meta_line = lines.next().ok_or_else(|| {
        IronclawError::Memory(format!("empty episode file at {}", path.display()))
    })?;

    let meta: EpisodeMeta = serde_json::from_str(meta_line).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse episode meta at {}: {e}",
            path.display()
        ))
    })?;

    let date = meta.date.to_string();

    let mut text_parts = vec![meta.id, meta.context, meta.start, meta.end];

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(msg) = serde_json::from_str::<Message>(line)
            && !msg.content.is_empty()
        {
            text_parts.push(msg.content);
        }
    }

    Ok((date, text_parts.join(" ")))
}

/// Extract a date string from a daily log file path.
///
/// Expects the structure `…/daily_log/{YYYY-MM}/daily-log-{DD}.md`.
/// Returns an empty string if the path does not match this structure.
fn extract_daily_log_date(path: &Path) -> String {
    let day = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("daily-log-"))
        .unwrap_or("");

    let month = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");

    if day.is_empty() || month.is_empty() {
        String::new()
    } else {
        format!("{month}-{day}")
    }
}

/// Create a shared `MemoryIndex` wrapped in an `Arc` for tool sharing.
///
/// # Errors
/// Returns an error if the index cannot be opened.
pub fn create_shared_index(index_dir: &Path) -> Result<Arc<MemoryIndex>, IronclawError> {
    let index = MemoryIndex::open_or_create(index_dir)?;
    Ok(Arc::new(index))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    fn create_test_index() -> (tempfile::TempDir, MemoryIndex) {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();
        (dir, index)
    }

    #[test]
    fn open_or_create_index() {
        let (_dir, _index) = create_test_index();
    }

    #[test]
    fn index_and_search_document() {
        let (_dir, index) = create_test_index();

        index
            .index_file(
                "episodes/ep-001.jsonl",
                "the quick brown fox jumps over the lazy dog",
            )
            .unwrap();

        let results = index.search("quick fox", 5).unwrap();
        assert!(!results.is_empty(), "should find matching document");
        assert!(
            results
                .first()
                .is_some_and(|r| r.file_path.contains("ep-001")),
            "should match the indexed file"
        );
    }

    #[test]
    fn search_empty_index() {
        let (_dir, index) = create_test_index();
        let results = index.search("anything", 5).unwrap();
        assert!(results.is_empty(), "empty index should return no results");
    }

    #[test]
    fn search_empty_query() {
        let (_dir, index) = create_test_index();
        let results = index.search("", 5).unwrap();
        assert!(results.is_empty(), "empty query should return no results");
    }

    #[test]
    fn rebuild_indexes_files() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        let day_dir = memory_dir.join("episodes/2026-02/19");
        std::fs::create_dir_all(&day_dir).unwrap();

        // Write a valid JSONL episode file
        std::fs::write(
            day_dir.join("ep-001.jsonl"),
            r#"{"type":"meta","id":"ep-001","date":"2026-02-19","start":"workspace layout discussion","end":"finished","context":"ironclaw"}
{"role":"user","content":"tell me about workspace layout","tool_calls":null,"tool_call_id":null}
"#,
        )
        .unwrap();

        let index_dir = memory_dir.join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();
        let count = index.rebuild(&memory_dir).unwrap();

        assert!(count >= 1, "should index at least one file");

        let results = index.search("workspace layout", 5).unwrap();
        assert!(!results.is_empty(), "should find indexed content");
    }

    #[test]
    fn rebuild_with_daily_logs() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        let log_dir = memory_dir.join("daily_log/2026-02");
        std::fs::create_dir_all(&log_dir).unwrap();

        std::fs::write(
            log_dir.join("daily-log-19.md"),
            "# 2026-02-19\n\n- **10:30** discussed kubernetes migration",
        )
        .unwrap();

        let index_dir = memory_dir.join(".index");
        let index = MemoryIndex::open_or_create(&index_dir).unwrap();
        let count = index.rebuild(&memory_dir).unwrap();

        assert_eq!(count, 1, "should index the daily log");

        let results = index.search("kubernetes", 5).unwrap();
        assert!(!results.is_empty(), "should find daily log content");
    }

    #[test]
    fn search_results_have_scores() {
        let (_dir, index) = create_test_index();

        index
            .index_file("test.jsonl", "rust programming language memory safety")
            .unwrap();

        let results = index.search("rust", 5).unwrap();
        assert!(!results.is_empty(), "should have results");
        let first = results.first().unwrap();
        assert!(first.score > 0.0, "score should be positive");
    }

    #[test]
    fn snippet_extraction() {
        let (_dir, index) = create_test_index();

        index
            .index_file(
                "test.jsonl",
                "the observer monitors token counts and fires when the threshold is exceeded",
            )
            .unwrap();

        let results = index.search("observer threshold", 5).unwrap();
        assert!(!results.is_empty(), "should have results");
        let first = results.first().unwrap();
        assert!(!first.snippet.is_empty(), "snippet should not be empty");
    }

    #[test]
    fn extract_daily_log_date_valid() {
        let path = Path::new("/memory/daily_log/2026-02/daily-log-19.md");
        assert_eq!(
            extract_daily_log_date(path),
            "2026-02-19",
            "should extract full date"
        );
    }

    #[test]
    fn extract_daily_log_date_non_matching() {
        let path = Path::new("/memory/episodes/2026-02/19/ep-001.jsonl");
        assert_eq!(
            extract_daily_log_date(path),
            "",
            "non-daily-log path should return empty string"
        );
    }
}
