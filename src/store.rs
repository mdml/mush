mod delivery;
mod execution;
mod graph;
mod observation;
mod query;
mod recovery;
mod schema;
mod tasks;
mod workflow;

use crate::domain::{
    Agent, CheckpointDecision, DomainError, ExecutionStatus, Project, ReadinessStatus,
    RunnerCounts, Task, TaskKind, TaskObservation, TaskStatus,
};
use observation::*;
use query::*;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
};
use workflow::*;

pub struct Store {
    connection: Connection,
    path: PathBuf,
}

pub struct AgentRegistration<'a> {
    pub project_id: i64,
    pub name: &'a str,
    pub harness: &'a str,
    pub model: &'a str,
    pub settings: &'a str,
    pub checkpoint: bool,
}

pub struct WorkTaskRequest<'a> {
    pub project_id: i64,
    pub agent_id: i64,
    pub description: &'a str,
    pub parent_task_id: Option<i64>,
}

pub struct LaunchClaim<'a> {
    pub task_id: i64,
    pub runner_id: &'a str,
    pub runner_boot_id: &'a str,
    pub runner_pid: u32,
}

pub struct ExecutionStart<'a> {
    pub task_id: i64,
    pub session_id: Option<&'a str>,
    pub worktree_name: Option<&'a str>,
    pub artifact_dir: &'a Path,
    pub boot_id: &'a str,
    pub claimed_runner_id: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub struct ExecutionOwner {
    pub task_id: i64,
    pub execution_attempt: i64,
}

pub struct WorkExecutionResult<'a> {
    pub owner: ExecutionOwner,
    pub result: &'a str,
    pub evidence: &'a str,
}

/// The schema this binary writes and reads.
const SCHEMA_VERSION: i64 = 11;

/// The oldest binary permitted to open a version 11 database. A binary older
/// than the version a database records refuses rather than opening, so the
/// pre-M4 hazard where an old binary silently re-stamps `user_version` cannot
/// recur.
const MIN_BINARY_VERSION: &str = "0.1.0";

/// The last shipped schema, and therefore the only older input the single
/// migration to version 11 accepts. Versions 4 through 10 were never released.
const LAST_SHIPPED_SCHEMA_VERSION: i64 = 3;

const MAX_PREREQUISITES: i64 = 8;
const MAX_DIAGNOSTIC_BYTES: usize = 1024;
const MAX_FIELD_BYTES: usize = 4096;
const MAX_OBSERVATION_TEXT_BYTES: usize = 32 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 1024;
