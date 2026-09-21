//! Keeps the knowledge wiki's pages in the memory search index.
//!
//! Each concept page under `wiki/` is one search document, keyed by its
//! workspace-relative path (`wiki/homelab/cluster.md`) so a hit is a path the
//! agent can open directly. `index.md` and `log.md` are reserved OKF names at
//! every folder level and are not indexed.
//!
//! Pages change whenever the agent edits them, so instead of a startup-only
//! sync the searcher calls [`WikiIndexer::sync`] before every search: a walk of
//! modification times finds changed, new, and deleted pages, and only those are
//! reindexed. Embeddings are reused across restarts when a page's text is
//! unchanged.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Context;

use crate::inference::EmbeddingProvider;
use crate::memory::search::MemoryIndex;
use crate::memory::vector_store::{VectorStore, WikiVector};

/// OKF reserved file names; they are catalogs and history, not concepts.
const RESERVED_FILE_NAMES: [&str; 2] = ["index.md", "log.md"];

/// Pages embedded per provider call.
const EMBED_BATCH_SIZE: usize = 64;

/// A wiki page prepared for indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WikiPage {
    /// Workspace-relative path with `/` separators, e.g. `wiki/homelab/cluster.md`.
    pub(crate) id: String,
    /// Date of the page's last modification (YYYY-MM-DD).
    pub(crate) date: String,
    /// Title, description, and body — the text that is indexed and embedded.
    pub(crate) content: String,
}

/// Incrementally syncs `wiki/` pages into the BM25 index and vector store.
pub struct WikiIndexer {
    workspace_root: PathBuf,
    wiki_dir: PathBuf,
    /// Modification time of each page as of the last successful sync, keyed by
    /// page ID. `None` until the first sync, which replaces every wiki document.
    synced: tokio::sync::Mutex<Option<HashMap<String, SystemTime>>>,
}

impl WikiIndexer {
    /// Create an indexer for the wiki at `wiki_dir` inside `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>, wiki_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            wiki_dir: wiki_dir.into(),
            synced: tokio::sync::Mutex::new(None),
        }
    }

    /// Reindex pages that changed since the last sync and drop deleted ones.
    ///
    /// The recorded state only advances when the BM25 index and (if configured)
    /// the vector store were both updated, so a failed sync is retried in full
    /// on the next call.
    ///
    /// # Errors
    /// Returns an error if the wiki cannot be scanned, a page cannot be read,
    /// or the index, embedding provider, or vector store fails.
    pub(crate) async fn sync(
        &self,
        bm25: &Arc<MemoryIndex>,
        vector: Option<(&Arc<VectorStore>, &Arc<dyn EmbeddingProvider>)>,
    ) -> anyhow::Result<()> {
        let mut synced = self.synced.lock().await;
        let first_sync = synced.is_none();

        let workspace_root = self.workspace_root.clone();
        let wiki_dir = self.wiki_dir.clone();
        let scanned = tokio::task::spawn_blocking(move || scan_pages(&workspace_root, &wiki_dir))
            .await
            .context("wiki scan task failed")??;

        let known = synced.clone().unwrap_or_default();
        let changed: Vec<(String, PathBuf)> = scanned
            .iter()
            .filter(|(id, (_, mtime))| known.get(id.as_str()) != Some(mtime))
            .map(|(id, (path, _))| (id.clone(), path.clone()))
            .collect();
        let removed: Vec<String> = known
            .keys()
            .filter(|id| !scanned.contains_key(*id))
            .cloned()
            .collect();

        if !first_sync && changed.is_empty() && removed.is_empty() {
            return Ok(());
        }

        let pages = tokio::task::spawn_blocking(move || {
            changed
                .iter()
                .map(|(id, path)| read_page(id, path))
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .await
        .context("wiki read task failed")??;

        {
            let bm25 = Arc::clone(bm25);
            let pages = pages.clone();
            let removed = removed.clone();
            tokio::task::spawn_blocking(move || {
                bm25.replace_wiki_documents(&pages, &removed, first_sync)
            })
            .await
            .context("wiki index task failed")??;
        }

        if let Some((store, embedder)) = vector {
            sync_vectors(store, embedder.as_ref(), &pages, &removed, &scanned).await?;
        }

        tracing::debug!(
            reindexed = pages.len(),
            removed = removed.len(),
            first_sync,
            "synced wiki pages into the search index"
        );
        *synced = Some(
            scanned
                .into_iter()
                .map(|(id, (_, mtime))| (id, mtime))
                .collect(),
        );
        Ok(())
    }
}

/// Embed pages whose text differs from what the vector store holds, and delete
/// vectors for pages that no longer exist.
async fn sync_vectors(
    store: &Arc<VectorStore>,
    embedder: &dyn EmbeddingProvider,
    pages: &[WikiPage],
    removed: &[String],
    scanned: &HashMap<String, (PathBuf, SystemTime)>,
) -> anyhow::Result<()> {
    let stored = {
        let store = Arc::clone(store);
        tokio::task::spawn_blocking(move || store.wiki_page_contents())
            .await
            .context("wiki vector read task failed")??
    };

    let to_embed: Vec<&WikiPage> = pages
        .iter()
        .filter(|p| stored.get(&p.id) != Some(&p.content))
        .collect();

    let mut new_vectors: Vec<(&WikiPage, Vec<f32>)> = Vec::with_capacity(to_embed.len());
    for batch in to_embed.chunks(EMBED_BATCH_SIZE) {
        let texts: Vec<&str> = batch.iter().map(|p| p.content.as_str()).collect();
        let response = embedder
            .embed(&texts)
            .await
            .context("failed to embed wiki pages")?;
        if response.embeddings.len() != batch.len() {
            anyhow::bail!(
                "embedding provider returned {} embeddings for {} wiki pages",
                response.embeddings.len(),
                batch.len()
            );
        }
        new_vectors.extend(batch.iter().copied().zip(response.embeddings));
    }

    // Vectors for deleted pages, plus any stored for pages that disappeared
    // while no sync state was held (e.g. across a restart).
    let stale: Vec<String> = stored
        .keys()
        .filter(|id| !scanned.contains_key(*id))
        .chain(removed.iter())
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let rows: Vec<(String, String, String, Vec<f32>)> = new_vectors
        .into_iter()
        .map(|(p, emb)| (p.id.clone(), p.date.clone(), p.content.clone(), emb))
        .collect();
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || {
        let vectors: Vec<WikiVector<'_>> = rows
            .iter()
            .map(|(page_id, date, content, embedding)| WikiVector {
                page_id,
                date,
                content,
                embedding,
            })
            .collect();
        store.upsert_wiki_pages(&vectors)?;
        store.delete_by_doc_ids(&stale)
    })
    .await
    .context("wiki vector write task failed")?
}

/// Walk `wiki_dir` for concept pages, returning each page's path and mtime by ID.
fn scan_pages(
    workspace_root: &Path,
    wiki_dir: &Path,
) -> anyhow::Result<HashMap<String, (PathBuf, SystemTime)>> {
    let mut pages = HashMap::new();
    if !wiki_dir.exists() {
        return Ok(pages);
    }
    let mut dirs = vec![wiki_dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let entries = std::fs::read_dir(&dir)
            .with_context(|| format!("failed to read wiki directory {}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("failed to stat {}", path.display()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            if file_type.is_dir() {
                dirs.push(path);
            } else if file_type.is_file()
                && name.ends_with(".md")
                && !RESERVED_FILE_NAMES.contains(&name.as_ref())
            {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .with_context(|| format!("failed to read mtime of {}", path.display()))?;
                pages.insert(page_id(workspace_root, &path), (path, mtime));
            }
        }
    }
    Ok(pages)
}

/// Workspace-relative page path with `/` separators on every platform.
fn page_id(workspace_root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(workspace_root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Read a page from disk and build its indexed text.
fn read_page(id: &str, path: &Path) -> anyhow::Result<WikiPage> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read wiki page {}", path.display()))?;
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .with_context(|| format!("failed to read mtime of {}", path.display()))?;
    let date = chrono::DateTime::<chrono::Local>::from(modified)
        .format("%Y-%m-%d")
        .to_string();
    Ok(WikiPage {
        id: id.to_string(),
        date,
        content: page_search_text(id, &raw),
    })
}

/// The text a page is indexed by: title, description, then the body.
///
/// Frontmatter keys other than `title` and `description` (sources, dates,
/// status) are bookkeeping and would only add noise to matching. A page with
/// missing or unparseable frontmatter is indexed by its full text.
fn page_search_text(id: &str, raw: &str) -> String {
    let Some((frontmatter, body)) = split_frontmatter(raw) else {
        return raw.trim().to_string();
    };
    let fields: serde_yaml_ng::Value = match serde_yaml_ng::from_str(frontmatter) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(page = id, error = %e, "wiki page frontmatter is not valid YAML; indexing its full text");
            return raw.trim().to_string();
        }
    };
    let field = |key: &str| {
        fields
            .get(key)
            .and_then(serde_yaml_ng::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    [field("title"), field("description"), Some(body.trim())]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Split a leading `---`-delimited YAML block from the body.
fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let rest = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((rest.get(..offset)?, rest.get(offset + line.len()..)?));
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::search::SearchFilters;
    use crate::memory::types::DocSource;

    const PAGE: &str = "---\ntype: Project\ntitle: Grizzly Platform\ndescription: Self-hosted infrastructure repo.\nsources:\n  - resource: episode:ep-042\n---\n\n# Grizzly Platform\n\nRuns Flux on the homelab cluster.\n";

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn wiki_filter() -> SearchFilters {
        SearchFilters {
            source: Some(DocSource::Wiki),
            ..SearchFilters::default()
        }
    }

    /// Bump a file's mtime so the mtime comparison sees it as changed even
    /// when the rewrite lands within the filesystem's timestamp resolution.
    fn touch_later(path: &Path) {
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    #[test]
    fn search_text_uses_title_description_and_body() {
        let text = page_search_text("wiki/p.md", PAGE);
        assert!(text.starts_with("Grizzly Platform\n\nSelf-hosted infrastructure repo."));
        assert!(text.contains("Runs Flux on the homelab cluster."));
        assert!(
            !text.contains("episode:ep-042"),
            "bookkeeping frontmatter should not be indexed"
        );
    }

    #[test]
    fn search_text_falls_back_to_full_text_without_frontmatter() {
        assert_eq!(
            page_search_text("wiki/p.md", "# Plain\n\nbody\n"),
            "# Plain\n\nbody"
        );
        let unterminated = "---\ntitle: x\nbody without a closing fence";
        assert_eq!(page_search_text("wiki/p.md", unterminated), unterminated);
    }

    #[test]
    fn scan_skips_reserved_and_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "wiki/index.md", "# Wiki");
        write(root, "wiki/log.md", "# Log");
        write(root, "wiki/homelab/index.md", "# Homelab");
        write(root, "wiki/homelab/cluster.md", PAGE);
        write(root, "wiki/notes.txt", "not markdown");
        write(root, "wiki/.drafts/hidden.md", PAGE);

        let pages = scan_pages(root, &root.join("wiki")).unwrap();
        let ids: Vec<&str> = pages.keys().map(String::as_str).collect();
        assert_eq!(ids, ["wiki/homelab/cluster.md"]);
    }

    #[tokio::test]
    async fn sync_indexes_updates_and_removes_pages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "wiki/index.md", "# Wiki");
        write(root, "wiki/grizzly-platform.md", PAGE);
        write(
            root,
            "wiki/tools/fish.md",
            "---\ntype: Tool\ntitle: Fish shell\n---\nThe user's login shell.\n",
        );

        let bm25 = Arc::new(MemoryIndex::empty().unwrap());
        let indexer = WikiIndexer::new(root, root.join("wiki"));
        indexer.sync(&bm25, None).await.unwrap();

        let hits = bm25.search("Flux", 5, &wiki_filter()).unwrap();
        let [hit] = hits.as_slice() else {
            panic!("page body should be searchable as one hit, got {hits:?}");
        };
        assert_eq!(hit.id, "wiki/grizzly-platform.md");
        assert_eq!(hit.source_type, DocSource::Wiki);
        assert!(
            bm25.search("Wiki", 5, &wiki_filter()).unwrap().is_empty(),
            "index.md should not be indexed"
        );

        // Edit one page, delete the other.
        let page = root.join("wiki/grizzly-platform.md");
        std::fs::write(&page, PAGE.replace("Flux", "ArgoCD")).unwrap();
        touch_later(&page);
        std::fs::remove_file(root.join("wiki/tools/fish.md")).unwrap();
        indexer.sync(&bm25, None).await.unwrap();

        assert!(bm25.search("Flux", 5, &wiki_filter()).unwrap().is_empty());
        let edited = bm25.search("ArgoCD", 5, &wiki_filter()).unwrap();
        assert_eq!(edited.len(), 1, "edited page should be reindexed once");
        assert!(
            bm25.search("shell", 5, &wiki_filter()).unwrap().is_empty(),
            "deleted page should be removed"
        );
    }

    #[tokio::test]
    async fn first_sync_drops_wiki_documents_left_from_a_previous_run() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "wiki/gone.md",
            "---\ntitle: Gone\n---\nobsolete page\n",
        );

        let bm25 = Arc::new(MemoryIndex::empty().unwrap());
        WikiIndexer::new(root, root.join("wiki"))
            .sync(&bm25, None)
            .await
            .unwrap();
        std::fs::remove_file(root.join("wiki/gone.md")).unwrap();

        // A fresh indexer (as after a restart) holds no state about "gone.md".
        WikiIndexer::new(root, root.join("wiki"))
            .sync(&bm25, None)
            .await
            .unwrap();
        assert!(
            bm25.search("obsolete", 5, &wiki_filter())
                .unwrap()
                .is_empty()
        );
    }

    /// Embedder that counts how many texts it has embedded.
    struct CountingEmbedder(std::sync::atomic::AtomicUsize);

    #[async_trait::async_trait]
    impl EmbeddingProvider for CountingEmbedder {
        async fn embed(
            &self,
            texts: &[&str],
        ) -> Result<crate::inference::EmbeddingResponse, crate::inference::InferenceError> {
            self.0
                .fetch_add(texts.len(), std::sync::atomic::Ordering::SeqCst);
            Ok(crate::inference::EmbeddingResponse {
                embeddings: texts.iter().map(|_| vec![0.1, 0.2, 0.3, 0.4]).collect(),
                dimensions: 4,
            })
        }

        fn model_name(&self) -> &'static str {
            "counting-embedder"
        }
    }

    #[tokio::test]
    async fn vectors_are_reused_across_restarts_and_follow_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "wiki/grizzly-platform.md", PAGE);
        write(
            root,
            "wiki/fish.md",
            "---\ntitle: Fish shell\n---\nlogin shell\n",
        );

        let bm25 = Arc::new(MemoryIndex::empty().unwrap());
        let store = Arc::new(VectorStore::open_or_create(&root.join("vectors.db"), 4).unwrap());
        let counter = Arc::new(CountingEmbedder(std::sync::atomic::AtomicUsize::new(0)));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::<CountingEmbedder>::clone(&counter);
        let embed_count = || counter.0.load(std::sync::atomic::Ordering::SeqCst);

        WikiIndexer::new(root, root.join("wiki"))
            .sync(&bm25, Some((&store, &embedder)))
            .await
            .unwrap();
        assert_eq!(embed_count(), 2, "first sync embeds every page");

        // A fresh indexer, as after a restart: unchanged pages keep their vectors.
        let indexer = WikiIndexer::new(root, root.join("wiki"));
        indexer
            .sync(&bm25, Some((&store, &embedder)))
            .await
            .unwrap();
        assert_eq!(embed_count(), 2, "unchanged pages must not be re-embedded");

        let page = root.join("wiki/fish.md");
        std::fs::write(
            &page,
            "---\ntitle: Fish shell\n---\nlogin shell, with abbreviations\n",
        )
        .unwrap();
        touch_later(&page);
        std::fs::remove_file(root.join("wiki/grizzly-platform.md")).unwrap();
        indexer
            .sync(&bm25, Some((&store, &embedder)))
            .await
            .unwrap();

        assert_eq!(embed_count(), 3, "only the edited page is re-embedded");
        let contents = store.wiki_page_contents().unwrap();
        assert!(
            !contents.contains_key("wiki/grizzly-platform.md"),
            "deleted page's vector should be removed"
        );
        assert!(
            contents
                .get("wiki/fish.md")
                .is_some_and(|c| c.contains("abbreviations")),
            "edited page's vector should hold the new text"
        );
    }

    #[tokio::test]
    async fn sync_without_wiki_dir_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let bm25 = Arc::new(MemoryIndex::empty().unwrap());
        WikiIndexer::new(dir.path(), dir.path().join("wiki"))
            .sync(&bm25, None)
            .await
            .unwrap();
    }
}
