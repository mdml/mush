//! Advisory file locks, the only primitive Mush uses to decide whether an owner
//! is alive.
//!
//! An execution holds an exclusive advisory lock on its task's lock file for the
//! whole of its life. The kernel releases the lock when the process ends for any
//! reason, including a crash or a kill, so liveness is a non-blocking lock
//! attempt with an exact answer rather than a reconstruction from process
//! identity. There is no PID reuse hazard, no ambiguity between an owner and its
//! child, and no grace period.
//!
//! The locks come from the standard library (`File::try_lock`), so no dependency
//! carries this invariant.

use crate::DomainError;
use std::{
    fs::{File, OpenOptions, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Where the lock files for one database live.
fn lock_directory(database: &Path) -> PathBuf {
    database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("locks")
}

/// The per-task execution lock file.
pub fn task_lock_path(database: &Path, task_id: i64) -> PathBuf {
    lock_directory(database).join(format!("task-{task_id}.lock"))
}

/// The one serve lock file for a database. It bounds `runner serve` to one
/// process per database and gates schema migration.
pub fn serve_lock_path(database: &Path) -> PathBuf {
    lock_directory(database).join("serve.lock")
}

/// An exclusive advisory lock held for as long as this value lives. Dropping it
/// releases the lock, and so does the process ending for any reason.
#[derive(Debug)]
pub struct FileLock {
    file: File,
    path: PathBuf,
}

impl FileLock {
    /// Take the lock without blocking. `Ok(None)` means another process holds
    /// it; that is an answer, not a failure.
    pub fn try_acquire(path: &Path) -> Result<Option<Self>, DomainError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self {
                file,
                path: path.to_path_buf(),
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }

    /// Record who holds the lock so a refusal can name the holder. The content
    /// is diagnostic only; the lock itself is the decision input.
    pub fn describe(&mut self, holder: &str) -> Result<(), DomainError> {
        self.file.set_len(0)?;
        self.file.write_all(holder.as_bytes())?;
        self.file.sync_all()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FileLock {
    /// Release the lock explicitly rather than by closing the file.
    ///
    /// An advisory lock belongs to the open file description, not to the
    /// descriptor, so a descriptor duplicated into a child during the window
    /// between fork and exec shares the same lock, and closing one descriptor
    /// releases nothing until the last one closes. A process that spawns a
    /// harness while holding a lock would therefore appear to hold that lock
    /// after dropping it, for as long as the child took to reach `exec`.
    /// Unlocking the description releases it at once, however many descriptors
    /// refer to it.
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Whether a lock is currently held, answered by trying to take it. Taking and
/// immediately releasing it is safe: a lock nobody holds carries no state.
pub fn is_held(path: &Path) -> Result<bool, DomainError> {
    Ok(FileLock::try_acquire(path)?.is_none())
}

/// Whether a task has a live execution.
pub fn task_is_live(database: &Path, task_id: i64) -> Result<bool, DomainError> {
    is_held(&task_lock_path(database, task_id))
}

/// The recorded description of whoever holds the serve lock, or `None` when the
/// lock is free. A held lock with unreadable content still reports a holder,
/// because the lock, not the description, is the fact.
pub fn serve_lock_holder(database: &Path) -> Result<Option<String>, DomainError> {
    let path = serve_lock_path(database);
    if !is_held(&path)? {
        return Ok(None);
    }
    let mut description = String::new();
    if let Ok(mut file) = File::open(&path) {
        let _ = file.read_to_string(&mut description);
    }
    let description = description.trim();
    Ok(Some(if description.is_empty() {
        "an unidentified process".to_owned()
    } else {
        description.to_owned()
    }))
}

/// Take the serve lock for this process, describing the holder.
pub fn acquire_serve_lock(database: &Path, role: &str) -> Result<Option<FileLock>, DomainError> {
    let Some(mut lock) = FileLock::try_acquire(&serve_lock_path(database))? else {
        return Ok(None);
    };
    lock.describe(&format!("{role} (pid {})", std::process::id()))?;
    Ok(Some(lock))
}
