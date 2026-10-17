//! Test fixtures for code that builds session resources. Behind
//! `test-support`, and compiled for this crate's own tests.

use std::sync::Arc;

use residuum_config::WorkspaceLayout;
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::Observer;
use residuum_memory::reflector::Reflector;
use residuum_memory::search::MemoryIndex;

/// A layout, observer, and merge writer for a session's `SubAgentResources`,
/// backed by a temporary directory the layout keeps alive. `Observer::disabled`
/// and a disabled reflector mean neither ever fires unless a test configures
/// otherwise.
///
/// # Panics
///
/// Panics when the temporary directory or its search index cannot be created.
#[must_use]
#[expect(
    clippy::unwrap_used,
    reason = "test-support helper: a fixture that cannot be built fails the test that asked for it"
)]
pub fn test_memory_extras() -> (WorkspaceLayout, Arc<Observer>, Arc<MemoryMergeWriter>) {
    let dir = tempfile::tempdir().unwrap();
    let layout = WorkspaceLayout::new(dir.path()).with_tempdir_guard(dir);
    let search_index = Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
    let reflector = Reflector::disabled(chrono_tz::UTC);
    let merge_writer = Arc::new(MemoryMergeWriter::new(
        reflector,
        layout.clone(),
        search_index,
        None,
        None,
    ));
    let observer = Arc::new(Observer::disabled(chrono_tz::UTC));
    (layout, observer, merge_writer)
}
