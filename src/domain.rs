use serde::Serialize;
use std::{fmt, str::FromStr};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0} not found: {1}")]
    NotFound(&'static str, i64),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Work,
    Checkpoint,
}

impl fmt::Display for TaskKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Work => "work",
            Self::Checkpoint => "checkpoint",
        })
    }
}

impl FromStr for TaskKind {
    type Err = DomainError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_named(
            value,
            "task kind",
            &[("work", Self::Work), ("checkpoint", Self::Checkpoint)],
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Running,
    Succeeded,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    Unqueued,
    Blocked,
    Ready,
    Claimed,
    Running,
    Completed,
    InterventionRequired,
}

impl fmt::Display for ReadinessStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unqueued => "unqueued",
            Self::Blocked => "blocked",
            Self::Ready => "ready",
            Self::Claimed => "claimed",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::InterventionRequired => "intervention_required",
        })
    }
}

impl FromStr for ReadinessStatus {
    type Err = DomainError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_named(
            value,
            "readiness status",
            &[
                ("unqueued", Self::Unqueued),
                ("blocked", Self::Blocked),
                ("ready", Self::Ready),
                ("claimed", Self::Claimed),
                ("running", Self::Running),
                ("completed", Self::Completed),
                ("intervention_required", Self::InterventionRequired),
            ],
        )
    }
}

impl fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Interrupted => "interrupted",
        })
    }
}

impl FromStr for ExecutionStatus {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_named(
            value,
            "execution status",
            &[
                ("running", Self::Running),
                ("succeeded", Self::Succeeded),
                ("interrupted", Self::Interrupted),
            ],
        )
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
        })
    }
}

impl FromStr for TaskStatus {
    type Err = DomainError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_named(
            value,
            "task status",
            &[("pending", Self::Pending), ("completed", Self::Completed)],
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointDecision {
    Met,
    NotMet,
    Blocked,
}

impl fmt::Display for CheckpointDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Met => "met",
            Self::NotMet => "not_met",
            Self::Blocked => "blocked",
        })
    }
}

impl FromStr for CheckpointDecision {
    type Err = DomainError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "met" => Ok(Self::Met),
            "not_met" | "not-met" => Ok(Self::NotMet),
            "blocked" => Ok(Self::Blocked),
            _ => Err(DomainError::Invalid(format!(
                "unknown checkpoint decision: {value}"
            ))),
        }
    }
}

fn parse_named<T: Copy>(value: &str, name: &str, variants: &[(&str, T)]) -> Result<T, DomainError> {
    variants
        .iter()
        .find_map(|(candidate, parsed)| (*candidate == value).then_some(*parsed))
        .ok_or_else(|| DomainError::Invalid(format!("unknown {name}: {value}")))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Agent {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub harness: String,
    pub model: String,
    pub settings: String,
    pub checkpoint: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Task {
    pub id: i64,
    pub project_id: i64,
    pub agent_id: Option<i64>,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub description: String,
    pub result: Option<String>,
    pub evidence: Option<String>,
    pub criteria: Option<String>,
    pub decision: Option<CheckpointDecision>,
    pub parent_task_id: Option<i64>,
    pub previous_task_id: Option<i64>,
    pub subject_task_id: Option<i64>,
    pub loop_id: Option<i64>,
    pub execution_status: Option<ExecutionStatus>,
    pub execution_attempt: i64,
    pub session_id: Option<String>,
    pub worktree_name: Option<String>,
    pub artifact_dir: Option<String>,
    pub execution_boot_id: Option<String>,
    pub execution_pid: Option<i64>,
    pub readiness_status: ReadinessStatus,
    pub queue_generation: i64,
    pub intervention: Option<String>,
}

/// What the runner surface reports about a database: how much queued work is
/// waiting, owned, and parked.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunnerCounts {
    pub pending: i64,
    pub claimed: i64,
    pub running: i64,
    pub parked: i64,
}

/// A runner-surface report: the durable counts plus whoever holds the serve
/// lock, which is a filesystem fact rather than a stored one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunnerReport {
    #[serde(flatten)]
    pub counts: RunnerCounts,
    pub serve_lock_holder: Option<String>,
    pub concurrency_bound: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskObservation {
    pub cursor: i64,
    pub tasks: Vec<Task>,
    pub terminal: bool,
    pub timed_out: bool,
    pub elided: bool,
}
