mod harness;

use crate::{DomainError, ExecutionStatus, Store, Task, TaskKind};
use harness::{Harness, Invocation};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::OnceLock,
};

/// Validate agent settings at registration time so configuration defects
/// surface when the agent is registered, not when a task first executes. An
/// executable checkpoint agent must carry a non-empty review prompt; a harness
/// Mush cannot execute (for example a manual human reviewer) carries no such
/// requirement.
pub fn validate_agent_registration(
    harness: &str,
    settings: &str,
    checkpoint: bool,
) -> Result<(), DomainError> {
    harness::validate_registration(harness, settings, checkpoint)
}

/// Whether Mush can execute agents registered on this harness. A harness name
/// outside this set registers fine — it names a reviewer Mush cannot launch,
/// such as a manual human reviewer — but its tasks cannot be queued or run.
pub fn harness_is_executable(harness: &str) -> bool {
    harness::is_executable(harness)
}

/// Take a task's execution lock, or refuse because a live execution holds it.
/// The lock is taken before the execution claim commits and released by the
/// kernel when this process ends for any reason.
pub fn acquire_execution_lock(
    database: &Path,
    task_id: i64,
) -> Result<crate::lock::FileLock, DomainError> {
    let path = crate::lock::task_lock_path(database, task_id);
    let mut lock = crate::lock::FileLock::try_acquire(&path)?.ok_or_else(|| {
        DomainError::Invalid(format!(
            "task {task_id} is already executing: a live execution holds {}",
            path.display()
        ))
    })?;
    lock.describe(&format!(
        "execution of task {task_id} (pid {}, boot {})",
        std::process::id(),
        boot_id()
    ))?;
    Ok(lock)
}

/// What a caller may vary about one execution. The human front door carries
/// these; a worker claiming its own task leaves them at their defaults.
#[derive(Debug, Default, Clone, Copy)]
pub struct RunOptions<'a> {
    /// The worktree name to use when the task does not already have one.
    pub worktree: Option<&'a str>,
    /// Abandon any persisted harness session and start a fresh one.
    pub restart_session: bool,
    /// Follow-up context, which only a checkpoint execution accepts.
    pub prompt_override: Option<&'a str>,
}

/// A run whose caller already holds the task execution lock.
pub struct LockedRun<'a> {
    pub task_id: i64,
    pub options: &'a RunOptions<'a>,
    pub runner_id: Option<&'a str>,
    pub execution_lock: &'a crate::lock::FileLock,
}

pub struct Executor {
    artifacts_root: PathBuf,
}

impl Executor {
    pub fn new(database: &Path) -> Self {
        let root = database.parent().unwrap_or_else(|| Path::new("."));
        Self {
            artifacts_root: root.join("artifacts"),
        }
    }

    /// Run a task this process does not yet hold, acquiring its execution lock
    /// first and claiming no launch delivery. The command surfaces go through
    /// [`crate::runner`] instead, because they decide from durable state
    /// whether the execution also owes the queue its claim.
    pub fn run(
        &self,
        store: &mut Store,
        task_id: i64,
        options: &RunOptions<'_>,
    ) -> Result<Task, DomainError> {
        let lock = acquire_execution_lock(store.database_path(), task_id)?;
        self.run_locked(
            store,
            LockedRun {
                task_id,
                options,
                runner_id: None,
                execution_lock: &lock,
            },
        )
    }

    /// Run a task whose execution lock the caller already holds.
    ///
    /// `runner_id` is `Some` when this execution owns the task's claimed launch
    /// delivery. That is a fact about the task's durable readiness rather than
    /// about which command the user typed: queued work has a delivery to claim
    /// and readiness transitions to record, and unqueued work has neither.
    pub fn run_locked(
        &self,
        store: &mut Store,
        request: LockedRun<'_>,
    ) -> Result<Task, DomainError> {
        let _execution_lock = request.execution_lock;
        let prepared = self.prepare(store, &request)?;
        let outcome = prepared.execute(store);
        prepared.record_failure(store, outcome)
    }

    fn prepare(
        &self,
        store: &mut Store,
        request: &LockedRun<'_>,
    ) -> Result<PreparedExecution, DomainError> {
        PreparedExecution::new(store, &self.artifacts_root, request)
    }
}

struct PreparedExecution {
    task_id: i64,
    task_kind: TaskKind,
    harness: Harness,
    harness_version: String,
    project_path: PathBuf,
    worktree_root: PathBuf,
    reuse_worktree: bool,
    resuming: bool,
    session_id: Option<String>,
    worktree_name: Option<String>,
    prompt: String,
    artifact_dir: PathBuf,
    running: Task,
    runner_owned: bool,
    prompt_overridden: bool,
}

impl PreparedExecution {
    fn new(
        store: &mut Store,
        artifacts_root: &Path,
        request: &LockedRun<'_>,
    ) -> Result<Self, DomainError> {
        let initial = execution_task(store, request.task_id)?;
        let agent_id = initial
            .agent_id
            .ok_or_else(|| DomainError::Invalid("task has no assigned agent".into()))?;
        let harness = Harness::from_agent(&store.agent(agent_id)?)?;
        let harness_version = harness.resolve_version()?;
        let project_path = PathBuf::from(store.project(initial.project_id)?.path);
        let worktree_root = project_path.join(harness.worktree_root());
        let reuse_worktree = reuse_worktree(&initial, &worktree_root, request.options);
        let resuming = initial.session_id.is_some() && !request.options.restart_session;
        let session_id = execution_session(store, &initial, &harness, request.options)?;
        let worktree_name = execution_worktree(&initial, request.task_id, request.options.worktree);
        let prompt = execution_prompt(&initial, &harness, request.options.prompt_override)?;
        let artifact_dir = artifacts_root.join(format!("task-{}", request.task_id));
        fs::create_dir_all(&artifact_dir)?;
        write_once(&artifact_dir.join("prompt.md"), prompt.as_bytes())?;
        let boot_id = boot_id();
        let running = store.begin_execution_owned(crate::store::ExecutionStart {
            task_id: request.task_id,
            session_id: session_id.as_deref(),
            worktree_name: worktree_name.as_deref(),
            artifact_dir: &artifact_dir,
            boot_id: &boot_id,
            claimed_runner_id: request.runner_id,
        })?;
        Ok(Self {
            task_id: request.task_id,
            task_kind: initial.kind,
            harness,
            harness_version,
            project_path,
            worktree_root,
            reuse_worktree,
            resuming,
            session_id,
            worktree_name,
            prompt,
            artifact_dir,
            running,
            runner_owned: request.runner_id.is_some(),
            prompt_overridden: request.options.prompt_override.is_some(),
        })
    }

    fn execute(&self, store: &mut Store) -> Result<Task, DomainError> {
        let attempt = self.running.execution_attempt;
        if self.prompt_overridden {
            write_once(
                &self.artifact_dir.join(format!("prompt-{attempt}.md")),
                self.prompt.as_bytes(),
            )?;
        }
        let paths = AttemptPaths::new(&self.artifact_dir, attempt);
        let cwd = self.working_directory()?;
        let arguments = self.harness.arguments(&Invocation {
            session_id: self.session_id.as_deref(),
            worktree: (!self.reuse_worktree)
                .then_some(self.worktree_name.as_deref())
                .flatten(),
            resuming: self.resuming,
        });
        self.write_launch(&paths, &cwd, &arguments)?;
        let mut child = self.spawn(&paths, &cwd, &arguments)?;
        if let Err(error) = store.record_execution_pid(self.owner(attempt), child.id()) {
            terminate_child(&mut child);
            return Err(error);
        }
        let streamed_session = self.exchange_prompt(store, &paths, &mut child)?;
        ensure_child_succeeded(&mut child, &self.harness, &paths.stderr)?;
        let result = self.harness.parse_result(&paths.stdout)?;
        let evidence = self.evidence(EvidenceInput {
            attempt,
            paths: &paths,
            streamed_session: streamed_session.as_deref(),
            result: &result,
        });
        self.finish(
            store,
            HarnessOutput {
                attempt,
                result,
                evidence,
            },
        )
    }

    fn working_directory(&self) -> Result<PathBuf, DomainError> {
        if self.task_kind != TaskKind::Work {
            return Ok(self.project_path.clone());
        }
        let name = self.worktree_name.as_deref().expect("work task worktree");
        let path = self.worktree_root.join(name);
        if self.resuming || self.reuse_worktree {
            if !path.is_dir() {
                return Err(DomainError::Invalid(format!(
                    "cannot resume: worktree is missing: {}",
                    path.display()
                )));
            }
            return Ok(path);
        }
        if self.harness.mush_owns_worktree() {
            add_git_worktree(&self.project_path, &path)?;
            return Ok(path);
        }
        Ok(self.project_path.clone())
    }

    fn write_launch(
        &self,
        paths: &AttemptPaths,
        cwd: &Path,
        arguments: &[String],
    ) -> Result<(), DomainError> {
        let launch = serde_json::json!({
            "executable": self.harness.executable(),
            "version": self.harness_version,
            "arguments": arguments,
            "cwd": cwd,
            "session_id": self.session_id,
            "attempt": self.running.execution_attempt,
        });
        fs::write(&paths.launch, serde_json::to_vec_pretty(&launch)?)?;
        Ok(())
    }

    fn spawn(
        &self,
        paths: &AttemptPaths,
        cwd: &Path,
        arguments: &[String],
    ) -> Result<Child, DomainError> {
        let mut command = Command::new(self.harness.executable());
        command
            .args(arguments)
            .current_dir(cwd)
            .env("MUSH_TASK_ID", self.task_id.to_string())
            .stdin(Stdio::piped())
            .stderr(Stdio::from(File::create(&paths.stderr)?));
        if self.harness.captures_session_from_stream() {
            command.stdout(Stdio::piped());
        } else {
            command.stdout(Stdio::from(File::create(&paths.stdout)?));
        }
        command.spawn().map_err(|error| {
            DomainError::Invalid(format!(
                "failed to start {}: {error}",
                self.harness.display_name()
            ))
        })
    }

    fn exchange_prompt(
        &self,
        store: &Store,
        paths: &AttemptPaths,
        child: &mut Child,
    ) -> Result<Option<String>, DomainError> {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            DomainError::Invalid(format!(
                "{} stdin is unavailable",
                self.harness.display_name()
            ))
        })?;
        let Some(session_field) = self.harness.streamed_session_field() else {
            stdin.write_all(self.prompt.as_bytes())?;
            return Ok(None);
        };
        let stdout = child.stdout.take().ok_or_else(|| {
            DomainError::Invalid(format!(
                "{} stdout is unavailable",
                self.harness.display_name()
            ))
        })?;
        let prompt = self.prompt.clone().into_bytes();
        let writer = std::thread::spawn(move || stdin.write_all(&prompt));
        let session = persist_streamed_session(
            store,
            StreamCapture {
                task_id: self.task_id,
                execution_attempt: self.running.execution_attempt,
                stream: stdout,
                transcript: &paths.stdout,
                session_field,
            },
        )?;
        writer
            .join()
            .map_err(|_| DomainError::Invalid("prompt writer thread panicked".into()))??;
        Ok(session)
    }

    fn evidence(&self, input: EvidenceInput<'_>) -> String {
        let session = self
            .session_id
            .as_deref()
            .or(input.streamed_session)
            .unwrap_or("unknown");
        format!(
            "## {} execution\n\n- version: `{}`\n- session: `{session}`\n- attempt: {}\n- output: `{}`\n- errors: `{}`\n\n## Result\n\n{}",
            self.harness.display_name(),
            self.harness_version,
            input.attempt,
            input.paths.stdout.display(),
            input.paths.stderr.display(),
            input.result
        )
    }

    fn finish(&self, store: &mut Store, output: HarnessOutput) -> Result<Task, DomainError> {
        match self.task_kind {
            TaskKind::Work => store.finish_work_execution(crate::store::WorkExecutionResult {
                owner: self.owner(output.attempt),
                result: &output.result,
                evidence: &output.evidence,
            }),
            TaskKind::Checkpoint => {
                store.finish_checkpoint_execution(self.owner(output.attempt), &output.evidence)
            }
        }
    }

    fn record_failure(
        &self,
        store: &mut Store,
        outcome: Result<Task, DomainError>,
    ) -> Result<Task, DomainError> {
        let Err(error) = &outcome else {
            return outcome;
        };
        let diagnostic = (!self.runner_owned
            && self.running.readiness_status != crate::ReadinessStatus::Unqueued)
            .then(|| format!("foreground execution failed: {error}; inspect task artifacts and run task recover to relaunch"));
        if let Err(cleanup_error) = store.interrupt_execution_owned_with_intervention(
            self.owner(self.running.execution_attempt),
            diagnostic.as_deref(),
        ) {
            eprintln!(
                "warning: could not persist failed execution state for task {}: {cleanup_error}",
                self.task_id
            );
        }
        outcome
    }

    fn owner(&self, execution_attempt: i64) -> crate::store::ExecutionOwner {
        crate::store::ExecutionOwner {
            task_id: self.task_id,
            execution_attempt,
        }
    }
}

struct EvidenceInput<'a> {
    attempt: i64,
    paths: &'a AttemptPaths,
    streamed_session: Option<&'a str>,
    result: &'a str,
}

struct HarnessOutput {
    attempt: i64,
    result: String,
    evidence: String,
}

struct AttemptPaths {
    stdout: PathBuf,
    stderr: PathBuf,
    launch: PathBuf,
}

impl AttemptPaths {
    fn new(artifact_dir: &Path, attempt: i64) -> Self {
        Self {
            stdout: artifact_dir.join(format!("stdout-{attempt}.jsonl")),
            stderr: artifact_dir.join(format!("stderr-{attempt}.log")),
            launch: artifact_dir.join(format!("launch-{attempt}.json")),
        }
    }
}

fn execution_task(store: &mut Store, task_id: i64) -> Result<Task, DomainError> {
    let task = store.task(task_id)?;
    if task.execution_status == Some(ExecutionStatus::Running) {
        return Err(DomainError::Invalid(format!(
            "task {task_id} is already executing"
        )));
    }
    if task.kind == TaskKind::Checkpoint {
        store.sync_checkpoint_with_subject(task_id)?;
    }
    Ok(task)
}

fn reuse_worktree(initial: &Task, root: &Path, options: &RunOptions<'_>) -> bool {
    let persisted_exists = initial.session_id.is_none()
        && initial
            .worktree_name
            .as_deref()
            .map(|name| root.join(name))
            .is_some_and(|path| path.is_dir());
    persisted_exists || options.restart_session && initial.worktree_name.is_some()
}

fn execution_session(
    store: &mut Store,
    initial: &Task,
    harness: &Harness,
    options: &RunOptions<'_>,
) -> Result<Option<String>, DomainError> {
    if !options.restart_session {
        return Ok(harness.assigned_session(initial.session_id.clone()));
    }
    let replacement = harness.assigned_session(None);
    store.replace_execution_session(
        crate::store::ExecutionOwner {
            task_id: initial.id,
            execution_attempt: initial.execution_attempt,
        },
        replacement.as_deref(),
    )?;
    Ok(replacement)
}

fn execution_worktree(initial: &Task, task_id: i64, requested: Option<&str>) -> Option<String> {
    (initial.kind == TaskKind::Work).then(|| {
        initial
            .worktree_name
            .clone()
            .or_else(|| requested.map(str::to_owned))
            .unwrap_or_else(|| format!("mush-task-{task_id}"))
    })
}

fn execution_prompt(
    initial: &Task,
    harness: &Harness,
    override_prompt: Option<&str>,
) -> Result<String, DomainError> {
    if let Some(prompt) = override_prompt {
        if initial.kind != TaskKind::Checkpoint {
            return Err(DomainError::Invalid(
                "only checkpoint execution accepts follow-up context".into(),
            ));
        }
        return Ok(prompt.to_owned());
    }
    match initial.kind {
        TaskKind::Work => Ok(initial.description.clone()),
        TaskKind::Checkpoint => harness
            .review_prompt()
            .map(str::to_owned)
            .ok_or_else(|| DomainError::Invalid("checkpoint agent has no review_prompt".into())),
    }
}

fn ensure_child_succeeded(
    child: &mut Child,
    harness: &Harness,
    stderr: &Path,
) -> Result<(), DomainError> {
    let status = child.wait()?;
    if status.success() {
        return Ok(());
    }
    Err(DomainError::Invalid(format!(
        "{} exited with {status}; see {}",
        harness.display_name(),
        stderr.display()
    )))
}

struct StreamCapture<'a, R> {
    task_id: i64,
    execution_attempt: i64,
    stream: R,
    transcript: &'a Path,
    session_field: &'a str,
}

/// Copy the harness stream to the durable transcript, persisting the
/// harness-assigned session id from the first event that carries one so an
/// interrupted run can still resume the same chat.
fn persist_streamed_session<R: std::io::Read>(
    store: &Store,
    capture: StreamCapture<'_, R>,
) -> Result<Option<String>, DomainError> {
    let mut file = File::create(capture.transcript)?;
    let mut session = None;
    for line in BufReader::new(capture.stream).lines() {
        let line = line?;
        writeln!(file, "{line}")?;
        if session.is_none()
            && let Some(id) = session_id(&line, capture.session_field)
        {
            store.record_execution_session(
                crate::store::ExecutionOwner {
                    task_id: capture.task_id,
                    execution_attempt: capture.execution_attempt,
                },
                &id,
            )?;
            session = Some(id);
        }
    }
    Ok(session)
}

fn session_id(line: &str, field: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()?
        .get(field)?
        .as_str()
        .map(str::to_owned)
}

/// Create a Mush-owned task worktree under the registered project.
fn add_git_worktree(project: &Path, worktree: &Path) -> Result<(), DomainError> {
    if worktree.is_dir() {
        return Ok(());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["worktree", "add"])
        .arg(worktree)
        .output()
        .map_err(|error| DomainError::Invalid(format!("cannot run git worktree add: {error}")))?;
    if !output.status.success() {
        return Err(DomainError::Invalid(format!(
            "failed to create worktree {}: {}",
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn write_once(path: &Path, contents: &[u8]) -> Result<(), DomainError> {
    if path.exists() {
        return Ok(());
    }
    let mut file = File::create(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn cached_boot_id() -> Option<&'static str> {
    static BOOT_ID: OnceLock<Option<String>> = OnceLock::new();
    BOOT_ID
        .get_or_init(|| {
            fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .as_deref()
}

/// The host boot identity, recorded with executions and claims as a diagnostic.
/// It is not a decision input: liveness comes from the execution lock, and a
/// boot leaves no lock held, so a reboot needs no special proof.
pub fn boot_id() -> String {
    cached_boot_id().unwrap_or_default().to_owned()
}
