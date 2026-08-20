//! M4 reviewed unit 2: the smallest executable work-task dependency chain, and
//! the explicit runner surface that executes it.

use mush::{
    Executor, ReadinessStatus, Store,
    executor::RunOptions,
    lock,
    runner::{CONCURRENCY_BOUND, EXIT_DID_NOT_START},
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
mod common;
use common::StoreTestExt;
#[cfg(unix)]
use common::{claude_settings, fake_claude, git_project, install_script};

fn graph() -> (tempfile::TempDir, tempfile::TempDir, Store, i64, i64) {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let agent = store
        .register_agent_args(project.id, "worker", "manual", "model", "{}", false)
        .unwrap();
    let root = store
        .add_work_task_args(project.id, agent.id, "root", None)
        .unwrap();
    let dependent = store
        .add_work_task_args(project.id, agent.id, "dependent", None)
        .unwrap();
    (state, project_dir, store, root.id, dependent.id)
}

fn mush(database: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mush"))
        .arg("--database")
        .arg(database)
        .args(arguments)
        .output()
        .unwrap()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Poll a condition to a deadline. Tests observe durable rows written by other
/// processes, so a bounded poll is the honest way to wait for them.
fn wait_until(mut satisfied: impl FnMut() -> bool, seconds: u64, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !satisfied() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(25));
    }
}

/// A project whose agent runs a fake harness, so the runner surface can be
/// exercised end to end without a real agent.
#[cfg(unix)]
struct Runnable {
    state: tempfile::TempDir,
    _project_dir: tempfile::TempDir,
    database: PathBuf,
    project_id: i64,
    agent_id: i64,
}

#[cfg(unix)]
impl Runnable {
    fn new(harness_body: Option<&str>) -> Self {
        let state = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        git_project(project_dir.path());
        let harness = state.path().join("claude");
        match harness_body {
            None => fake_claude(&harness, true),
            Some(body) => install_script(
                &harness,
                &format!(
                    "#!/usr/bin/env bash\nif [[ ${{1:-}} == --version ]]; then echo '2.1.222 (Claude Code)'; exit 0; fi\nprompt=$(cat)\n{body}\n"
                ),
            ),
        }
        let database = state.path().join("mush.sqlite");
        let store = Store::open(&database).unwrap();
        let project = store
            .register_project("project", project_dir.path())
            .unwrap();
        let agent = store
            .register_agent_args(
                project.id,
                "worker",
                "claude-code",
                "model",
                &claude_settings(&harness, None),
                false,
            )
            .unwrap();
        Self {
            state,
            _project_dir: project_dir,
            database,
            project_id: project.id,
            agent_id: agent.id,
        }
    }

    /// A project whose agent points at an executable that does not exist, so
    /// every execution of it fails before the harness runs.
    fn unrunnable() -> Self {
        let runnable = Self::new(None);
        std::fs::remove_file(runnable.state.path().join("claude")).unwrap();
        runnable
    }

    fn store(&self) -> Store {
        Store::open(&self.database).unwrap()
    }

    fn task(&self, description: &str) -> i64 {
        self.store()
            .add_work_task_args(self.project_id, self.agent_id, description, None)
            .unwrap()
            .id
    }

    fn queue(&self, id: i64) -> Output {
        let output = mush(&self.database, &["task", "queue", &id.to_string()]);
        assert!(output.status.success(), "{}", stderr_of(&output));
        output
    }

    fn run(&self, arguments: &[&str]) -> Output {
        mush(&self.database, arguments)
    }

    fn spawn(&self, arguments: &[&str]) -> std::process::Child {
        Command::new(env!("CARGO_BIN_EXE_mush"))
            .arg("--database")
            .arg(&self.database)
            .args(arguments)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    /// Spawn a command whose output the test reads once it exits, which is how
    /// two racing processes are compared without either of them blocking on a
    /// pipe the test is not draining yet.
    fn spawn_captured(&self, arguments: &[&str]) -> std::process::Child {
        Command::new(env!("CARGO_BIN_EXE_mush"))
            .arg("--database")
            .arg(&self.database)
            .args(arguments)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// Wait until the execution has recorded its harness process, which is the
    /// moment the runner finished spawning it.
    ///
    /// A test that kills a runner uses this rather than waiting for `running`
    /// readiness, because readiness is committed before the harness is spawned:
    /// a kill landing inside that spawn leaves a forked child sharing the
    /// lock's open file description for as long as it takes to reach `exec`,
    /// and the lock therefore reads as held for an instant after the kill.
    fn wait_for_harness(&self, id: i64, runner: &std::process::Child) {
        let runner_pid = i64::from(runner.id());
        wait_until(
            || {
                self.store()
                    .task(id)
                    .unwrap()
                    .execution_pid
                    .is_some_and(|pid| pid != runner_pid)
            },
            15,
            "the harness process to be recorded",
        );
    }

    fn readiness(&self, id: i64) -> ReadinessStatus {
        self.store().task(id).unwrap().readiness_status
    }

    fn is_completed(&self, id: i64) -> bool {
        self.store().task(id).unwrap().status == mush::TaskStatus::Completed
    }

    /// Register the project's checkpoint reviewer on the same fake harness.
    fn reviewer(&self) -> i64 {
        self.store()
            .register_agent_args(
                self.project_id,
                "reviewer",
                "claude-code",
                "model",
                &claude_settings(
                    &self.state.path().join("claude"),
                    Some("Review the subject work."),
                ),
                true,
            )
            .unwrap()
            .id
    }
}

// ---------------------------------------------------------------------------
// Nothing starts work implicitly
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn no_command_outside_the_execution_front_doors_starts_queued_work() {
    let harness = Runnable::new(None);
    let root = harness.task("root");
    let dependent = harness.task("dependent");
    harness
        .store()
        .add_dependency(root, dependent)
        .unwrap_or_else(|error| panic!("{error}"));
    harness.queue(dependent);
    harness.queue(root);

    // Every other surface, in an ordering that used to pump launches.
    let root_id = root.to_string();
    let root_id = root_id.as_str();
    for arguments in [
        vec!["task", "show", root_id],
        vec!["task", "list"],
        vec!["task", "status", root_id],
        vec!["task", "wait", root_id, "--until", "any", "--timeout", "1"],
        vec!["tui", "--snapshot"],
        vec!["runner", "status"],
    ] {
        let output = harness.run(&arguments);
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            stderr_of(&output)
        );
        assert_eq!(
            harness.readiness(root),
            ReadinessStatus::Ready,
            "{arguments:?} advanced queued work"
        );
    }
    assert_eq!(
        harness.store().task(root).unwrap().execution_attempt,
        0,
        "a command that was not asked to run work executed the root"
    );

    let start = harness.run(&["runner", "start", &root.to_string()]);
    assert!(start.status.success(), "{}", stderr_of(&start));
    assert!(harness.is_completed(root));
    assert_eq!(harness.readiness(dependent), ReadinessStatus::Ready);
}

#[cfg(unix)]
#[test]
fn tick_and_serve_each_execute_queued_ready_work() {
    let ticked = Runnable::new(None);
    let first = ticked.task("ticked");
    ticked.queue(first);
    let tick = ticked.run(&["runner", "tick"]);
    assert!(tick.status.success(), "{}", stderr_of(&tick));
    wait_until(|| ticked.is_completed(first), 15, "the ticked task");

    let served = Runnable::new(None);
    let second = served.task("served");
    served.queue(second);
    // `serve` runs until terminated, so the test observes the durable effect it
    // is asserting on and then ends the process.
    let mut serve = served.spawn(&["runner", "serve"]);
    wait_until(|| served.is_completed(second), 15, "the served task");
    serve.kill().unwrap();
    serve.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn start_claims_the_next_eligible_task_and_neither_front_door_refuses_the_others_case() {
    let harness = Runnable::new(None);
    let first = harness.task("first");
    let second = harness.task("second");
    let unqueued = harness.task("unqueued");
    harness.queue(first);
    harness.queue(second);

    let start = harness.run(&["runner", "start"]);
    assert!(start.status.success(), "{}", stderr_of(&start));
    assert!(
        harness.is_completed(first),
        "start without an id took the next eligible task"
    );
    assert_eq!(harness.readiness(second), ReadinessStatus::Ready);

    let targeted = harness.run(&["runner", "start", &second.to_string()]);
    assert!(targeted.status.success(), "{}", stderr_of(&targeted));
    assert!(harness.is_completed(second));

    // The worker front door given an unqueued task executes it rather than
    // refusing: an unqueued task simply owes the queue nothing.
    let unqueued_start = harness.run(&["runner", "start", &unqueued.to_string()]);
    assert!(
        unqueued_start.status.success(),
        "{}",
        stderr_of(&unqueued_start)
    );
    assert!(harness.is_completed(unqueued));
    assert_eq!(harness.readiness(unqueued), ReadinessStatus::Unqueued);

    // And the human front door given a queued task executes it, with the
    // options only it carries.
    let third = harness.task("third");
    harness.queue(third);
    let queued_run = harness.run(&[
        "task",
        "run",
        &third.to_string(),
        "--worktree",
        "mush-task-third",
    ]);
    assert!(queued_run.status.success(), "{}", stderr_of(&queued_run));
    assert!(harness.is_completed(third));
    assert_eq!(
        harness
            .store()
            .task(third)
            .unwrap()
            .worktree_name
            .as_deref(),
        Some("mush-task-third")
    );
}

/// `task run` on a queued task does the queue's bookkeeping rather than
/// refusing it: the delivery is claimed by this execution, the readiness
/// transitions are journalled, and completion advances declared dependents
/// exactly as a `runner start` execution would.
#[cfg(unix)]
#[test]
fn task_run_of_a_queued_task_claims_its_delivery_and_advances_dependents() {
    let harness = Runnable::new(None);
    let root = harness.task("root");
    let dependent = harness.task("dependent");
    harness.store().add_dependency(root, dependent).unwrap();
    harness.queue(dependent);
    harness.queue(root);
    assert_eq!(harness.readiness(dependent), ReadinessStatus::Blocked);

    let run = harness.run(&["task", "run", &root.to_string()]);
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert!(harness.is_completed(root));

    let connection = rusqlite::Connection::open(&harness.database).unwrap();
    let (state, runner_id): (String, Option<String>) = connection
        .query_row(
            "SELECT state,runner_id FROM launch_deliveries WHERE task_id=?1",
            [root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "delivered");
    assert!(
        runner_id.is_some(),
        "the foreground execution claimed the delivery"
    );
    let transitions: Vec<String> = connection
        .prepare("SELECT kind FROM task_transitions WHERE task_id=?1 ORDER BY id")
        .unwrap()
        .query_map([root], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for kind in ["queued", "launch_claimed", "running", "completed"] {
        assert!(
            transitions.iter().any(|recorded| recorded == kind),
            "expected a {kind} transition, got {transitions:?}"
        );
    }

    // The graph advanced, so the dependent is a runner's next candidate.
    assert_eq!(harness.readiness(dependent), ReadinessStatus::Ready);
    let start = harness.run(&["runner", "start"]);
    assert!(start.status.success(), "{}", stderr_of(&start));
    assert!(harness.is_completed(dependent));
}

/// Whichever front door takes the task's execution lock is the one that runs
/// it. The loser reports a live execution and starts nothing, and the task is
/// executed exactly once.
#[cfg(unix)]
#[test]
fn a_task_run_racing_a_runner_start_leaves_exactly_one_execution() {
    let harness = Runnable::new(Some(
        "sleep 2\nprintf '%s\\n' '{\"type\":\"result\",\"result\":\"contested result\"}'",
    ));
    let task = harness.task("contested");
    harness.queue(task);

    // A two-second harness makes the overlap certain: whichever process is
    // second reaches the lock long before the first releases it.
    let id = task.to_string();
    let racing = [
        harness.spawn_captured(&["task", "run", &id]),
        harness.spawn_captured(&["runner", "start", &id]),
    ];
    let mut contenders = racing.map(|child| child.wait_with_output().unwrap());
    contenders.sort_by_key(|output| output.status.code());

    let (winner, loser) = (&contenders[0], &contenders[1]);
    assert!(winner.status.success(), "{}", stderr_of(winner));
    assert_eq!(
        loser.status.code(),
        Some(EXIT_DID_NOT_START),
        "{}",
        stderr_of(loser)
    );
    assert!(
        stderr_of(loser).contains("already has a live execution"),
        "{}",
        stderr_of(loser)
    );

    let executed = harness.store().task(task).unwrap();
    assert_eq!(executed.status, mush::TaskStatus::Completed);
    assert_eq!(
        executed.execution_attempt, 1,
        "the task was executed more than once"
    );
    assert_eq!(executed.intervention, None);
    let claims: i64 = rusqlite::Connection::open(&harness.database)
        .unwrap()
        .query_row(
            "SELECT attempts FROM launch_deliveries WHERE task_id=?1",
            [task],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(claims, 1, "the delivery was claimed more than once");
}

/// A killed foreground execution of a queued task is reconciled exactly as a
/// killed `runner start` is: the kernel releases its lock, and the delivery
/// returns to the queue.
#[cfg(unix)]
#[test]
fn a_killed_task_run_of_a_queued_task_is_reconciled_like_a_runner_start() {
    let harness = Runnable::new(Some(
        "sleep 10\nprintf '%s\\n' '{\"type\":\"result\",\"result\":\"never\"}'",
    ));
    let task = harness.task("killed foreground");
    harness.queue(task);
    let mut execution = harness.spawn(&["task", "run", &task.to_string()]);
    harness.wait_for_harness(task, &execution);
    assert_eq!(harness.readiness(task), ReadinessStatus::Running);
    assert!(lock::task_is_live(&harness.database, task).unwrap());

    execution.kill().unwrap();
    execution.wait().unwrap();
    assert!(!lock::task_is_live(&harness.database, task).unwrap());

    let mut store = harness.store();
    store.reconcile().unwrap();
    assert_eq!(
        store.task(task).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert_eq!(store.task(task).unwrap().intervention, None);
}

#[cfg(unix)]
#[test]
fn concurrent_start_processes_drain_a_queue_without_double_claiming() {
    let harness = Runnable::new(None);
    let ids = (0..6)
        .map(|index| {
            let id = harness.task(&format!("queued {index}"));
            harness.queue(id);
            id
        })
        .collect::<Vec<_>>();

    // Three restart loops over `runner start` are a worker pool; each exits
    // when the queue is empty.
    let workers = (0..3)
        .map(|_| {
            Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "while '{}' --database '{}' runner start >/dev/null 2>&1; do :; done",
                    env!("CARGO_BIN_EXE_mush"),
                    harness.database.display()
                ))
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    for mut worker in workers {
        worker.wait().unwrap();
    }

    let store = harness.store();
    for id in ids {
        let task = store.task(id).unwrap();
        assert_eq!(task.status, mush::TaskStatus::Completed, "task {id}");
        assert_eq!(
            task.execution_attempt, 1,
            "task {id} was claimed more than once"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_chain_advances_across_tick_boundaries_with_the_queueing_shell_gone() {
    let harness = Runnable::new(None);
    let root = harness.task("root");
    let dependent = harness.task("dependent");
    harness.store().add_dependency(root, dependent).unwrap();
    harness.queue(dependent);
    harness.queue(root);

    // One tick starts the root and exits without waiting, so the dependent can
    // only advance on a later pass.
    let first = harness.run(&["runner", "tick"]);
    assert!(first.status.success(), "{}", stderr_of(&first));
    assert!(!harness.is_completed(dependent));
    wait_until(|| harness.is_completed(root), 15, "the root");

    assert_eq!(harness.readiness(dependent), ReadinessStatus::Ready);
    let second = harness.run(&["runner", "tick"]);
    assert!(second.status.success(), "{}", stderr_of(&second));
    wait_until(|| harness.is_completed(dependent), 15, "the dependent");
}

#[cfg(unix)]
#[test]
fn a_chain_advances_across_a_serve_restart() {
    let harness = Runnable::new(None);
    let root = harness.task("root");
    let dependent = harness.task("dependent");
    harness.store().add_dependency(root, dependent).unwrap();
    harness.queue(root);

    let mut first = harness.spawn(&["runner", "serve"]);
    wait_until(|| harness.is_completed(root), 15, "the root");
    first.kill().unwrap();
    first.wait().unwrap();
    assert!(!harness.is_completed(dependent));

    // The dependent joins the queue only after the first serve is gone, so a
    // fresh one has to pick the chain up from durable state alone.
    harness.queue(dependent);
    assert_eq!(harness.readiness(dependent), ReadinessStatus::Ready);
    let mut second = harness.spawn(&["runner", "serve"]);
    wait_until(|| harness.is_completed(dependent), 15, "the dependent");
    second.kill().unwrap();
    second.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn serve_records_a_child_failure_from_its_exit_status() {
    let harness = Runnable::unrunnable();
    let task = harness.task("cannot launch");
    harness.queue(task);

    // The failing child parks itself first; serve's own verdict lands when it
    // reaps that child, so wait for the exit status rather than for readiness.
    let mut serve = harness.spawn(&["runner", "serve"]);
    wait_until(
        || {
            harness
                .store()
                .task(task)
                .unwrap()
                .intervention
                .is_some_and(|diagnostic| diagnostic.contains("exited with"))
        },
        15,
        "serve to record the child's exit status",
    );
    serve.kill().unwrap();
    serve.wait().unwrap();
    let parked = harness.store().task(task).unwrap();
    assert_eq!(
        parked.readiness_status,
        ReadinessStatus::InterventionRequired
    );
    let intervention = parked.intervention.unwrap();
    assert!(
        intervention.contains("exited with"),
        "serve should record the exit status it observed, got: {intervention}"
    );
}

#[cfg(unix)]
#[test]
fn start_reports_that_it_started_nothing_with_a_distinct_exit_code() {
    let harness = Runnable::new(None);

    // Nothing eligible: the queue is empty, which is the queue behaving
    // normally rather than a fault.
    let idle = harness.run(&["runner", "start"]);
    assert_eq!(idle.status.code(), Some(EXIT_DID_NOT_START));
    assert!(
        stderr_of(&idle).contains("no queued task is ready to start"),
        "{}",
        stderr_of(&idle)
    );

    // The task is already held: another runner owns it, so this process starts
    // nothing and says so with the same code.
    let task = harness.task("held");
    harness.queue(task);
    let _held = lock::FileLock::try_acquire(&lock::task_lock_path(&harness.database, task))
        .unwrap()
        .expect("the task lock is free");
    for arguments in [
        vec!["runner", "start", &task.to_string()],
        vec!["runner", "start"],
    ] {
        let output = harness.run(&arguments);
        assert_eq!(
            output.status.code(),
            Some(EXIT_DID_NOT_START),
            "{arguments:?}: {}",
            stderr_of(&output)
        );
        assert!(
            stderr_of(&output).contains("already has a live execution"),
            "{arguments:?}: {}",
            stderr_of(&output)
        );
    }
    assert_eq!(harness.readiness(task), ReadinessStatus::Ready);
    assert_eq!(harness.store().task(task).unwrap().intervention, None);
}

/// The human front door reports a live execution the same way the worker front
/// door does when another process already holds the task lock.
#[cfg(unix)]
#[test]
fn task_run_reports_that_it_started_nothing_when_the_lock_is_held() {
    let harness = Runnable::new(None);
    let task = harness.task("held");
    harness.queue(task);
    let _held = lock::FileLock::try_acquire(&lock::task_lock_path(&harness.database, task))
        .unwrap()
        .expect("the task lock is free");
    let output = harness.run(&["task", "run", &task.to_string()]);
    assert_eq!(output.status.code(), Some(EXIT_DID_NOT_START));
    assert!(
        stderr_of(&output).contains("already has a live execution"),
        "{}",
        stderr_of(&output)
    );
    assert_eq!(harness.readiness(task), ReadinessStatus::Ready);
    assert_eq!(harness.store().task(task).unwrap().intervention, None);
}

/// A queued failure reached through `runner start` is runner-owned: the
/// intervention names the execution failure, not the foreground diagnostic.
#[cfg(unix)]
#[test]
fn a_runner_owned_failure_on_queued_work_parks_without_a_foreground_diagnostic() {
    let harness = Runnable::new(Some("echo 'simulated interruption' >&2; exit 7"));
    let task = harness.task("failing queued");
    harness.queue(task);

    let failed = harness.run(&["runner", "start", &task.to_string()]);
    let code = failed.status.code().expect("start exited normally");
    assert_ne!(code, 0, "a failed execution is not a success");
    assert_ne!(
        code, EXIT_DID_NOT_START,
        "an execution that started and failed is not a start that did nothing"
    );

    let parked = harness.store().task(task).unwrap();
    assert_eq!(
        parked.readiness_status,
        ReadinessStatus::InterventionRequired
    );
    let intervention = parked.intervention.unwrap();
    assert!(
        intervention.contains(&format!("execution of task {task} failed")),
        "{intervention}"
    );
    assert!(
        !intervention.contains("foreground execution failed"),
        "runner-owned failures must not use the foreground diagnostic: {intervention}"
    );
}

/// A queued failure reached through `Executor::run` is foreground-owned and
/// records the distinct diagnostic that tells the operator to use task recover.
#[cfg(unix)]
#[test]
fn a_foreground_executor_failure_on_queued_work_records_a_distinct_diagnostic() {
    let harness = Runnable::new(Some("echo 'simulated interruption' >&2; exit 7"));
    let task = harness.task("failing foreground");
    harness.queue(task);

    let error = Executor::new(&harness.database)
        .run(&mut harness.store(), task, &RunOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("exited with"), "{}", error);

    let parked = harness.store().task(task).unwrap();
    assert_eq!(
        parked.readiness_status,
        ReadinessStatus::InterventionRequired
    );
    let intervention = parked.intervention.unwrap();
    assert!(
        intervention.contains("foreground execution failed"),
        "{intervention}"
    );
    assert!(intervention.contains("task recover"));
}

#[cfg(unix)]
#[test]
fn a_failed_execution_exits_with_a_different_code_than_a_start_that_did_nothing() {
    let harness = Runnable::unrunnable();
    let task = harness.task("cannot launch");
    harness.queue(task);

    let failed = harness.run(&["runner", "start", &task.to_string()]);
    let code = failed.status.code().expect("start exited normally");
    assert_ne!(code, 0, "a failed execution is not a success");
    assert_ne!(
        code, EXIT_DID_NOT_START,
        "an execution that started and failed is not a start that did nothing"
    );
    assert_eq!(
        harness.store().task(task).unwrap().readiness_status,
        ReadinessStatus::InterventionRequired
    );
}

#[cfg(unix)]
#[test]
fn serve_does_not_park_a_child_that_only_lost_a_race() {
    let harness = Runnable::new(None);
    let contested = harness.task("contested");
    let ordinary = harness.task("ordinary");
    harness.queue(contested);
    harness.queue(ordinary);

    // Stand in for a concurrent starter by holding the contested task's
    // execution lock. Every `runner start` serve spawns for it loses the race
    // and exits with the did-not-start code.
    let _held = lock::FileLock::try_acquire(&lock::task_lock_path(&harness.database, contested))
        .unwrap()
        .expect("the task lock is free");
    let mut serve = harness.spawn(&["runner", "serve"]);

    // The ordinary task completing proves serve made passes and reaped what
    // they spawned; a few more passes then give a wrong verdict time to land.
    wait_until(|| harness.is_completed(ordinary), 15, "the ordinary task");
    thread::sleep(Duration::from_secs(1));
    let observed = harness.store().task(contested).unwrap();
    serve.kill().unwrap();
    serve.wait().unwrap();

    assert_eq!(
        observed.intervention, None,
        "a child that started nothing is not the task's failure"
    );
    assert_eq!(observed.readiness_status, ReadinessStatus::Ready);
    assert_eq!(observed.execution_attempt, 0);
}

#[cfg(unix)]
#[test]
fn a_second_serve_refuses_and_a_concurrent_tick_does_not_double_start() {
    let harness = Runnable::new(Some(
        "sleep 2\nprintf '%s\\n' '{\"type\":\"result\",\"result\":\"slow result\"}'",
    ));
    let task = harness.task("slow");
    harness.queue(task);
    let mut serve = harness.spawn(&["runner", "serve"]);
    wait_until(
        || harness.readiness(task) == ReadinessStatus::Running,
        15,
        "the served execution to start",
    );

    let second = harness.run(&["runner", "serve"]);
    assert!(!second.status.success());
    assert!(
        stderr_of(&second).contains("serve lock"),
        "{}",
        stderr_of(&second)
    );

    let tick = harness.run(&["runner", "tick"]);
    assert!(tick.status.success(), "{}", stderr_of(&tick));
    assert_eq!(
        harness.readiness(task),
        ReadinessStatus::Running,
        "a concurrent tick must not take over a live execution"
    );

    wait_until(|| harness.is_completed(task), 20, "the slow task");
    assert_eq!(
        harness.store().task(task).unwrap().execution_attempt,
        1,
        "the task was started twice"
    );
    let _ = serve.kill();
    let _ = serve.wait();
}

#[cfg(unix)]
#[test]
fn serve_replenishes_a_freed_slot_while_its_other_children_are_still_live() {
    // Mixed durations make the replenishment observable: the first task frees a
    // slot while the other three are still deep in their harness, so a serve
    // that only counted its own children once starts the fifth immediately,
    // while one that counted them twice waits for the whole batch to drain.
    // A lengthy execution runs until the test releases it, so the assertions
    // have a wide margin on a loaded machine and the children still exit
    // promptly once the test is done with them.
    let harness = Runnable::new(Some(
        "here=$(cd \"$(dirname \"$0\")\" && pwd)\ncase $prompt in\n  *brief*) sleep 1 ;;\n  *) for _ in $(seq 1 600); do [ -e \"$here/release\" ] && break; sleep 0.1; done ;;\nesac\nprintf '%s\\n' '{\"type\":\"result\",\"result\":\"paced result\"}'",
    ));
    let brief = harness.task("brief task");
    let lengthy: Vec<i64> = (0..3)
        .map(|index| harness.task(&format!("lengthy task {index}")))
        .collect();
    // The queue is drained in task order, so the fifth task is the one left
    // over once the bound is full.
    let fifth = harness.task("lengthy fifth task");
    for id in std::iter::once(brief)
        .chain(lengthy.iter().copied())
        .chain(std::iter::once(fifth))
    {
        harness.queue(id);
    }

    // Sample the live executions rather than the rows: a held task lock is what
    // the bound actually counts, and the peak is the only honest way to show it
    // was never exceeded.
    let peak = Arc::new(AtomicUsize::new(0));
    let sampling = Arc::new(AtomicBool::new(true));
    let sampler = {
        let (peak, sampling, database) = (
            Arc::clone(&peak),
            Arc::clone(&sampling),
            harness.database.clone(),
        );
        thread::spawn(move || {
            let store = Store::open(&database).unwrap();
            while sampling.load(Ordering::Relaxed) {
                let live = store.live_executions().unwrap().len();
                peak.fetch_max(live, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(20));
            }
        })
    };

    let mut serve = harness.spawn(&["runner", "serve"]);
    wait_until(
        || peak.load(Ordering::Relaxed) >= CONCURRENCY_BOUND,
        20,
        "serve to start the first four executions",
    );
    wait_until(|| harness.is_completed(brief), 30, "the brief task");
    wait_until(
        || harness.readiness(fifth) == ReadinessStatus::Running,
        20,
        "serve to replenish the freed slot with the fifth execution",
    );
    for id in &lengthy {
        assert!(
            !harness.is_completed(*id),
            "the fifth execution must start while task {id} is still live, not after the batch drains"
        );
    }

    sampling.store(false, Ordering::Relaxed);
    sampler.join().unwrap();
    std::fs::write(harness.state.path().join("release"), b"").unwrap();
    let _ = serve.kill();
    let _ = serve.wait();
    assert_eq!(
        peak.load(Ordering::Relaxed),
        CONCURRENCY_BOUND,
        "a serve pass must hold the concurrency bound across the replenishment"
    );
}

#[cfg(unix)]
#[test]
fn a_killed_execution_releases_its_lock_and_is_immediately_not_live() {
    let harness = Runnable::new(Some(
        "sleep 10\nprintf '%s\\n' '{\"type\":\"result\",\"result\":\"never\"}'",
    ));
    let task = harness.task("killed");
    harness.queue(task);
    let mut execution = harness.spawn(&["runner", "start", &task.to_string()]);
    harness.wait_for_harness(task, &execution);
    assert!(lock::task_is_live(&harness.database, task).unwrap());

    execution.kill().unwrap();
    execution.wait().unwrap();
    // No grace period, no boot identity: the kernel released the lock when the
    // process ended, so the answer is available at once.
    assert!(!lock::task_is_live(&harness.database, task).unwrap());

    let mut store = harness.store();
    store.reconcile().unwrap();
    assert_eq!(
        store.task(task).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
}

#[cfg(unix)]
#[test]
fn a_killed_serve_leaves_its_orphaned_execution_alone() {
    let harness = Runnable::new(Some(
        "sleep 3\nprintf '%s\\n' '{\"type\":\"result\",\"result\":\"orphaned result\"}'",
    ));
    let task = harness.task("orphaned");
    harness.queue(task);
    let mut serve = harness.spawn(&["runner", "serve"]);
    wait_until(
        || harness.readiness(task) == ReadinessStatus::Running,
        15,
        "the served execution to start",
    );

    serve.kill().unwrap();
    serve.wait().unwrap();
    assert!(
        lock::task_is_live(&harness.database, task).unwrap(),
        "the orphaned execution still holds its lock"
    );

    let tick = harness.run(&["runner", "tick"]);
    assert!(tick.status.success(), "{}", stderr_of(&tick));
    let observed = harness.store().task(task).unwrap();
    assert_eq!(observed.readiness_status, ReadinessStatus::Running);
    assert_eq!(observed.execution_attempt, 1);
    assert_eq!(observed.intervention, None);

    // The orphan owns the task and finishes it.
    wait_until(|| harness.is_completed(task), 20, "the orphaned execution");
}

// ---------------------------------------------------------------------------
// Warnings and observation
// ---------------------------------------------------------------------------

#[test]
fn runner_status_reports_the_queue_depth_and_the_serve_lock_holder() {
    let (state, _project, mut store, root, _dependent) = graph();
    let database = state.path().join("mush.sqlite");
    store.queue(root).unwrap();
    drop(store);
    let _serve_lock = lock::acquire_serve_lock(&database, "test serve")
        .unwrap()
        .expect("the serve lock is free");

    // Queued work waits until a runner drains it, and `runner status` is how
    // the queue depth and its drainer are read.
    let status = mush(&database, &["runner", "status", "--json"]);
    assert!(status.status.success());
    let report: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(report["pending"], 1);
    assert!(
        report["serve_lock_holder"]
            .as_str()
            .unwrap()
            .contains("test serve")
    );
}

#[test]
fn wait_neither_reconciles_nor_launches() {
    let (state, _project, mut store, root, _dependent) = graph();
    let database = state.path().join("mush.sqlite");
    store.queue(root).unwrap();
    // A claim whose owner never took the execution lock: reconciliation would
    // return this to `ready`, so waiting must leave it exactly as it is.
    assert!(
        store
            .claim_launch_args(root, "absent", &mush::executor::boot_id(), u32::MAX)
            .unwrap()
    );
    let cursor = |database: &Path| -> i64 {
        rusqlite::Connection::open(database)
            .unwrap()
            .query_row(
                "SELECT COALESCE(MAX(id),0) FROM task_transitions",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = cursor(&database);
    drop(store);

    let wait = mush(
        &database,
        &[
            "--json",
            "task",
            "wait",
            &root.to_string(),
            "--timeout",
            "1",
        ],
    );
    assert!(wait.status.success());
    let observation: Value = serde_json::from_slice(&wait.stdout).unwrap();
    assert_eq!(observation["timed_out"], true);
    assert_eq!(cursor(&database), before, "wait wrote a transition");
    let store = Store::open(&database).unwrap();
    assert_eq!(
        store.task(root).unwrap().readiness_status,
        ReadinessStatus::Claimed
    );
    assert_eq!(store.task(root).unwrap().execution_attempt, 0);
}

#[test]
fn waiting_on_unqueued_work_warns_instead_of_hanging_silently() {
    let (state, _project, _store, root, _dependent) = graph();
    let database = state.path().join("mush.sqlite");
    let wait = mush(
        &database,
        &[
            "--json",
            "task",
            "wait",
            &root.to_string(),
            "--timeout",
            "0",
        ],
    );
    assert!(wait.status.success());
    let stderr = stderr_of(&wait);
    assert!(
        stderr.contains(&format!("#{root} not queued")),
        "expected an unqueued-task warning, got: {stderr}"
    );
    let value: Value = serde_json::from_slice(&wait.stdout).unwrap();
    assert_eq!(value["timed_out"], true);
}

#[test]
fn cli_status_and_wait_timeout_are_bounded_json() {
    let (state, _project, _store, root, dependent) = graph();
    let database = state.path().join("mush.sqlite");
    let status = mush(
        &database,
        &[
            "--json",
            "task",
            "status",
            &root.to_string(),
            &dependent.to_string(),
        ],
    );
    assert!(status.status.success());
    let value: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["tasks"].as_array().unwrap().len(), 2);
    let wait = mush(
        &database,
        &[
            "--json",
            "task",
            "wait",
            &root.to_string(),
            "--until",
            "all",
            "--timeout",
            "0",
        ],
    );
    assert!(wait.status.success());
    let value: Value = serde_json::from_slice(&wait.stdout).unwrap();
    assert_eq!(value["timed_out"], true);
    assert!(wait.stdout.len() < 16_384);
}

#[test]
fn wait_observes_a_transition_committed_during_setup() {
    let (state, _project, mut store, root, dependent) = graph();
    let database = state.path().join("mush.sqlite");
    let database_for_wait = database.clone();
    let waiter = thread::spawn(move || {
        mush(
            &database_for_wait,
            &[
                "--json",
                "task",
                "wait",
                &root.to_string(),
                &dependent.to_string(),
                "--until",
                "any",
                "--timeout",
                "5",
            ],
        )
    });
    thread::sleep(Duration::from_millis(300));
    store.complete_work(root, "done", None).unwrap();
    let output = waiter.join().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["terminal"], true);
    assert_eq!(value["timed_out"], false);
}

// ---------------------------------------------------------------------------
// Lock-based liveness in the domain
// ---------------------------------------------------------------------------

#[test]
fn reconciliation_requeues_an_abandoned_claim_and_parks_it_after_the_budget() {
    let (_state, _project, mut store, root, _dependent) = graph();
    let boot = mush::executor::boot_id();
    store.queue(root).unwrap();
    for attempt in 1..=2 {
        assert!(
            store
                .claim_launch_args(root, "absent", &boot, u32::MAX)
                .unwrap()
        );
        store.reconcile().unwrap();
        assert_eq!(
            store.task(root).unwrap().readiness_status,
            ReadinessStatus::Ready,
            "attempt {attempt} should return to the queue"
        );
    }
    assert!(
        store
            .claim_launch_args(root, "absent", &boot, u32::MAX)
            .unwrap()
    );
    store.reconcile().unwrap();
    let parked = store.task(root).unwrap();
    assert_eq!(
        parked.readiness_status,
        ReadinessStatus::InterventionRequired
    );
    assert!(
        parked.intervention.unwrap().contains("3 launch attempts"),
        "the attempt budget is bounded and visible"
    );
}

#[test]
fn reconciliation_never_touches_a_locked_execution() {
    let (state, _project, mut store, root, _dependent) = graph();
    let database = state.path().join("mush.sqlite");
    store.queue(root).unwrap();
    let _lock = mush::executor::acquire_execution_lock(&database, root).unwrap();
    assert!(
        store
            .claim_launch_args(root, "live", &mush::executor::boot_id(), std::process::id())
            .unwrap()
    );

    for _ in 0..3 {
        store.reconcile().unwrap();
    }
    assert_eq!(
        store.task(root).unwrap().readiness_status,
        ReadinessStatus::Claimed
    );
}

#[test]
fn reconciling_a_healthy_graph_writes_nothing() {
    let (state, _project, mut store, root, dependent) = graph();
    let database = state.path().join("mush.sqlite");
    store.add_dependency(root, dependent).unwrap();
    store.queue(root).unwrap();
    store.queue(dependent).unwrap();

    let transitions = || -> i64 {
        rusqlite::Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT COALESCE(MAX(id),0) FROM task_transitions",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = transitions();
    for _ in 0..4 {
        store.reconcile().unwrap();
    }
    assert_eq!(transitions(), before);
    assert_eq!(
        store.task(root).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );
}

#[test]
fn recovery_skips_a_task_whose_execution_lock_is_held() {
    let (state, _project, mut store, root, _dependent) = graph();
    let database = state.path().join("mush.sqlite");
    let project_id = store.task(root).unwrap().project_id;
    let _lock = mush::executor::acquire_execution_lock(&database, root).unwrap();
    store
        .begin_execution_args(
            root,
            None,
            None,
            &state.path().join("artifacts/live-recovery"),
            &mush::executor::boot_id(),
        )
        .unwrap();

    assert!(
        store
            .recover_launches(&[], Some(project_id))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.task(root).unwrap().execution_status,
        Some(mush::ExecutionStatus::Running)
    );
}

#[test]
fn an_execution_lock_refuses_a_second_holder() {
    let (state, _project, _store, root, _dependent) = graph();
    let database = state.path().join("mush.sqlite");
    let _held = mush::executor::acquire_execution_lock(&database, root).unwrap();
    let error = mush::executor::acquire_execution_lock(&database, root)
        .unwrap_err()
        .to_string();
    assert!(error.contains("already executing"), "{error}");
}

// ---------------------------------------------------------------------------
// Schema migration
// ---------------------------------------------------------------------------

/// A database at schema version 3, the version M3 shipped and one of the two
/// older inputs the migration accepts.
fn version_three_database(database: &Path) {
    let connection = rusqlite::Connection::open(database).unwrap();
    connection.execute_batch("CREATE TABLE projects(id INTEGER PRIMARY KEY,name TEXT NOT NULL UNIQUE,path TEXT NOT NULL UNIQUE); CREATE TABLE agents(id INTEGER PRIMARY KEY,project_id INTEGER NOT NULL REFERENCES projects(id),name TEXT NOT NULL,harness TEXT NOT NULL,model TEXT NOT NULL,settings TEXT NOT NULL,checkpoint INTEGER NOT NULL DEFAULT 0); CREATE TABLE tasks(id INTEGER PRIMARY KEY,project_id INTEGER NOT NULL REFERENCES projects(id),agent_id INTEGER REFERENCES agents(id),kind TEXT NOT NULL,status TEXT NOT NULL,description TEXT NOT NULL,result TEXT,evidence TEXT,decision TEXT,parent_task_id INTEGER,previous_task_id INTEGER,subject_task_id INTEGER,execution_status TEXT,execution_attempt INTEGER NOT NULL DEFAULT 0,session_id TEXT,worktree_name TEXT,artifact_dir TEXT,execution_boot_id TEXT,execution_pid INTEGER); INSERT INTO projects(id,name,path) VALUES(1,'legacy','/tmp/legacy'); INSERT INTO agents(id,project_id,name,harness,model,settings) VALUES(1,1,'worker','manual','model','{}'); INSERT INTO tasks(id,project_id,agent_id,kind,status,description,execution_status) VALUES(1,1,1,'work','pending','live legacy execution','running');").unwrap();
    connection.pragma_update(None, "user_version", 3).unwrap();
}

fn schema_version(database: &Path) -> i64 {
    rusqlite::Connection::open(database)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

#[test]
fn migration_backs_up_the_database_before_the_one_way_step() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    version_three_database(&database);

    let store = Store::open(&database).unwrap();
    assert_eq!(schema_version(&database), 12);
    let backup = state.path().join("mush.sqlite.pre-v12.bak");
    assert!(backup.is_file(), "the one-way step took a backup first");
    assert_eq!(
        schema_version(&backup),
        3,
        "the backup is the pre-migration database"
    );
    let task = store.task(1).unwrap();
    assert_eq!(task.execution_status, Some(mush::ExecutionStatus::Running));
    assert_eq!(task.readiness_status, ReadinessStatus::Unqueued);
}

#[test]
fn migration_refuses_while_another_process_holds_the_serve_lock() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    version_three_database(&database);
    let _serve_lock = lock::acquire_serve_lock(&database, "mush runner serve")
        .unwrap()
        .expect("the serve lock is free");

    let error = Store::open(&database)
        .err()
        .expect("open refused")
        .to_string();
    assert!(error.contains("serve lock"), "{error}");
    assert!(
        error.contains("mush runner serve"),
        "the holder is named: {error}"
    );
    assert_eq!(
        schema_version(&database),
        3,
        "nothing migrated underneath the holder"
    );
    assert!(
        !state.path().join("mush.sqlite.pre-v12.bak").exists(),
        "a refused migration takes no backup"
    );
}

#[test]
fn an_older_binary_refuses_a_newer_database() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    Store::open(&database).unwrap();
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute(
            "UPDATE schema_meta SET min_binary_version='99.0.0' WHERE id=1",
            [],
        )
        .unwrap();

    let error = Store::open(&database)
        .err()
        .expect("open refused")
        .to_string();
    assert!(error.contains("requires mush 99.0.0"), "{error}");
}

#[test]
fn unshipped_and_future_schema_versions_are_refused() {
    for (version, expected) in [(7_i64, "never shipped"), (13, "newer than supported")] {
        let state = tempfile::tempdir().unwrap();
        let database = state.path().join("mush.sqlite");
        rusqlite::Connection::open(&database)
            .unwrap()
            .pragma_update(None, "user_version", version)
            .unwrap();
        let error = Store::open(&database)
            .err()
            .expect("open refused")
            .to_string();
        assert!(error.contains(expected), "version {version}: {error}");
    }
}

// ---------------------------------------------------------------------------
// Dependency, readiness, delivery, journal, and recovery semantics
// ---------------------------------------------------------------------------

#[test]
fn dependencies_reject_self_duplicate_cycle_and_advance_blocked_queueing() {
    let (_state, _project, mut store, root, dependent) = graph();
    assert!(
        store
            .add_dependency(root, root)
            .unwrap_err()
            .to_string()
            .contains("itself")
    );
    store.add_dependency(root, dependent).unwrap();
    assert!(
        store
            .add_dependency(root, dependent)
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    assert!(
        store
            .add_dependency(dependent, root)
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
    assert_eq!(
        store.queue(dependent).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );
    assert_eq!(
        store.queue(root).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    store.complete_work(root, "done", None).unwrap();
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert_eq!(store.pending_launches(10).unwrap(), vec![dependent]);
}

#[test]
fn launch_claim_is_idempotent_across_connections_and_status_has_a_cursor() {
    let (state, _project, mut first, root, _dependent) = graph();
    first.queue(root).unwrap();
    let mut second = Store::open(&state.path().join("mush.sqlite")).unwrap();
    let boot = mush::executor::boot_id();
    assert!(
        first
            .claim_launch_args(root, "one", &boot, std::process::id())
            .unwrap()
    );
    assert!(
        !second
            .claim_launch_args(root, "two", &boot, std::process::id())
            .unwrap()
    );
    let observation = second.observe(&[root], true).unwrap();
    assert!(observation.cursor > 0);
    assert_eq!(
        observation.tasks[0].readiness_status,
        ReadinessStatus::Claimed
    );
}

#[test]
fn manual_execution_and_completion_preserve_queued_launch_invariants() {
    let (state, _project, mut store, root, dependent) = graph();
    store.add_dependency(root, dependent).unwrap();
    store.queue(dependent).unwrap();
    let completed = store
        .complete_work(dependent, "operator supplied result", None)
        .unwrap();
    assert_eq!(completed.readiness_status, ReadinessStatus::Completed);

    store.queue(root).unwrap();
    store
        .begin_execution_args(
            root,
            None,
            None,
            &state.path().join("artifacts/task-root"),
            &mush::executor::boot_id(),
        )
        .unwrap();
    assert_eq!(
        store.task(root).unwrap().readiness_status,
        ReadinessStatus::Running
    );
    let mut second = Store::open(&state.path().join("mush.sqlite")).unwrap();
    assert!(
        second
            .begin_execution_args(
                root,
                None,
                None,
                &state.path().join("artifacts/task-root"),
                &mush::executor::boot_id(),
            )
            .unwrap_err()
            .to_string()
            .contains("already executing")
    );
}

#[test]
fn failed_queued_foreground_execution_is_parked_atomically() {
    let (state, _project, mut store, root, _dependent) = graph();
    store.queue(root).unwrap();
    let running = store
        .begin_execution_args(
            root,
            None,
            None,
            &state.path().join("artifacts/task-root"),
            &mush::executor::boot_id(),
        )
        .unwrap();
    store
        .interrupt_execution_owned_with_intervention(
            mush::store::ExecutionOwner {
                task_id: root,
                execution_attempt: running.execution_attempt,
            },
            Some("foreground failed"),
        )
        .unwrap();
    let task = store.task(root).unwrap();
    assert_eq!(
        task.execution_status,
        Some(mush::ExecutionStatus::Interrupted)
    );
    assert_eq!(task.readiness_status, ReadinessStatus::InterventionRequired);
    assert_eq!(task.intervention.as_deref(), Some("foreground failed"));
}

#[test]
fn recovery_and_completion_preserve_prior_generation_diagnostic() {
    let (state, _project, mut store, root, _dependent) = graph();
    store.queue(root).unwrap();
    store
        .require_intervention(root, "generation one diagnostic")
        .unwrap();
    store.recover_launches(&[root], None).unwrap();
    store.complete_work(root, "manual result", None).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(state.path().join("mush.sqlite")).unwrap();
    let diagnostic: Option<String> = connection
        .query_row(
            "SELECT diagnostic FROM launch_deliveries WHERE task_id=?1 AND generation=1",
            [root],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(diagnostic.as_deref(), Some("generation one diagnostic"));
}

#[test]
fn recovery_is_explicitly_scoped() {
    let (_state, _project, mut store, root, dependent) = graph();
    store.queue(root).unwrap();
    store.queue(dependent).unwrap();
    store.require_intervention(root, "park root").unwrap();
    store
        .require_intervention(dependent, "park dependent")
        .unwrap();

    assert!(store.recover_launches(&[], None).is_err());
    assert_eq!(store.recover_launches(&[root], None).unwrap(), vec![root]);
    assert_eq!(
        store.task(root).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::InterventionRequired
    );
}

#[test]
fn recovery_re_derives_readiness_from_the_graph() {
    let (state, _project, mut store, root, dependent) = graph();
    let database = state.path().join("mush.sqlite");
    store.add_dependency(root, dependent).unwrap();
    store.queue(dependent).unwrap();
    // No public flow parks a task whose prerequisites are still incomplete —
    // intervention follows a claim, and a claim requires readiness. Write the
    // state directly so recovery is still tested against it.
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE tasks SET readiness_status='intervention_required',intervention='parked' WHERE id=?1",
            [dependent],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO launch_deliveries(task_id,generation,state) SELECT id,queue_generation,'intervention_required' FROM tasks WHERE id=?1",
            [dependent],
        )
        .unwrap();
    drop(connection);

    assert_eq!(
        store.recover_launches(&[dependent], None).unwrap(),
        vec![dependent]
    );
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );
    assert!(!store.pending_launches(10).unwrap().contains(&dependent));

    store.complete_work(root, "done", None).unwrap();
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert!(store.pending_launches(10).unwrap().contains(&dependent));
}

#[test]
fn completed_task_is_not_resurrected_by_recovery() {
    let (_state, _project, mut store, root, _dependent) = graph();
    store.queue(root).unwrap();
    let boot = mush::executor::boot_id();
    assert!(
        store
            .claim_launch_args(root, "absent", &boot, u32::MAX)
            .unwrap()
    );
    store.reconcile().unwrap();
    store
        .complete_work(root, "manual completion", None)
        .unwrap();
    store.recover_launches(&[root], None).unwrap();
    let task = store.task(root).unwrap();
    assert_eq!(task.status, mush::TaskStatus::Completed);
    assert_eq!(task.readiness_status, ReadinessStatus::Completed);
    assert!(!store.pending_launches(32).unwrap().contains(&root));
}

#[test]
fn transition_audit_window_is_bounded() {
    let (state, _project, mut store, root, _dependent) = graph();
    let database = state.path().join("mush.sqlite");
    let mut connection = rusqlite::Connection::open(&database).unwrap();
    let transaction = connection.transaction().unwrap();
    {
        let mut insert = transaction.prepare("INSERT INTO task_transitions(task_id,kind,status,execution_status,readiness_status) VALUES(?1,'audit-probe','pending',NULL,'unqueued')").unwrap();
        for _ in 0..10_005 {
            insert.execute([root]).unwrap();
        }
    }
    transaction.commit().unwrap();
    drop(connection);

    store.queue(root).unwrap();
    let count: i64 = rusqlite::Connection::open(database)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM task_transitions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 10_000);
}

#[test]
fn tui_snapshot_exposes_dependency_readiness() {
    let (_state, _project, mut store, root, dependent) = graph();
    store.add_dependency(root, dependent).unwrap();
    store.queue(dependent).unwrap();
    let snapshot = mush::tui::snapshot(&store, None).unwrap();
    assert!(snapshot.contains("readiness: blocked"));
}

#[test]
fn dependency_validation_covers_projects_kinds_late_edges_and_limit() {
    let (state, _project_dir, mut store, root, dependent) = graph();
    let other_project_dir = tempfile::tempdir().unwrap();
    let other = store
        .register_project("other", other_project_dir.path())
        .unwrap();
    let other_agent = store
        .register_agent_args(other.id, "worker", "manual", "model", "{}", false)
        .unwrap();
    let foreign = store
        .add_work_task_args(other.id, other_agent.id, "foreign", None)
        .unwrap();
    assert!(
        store
            .add_dependency(foreign.id, dependent)
            .unwrap_err()
            .to_string()
            .contains("same project")
    );
    let project_id = store.task(root).unwrap().project_id;
    store
        .register_agent_args(
            project_id,
            "reviewer",
            "manual-reviewer",
            "model",
            "{}",
            true,
        )
        .unwrap();
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap();
    assert!(
        store
            .add_dependency(root, checkpoint.id)
            .unwrap_err()
            .to_string()
            .contains("only a work task can be a dependent")
    );
    let boot = mush::executor::boot_id();
    store.queue(dependent).unwrap();
    assert!(
        store
            .claim_launch_args(dependent, "runner", &boot, std::process::id())
            .unwrap()
    );
    assert!(
        store
            .add_dependency(root, dependent)
            .unwrap_err()
            .to_string()
            .contains("after the dependent starts")
    );

    let database = state.path().join("limit.sqlite");
    let limit_project_dir = tempfile::tempdir().unwrap();
    let limit_store = Store::open(&database).unwrap();
    let project = limit_store
        .register_project("limit", limit_project_dir.path())
        .unwrap();
    let agent = limit_store
        .register_agent_args(project.id, "worker", "manual", "model", "{}", false)
        .unwrap();
    let target = limit_store
        .add_work_task_args(project.id, agent.id, "target", None)
        .unwrap();
    let mut limit_store = limit_store;
    for index in 0..8 {
        let prerequisite = limit_store
            .add_work_task_args(project.id, agent.id, &format!("p{index}"), None)
            .unwrap();
        limit_store
            .add_dependency(prerequisite.id, target.id)
            .unwrap();
    }
    let ninth = limit_store
        .add_work_task_args(project.id, agent.id, "ninth", None)
        .unwrap();
    assert!(
        limit_store
            .add_dependency(ninth.id, target.id)
            .unwrap_err()
            .to_string()
            .contains("more than 8")
    );
}

#[test]
fn completed_never_queued_dependents_reject_dependency_edits() {
    let (_state, _project, mut store, prerequisite, dependent) = graph();
    let task = store.task(prerequisite).unwrap();
    store.complete_work(dependent, "done", None).unwrap();
    assert!(store.add_dependency(prerequisite, dependent).is_err());

    let prerequisite = store
        .add_work_task_args(
            task.project_id,
            task.agent_id.unwrap(),
            "another prerequisite",
            None,
        )
        .unwrap();
    let dependent = store
        .add_work_task_args(
            task.project_id,
            task.agent_id.unwrap(),
            "another dependent",
            None,
        )
        .unwrap();
    store.add_dependency(prerequisite.id, dependent.id).unwrap();
    store.complete_work(dependent.id, "done", None).unwrap();
    assert!(
        store
            .remove_dependency(prerequisite.id, dependent.id)
            .is_err()
    );
}

#[test]
fn graph_edits_recompute_queued_readiness_and_fanout_join() {
    let (_state, _project, mut store, root, dependent) = graph();
    store.queue(dependent).unwrap();
    store.add_dependency(root, dependent).unwrap();
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );
    assert!(!store.pending_launches(10).unwrap().contains(&dependent));
    store.remove_dependency(root, dependent).unwrap();
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert!(store.pending_launches(10).unwrap().contains(&dependent));

    let project = store.task(root).unwrap().project_id;
    let agent = store.task(root).unwrap().agent_id.unwrap();
    let left = store
        .add_work_task_args(project, agent, "left", None)
        .unwrap();
    let right = store
        .add_work_task_args(project, agent, "right", None)
        .unwrap();
    let join = store
        .add_work_task_args(project, agent, "join", None)
        .unwrap();
    store.add_dependency(root, left.id).unwrap();
    store.add_dependency(root, right.id).unwrap();
    store.add_dependency(left.id, join.id).unwrap();
    store.add_dependency(right.id, join.id).unwrap();
    store.queue(left.id).unwrap();
    store.queue(right.id).unwrap();
    store.queue(join.id).unwrap();
    store.complete_work(root, "root", None).unwrap();
    assert_eq!(
        store.task(left.id).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert_eq!(
        store.task(right.id).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    store.complete_work(left.id, "left", None).unwrap();
    assert_eq!(
        store.task(join.id).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );
    store.complete_work(right.id, "right", None).unwrap();
    assert_eq!(
        store.task(join.id).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
}

#[test]
fn live_foreground_execution_cannot_be_queued() {
    let (state, _project, mut store, root, _dependent) = graph();
    store
        .begin_execution_args(
            root,
            None,
            None,
            &state.path().join("artifacts/task-live"),
            &mush::executor::boot_id(),
        )
        .unwrap();

    let completion_error = store
        .complete_work(root, "too early", None)
        .unwrap_err()
        .to_string();
    assert!(
        completion_error.contains("already executing"),
        "{completion_error}"
    );
    let error = store.queue(root).unwrap_err().to_string();
    assert!(error.contains("already executing"), "{error}");
    assert_eq!(
        store.task(root).unwrap().readiness_status,
        ReadinessStatus::Unqueued
    );
}

#[test]
fn queueing_clears_a_stale_intervention_message() {
    let (state, _project, mut store, root, _dependent) = graph();
    rusqlite::Connection::open(state.path().join("mush.sqlite"))
        .unwrap()
        .execute(
            "UPDATE tasks SET intervention='stale diagnostic' WHERE id=?1",
            [root],
        )
        .unwrap();

    let queued = store.queue(root).unwrap();
    assert_eq!(queued.intervention, None);
}

#[test]
fn stale_checkpoint_executor_cannot_finish_a_replacement_attempt() {
    let (state, _project, mut store, root, _dependent) = graph();
    let project = store.task(root).unwrap().project_id;
    store
        .register_agent_args(project, "reviewer", "manual", "human", "{}", true)
        .unwrap();
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap();
    let boot = mush::executor::boot_id();
    let first = store
        .begin_execution_args(
            checkpoint.id,
            None,
            None,
            &state.path().join("artifacts/checkpoint"),
            &boot,
        )
        .unwrap();
    store.interrupt_execution(checkpoint.id).unwrap();
    store
        .begin_execution_args(
            checkpoint.id,
            None,
            None,
            &state.path().join("artifacts/checkpoint"),
            &boot,
        )
        .unwrap();

    let error = store
        .finish_checkpoint_execution(
            mush::store::ExecutionOwner {
                task_id: checkpoint.id,
                execution_attempt: first.execution_attempt,
            },
            "stale",
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("ownership changed"), "{error}");
}

#[test]
fn stale_executor_cannot_finish_a_replacement_attempt() {
    let (state, _project, mut store, root, _dependent) = graph();
    let boot = mush::executor::boot_id();
    let first = store
        .begin_execution_args(
            root,
            None,
            None,
            &state.path().join("artifacts/task-owned"),
            &boot,
        )
        .unwrap();
    store.interrupt_execution(root).unwrap();
    let second = store
        .begin_execution_args(
            root,
            None,
            None,
            &state.path().join("artifacts/task-owned"),
            &boot,
        )
        .unwrap();
    assert!(second.execution_attempt > first.execution_attempt);

    let error = store
        .finish_work_execution_args(root, first.execution_attempt, "stale", "stale")
        .unwrap_err()
        .to_string();
    assert!(error.contains("ownership changed"), "{error}");
    assert_eq!(store.task(root).unwrap().status, mush::TaskStatus::Pending);
}

#[test]
fn delivered_spawn_diagnostics_are_not_reported_as_interventions() {
    let (state, _project, mut store, root, _dependent) = graph();
    store.queue(root).unwrap();
    rusqlite::Connection::open(state.path().join("mush.sqlite"))
        .unwrap()
        .execute(
            "UPDATE launch_deliveries SET state='delivered',diagnostic='stale spawn failure' WHERE task_id=?1",
            [root],
        )
        .unwrap();

    assert_eq!(store.task(root).unwrap().intervention, None);
}

#[test]
fn observation_enforces_aggregate_text_bound() {
    let (_state, _project, mut store, root, dependent) = graph();
    let huge = "x".repeat(100_000);
    store.complete_work(root, &huge, None).unwrap();
    let project = store.task(root).unwrap().project_id;
    let agent = store.task(root).unwrap().agent_id.unwrap();
    let mut ids = vec![root, dependent];
    for index in 0..30 {
        ids.push(
            store
                .add_work_task_args(project, agent, &format!("{index}-{huge}"), None)
                .unwrap()
                .id,
        );
    }
    let json = serde_json::to_vec(&store.observe(&ids, true).unwrap()).unwrap();
    let observation = store.observe(&ids, true).unwrap();
    assert!(observation.elided);
    assert_eq!(observation.tasks[0].result, None);
    assert!(
        observation
            .tasks
            .iter()
            .skip(2)
            .all(|task| task.description == "[elided; inspect task artifacts]")
    );
    let observed_text: usize = observation
        .tasks
        .iter()
        .map(|task| task.description.len())
        .sum();
    assert!(observed_text <= 32 * 1024);
    assert!(
        json.len() < 64 * 1024,
        "observation was {} bytes",
        json.len()
    );
}

#[test]
fn completed_graph_stops_until_a_new_coordinator_extends_it() {
    let (_state, _project, mut store, root, dependent) = graph();
    store.add_dependency(root, dependent).unwrap();
    store.complete_work(root, "root", None).unwrap();
    store.complete_work(dependent, "dependent", None).unwrap();
    assert_eq!(store.tasks(None).unwrap().len(), 2);
    let project = store.task(root).unwrap().project_id;
    let agent = store.task(root).unwrap().agent_id.unwrap();
    let extension = store
        .add_work_task_args(project, agent, "fresh coordinator judgment", None)
        .unwrap();
    store.add_dependency(dependent, extension.id).unwrap();
    assert_eq!(
        store.queue(extension.id).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert_eq!(store.tasks(None).unwrap().len(), 3);
}

#[cfg(unix)]
#[test]
fn a_missing_executable_becomes_a_specific_intervention_and_can_be_recovered() {
    let harness = Runnable::unrunnable();
    let task = harness.task("cannot launch");
    harness.queue(task);

    let start = harness.run(&["runner", "start", &task.to_string()]);
    assert!(!start.status.success());
    let parked = harness.store().task(task).unwrap();
    assert_eq!(
        parked.readiness_status,
        ReadinessStatus::InterventionRequired
    );
    let intervention = parked.intervention.unwrap();
    assert!(
        intervention.contains(&format!("execution of task {task} failed")),
        "{intervention}"
    );
    assert!(
        intervention.contains("version") || intervention.contains("I/O error"),
        "{intervention}"
    );

    let mut store = harness.store();
    let generation = store.task(task).unwrap().queue_generation;
    store.recover_launches(&[task], None).unwrap();
    assert_eq!(store.task(task).unwrap().queue_generation, generation + 1);
}

#[test]
fn invalid_runtime_configuration_becomes_intervention() {
    let (state, _project, mut store, root, _dependent) = graph();
    let database = state.path().join("mush.sqlite");
    store.queue(root).unwrap();
    drop(store);

    let start = mush(&database, &["runner", "start", &root.to_string()]);
    assert!(!start.status.success());
    let task = Store::open(&database).unwrap().task(root).unwrap();
    assert_eq!(task.readiness_status, ReadinessStatus::InterventionRequired);
    assert!(task.intervention.unwrap().contains("unsupported harness"));
}

// ---------------------------------------------------------------------------
// Checkpoint subject prerequisites and decision-sensitive readiness
// ---------------------------------------------------------------------------

/// Register a manual human reviewer as the project's checkpoint agent.
fn manual_reviewer(store: &Store, task_id: i64) -> i64 {
    let project_id = store.task(task_id).unwrap().project_id;
    store
        .register_agent_args(project_id, "reviewer", "manual", "human", "{}", true)
        .unwrap()
        .id
}

/// Register an executable checkpoint reviewer whose binary never runs; these
/// tests drive the store directly, so only the registration must be executable.
#[cfg(unix)]
fn executable_reviewer(store: &Store, task_id: i64) -> i64 {
    let project_id = store.task(task_id).unwrap().project_id;
    store
        .register_agent_args(
            project_id,
            "reviewer",
            "claude-code",
            "model",
            &claude_settings(Path::new("/nonexistent/claude"), Some("Review.")),
            true,
        )
        .unwrap()
        .id
}

#[cfg(unix)]
#[test]
fn a_declared_work_and_checkpoint_chain_advances_unattended_through_the_runner() {
    let runnable = Runnable::new(None);
    runnable.reviewer();
    let work = runnable.task("work under review");
    let mut store = runnable.store();
    let checkpoint = store.create_checkpoint(work).unwrap().id;
    let gated = runnable.task("gated by acceptance");
    store.add_dependency(checkpoint, gated).unwrap();
    for id in [work, checkpoint, gated] {
        runnable.queue(id);
    }
    assert_eq!(runnable.readiness(work), ReadinessStatus::Ready);
    assert_eq!(runnable.readiness(checkpoint), ReadinessStatus::Blocked);
    assert_eq!(runnable.readiness(gated), ReadinessStatus::Blocked);

    // The worker door claims the only ready task: the work. Its completion
    // advances the checkpoint whose subject it is, but not the gated task.
    assert!(runnable.run(&["runner", "start"]).status.success());
    assert!(runnable.is_completed(work));
    assert_eq!(runnable.readiness(checkpoint), ReadinessStatus::Ready);
    assert_eq!(runnable.readiness(gated), ReadinessStatus::Blocked);

    // The next start runs the review. The queue is then done with the
    // checkpoint, while the pending status says the decision is still owed.
    assert!(runnable.run(&["runner", "start"]).status.success());
    let reviewed = runnable.store().task(checkpoint).unwrap();
    assert_eq!(reviewed.status, mush::TaskStatus::Pending);
    assert_eq!(
        reviewed.execution_status,
        Some(mush::ExecutionStatus::Succeeded)
    );
    assert_eq!(reviewed.readiness_status, ReadinessStatus::Completed);
    assert!(
        !runnable
            .store()
            .observe(&[checkpoint], true)
            .unwrap()
            .terminal,
        "an undecided review is not terminal for observation"
    );
    assert_eq!(runnable.readiness(gated), ReadinessStatus::Blocked);
    let snapshot = runnable.run(&["tui", "--snapshot"]);
    assert!(
        String::from_utf8_lossy(&snapshot.stdout).contains("awaiting decision"),
        "the snapshot names the adjudication wait"
    );

    // Acceptance settles the subject and readies the gated dependent, which
    // the next start then executes.
    let decided = runnable.run(&[
        "checkpoint",
        "decide",
        &checkpoint.to_string(),
        "--decision",
        "accepted",
        "--evidence",
        "review passed",
    ]);
    assert!(decided.status.success(), "{}", stderr_of(&decided));
    assert!(
        runnable
            .store()
            .observe(&[checkpoint], true)
            .unwrap()
            .terminal
    );
    assert_eq!(runnable.readiness(gated), ReadinessStatus::Ready);
    assert!(runnable.run(&["runner", "start"]).status.success());
    assert!(runnable.is_completed(gated));
}

#[test]
fn acceptance_and_requested_revision_advance_the_graph_differently() {
    // Requested revision completes the checkpoint, creates the linked
    // revision, and leaves the gated dependent blocked.
    let (_state, _project_dir, mut store, root, dependent) = graph();
    manual_reviewer(&store, root);
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    store.add_dependency(checkpoint, dependent).unwrap();
    store.queue(dependent).unwrap();
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );
    let follow_up = store
        .decide_checkpoint(
            checkpoint,
            mush::CheckpointDecision::RevisionRequested,
            "needs another pass",
        )
        .unwrap()
        .expect("a requested revision creates the linked follow-up");
    assert_eq!(follow_up.previous_task_id, Some(root));
    let decided = store.task(checkpoint).unwrap();
    assert_eq!(decided.status, mush::TaskStatus::Completed);
    assert_eq!(decided.readiness_status, ReadinessStatus::Unqueued);
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Blocked,
        "a requested revision never satisfies a checkpoint prerequisite"
    );
    assert!(store.pending_launches(8).unwrap().is_empty());

    // Acceptance readies the gated dependent atomically with the decision.
    let (_state, _project_dir, mut store, root, dependent) = graph();
    manual_reviewer(&store, root);
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    store.add_dependency(checkpoint, dependent).unwrap();
    store.queue(dependent).unwrap();
    assert!(
        store
            .decide_checkpoint(checkpoint, mush::CheckpointDecision::Accepted, "fine")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert_eq!(store.pending_launches(8).unwrap(), vec![dependent]);

    // A blocked decision completes the checkpoint and advances nothing.
    let (_state, _project_dir, mut store, root, dependent) = graph();
    manual_reviewer(&store, root);
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    store.add_dependency(checkpoint, dependent).unwrap();
    store.queue(dependent).unwrap();
    assert!(
        store
            .decide_checkpoint(
                checkpoint,
                mush::CheckpointDecision::Blocked,
                "missing authority"
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.task(checkpoint).unwrap().status,
        mush::TaskStatus::Completed
    );
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );
}

#[cfg(unix)]
#[test]
fn a_queued_checkpoint_blocks_until_its_subject_completes() {
    let (state, _project_dir, mut store, root, _dependent) = graph();
    executable_reviewer(&store, root);
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    store.queue(checkpoint).unwrap();
    assert_eq!(
        store.task(checkpoint).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );
    assert!(store.pending_launches(8).unwrap().is_empty());
    let error = store
        .begin_execution_args(
            checkpoint,
            None,
            None,
            &state.path().join("artifacts"),
            "boot",
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("prerequisites are not completed"), "{error}");
    let error = store
        .decide_checkpoint(checkpoint, mush::CheckpointDecision::Accepted, "early")
        .unwrap_err()
        .to_string();
    assert!(error.contains("awaiting its subject"), "{error}");

    store.complete_work(root, "done", None).unwrap();
    assert_eq!(
        store.task(checkpoint).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
    assert_eq!(store.pending_launches(8).unwrap(), vec![checkpoint]);

    // Deciding a ready checkpoint by hand settles its queue bookkeeping too.
    store
        .decide_checkpoint(checkpoint, mush::CheckpointDecision::Accepted, "fine")
        .unwrap();
    assert_eq!(
        store.task(checkpoint).unwrap().readiness_status,
        ReadinessStatus::Completed
    );
    assert!(store.pending_launches(8).unwrap().is_empty());
}

#[test]
fn queueing_a_checkpoint_with_a_manual_reviewer_is_refused() {
    let (_state, _project_dir, mut store, root, _dependent) = graph();
    manual_reviewer(&store, root);
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    let error = store.queue(checkpoint).unwrap_err().to_string();
    assert!(error.contains("reviewer Mush cannot execute"), "{error}");
    assert_eq!(
        store.task(checkpoint).unwrap().readiness_status,
        ReadinessStatus::Unqueued
    );
}

#[cfg(unix)]
#[test]
fn an_executed_undecided_review_rests_until_its_decision() {
    let (state, _project_dir, mut store, root, _dependent) = graph();
    executable_reviewer(&store, root);
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    store.queue(checkpoint).unwrap();
    let boot = mush::executor::boot_id();
    assert!(
        store
            .claim_launch_args(checkpoint, "runner", &boot, std::process::id())
            .unwrap()
    );
    store
        .begin_execution_owned(mush::store::ExecutionStart {
            task_id: checkpoint,
            session_id: Some("review-session"),
            worktree_name: None,
            artifact_dir: &state.path().join("artifacts"),
            boot_id: &boot,
            claimed_runner_id: Some("runner"),
        })
        .unwrap();
    let rested = store
        .finish_checkpoint_execution(
            mush::store::ExecutionOwner {
                task_id: checkpoint,
                execution_attempt: 1,
            },
            "## Review\n\nlooks plausible",
        )
        .unwrap();
    assert_eq!(rested.status, mush::TaskStatus::Pending);
    assert_eq!(rested.readiness_status, ReadinessStatus::Completed);

    // Neither reconciliation nor recovery restarts an executed review that is
    // only waiting on its decision.
    store.reconcile().unwrap();
    assert!(
        store
            .recover_launches(&[checkpoint], None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.task(checkpoint).unwrap().readiness_status,
        ReadinessStatus::Completed
    );

    // An interrupted re-review is ordinary abandoned work and is requeued.
    store
        .begin_execution_args(
            checkpoint,
            None,
            None,
            &state.path().join("artifacts"),
            &boot,
        )
        .unwrap();
    store.interrupt_execution(checkpoint).unwrap();
    store.reconcile().unwrap();
    assert_eq!(
        store.task(checkpoint).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
}

#[cfg(unix)]
#[test]
fn deciding_a_checkpoint_during_its_own_execution_keeps_one_coherent_outcome() {
    let (state, _project_dir, mut store, root, dependent) = graph();
    executable_reviewer(&store, root);
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    store.add_dependency(checkpoint, dependent).unwrap();
    store.queue(dependent).unwrap();
    store.queue(checkpoint).unwrap();
    let boot = mush::executor::boot_id();
    assert!(
        store
            .claim_launch_args(checkpoint, "runner", &boot, std::process::id())
            .unwrap()
    );
    store
        .begin_execution_owned(mush::store::ExecutionStart {
            task_id: checkpoint,
            session_id: Some("review-session"),
            worktree_name: None,
            artifact_dir: &state.path().join("artifacts"),
            boot_id: &boot,
            claimed_runner_id: Some("runner"),
        })
        .unwrap();

    // The reviewing agent adjudicates through the CLI while its execution owns
    // the task; the graph advances on the decision.
    store
        .decide_checkpoint(
            checkpoint,
            mush::CheckpointDecision::Accepted,
            "adjudicated during review",
        )
        .unwrap();
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Ready
    );

    // The execution then finishes by recording its success without disturbing
    // the decision or its evidence.
    let finished = store
        .finish_checkpoint_execution(
            mush::store::ExecutionOwner {
                task_id: checkpoint,
                execution_attempt: 1,
            },
            "executor-assembled evidence",
        )
        .unwrap();
    assert_eq!(finished.status, mush::TaskStatus::Completed);
    assert_eq!(finished.decision, Some(mush::CheckpointDecision::Accepted));
    assert_eq!(
        finished.evidence.as_deref(),
        Some("adjudicated during review")
    );
    assert_eq!(
        finished.execution_status,
        Some(mush::ExecutionStatus::Succeeded)
    );
}

#[test]
fn an_edge_gating_a_checkpoints_subject_is_rejected_directly_and_transitively() {
    let (_state, _project_dir, mut store, root, other) = graph();
    manual_reviewer(&store, root);
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    let error = store
        .add_dependency(checkpoint, root)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot gate its own subject"), "{error}");
    store.add_dependency(checkpoint, other).unwrap();
    let error = store.add_dependency(other, root).unwrap_err().to_string();
    assert!(
        error.contains("cycle"),
        "subject links count as readiness edges: {error}"
    );
}

#[cfg(unix)]
#[test]
fn concurrent_starts_do_not_double_run_a_ready_checkpoint() {
    let runnable = Runnable::new(None);
    runnable.reviewer();
    let work = runnable.task("work under review");
    runnable.queue(work);
    assert!(runnable.run(&["runner", "start"]).status.success());
    let mut store = runnable.store();
    let checkpoint = store.create_checkpoint(work).unwrap().id;
    runnable.queue(checkpoint);
    let id = checkpoint.to_string();
    let racers = [
        runnable.spawn_captured(&["runner", "start", &id]),
        runnable.spawn_captured(&["runner", "start", &id]),
    ];
    let mut codes: Vec<i32> = racers
        .map(|racer| racer.wait_with_output().unwrap().status.code().unwrap())
        .into_iter()
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, vec![0, EXIT_DID_NOT_START]);
    let reviewed = runnable.store().task(checkpoint).unwrap();
    assert_eq!(reviewed.execution_attempt, 1);
    assert_eq!(reviewed.readiness_status, ReadinessStatus::Completed);
}

#[test]
fn tui_snapshot_exposes_checkpoint_decision_states() {
    let (_state, _project_dir, mut store, root, _dependent) = graph();
    manual_reviewer(&store, root);
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    let snapshot = mush::tui::snapshot(&store, None).unwrap();
    assert!(snapshot.contains(&format!("awaiting subject: #{root} not completed")));

    store.complete_work(root, "done", None).unwrap();
    let snapshot = mush::tui::snapshot(&store, None).unwrap();
    assert!(snapshot.contains("awaiting decision"), "{snapshot}");

    store
        .decide_checkpoint(checkpoint, mush::CheckpointDecision::Accepted, "fine")
        .unwrap();
    let snapshot = mush::tui::snapshot(&store, None).unwrap();
    assert!(!snapshot.contains("awaiting decision"), "{snapshot}");
    assert!(snapshot.contains("decision: accepted"), "{snapshot}");
}

#[test]
fn a_version_eleven_database_migrates_to_twelve_with_the_new_triggers() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    drop(Store::open(&database).unwrap());
    {
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "DROP TRIGGER checkpoint_subject_gate_on_insert;
                 DROP TRIGGER checkpoint_readiness_requires_completed_subject;
                 DROP TRIGGER checkpoint_execution_requires_completed_subject;
                 DROP TRIGGER checkpoint_decision_requires_completed_subject;",
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 11).unwrap();
    }

    drop(Store::open(&database).unwrap());
    assert_eq!(schema_version(&database), 12);
    assert!(
        state.path().join("mush.sqlite.pre-v12.bak").is_file(),
        "the one-way step took a backup first"
    );
    let connection = rusqlite::Connection::open(&database).unwrap();
    let triggers: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'checkpoint_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(triggers, 4, "migration reinstalled the subject gates");
}

#[cfg(unix)]
#[test]
fn a_decision_adjudicates_past_a_parked_review() {
    let (state, _project_dir, mut store, root, dependent) = graph();
    executable_reviewer(&store, root);
    store.complete_work(root, "done", None).unwrap();
    let checkpoint = store.create_checkpoint(root).unwrap().id;
    store.add_dependency(checkpoint, dependent).unwrap();
    store.queue(dependent).unwrap();
    store.queue(checkpoint).unwrap();
    let boot = mush::executor::boot_id();
    assert!(
        store
            .claim_launch_args(checkpoint, "runner", &boot, std::process::id())
            .unwrap()
    );
    store
        .begin_execution_owned(mush::store::ExecutionStart {
            task_id: checkpoint,
            session_id: Some("review-session"),
            worktree_name: None,
            artifact_dir: &state.path().join("artifacts"),
            boot_id: &boot,
            claimed_runner_id: Some("runner"),
        })
        .unwrap();
    store
        .interrupt_execution_owned_with_intervention(
            mush::store::ExecutionOwner {
                task_id: checkpoint,
                execution_attempt: 1,
            },
            Some("review harness failed"),
        )
        .unwrap();
    assert_eq!(
        store.task(checkpoint).unwrap().readiness_status,
        ReadinessStatus::InterventionRequired
    );

    // A human can still adjudicate from the subject's recorded evidence; the
    // decision clears the parked state and advances the gated dependent.
    store
        .decide_checkpoint(
            checkpoint,
            mush::CheckpointDecision::Accepted,
            "reviewed by hand",
        )
        .unwrap();
    let decided = store.task(checkpoint).unwrap();
    assert_eq!(decided.status, mush::TaskStatus::Completed);
    assert_eq!(decided.readiness_status, ReadinessStatus::Completed);
    assert_eq!(decided.intervention, None);
    assert_eq!(
        store.task(dependent).unwrap().readiness_status,
        ReadinessStatus::Ready
    );
}
