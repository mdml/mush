//! Mush domain operations and persistent store.

pub mod domain;
pub mod executor;
pub mod lock;
pub mod runner;
pub mod store;
pub mod tui;

use std::path::{Path, PathBuf};

pub use domain::{
    Agent, CheckpointDecision, DecisionOutcome, DomainError, ExecutionStatus, Loop, LoopAttempt,
    LoopReport, LoopStage, LoopStatus, Project, ReadinessStatus, RunnerCounts, RunnerReport, Task,
    TaskKind, TaskObservation, TaskStatus,
};
pub use executor::Executor;
pub use store::Store;

/// Resolve the state database from an explicit path or the conventional state root.
pub fn database_path(explicit: Option<&Path>) -> Result<PathBuf, DomainError> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(root) = std::env::var_os("MUSH_STATE_DIR") {
        return Ok(PathBuf::from(root).join("mush.sqlite"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        DomainError::Invalid("HOME is unset; pass --database or set MUSH_STATE_DIR".into())
    })?;
    Ok(PathBuf::from(home).join(".mush/mush.sqlite"))
}
