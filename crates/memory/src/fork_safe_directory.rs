//! A tantivy directory whose lock files are released explicitly.
//!
//! Tantivy's `MmapDirectory` holds each lock (the index writer lock and the
//! meta lock) as an OS file lock on an open file, and releases it by closing
//! that file. A file lock belongs to the open file description, which every
//! child process forked by this process shares until it execs. So when a
//! writer is dropped while another thread is spawning a child (a tool
//! command, an MCP server), the child's inherited copy keeps the writer lock
//! held, and the next writer fails with `LockBusy`: the operation that wanted
//! it (indexing new observations, a startup resync) fails for no reason
//! visible to the user. Unlocking explicitly before closing releases the lock
//! for every copy of the description, so it is free the moment the writer is
//! gone.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tantivy::directory::error::{
    DeleteError, LockError, OpenDirectoryError, OpenReadError, OpenWriteError,
};
use tantivy::directory::{
    Directory, DirectoryLock, FileHandle, FileSlice, Lock, MmapDirectory, WatchCallback,
    WatchHandle, WritePtr,
};

/// An `MmapDirectory` that releases its lock files explicitly when a lock is
/// dropped, so child processes spawned meanwhile never keep a lock held.
#[derive(Clone, Debug)]
pub(crate) struct ForkSafeMmapDirectory {
    inner: MmapDirectory,
    root: PathBuf,
}

impl ForkSafeMmapDirectory {
    /// Open the directory at `root`, which must already exist.
    ///
    /// # Errors
    /// Returns an error if `root` does not exist or is not a directory.
    pub(crate) fn open(root: &Path) -> Result<Self, OpenDirectoryError> {
        Ok(Self {
            inner: MmapDirectory::open(root)?,
            root: root.to_path_buf(),
        })
    }
}

/// A held lock file, unlocked explicitly before the file is closed.
struct ExplicitlyReleasedLock {
    file: File,
    path: PathBuf,
}

impl ExplicitlyReleasedLock {
    /// Lock the file at `path`, creating it when missing. A blocking lock
    /// waits for the current holder to release it; a non-blocking one fails
    /// with `LockBusy` while it is held.
    fn acquire(path: PathBuf, blocking: bool) -> Result<Self, LockError> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(LockError::wrap_io_error)?;
        if blocking {
            file.lock().map_err(LockError::wrap_io_error)?;
        } else {
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Err(LockError::LockBusy),
                Err(TryLockError::Error(err)) => return Err(LockError::wrap_io_error(err)),
            }
        }
        Ok(Self { file, path })
    }
}

impl Drop for ExplicitlyReleasedLock {
    fn drop(&mut self) {
        if let Err(err) = self.file.unlock() {
            tracing::warn!(
                error = %err,
                path = %self.path.display(),
                "failed to release a search index lock; it is released once every process holding the file closes it"
            );
        }
    }
}

impl Directory for ForkSafeMmapDirectory {
    fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
        self.inner.get_file_handle(path)
    }

    fn open_read(&self, path: &Path) -> Result<FileSlice, OpenReadError> {
        self.inner.open_read(path)
    }

    fn delete(&self, path: &Path) -> Result<(), DeleteError> {
        self.inner.delete(path)
    }

    fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
        self.inner.exists(path)
    }

    fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        self.inner.open_write(path)
    }

    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        self.inner.atomic_read(path)
    }

    fn atomic_write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        self.inner.atomic_write(path, data)
    }

    fn sync_directory(&self) -> std::io::Result<()> {
        self.inner.sync_directory()
    }

    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        let held =
            ExplicitlyReleasedLock::acquire(self.root.join(&lock.filepath), lock.is_blocking)?;
        Ok(DirectoryLock::from(Box::new(held)))
    }

    fn watch(&self, watch_callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        self.inner.watch(watch_callback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::directory::INDEX_WRITER_LOCK;

    #[test]
    fn writer_lock_is_free_once_dropped_while_a_child_holds_an_inherited_copy() {
        let dir = tempfile::tempdir().unwrap();
        let directory = ForkSafeMmapDirectory::open(dir.path()).unwrap();

        let held = ExplicitlyReleasedLock::acquire(
            dir.path().join(&INDEX_WRITER_LOCK.filepath),
            INDEX_WRITER_LOCK.is_blocking,
        )
        .unwrap();
        // A duplicate of the locked file shares its open file description,
        // as the copy a forked child inherits does until it execs.
        let inherited_copy = held.file.try_clone().unwrap();
        drop(held);

        let reacquired = directory.acquire_lock(&INDEX_WRITER_LOCK);
        assert!(
            reacquired.is_ok(),
            "the writer lock should be free once dropped, even with an inherited copy open: {:?}",
            reacquired.err()
        );
        drop(inherited_copy);
    }

    #[test]
    fn writer_lock_is_exclusive_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let directory = ForkSafeMmapDirectory::open(dir.path()).unwrap();

        let held = directory.acquire_lock(&INDEX_WRITER_LOCK).unwrap();
        assert!(matches!(
            directory.acquire_lock(&INDEX_WRITER_LOCK),
            Err(LockError::LockBusy)
        ));
        drop(held);
        assert!(directory.acquire_lock(&INDEX_WRITER_LOCK).is_ok());
    }
}
