//! The team wiki's search index: one shared service for the whole hub.
//!
//! A [`TeamWikiIndex`] owns a tantivy index (`team/.index/`) and, when an
//! embedding provider is configured, a vector store (`team/vectors.db`), both
//! holding only the pages of `team/wiki/`. It is constructed once and handed
//! out as an `Arc`; every agent's [`HybridSearcher`](crate::memory::search::HybridSearcher)
//! holds a clone of the same handle. Wiki pages are synced into the index
//! before each search, and those syncs are serialized inside the index, so
//! concurrent searches from several agents never write to it at the same time.
//!
//! Pages are embedded with the embedding model the index was opened with. With
//! no embedding provider (or a store that cannot be opened) the index is text
//! only.

use std::path::Path;
use std::sync::Arc;

use crate::config::SearchConfig;
use crate::config::paths::TeamPaths;
use crate::inference::EmbeddingProvider;
use crate::memory::search::{MemoryIndex, SearchFilters, SideOutcome, search_side};
use crate::memory::types::DocSource;
use crate::memory::vector_store::VectorStore;
use crate::memory::wiki_index::WikiIndexer;

/// A vector store and the provider that embeds pages and queries into it.
struct TeamVectors {
    store: Arc<VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
}

/// Shared search index over the team wiki (`team/wiki/`).
pub struct TeamWikiIndex {
    bm25: Arc<MemoryIndex>,
    vector: Option<TeamVectors>,
    indexer: WikiIndexer,
}

impl TeamWikiIndex {
    /// Open the team wiki index under `team`, creating it if needed.
    ///
    /// `embedding` is the provider that embeds wiki pages and queries. `None`
    /// leaves the index text only. An unreadable full-text index or vector
    /// store is discarded and rebuilt from the wiki pages, which are the source
    /// of truth. If the full-text index cannot be recreated on disk, search
    /// degrades to an in-memory index rebuilt on each start.
    ///
    /// The returned handle is meant to be cloned and shared: opening a second
    /// instance over the same directory would contend for the index's writer
    /// lock.
    ///
    /// # Errors
    /// Returns an error only if not even an in-memory index can be created.
    pub async fn open(
        team: &TeamPaths,
        embedding: Option<Arc<dyn EmbeddingProvider>>,
    ) -> anyhow::Result<Arc<Self>> {
        let bm25 = open_bm25(&team.search_index_dir())?;
        let vector = match embedding {
            Some(embedder) => open_vectors(team, embedder).await,
            None => None,
        };
        Ok(Arc::new(Self {
            bm25: Arc::new(bm25),
            vector,
            indexer: WikiIndexer::new(team),
        }))
    }

    /// Whether wiki search uses vector similarity as well as full-text.
    #[must_use]
    pub fn has_vector(&self) -> bool {
        self.vector.is_some()
    }

    /// Bring the index up to date with the files on disk.
    ///
    /// A failed sync leaves the previous documents searchable, so it is
    /// logged rather than failing the search; the next search retries it.
    async fn sync(&self) {
        let vector = self.vector.as_ref().map(|v| (&v.store, &v.embedder));
        if let Err(e) = self.indexer.sync(&self.bm25, vector).await {
            tracing::warn!(error = %format!("{e:#}"), "failed to sync team wiki pages into the search index; wiki results may be stale");
        }
    }

    /// Run the hybrid pipeline against the team wiki with the caller's
    /// `[memory.search]` settings.
    ///
    /// # Errors
    /// Returns an error if BM25 search or embedding generation fails.
    pub(crate) async fn search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilters,
        cfg: &SearchConfig,
        min_score: f32,
    ) -> anyhow::Result<SideOutcome> {
        self.sync().await;
        let wiki_filters = SearchFilters {
            source: Some(DocSource::Wiki),
            ..filters.clone()
        };
        let vector = self.vector.as_ref().map(|v| (&v.store, &v.embedder));
        search_side(
            &self.bm25,
            vector,
            cfg,
            query,
            limit,
            &wiki_filters,
            min_score,
        )
        .await
    }
}

/// Open the full-text index, discarding and recreating one that cannot be read.
fn open_bm25(index_dir: &Path) -> anyhow::Result<MemoryIndex> {
    match MemoryIndex::open_or_create(index_dir) {
        Ok(index) => Ok(index),
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "team wiki search index unreadable, rebuilding from wiki pages");
            if let Err(clear_err) = std::fs::remove_dir_all(index_dir) {
                tracing::warn!(error = %clear_err, "failed to clear unreadable team wiki search index");
            }
            match MemoryIndex::open_or_create(index_dir) {
                Ok(index) => Ok(index),
                Err(recreate_err) => {
                    tracing::error!(
                        error = %format!("{recreate_err:#}"),
                        "team wiki search index could not be recreated: wiki search degraded to an in-memory index"
                    );
                    MemoryIndex::empty()
                }
            }
        }
    }
}

/// Open the team vector store for `embedder`'s model.
///
/// Vectors from a different embedding model are meaningless to this one, so a
/// store recorded under another model is deleted and refilled by the next sync.
/// Any failure leaves the index text only.
async fn open_vectors(
    team: &TeamPaths,
    embedder: Arc<dyn EmbeddingProvider>,
) -> Option<TeamVectors> {
    let probe = match embedder.embed(&["dimension probe"]).await {
        Ok(probe) => probe,
        Err(e) => {
            tracing::warn!(error = %e, "team wiki embedding dimension probe failed; wiki search is text only");
            return None;
        }
    };
    let dim = probe.dimensions;
    let model = embedder.model_name().to_string();
    let db_path = team.vectors_db();

    let open = |path: &Path| match VectorStore::open_or_create(path, dim) {
        Ok(store) => Some(store),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "failed to open team wiki vector store; wiki search is text only");
            None
        }
    };
    let mut store = open(&db_path)?;

    let recorded = match store.embedding_model() {
        Ok(recorded) => recorded,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "failed to read team wiki vector store metadata; wiki search is text only");
            return None;
        }
    };
    if recorded.as_deref().is_some_and(|m| m != model) {
        tracing::info!(
            old_model = recorded.as_deref().unwrap_or("none"),
            new_model = model.as_str(),
            "embedding model changed, clearing team wiki vector store"
        );
        drop(store);
        remove_vector_files(&db_path);
        store = open(&db_path)?;
    }
    if let Err(e) = store.set_embedding_model(&model) {
        tracing::warn!(error = %format!("{e:#}"), "failed to record team wiki embedding model; wiki search is text only");
        return None;
    }
    tracing::info!(dim, model = model.as_str(), "team wiki vector store ready");
    Some(TeamVectors {
        store: Arc::new(store),
        embedder,
    })
}

/// Remove a `SQLite` database and its WAL sidecar files.
fn remove_vector_files(db_path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = db_path.as_os_str().to_os_string();
        name.push(suffix);
        if let Err(e) = std::fs::remove_file(&name)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, path = ?name, "failed to remove old team wiki vector store file");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PAGE: &str = "---\ntype: Project\ntitle: Grizzly Platform\ndescription: Self-hosted infrastructure repo.\n---\n\nRuns Flux on the homelab cluster.\n";

    fn write_page(team: &TeamPaths, rel: &str, content: &str) {
        let path = team.wiki_dir().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    async fn search_wiki(index: &TeamWikiIndex, query: &str) -> SideOutcome {
        index
            .search(
                query,
                10,
                &SearchFilters::default(),
                &SearchConfig::default(),
                0.0,
            )
            .await
            .unwrap()
    }

    /// Embeds text onto two axes so "flux" and "fish" pages are separable, and
    /// counts how many texts it embedded.
    struct AxisEmbedder {
        model: &'static str,
        embedded: AtomicUsize,
    }

    impl AxisEmbedder {
        fn new(model: &'static str) -> Arc<Self> {
            Arc::new(Self {
                model,
                embedded: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for AxisEmbedder {
        async fn embed(
            &self,
            texts: &[&str],
        ) -> Result<crate::inference::EmbeddingResponse, crate::inference::InferenceError> {
            self.embedded.fetch_add(texts.len(), Ordering::SeqCst);
            Ok(crate::inference::EmbeddingResponse {
                embeddings: texts
                    .iter()
                    .map(|t| {
                        let t = t.to_lowercase();
                        vec![
                            if t.contains("flux") { 1.0 } else { 0.0 },
                            if t.contains("shell") { 1.0 } else { 0.0 },
                            0.1,
                            0.1,
                        ]
                    })
                    .collect(),
                dimensions: 4,
            })
        }

        fn model_name(&self) -> &str {
            self.model
        }
    }

    #[tokio::test]
    async fn pages_are_indexed_from_the_team_wiki_with_team_prefixed_ids() {
        let dir = tempfile::tempdir().unwrap();
        let team = TeamPaths::new(dir.path().join("team"));
        write_page(&team, "index.md", "# Wiki");
        write_page(&team, "platform/grizzly.md", PAGE);
        let index = TeamWikiIndex::open(&team, None).await.unwrap();

        let outcome = search_wiki(&index, "Flux").await;
        let [hit] = outcome.results.as_slice() else {
            panic!("expected one hit, got {:?}", outcome.results);
        };
        assert_eq!(hit.id, "team/wiki/platform/grizzly.md");
        assert_eq!(hit.source_type, DocSource::Wiki);
        assert!(team.search_index_dir().is_dir(), "index lives under team/");
        assert!(search_wiki(&index, "Wiki").await.results.is_empty());
    }

    #[tokio::test]
    async fn text_only_index_degrades_without_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let team = TeamPaths::new(dir.path().join("team"));
        write_page(&team, "grizzly.md", PAGE);
        let index = TeamWikiIndex::open(&team, None).await.unwrap();

        assert!(!index.has_vector());
        let outcome = search_wiki(&index, "Flux").await;
        assert_eq!(outcome.results.len(), 1);
        assert!(!outcome.semantic, "text-only search must not claim vectors");
        assert!(
            !team.vectors_db().exists(),
            "no vector store without embeddings"
        );
    }

    #[tokio::test]
    async fn embedded_index_reports_semantic_and_reuses_vectors_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let team = TeamPaths::new(dir.path().join("team"));
        write_page(&team, "grizzly.md", PAGE);
        write_page(&team, "fish.md", "---\ntitle: Fish\n---\nlogin shell\n");
        let embedder = AxisEmbedder::new("axis-a");
        let provider: Arc<dyn EmbeddingProvider> = Arc::<AxisEmbedder>::clone(&embedder);

        let index = TeamWikiIndex::open(&team, Some(Arc::clone(&provider)))
            .await
            .unwrap();
        assert!(index.has_vector());
        let outcome = search_wiki(&index, "Flux").await;
        assert!(outcome.semantic);
        assert_eq!(
            outcome.results.first().map(|r| r.id.as_str()),
            Some("team/wiki/grizzly.md")
        );
        drop(index);

        let before = embedder.embedded.load(Ordering::SeqCst);
        let reopened = TeamWikiIndex::open(&team, Some(provider)).await.unwrap();
        let _ = search_wiki(&reopened, "Flux").await;
        // Reopening probes the dimension (1) and embeds the query (1); the
        // unchanged pages keep their stored vectors.
        assert_eq!(embedder.embedded.load(Ordering::SeqCst) - before, 2);
    }

    #[tokio::test]
    async fn changing_the_embedding_model_clears_the_vector_store() {
        let dir = tempfile::tempdir().unwrap();
        let team = TeamPaths::new(dir.path().join("team"));
        write_page(&team, "grizzly.md", PAGE);

        let first: Arc<dyn EmbeddingProvider> = AxisEmbedder::new("axis-a");
        let first_index = TeamWikiIndex::open(&team, Some(first)).await.unwrap();
        let _ = search_wiki(&first_index, "Flux").await;
        drop(first_index);

        let second = AxisEmbedder::new("axis-b");
        let second_provider: Arc<dyn EmbeddingProvider> = Arc::<AxisEmbedder>::clone(&second);
        let second_index = TeamWikiIndex::open(&team, Some(second_provider))
            .await
            .unwrap();
        let _ = search_wiki(&second_index, "Flux").await;
        // Probe + the page re-embedded under the new model + the query.
        assert_eq!(second.embedded.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn handles_to_one_index_do_not_corrupt_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let team = TeamPaths::new(dir.path().join("team"));
        for n in 0..6 {
            write_page(
                &team,
                &format!("page-{n}.md"),
                &format!("---\ntitle: Page {n}\n---\nshared marker text {n}\n"),
            );
        }
        let embedder: Arc<dyn EmbeddingProvider> = AxisEmbedder::new("axis-a");
        let first = TeamWikiIndex::open(&team, Some(embedder)).await.unwrap();
        let second = Arc::clone(&first);

        // Both handles search at once (each syncing first) while a page changes.
        let edit = team.wiki_dir().join("page-0.md");
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        let searches = async {
            let mut outcomes = Vec::new();
            for _ in 0..4 {
                let (a, b) = tokio::join!(
                    search_wiki(&first, "marker"),
                    search_wiki(&second, "marker")
                );
                outcomes.push(a);
                outcomes.push(b);
            }
            outcomes
        };
        let editing = async {
            std::fs::write(
                &edit,
                "---\ntitle: Page 0\n---\nshared marker text edited\n",
            )
            .unwrap();
            std::fs::File::options()
                .write(true)
                .open(&edit)
                .unwrap()
                .set_modified(later)
                .unwrap();
        };
        let (outcomes, ()) = tokio::join!(searches, editing);

        for outcome in &outcomes {
            assert!(
                outcome.results.len() == 6,
                "every search sees all six pages"
            );
        }
        let settled = search_wiki(&second, "edited").await;
        assert_eq!(
            settled.results.first().map(|r| r.id.as_str()),
            Some("team/wiki/page-0.md")
        );
        let from_first = search_wiki(&first, "edited").await;
        assert_eq!(from_first.results.len(), settled.results.len());
    }

    #[tokio::test]
    async fn unreadable_index_directory_is_rebuilt_from_the_wiki() {
        let dir = tempfile::tempdir().unwrap();
        let team = TeamPaths::new(dir.path().join("team"));
        write_page(&team, "grizzly.md", PAGE);
        std::fs::create_dir_all(team.search_index_dir()).unwrap();
        std::fs::write(team.search_index_dir().join("meta.json"), "not json").unwrap();

        let index = TeamWikiIndex::open(&team, None).await.unwrap();
        assert_eq!(search_wiki(&index, "Flux").await.results.len(), 1);
    }
}
