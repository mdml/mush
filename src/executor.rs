use crate::{Agent, DomainError, ExecutionStatus, Store, Task, TaskKind};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::OnceLock,
};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeSettings {
    executable: String,
    #[serde(default)]
    version: Option<String>,
    model: String,
    effort: String,
    permission_mode: String,
    tools: Vec<String>,
    allowed_tools: Vec<String>,
    /// Native subagents hide delegation from Mush, so they are disabled unless
    /// the agent configuration explicitly opts in; delegation goes through
    /// `mush` instead.
    #[serde(default)]
    native_subagents: bool,
    #[serde(default)]
    review_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexSettings {
    executable: String,
    #[serde(default)]
    version: Option<String>,
    model: String,
    /// Codex exposes reasoning effort, sandbox policy and approval policy as
    /// configuration keys rather than flags, so they are named here as the
    /// harness names them and passed through `-c`.
    effort: String,
    sandbox: String,
    approval_policy: String,
    /// Codex's `multi_agent` feature spawns agents Mush cannot see, so it is
    /// disabled unless the agent configuration explicitly opts in; delegation
    /// goes through `mush` instead. This mirrors the Claude Code rule.
    #[serde(default)]
    native_subagents: bool,
    #[serde(default)]
    review_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorSettings {
    executable: String,
    #[serde(default)]
    version: Option<String>,
    model: String,
    #[serde(default)]
    review_prompt: Option<String>,
}

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
    let review_prompt = match harness {
        "claude-code" => {
            serde_json::from_str::<ClaudeSettings>(settings)
                .map_err(|error| {
                    DomainError::Invalid(format!("invalid claude-code settings: {error}"))
                })?
                .review_prompt
        }
        "codex" => {
            serde_json::from_str::<CodexSettings>(settings)
                .map_err(|error| DomainError::Invalid(format!("invalid codex settings: {error}")))?
                .review_prompt
        }
        "cursor" => {
            serde_json::from_str::<CursorSettings>(settings)
                .map_err(|error| DomainError::Invalid(format!("invalid cursor settings: {error}")))?
                .review_prompt
        }
        _ => return Ok(()),
    };
    if checkpoint && review_prompt.as_deref().is_none_or(|p| p.trim().is_empty()) {
        return Err(DomainError::Invalid(
            "an executable checkpoint agent requires a non-empty review_prompt".into(),
        ));
    }
    Ok(())
}

/// The harness-specific half of execution: invocation, version identity, session
/// semantics, worktree ownership, and result parsing. The shared task lifecycle in
/// [`Executor::run`] owns everything else.
enum Harness {
    ClaudeCode(ClaudeSettings),
    Codex(CodexSettings),
    Cursor(CursorSettings),
}

impl Harness {
    fn from_agent(agent: &Agent) -> Result<Self, DomainError> {
        match agent.harness.as_str() {
            "claude-code" => serde_json::from_str(&agent.settings)
                .map(Self::ClaudeCode)
                .map_err(|error| {
                    DomainError::Invalid(format!("invalid claude-code settings: {error}"))
                }),
            "codex" => serde_json::from_str(&agent.settings)
                .map(Self::Codex)
                .map_err(|error| DomainError::Invalid(format!("invalid codex settings: {error}"))),
            "cursor" => serde_json::from_str(&agent.settings)
                .map(Self::Cursor)
                .map_err(|error| DomainError::Invalid(format!("invalid cursor settings: {error}"))),
            other => Err(DomainError::Invalid(format!(
                "unsupported harness: {other}"
            ))),
        }
    }

    fn display_name(&self) -> &'static str {
        match self {
            Self::ClaudeCode(_) => "Claude Code",
            Self::Codex(_) => "Codex",
            Self::Cursor(_) => "Cursor Agent",
        }
    }

    fn executable(&self) -> &str {
        match self {
            Self::ClaudeCode(settings) => &settings.executable,
            Self::Codex(settings) => &settings.executable,
            Self::Cursor(settings) => &settings.executable,
        }
    }

    /// The pinned version, if the agent pins one. `None` means the agent runs
    /// whatever version of its executable is installed.
    fn expected_version(&self) -> Option<&str> {
        match self {
            Self::ClaudeCode(settings) => settings.version.as_deref(),
            Self::Codex(settings) => settings.version.as_deref(),
            Self::Cursor(settings) => settings.version.as_deref(),
        }
    }

    fn review_prompt(&self) -> Option<&str> {
        match self {
            Self::ClaudeCode(settings) => settings.review_prompt.as_deref(),
            Self::Codex(settings) => settings.review_prompt.as_deref(),
            Self::Cursor(settings) => settings.review_prompt.as_deref(),
        }
    }

    /// Whether Mush creates and owns the task worktree. Claude Code creates its
    /// native worktree itself via `--worktree`; for Cursor, Mush runs
    /// `git worktree add` and never uses cursor-agent's `--worktree` flag, which
    /// would place the worktree under `~/.cursor/worktrees`. Codex has no
    /// worktree concept at all and simply works in the directory it is launched
    /// in, so Mush owns that worktree too.
    fn mush_owns_worktree(&self) -> bool {
        matches!(self, Self::Codex(_) | Self::Cursor(_))
    }

    /// Where task worktrees live under the registered project.
    fn worktree_root(&self) -> &'static str {
        match self {
            Self::ClaudeCode(_) => ".claude/worktrees",
            Self::Codex(_) | Self::Cursor(_) => ".mush/worktrees",
        }
    }

    /// The stream field carrying a harness-assigned session id, for harnesses
    /// that assign their own. Claude Code accepts a pre-assigned session id and
    /// so has none; cursor-agent names its chat id `session_id`, and Codex names
    /// its thread id `thread_id` on the opening `thread.started` event. The
    /// shared lifecycle captures whichever field applies and persists it so a
    /// later attempt can resume the same conversation.
    fn streamed_session_field(&self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode(_) => None,
            Self::Codex(_) => Some("thread_id"),
            Self::Cursor(_) => Some("session_id"),
        }
    }

    fn captures_session_from_stream(&self) -> bool {
        self.streamed_session_field().is_some()
    }

    /// The session to run under, given any persisted session. `None` means the
    /// harness will assign one that must be captured from its output.
    fn assigned_session(&self, persisted: Option<String>) -> Option<String> {
        match self {
            Self::ClaudeCode(_) => Some(persisted.unwrap_or_else(|| Uuid::new_v4().to_string())),
            Self::Codex(_) | Self::Cursor(_) => persisted,
        }
    }

    /// Establish which harness version this run will use, before anything is
    /// spawned. The observed version is returned so the launch manifest and the
    /// evidence record what actually ran rather than what was configured.
    ///
    /// A pinned version is enforced exactly. An agent with no pinned version
    /// accepts whatever is installed, which is what keeps a configuration
    /// working across harness upgrades regardless of who performs them; its
    /// exactness then lives in the execution record instead of in this gate.
    fn resolve_version(&self) -> Result<String, DomainError> {
        let name = self.display_name();
        let output = Command::new(self.executable())
            .arg("--version")
            .output()
            .map_err(|error| {
                DomainError::Invalid(format!("cannot inspect {name} version: {error}"))
            })?;
        let observed = String::from_utf8_lossy(&output.stdout);
        let observed = observed.trim();
        if !output.status.success() {
            return Err(DomainError::Invalid(format!(
                "cannot inspect {name} version: {} exited with {}",
                self.executable(),
                output.status
            )));
        }
        let Some(pinned) = self.expected_version() else {
            return Ok(observed.to_owned());
        };
        // Each CLI decorates its version line differently: `claude --version`
        // prints `2.1.228 (Claude Code)`, `codex --version` prints
        // `codex-cli 0.146.1`, and `cursor-agent --version` prints
        // `2026.08.04-aaa8809` alone. A pin therefore matches when it equals a
        // whole whitespace-separated token, which stays exact for all three
        // without a per-harness parser to extend for the next adapter.
        if !observed.split_whitespace().any(|token| token == pinned) {
            return Err(DomainError::Invalid(format!(
                "{name} version mismatch: expected {pinned}, observed {observed}"
            )));
        }
        Ok(observed.to_owned())
    }

    /// Both harnesses run headless with stream-json output and receive the prompt
    /// through stdin.
    fn arguments(
        &self,
        session_id: Option<&str>,
        worktree: Option<&str>,
        resuming: bool,
    ) -> Vec<String> {
        match self {
            Self::ClaudeCode(settings) => {
                let session_id = session_id.expect("claude-code sessions are pre-assigned");
                let mut args = vec![
                    "--print".into(),
                    "--output-format".into(),
                    "stream-json".into(),
                    "--verbose".into(),
                ];
                if resuming {
                    args.extend(["--resume".into(), session_id.into()]);
                } else {
                    args.extend(["--session-id".into(), session_id.into()]);
                    if let Some(name) = worktree {
                        args.extend(["--worktree".into(), name.into()]);
                    }
                }
                args.extend([
                    "--model".into(),
                    settings.model.clone(),
                    "--effort".into(),
                    settings.effort.clone(),
                    "--permission-mode".into(),
                    settings.permission_mode.clone(),
                ]);
                if !settings.tools.is_empty() {
                    args.extend(["--tools".into(), settings.tools.join(",")]);
                }
                if !settings.allowed_tools.is_empty() {
                    args.extend(["--allowedTools".into(), settings.allowed_tools.join(",")]);
                }
                if !settings.native_subagents {
                    // `Task` is the subagent tool in the pinned Claude Code CLI.
                    args.extend(["--disallowedTools".into(), "Task".into()]);
                }
                args
            }
            Self::Codex(settings) => {
                // `codex exec` is the headless entry point and `codex exec resume`
                // continues a thread. Neither accepts a working directory Mush
                // needs, because the child is spawned in the task worktree, and
                // `resume` accepts no `--sandbox` flag, so the sandbox travels as
                // a `-c` override that both forms accept.
                let mut args = vec!["exec".into()];
                if resuming {
                    args.push("resume".into());
                }
                args.extend([
                    "--json".into(),
                    "--model".into(),
                    settings.model.clone(),
                    "-c".into(),
                    format!("model_reasoning_effort=\"{}\"", settings.effort),
                    "-c".into(),
                    format!("sandbox_mode=\"{}\"", settings.sandbox),
                    "-c".into(),
                    format!("approval_policy=\"{}\"", settings.approval_policy),
                ]);
                if !settings.native_subagents {
                    args.extend(["--disable".into(), "multi_agent".into()]);
                }
                if resuming {
                    args.push(
                        session_id
                            .expect("resume requires a captured thread id")
                            .into(),
                    );
                }
                // A literal `-` makes codex read the prompt from stdin, which is
                // how every adapter here delivers it.
                args.push("-".into());
                args
            }
            Self::Cursor(settings) => {
                let mut args = vec![
                    "-p".into(),
                    "--output-format".into(),
                    "stream-json".into(),
                    "--model".into(),
                    settings.model.clone(),
                    "--force".into(),
                    "--trust".into(),
                ];
                if resuming {
                    let session_id = session_id.expect("resume requires a captured chat id");
                    args.extend(["--resume".into(), session_id.into()]);
                }
                args
            }
        }
    }

    /// Extract the final result from the harness transcript.
    fn parse_result(&self, path: &Path) -> Result<String, DomainError> {
        match self {
            Self::Codex(_) => self.parse_codex_result(path),
            _ => self.parse_result_event(path),
        }
    }

    /// Codex emits no single result event. Its stream ends with `turn.completed`
    /// or `turn.failed`, and the agent's answer is the text of the last completed
    /// `agent_message` item — the last, because a run narrates intermediate
    /// messages before its conclusion.
    fn parse_codex_result(&self, path: &Path) -> Result<String, DomainError> {
        let name = self.display_name();
        let reader = BufReader::new(File::open(path)?);
        let mut message = None;
        for line in reader.lines() {
            let value: Value = serde_json::from_str(&line?)?;
            match value.get("type").and_then(Value::as_str) {
                Some("turn.failed") => {
                    let detail = value
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("no detail reported");
                    return Err(DomainError::Invalid(format!(
                        "{name} turn failed: {detail}; see {}",
                        path.display()
                    )));
                }
                Some("item.completed") => {
                    let item = value.get("item");
                    if item
                        .and_then(|item| item.get("type"))
                        .and_then(Value::as_str)
                        == Some("agent_message")
                        && let Some(text) = item
                            .and_then(|item| item.get("text"))
                            .and_then(Value::as_str)
                    {
                        message = Some(text.to_owned());
                    }
                }
                _ => {}
            }
        }
        message.ok_or_else(|| {
            DomainError::Invalid(format!(
                "{name} output has no agent message: {}",
                path.display()
            ))
        })
    }

    /// Extract the final result from a stream-json transcript that carries an
    /// explicit result event.
    fn parse_result_event(&self, path: &Path) -> Result<String, DomainError> {
        let name = self.display_name();
        let reader = BufReader::new(File::open(path)?);
        let mut event = None;
        for line in reader.lines() {
            let value: Value = serde_json::from_str(&line?)?;
            if value.get("type").and_then(Value::as_str) == Some("result") {
                event = Some(value);
            }
        }
        let event = event.ok_or_else(|| {
            DomainError::Invalid(format!(
                "{name} output has no result event: {}",
                path.display()
            ))
        })?;
        // cursor-agent reports failures through `is_error` on the result event.
        if matches!(self, Self::Cursor(_))
            && event.get("is_error").and_then(Value::as_bool) == Some(true)
        {
            return Err(DomainError::Invalid(format!(
                "{name} reported an error result; see {}",
                path.display()
            )));
        }
        event
            .get("result")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                DomainError::Invalid(format!(
                    "{name} result event has no result text: {}",
                    path.display()
                ))
            })
    }
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
        requested_worktree: Option<&str>,
        restart_session: bool,
        prompt_override: Option<&str>,
    ) -> Result<Task, DomainError> {
        let lock = acquire_execution_lock(store.database_path(), task_id)?;
        self.run_locked(
            store,
            task_id,
            &RunOptions {
                worktree: requested_worktree,
                restart_session,
                prompt_override,
            },
            None,
            &lock,
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
        task_id: i64,
        options: &RunOptions<'_>,
        runner_id: Option<&str>,
        // Held for the whole of the execution, and released by the kernel if
        // this process dies. Borrowing it here is what makes that structural.
        _execution_lock: &crate::lock::FileLock,
    ) -> Result<Task, DomainError> {
        let RunOptions {
            worktree: requested_worktree,
            restart_session,
            prompt_override,
        } = *options;
        let initial = store.task(task_id)?;
        if initial.execution_status == Some(ExecutionStatus::Running) {
            return Err(DomainError::Invalid(format!(
                "task {task_id} is already executing"
            )));
        }
        if initial.kind == TaskKind::Checkpoint {
            // A checkpoint awaiting its subject cannot run before that subject
            // completes, and a still-waiting evidence placeholder is replaced
            // with the subject's real evidence here, the first moment
            // completion is guaranteed.
            store.sync_checkpoint_with_subject(task_id)?;
        }
        let agent_id = initial
            .agent_id
            .ok_or_else(|| DomainError::Invalid("task has no assigned agent".into()))?;
        let agent = store.agent(agent_id)?;
        let harness = Harness::from_agent(&agent)?;
        let harness_version = harness.resolve_version()?;
        let project = store.project(initial.project_id)?;
        let project_path = PathBuf::from(&project.path);
        let worktree_root = project_path.join(harness.worktree_root());
        let persisted_worktree_path = initial
            .worktree_name
            .as_deref()
            .map(|name| worktree_root.join(name));
        let reuse_worktree = initial.session_id.is_none()
            && persisted_worktree_path
                .as_ref()
                .is_some_and(|path| path.is_dir())
            || restart_session && initial.worktree_name.is_some();
        let resuming = initial.session_id.is_some() && !restart_session;
        let session_id = if restart_session {
            let replacement = harness.assigned_session(None);
            store.replace_execution_session(
                task_id,
                initial.execution_attempt,
                replacement.as_deref(),
            )?;
            replacement
        } else {
            harness.assigned_session(initial.session_id.clone())
        };
        let worktree_name = if initial.kind == TaskKind::Work {
            Some(
                initial
                    .worktree_name
                    .clone()
                    .or_else(|| requested_worktree.map(str::to_owned))
                    .unwrap_or_else(|| format!("mush-task-{task_id}")),
            )
        } else {
            None
        };
        let prompt = if let Some(prompt) = prompt_override {
            if initial.kind != TaskKind::Checkpoint {
                return Err(DomainError::Invalid(
                    "only checkpoint execution accepts follow-up context".into(),
                ));
            }
            prompt.to_owned()
        } else {
            match initial.kind {
                TaskKind::Work => initial.description.clone(),
                TaskKind::Checkpoint => {
                    harness.review_prompt().map(str::to_owned).ok_or_else(|| {
                        DomainError::Invalid("checkpoint agent has no review_prompt".into())
                    })?
                }
            }
        };
        let artifact_dir = self.artifacts_root.join(format!("task-{task_id}"));
        fs::create_dir_all(&artifact_dir)?;
        write_once(&artifact_dir.join("prompt.md"), prompt.as_bytes())?;
        let boot_id = boot_id();
        let running = store.begin_execution_owned(
            task_id,
            session_id.as_deref(),
            worktree_name.as_deref(),
            &artifact_dir,
            &boot_id,
            runner_id,
        )?;
        let outcome = (|| {
            let attempt = running.execution_attempt;
            if prompt_override.is_some() {
                write_once(
                    &artifact_dir.join(format!("prompt-{attempt}.md")),
                    prompt.as_bytes(),
                )?;
            }
            let stdout_path = artifact_dir.join(format!("stdout-{attempt}.jsonl"));
            let stderr_path = artifact_dir.join(format!("stderr-{attempt}.log"));
            let launch_path = artifact_dir.join(format!("launch-{attempt}.json"));
            let cwd = if initial.kind != TaskKind::Work {
                project_path.clone()
            } else {
                let name = worktree_name.as_deref().expect("work task worktree");
                let path = worktree_root.join(name);
                if resuming || reuse_worktree {
                    if !path.is_dir() {
                        return Err(DomainError::Invalid(format!(
                            "cannot resume: worktree is missing: {}",
                            path.display()
                        )));
                    }
                    path
                } else if harness.mush_owns_worktree() {
                    add_git_worktree(&project_path, &path)?;
                    path
                } else {
                    // Claude Code creates its native worktree itself via `--worktree`.
                    project_path.clone()
                }
            };
            let arguments = harness.arguments(
                session_id.as_deref(),
                if reuse_worktree {
                    None
                } else {
                    worktree_name.as_deref()
                },
                resuming,
            );
            let launch = serde_json::json!({"executable":harness.executable(),"version":harness_version,"arguments":arguments,"cwd":cwd,"session_id":session_id,"attempt":attempt});
            fs::write(&launch_path, serde_json::to_vec_pretty(&launch)?)?;
            let stderr = File::create(&stderr_path)?;
            let mut command = Command::new(harness.executable());
            command
                .args(&arguments)
                .current_dir(&cwd)
                // The agent process learns its own task identity so a delegating
                // coordinator can create subtasks under itself.
                .env("MUSH_TASK_ID", task_id.to_string())
                .stdin(Stdio::piped())
                .stderr(Stdio::from(stderr));
            if harness.captures_session_from_stream() {
                command.stdout(Stdio::piped());
            } else {
                command.stdout(Stdio::from(File::create(&stdout_path)?));
            }
            let mut child = command.spawn().map_err(|error| {
                DomainError::Invalid(format!(
                    "failed to start {}: {error}",
                    harness.display_name()
                ))
            })?;
            if let Err(error) = store.record_execution_pid(task_id, attempt, child.id()) {
                terminate_child(&mut child);
                return Err(error);
            }
            let mut stdin = child.stdin.take().ok_or_else(|| {
                DomainError::Invalid(format!("{} stdin is unavailable", harness.display_name()))
            })?;
            let mut streamed_session = None;
            if let Some(session_field) = harness.streamed_session_field() {
                let stdout = child.stdout.take().ok_or_else(|| {
                    DomainError::Invalid(format!(
                        "{} stdout is unavailable",
                        harness.display_name()
                    ))
                })?;
                // Write the prompt on its own thread so a prompt larger than the OS
                // pipe buffer cannot deadlock against the stream this process must
                // drain; dropping stdin at the end of the thread closes it.
                let prompt_bytes = prompt.clone().into_bytes();
                let writer = std::thread::spawn(move || stdin.write_all(&prompt_bytes));
                streamed_session = persist_streamed_session(
                    store,
                    task_id,
                    attempt,
                    stdout,
                    &stdout_path,
                    session_field,
                )?;
                writer
                    .join()
                    .map_err(|_| DomainError::Invalid("prompt writer thread panicked".into()))??;
            } else {
                stdin.write_all(prompt.as_bytes())?;
                drop(stdin);
            }
            let status = child.wait()?;
            if !status.success() {
                return Err(DomainError::Invalid(format!(
                    "{} exited with {status}; see {}",
                    harness.display_name(),
                    stderr_path.display()
                )));
            }
            let result = harness.parse_result(&stdout_path)?;
            let session_label = session_id
                .as_deref()
                .or(streamed_session.as_deref())
                .unwrap_or("unknown");
            let evidence = format!(
                "## {} execution\n\n- version: `{harness_version}`\n- session: `{session_label}`\n- attempt: {attempt}\n- output: `{}`\n- errors: `{}`\n\n## Result\n\n{result}",
                harness.display_name(),
                stdout_path.display(),
                stderr_path.display()
            );
            match initial.kind {
                TaskKind::Work => store.finish_work_execution(task_id, attempt, &result, &evidence),
                TaskKind::Checkpoint => {
                    store.finish_checkpoint_execution(task_id, attempt, &evidence)
                }
            }
        })();
        if let Err(error) = &outcome {
            // An execution that owns a claim is parked by its caller, which
            // knows the delivery it claimed. One that runs queued work without
            // a claim has no such caller, so it parks itself here.
            let diagnostic = (runner_id.is_none()
                && running.readiness_status != crate::ReadinessStatus::Unqueued)
                .then(|| format!("foreground execution failed: {error}; inspect task artifacts and run task recover to relaunch"));
            if let Err(cleanup_error) = store.interrupt_execution_owned_with_intervention(
                task_id,
                running.execution_attempt,
                diagnostic.as_deref(),
            ) {
                eprintln!(
                    "warning: could not persist failed execution state for task {task_id}: {cleanup_error}"
                );
            }
        }
        outcome
    }
}

/// Copy the harness stream to the durable transcript, persisting the
/// harness-assigned session id from the first event that carries one so an
/// interrupted run can still resume the same chat.
fn persist_streamed_session(
    store: &Store,
    task_id: i64,
    execution_attempt: i64,
    stream: impl std::io::Read,
    transcript: &Path,
    session_field: &str,
) -> Result<Option<String>, DomainError> {
    let mut file = File::create(transcript)?;
    let mut session = None;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        writeln!(file, "{line}")?;
        if session.is_none()
            && let Ok(value) = serde_json::from_str::<Value>(&line)
            && let Some(id) = value.get(session_field).and_then(Value::as_str)
        {
            store.record_execution_session(task_id, execution_attempt, id)?;
            session = Some(id.to_owned());
        }
    }
    Ok(session)
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
