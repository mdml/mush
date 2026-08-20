//! The execution surface: one engine behind two front doors.
//!
//! [`run`] is the human front door, behind `mush task run <id>`: it executes
//! the named task in the foreground and carries the worktree, session, and
//! prompt options. [`start`] is the worker front door, behind
//! `mush runner start [id]`, and without an id it claims the next eligible
//! ready task — the one thing only it does. Neither refuses the other's case:
//! the shared engine reads the task's durable readiness to decide what
//! bookkeeping the execution owes, so the user never picks a command by a fact
//! the database already holds.
//!
//! Nothing here starts work implicitly. `tick` is one bounded pass and `serve`
//! repeats that pass until terminated; both start work only by spawning
//! `mush runner start <task-id>`, and Mush ships no supervisor: an operator, a
//! service manager, a container, a periodic scheduler, or an agent chooses how
//! the role is held.

use crate::{
    DomainError, Executor, ReadinessStatus, RunnerReport, Store,
    executor::{LockedRun, RunOptions},
    lock,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::Duration,
};

/// The exit codes of `mush runner start`, shared with `mush task run` because
/// both front doors run the same engine. They are a public contract: a restart
/// loop, a `systemd` unit, or a supervising `serve` reads the code and knows
/// what happened without probing the database or the task's lock afterwards.
///
/// `0` means an execution ran to a terminal state. [`EXIT_DID_NOT_START`] means
/// this process started no execution at all — nothing was eligible, or every
/// candidate was already held by another runner — which is the queue behaving
/// normally rather than a fault. Any other nonzero code means an execution was
/// started and failed, or the command was refused; both deserve attention.
pub const EXIT_EXECUTED: i32 = 0;

/// The exit code meaning "I did not start an execution".
///
/// Deliberately `3` rather than `2`, so it can never be confused with clap's
/// usage-error exit, and deliberately distinct from the generic `1` that a real
/// failure uses. A caller looping over `runner start` treats it as "nothing to
/// do, back off and retry", never as "something is broken".
pub const EXIT_DID_NOT_START: i32 = 3;

/// The outcome of one execution attempt: either an execution ran, or none
/// began.
///
/// Distinguishing these in the type is what lets the exit code be exact. "No
/// execution began" is not an error, so it is not reported as one.
#[derive(Debug)]
pub enum Started {
    /// This process executed the named task to a terminal state.
    Task(i64),
    /// No execution began, for the stated reason.
    Nothing(String),
}

/// How many executions one pass may have in flight. A single documented
/// constant, sized for agent harnesses; scheduling policy is deliberately not
/// part of this slice.
pub const CONCURRENCY_BOUND: usize = 4;

/// How long `serve` waits between passes. This is loop cadence, not a liveness
/// heuristic: correctness comes from the durable rows and the execution locks.
const SERVE_PASS_INTERVAL: Duration = Duration::from_millis(250);

/// The worker front door: execute one task in this process and return when it
/// reaches a terminal state, or report that no execution began.
///
/// Without an id this claims the next eligible ready task, which is the one
/// thing only this command does. With an id it executes that task in whatever
/// readiness state it is in; an unqueued one is not refused, it simply owes the
/// queue nothing.
///
/// Nothing eligible, and a candidate another process already holds, are both
/// [`Started::Nothing`]: this process did no work and nothing is wrong. Only a
/// started execution that fails is an error.
pub fn start(store: &mut Store, task_id: Option<i64>) -> Result<Started, DomainError> {
    let database = store.database_path().to_path_buf();
    let candidates = match task_id {
        Some(id) => {
            // Read the task so an unknown id is reported as such before any
            // lock is touched.
            store.task(id)?;
            vec![id]
        }
        None => store.pending_launches(CONCURRENCY_BOUND * 4)?,
    };
    if candidates.is_empty() {
        return Ok(Started::Nothing("no queued task is ready to start".into()));
    }
    let mut refusal = None;
    for id in candidates {
        // Another execution may hold this task, which is expected: concurrent
        // starts drain a queue by moving on rather than by waiting.
        match execute(store, &database, id, &RunOptions::default())? {
            Started::Task(id) => return Ok(Started::Task(id)),
            Started::Nothing(reason) => refusal = Some(reason),
        }
    }
    Ok(Started::Nothing(refusal.unwrap_or_else(|| {
        "no queued task is ready to start".into()
    })))
}

/// The human front door: execute the named task in the foreground, whatever
/// readiness state it is in, with the worktree, session, and prompt options.
///
/// This is `mush task run`, and it is explicit by construction — the user typed
/// it. It therefore never refuses a queued task; it does the queue's
/// bookkeeping instead, exactly as [`start`] would, so a concurrent runner
/// cannot double-start the task and the graph advances when this execution
/// finishes.
pub fn run(
    store: &mut Store,
    task_id: i64,
    options: &RunOptions<'_>,
) -> Result<Started, DomainError> {
    let database = store.database_path().to_path_buf();
    store.task(task_id)?;
    execute(store, &database, task_id, options)
}

/// The engine both front doors run: take the task's execution lock, work out
/// from durable state what bookkeeping this execution owes, and execute.
///
/// The lock is taken before the launch delivery is claimed, so nothing else can
/// believe the task is unowned while this process lives, and whichever process
/// takes the lock is the one that executes.
fn execute(
    store: &mut Store,
    database: &Path,
    id: i64,
    options: &RunOptions<'_>,
) -> Result<Started, DomainError> {
    let Some(mut lock) = lock::FileLock::try_acquire(&lock::task_lock_path(database, id))? else {
        return Ok(Started::Nothing(format!(
            "task {id} already has a live execution"
        )));
    };
    let pid = std::process::id();
    let boot_id = crate::executor::boot_id();
    lock.describe(&format!(
        "execution of task {id} (pid {pid}, boot {boot_id})"
    ))?;
    // Queued work owns a launch delivery and the readiness transitions that go
    // with it; unqueued work owns neither. The task's row says which, so the
    // caller never has to.
    let runner_id = if store.task(id)?.readiness_status == ReadinessStatus::Unqueued {
        None
    } else {
        let runner_id = format!("{pid}-{}", uuid::Uuid::new_v4());
        if !store.claim_launch(crate::store::LaunchClaim {
            task_id: id,
            runner_id: &runner_id,
            runner_boot_id: &boot_id,
            runner_pid: pid,
        })? {
            return Ok(Started::Nothing(format!(
                "task {id} is not ready to start; run `mush runner tick` to reconcile it, or `mush task recover {id}`"
            )));
        }
        Some(runner_id)
    };
    if let Err(error) = Executor::new(database).run_locked(
        store,
        LockedRun {
            task_id: id,
            options,
            runner_id: runner_id.as_deref(),
            execution_lock: &lock,
        },
    ) {
        // Parking belongs to the delivery this execution claimed. An unqueued
        // task has none, and the executor has already recorded its interruption.
        if runner_id.is_some() {
            store.require_intervention(
                id,
                &format!(
                    "execution of task {id} failed: {error}; correct the cause and use task recover"
                ),
            )?;
        }
        return Err(error);
    }
    Ok(Started::Task(id))
}

/// One bounded pass: reconcile abandoned rows, then start every eligible ready
/// task up to the concurrency bound, and exit without waiting for the work.
pub fn tick(store: &mut Store) -> Result<Vec<i64>, DomainError> {
    store.reconcile()?;
    let database = store.database_path().to_path_buf();
    let mut started = Vec::new();
    // A tick supervises nothing: it exits without waiting, so every execution it
    // has to count is somebody else's and is visible as a held lock.
    for id in eligible(store, &BTreeSet::new())? {
        match spawn_start(&database, id, Detached::Yes) {
            Ok(child) => {
                // Nothing waits for a detached execution, so reap the immediate
                // child in a thread rather than leaving a zombie behind.
                std::thread::spawn(move || {
                    let mut child = child;
                    child.wait()
                });
                started.push(id);
            }
            Err(error) => park_unstartable(store, id, &error)?,
        }
    }
    Ok(started)
}

/// Repeat [`tick`]'s pass until terminated, supervising the executions this
/// process starts as its own children so their exit status is direct evidence
/// rather than inference.
///
/// Only one `serve` holds a database at a time; a second refuses rather than
/// silently doubling the concurrency bound.
pub fn serve(store: &mut Store) -> Result<(), DomainError> {
    let database = store.database_path().to_path_buf();
    let Some(_serve_lock) = lock::acquire_serve_lock(&database, "mush runner serve")? else {
        let holder =
            lock::serve_lock_holder(&database)?.unwrap_or_else(|| "another process".to_owned());
        return Err(DomainError::Invalid(format!(
            "{holder} already holds the serve lock for {}; one serve per database",
            database.display()
        )));
    };
    let mut children: BTreeMap<i64, Child> = BTreeMap::new();
    loop {
        serve_pass(store, &database, &mut children)?;
        std::thread::sleep(SERVE_PASS_INTERVAL);
    }
}

/// Reap, reconcile, and refill one serve pass. The caller owns cadence and the
/// serve lock; this pass owns the bounded unit of runner work.
fn serve_pass(
    store: &mut Store,
    database: &Path,
    children: &mut BTreeMap<i64, Child>,
) -> Result<(), DomainError> {
    reap(store, children)?;
    if let Err(error) = store.reconcile() {
        eprintln!("warning: cannot reconcile abandoned executions: {error}");
    }
    spawn_eligible(store, database, children)
}

/// Fill the capacity left after counting this serve's children and every live
/// execution observed through its task lock.
fn spawn_eligible(
    store: &mut Store,
    database: &Path,
    children: &mut BTreeMap<i64, Child>,
) -> Result<(), DomainError> {
    let supervised = children.keys().copied().collect();
    for id in eligible(store, &supervised)? {
        match spawn_start(database, id, Detached::No) {
            Ok(child) => {
                children.insert(id, child);
            }
            Err(error) => park_unstartable(store, id, &error)?,
        }
    }
    Ok(())
}

/// Report what is queued, owned, and parked, and whether anything holds the
/// serve lock.
pub fn status(store: &Store) -> Result<RunnerReport, DomainError> {
    Ok(RunnerReport {
        counts: store.runner_counts()?,
        serve_lock_holder: lock::serve_lock_holder(store.database_path())?,
        concurrency_bound: CONCURRENCY_BOUND,
    })
}

/// The tasks this pass may start: ready deliveries, up to whatever is left of
/// the concurrency bound once every execution already in flight is counted.
///
/// `supervised` names the tasks whose executions this caller is already running
/// as its own children. They are in flight and count against the bound, but they
/// are counted exactly once: a supervised child that has taken its task's lock
/// also appears in [`Store::live_executions`], and counting both the child and
/// its lock would shrink the bound by one for every execution this process
/// started. That double count is what stalled a `serve` — a freed slot went
/// unused until every remaining child finished — so the two views are unioned
/// rather than added. A supervised child that has not reached its lock yet is
/// counted through `supervised` alone, and an execution belonging to a
/// concurrent `serve`, `tick`, or `runner start` is counted through its lock, so
/// a freed slot is replenished on the next pass without a pass ever holding more
/// than the bound.
///
/// This is a per-pass bound and not a database-wide reservation. Live locks are
/// observed rather than reserved, so two passes racing between the observation
/// and their children taking locks can briefly exceed it, and an explicitly
/// invoked `runner start` is not throttled by it at all — the caller named that
/// task. The bound exists to keep an automatic pass from swamping a machine with
/// agent harnesses, which it does; anything stronger would be the resident
/// scheduler this slice declines to build.
///
/// Supervised tasks are also excluded from the candidates: between spawning a
/// child and that child claiming its delivery, the task is still a pending
/// launch, and starting it again would be this process racing itself.
fn eligible(store: &Store, supervised: &BTreeSet<i64>) -> Result<Vec<i64>, DomainError> {
    let available = available_capacity(supervised, &store.live_executions()?);
    if available == 0 {
        return Ok(Vec::new());
    }
    // Ask for enough rows that the supervised ones this query still reports
    // cannot crowd out startable work.
    let mut candidates = store.pending_launches(available + supervised.len())?;
    candidates.retain(|id| !supervised.contains(id));
    candidates.truncate(available);
    Ok(candidates)
}

/// Capacity left for an automatic pass after unioning the children it owns
/// with executions observed through their locks.
fn available_capacity(supervised: &BTreeSet<i64>, live: &[i64]) -> usize {
    let externally_live = live.iter().filter(|id| !supervised.contains(id)).count();
    CONCURRENCY_BOUND.saturating_sub(supervised.len() + externally_live)
}

enum Detached {
    Yes,
    No,
}

/// The one way work starts. `tick` detaches its children because it exits
/// without waiting; `serve` keeps its own so it can reap them.
fn spawn_start(database: &Path, task_id: i64, detached: Detached) -> std::io::Result<Child> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--database")
        .arg(database)
        .arg("runner")
        .arg("start")
        .arg(task_id.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    if matches!(detached, Detached::Yes) {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()
}

/// A task Mush cannot even start is an intervention immediately: there is no
/// retry budget to age through, because nothing here is a timing question.
fn park_unstartable(
    store: &mut Store,
    task_id: i64,
    error: &std::io::Error,
) -> Result<(), DomainError> {
    let diagnostic = format!(
        "cannot spawn `mush runner start {task_id}`: {error}; correct the cause and run task recover"
    );
    eprintln!("warning: {diagnostic}");
    store.require_intervention(task_id, &diagnostic)
}

/// Collect finished children and record a failed execution from its exit
/// status, which is direct evidence that this process owned and outlived.
fn reap(store: &mut Store, children: &mut BTreeMap<i64, Child>) -> Result<(), DomainError> {
    let mut finished = Vec::new();
    for (id, child) in children.iter_mut() {
        if let Some(status) = child.try_wait()? {
            finished.push((*id, status));
        }
    }
    for (id, status) in finished {
        children.remove(&id);
        match classify_child_status(status) {
            ChildOutcome::Executed | ChildOutcome::DidNotStart => continue,
            ChildOutcome::Failed => {}
        }
        store.require_intervention(
            id,
            &format!(
                "runner start for task {id} exited with {status}; inspect task artifacts and run task recover"
            ),
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildOutcome {
    Executed,
    DidNotStart,
    Failed,
}

/// Interpret the child process's public exit-code contract without consulting
/// mutable task state after the child has exited.
fn classify_child_status(status: ExitStatus) -> ChildOutcome {
    if status.success() {
        ChildOutcome::Executed
    } else if status.code() == Some(EXIT_DID_NOT_START) {
        ChildOutcome::DidNotStart
    } else {
        ChildOutcome::Failed
    }
}

#[cfg(test)]
mod tests {
    include!("../tests/unit/runner.rs");
}
