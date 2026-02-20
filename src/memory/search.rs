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

    /// Rebuild the index by walking all episode and daily log files.
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

        // Index episodes
        let episodes_dir = memory_dir.join("episodes");
        if episodes_dir.exists() {
            count += self.index_directory(&mut writer, &episodes_dir)?;
        }

        // Index daily logs (*.md files directly in memory_dir)
        count += self.index_daily_logs(&mut writer, memory_dir)?;

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

        let date = extract_date_from_path(file_path);
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

    /// Recursively index all `.md` files in a directory tree.
    fn index_directory(
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

            // Recurse into subdirectories (e.g. YYYY-MM month dirs)
            if path.is_dir() {
                count += self.index_directory(writer, &path)?;
            } else if path.extension().is_some_and(|ext| ext == "md") {
                match std::fs::read_to_string(&path) {
                    Ok(file_content) => {
                        let path_str = path.to_string_lossy();
                        let date = extract_date_from_path(&path_str);
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
                        eprintln!("warning: skipping unreadable file {}: {e}", path.display());
                    }
                }
            }
        }

        Ok(count)
    }

    /// Index daily log files (YYYY-MM-DD.md) directly in the memory directory.
    fn index_daily_logs(
        &self,
        writer: &mut IndexWriter,
        memory_dir: &Path,
    ) -> Result<usize, IronclawError> {
        let mut count = 0;
        let Ok(entries) = std::fs::read_dir(memory_dir) else {
            return Ok(0);
        };

        for entry in entries {
            let entry = entry.map_err(|e| {
                IronclawError::Memory(format!("failed to read directory entry: {e}"))
            })?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                let filename = path.file_stem().map(|s| s.to_string_lossy().to_string());
                // Only index files that look like date-named daily logs
                if filename.as_ref().is_some_and(|n| is_date_filename(n)) {
                    match std::fs::read_to_string(&path) {
                        Ok(file_content) => {
                            let path_str = path.to_string_lossy();
                            let date = filename.unwrap_or_default();
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
                            eprintln!(
                                "warning: skipping unreadable daily log {}: {e}",
                                path.display()
                            );
                        }
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

/// Extract a date string from a file path (best effort).
fn extract_date_from_path(path: &str) -> String {
    // Try to extract YYYY-MM-DD from the filename
    let filename = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    if is_date_filename(&filename) {
        filename
    } else {
        String::new()
    }
}

/// Check if a filename looks like a date (YYYY-MM-DD).
fn is_date_filename(name: &str) -> bool {
    name.len() == 10
        && name.as_bytes().get(4) == Some(&b'-')
        && name.as_bytes().get(7) == Some(&b'-')
        && name.bytes().filter(u8::is_ascii_digit).count() == 8
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
                "episodes/ep-001.md",
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

        std::fs::write(
            day_dir.join("ep-001.md"),
            "---\nid: ep-001\n---\nworkspace layout discussion",
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
        std::fs::create_dir_all(&memory_dir).unwrap();

        std::fs::write(
            memory_dir.join("2026-02-19.md"),
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
    fn is_date_filename_valid() {
        assert!(is_date_filename("2026-02-19"), "valid date");
        assert!(!is_date_filename("ep-001"), "not a date");
        assert!(!is_date_filename("observations"), "not a date");
        assert!(!is_date_filename("2026-2-19"), "wrong format");
    }

    #[test]
    fn search_results_have_scores() {
        let (_dir, index) = create_test_index();

        index
            .index_file("test.md", "rust programming language memory safety")
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
                "test.md",
                "the observer monitors token counts and fires when the threshold is exceeded",
            )
            .unwrap();

        let results = index.search("observer threshold", 5).unwrap();
        assert!(!results.is_empty(), "should have results");
        let first = results.first().unwrap();
        assert!(!first.snippet.is_empty(), "snippet should not be empty");
    }
}
