use clap::{Args, Parser, Subcommand};
use mush::{
    CheckpointDecision, DomainError, ReadinessStatus, Store, TaskKind, TaskStatus, database_path,
    executor::RunOptions,
    runner,
    store::{AgentRegistration, CheckpointRequest, LoopDeclaration, StageSpec, WorkTaskRequest},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version, about)]
struct App {
    #[arg(long, global = true, env = "MUSH_DATABASE")]
    database: Option<PathBuf>,
    #[arg(long, global = true, help = "Emit JSON from headless commands")]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    Checkpoint {
        #[command(subcommand)]
        command: CheckpointCommand,
    },
    /// Declare and inspect bounded loops: an ordered path of work stages
    /// adjudicated by one checkpoint per attempt, with a semantic-attempt
    /// budget and an optional success continuation.
    Loop {
        #[command(subcommand)]
        command: LoopCommand,
    },
    Tui {
        #[arg(long)]
        snapshot: bool,
        #[arg(long)]
        project: Option<i64>,
    },
    /// The worker front door onto execution: run work without naming it, or
    /// hold the runner role continuously.
    Runner {
        #[command(subcommand)]
        command: RunnerCommand,
    },
}

#[derive(Subcommand)]
enum RunnerCommand {
    /// Execute one task in this process and exit when it reaches a terminal
    /// state. Without an id, claim the next eligible ready task, which is the
    /// one thing only this command does. Use `task run` to run a named task
    /// with the worktree, session, and prompt options.
    Start { id: Option<i64> },
    /// Reconcile abandoned rows, start every eligible ready task up to the
    /// concurrency bound, and exit without waiting for the work.
    Tick,
    /// Repeat the tick pass until terminated, supervising the executions this
    /// process starts as its own children.
    Serve,
    /// Report queued, owned, and parked work, and who holds the serve lock.
    Status,
}

#[derive(Subcommand)]
enum ProjectCommand {
    Register {
        #[arg(long)]
        name: String,
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum AgentCommand {
    Register(AgentRegister),
    Update {
        id: i64,
        #[arg(long, conflicts_with = "settings_file")]
        settings: Option<String>,
        #[arg(long, conflicts_with = "settings")]
        settings_file: Option<PathBuf>,
        #[arg(long)]
        review_prompt_file: Option<PathBuf>,
    },
}

#[derive(Args)]
struct AgentRegister {
    #[arg(long)]
    project: i64,
    #[arg(long)]
    name: String,
    #[arg(long)]
    harness: String,
    #[arg(long)]
    model: String,
    #[arg(long, conflicts_with = "settings_file")]
    settings: Option<String>,
    #[arg(long, conflicts_with = "settings")]
    settings_file: Option<PathBuf>,
    #[arg(long)]
    review_prompt_file: Option<PathBuf>,
    #[arg(long)]
    checkpoint: bool,
}

#[derive(Subcommand)]
enum TaskCommand {
    Add {
        #[arg(long)]
        project: i64,
        #[arg(long)]
        agent: i64,
        #[arg(long, conflicts_with = "description_file")]
        description: Option<String>,
        #[arg(long, conflicts_with = "description")]
        description_file: Option<PathBuf>,
        #[arg(long, help = "Create the work task as a subtask of this work task")]
        parent: Option<i64>,
    },
    Show {
        id: i64,
    },
    List {
        #[arg(long)]
        project: Option<i64>,
    },
    Complete {
        id: i64,
        #[arg(long)]
        result: String,
        #[arg(long)]
        evidence: Option<String>,
    },
    /// Run this task in the foreground now, whatever state it is in. The human
    /// front door: it names one task and takes the worktree, session, and
    /// prompt options. A queued task keeps its queue bookkeeping.
    Run {
        id: i64,
        #[arg(long)]
        worktree: Option<String>,
        #[arg(long)]
        restart_session: bool,
        #[arg(long)]
        prompt_file: Option<PathBuf>,
    },
    PrepareRevision {
        id: i64,
        #[arg(long)]
        description_file: PathBuf,
        #[arg(long)]
        worktree: String,
    },
    Depend {
        prerequisite: i64,
        dependent: i64,
    },
    Undepend {
        prerequisite: i64,
        dependent: i64,
    },
    Queue {
        id: i64,
    },
    Status {
        ids: Vec<i64>,
    },
    Wait {
        ids: Vec<i64>,
        #[arg(long, value_parser=["any","all"], default_value="all")]
        until: String,
        #[arg(long, help = "Timeout in seconds")]
        timeout: Option<u64>,
    },
    Recover {
        ids: Vec<i64>,
        #[arg(long)]
        project: Option<i64>,
    },
}

#[derive(Subcommand)]
enum CheckpointCommand {
    /// Declare the checkpoint adjudicating one work task's result against
    /// immutable criteria. Without --adjudicator, the project's default
    /// checkpoint agent adjudicates.
    Create {
        id: i64,
        #[arg(long, allow_hyphen_values = true, conflicts_with = "criteria_file")]
        criteria: Option<String>,
        #[arg(long, conflicts_with = "criteria")]
        criteria_file: Option<PathBuf>,
        #[arg(long)]
        adjudicator: Option<i64>,
    },
    Decide {
        id: i64,
        #[arg(long)]
        decision: CheckpointDecision,
        #[arg(long)]
        evidence: String,
    },
}

#[derive(Subcommand)]
enum LoopCommand {
    /// Declare a bounded loop and materialize its first attempt. Each --stage
    /// is AGENT_ID:DESCRIPTION or AGENT_ID:@FILE, in path order; materialized
    /// tasks whose agent Mush can execute are queued immediately.
    Declare {
        #[arg(long)]
        project: i64,
        #[arg(long = "stage", required = true, value_name = "AGENT:TEXT|AGENT:@FILE")]
        stages: Vec<String>,
        #[arg(long, allow_hyphen_values = true, conflicts_with = "criteria_file")]
        criteria: Option<String>,
        #[arg(long, conflicts_with = "criteria")]
        criteria_file: Option<PathBuf>,
        #[arg(long)]
        adjudicator: Option<i64>,
        #[arg(long)]
        max_attempts: i64,
        #[arg(long, help = "Later attempts reuse their stage counterpart's worktree")]
        reuse_worktree: bool,
    },
    /// Declare the work task the loop's met decision makes eligible. Declared
    /// once, before the named task begins.
    Continuation { loop_id: i64, task_id: i64 },
    /// Restate one loop: criteria, attempts, decisions, evidence, remaining
    /// budget, and the next eligible action.
    Show { id: i64 },
    List {
        #[arg(long)]
        project: Option<i64>,
    },
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

/// The process exit code. Every command exits `0` on success and `1` through
/// the error path; the two execution front doors, `runner start` and
/// `task run`, additionally distinguish "did not start an execution" — see
/// [`runner::EXIT_DID_NOT_START`].
fn run() -> Result<i32, DomainError> {
    let app = App::parse();
    let path = database_path(app.database.as_deref())?;
    // Nothing reconciles or launches at startup: the runner group is the only
    // surface that advances queued work.
    let mut store = Store::open(&path)?;
    dispatch(&mut store, app.command, app.json)
}

fn dispatch(store: &mut Store, command: Command, json: bool) -> Result<i32, DomainError> {
    match command {
        Command::Project {
            command: ProjectCommand::Register { name, path },
        } => output(&store.register_project(&name, &path)?, json)?,
        Command::Agent { command } => run_agent_command(store, command, json)?,
        Command::Task { command } => return run_task_command(store, command, json),
        Command::Checkpoint { command } => run_checkpoint_command(store, command, json)?,
        Command::Loop { command } => run_loop_command(store, command, json)?,
        Command::Tui { snapshot, project } => run_tui(store, snapshot, project)?,
        Command::Runner { command } => return run_runner_command(store, command, json),
    }
    Ok(0)
}

fn run_agent_command(store: &Store, command: AgentCommand, json: bool) -> Result<(), DomainError> {
    match command {
        AgentCommand::Register(args) => {
            let settings = input(
                args.settings,
                args.settings_file.as_deref(),
                "settings",
                "{}",
            )?;
            let settings = with_review_prompt(&settings, args.review_prompt_file.as_deref())?;
            output(
                &store.register_agent(AgentRegistration {
                    project_id: args.project,
                    name: &args.name,
                    harness: &args.harness,
                    model: &args.model,
                    settings: &settings,
                    checkpoint: args.checkpoint,
                })?,
                json,
            )
        }
        AgentCommand::Update {
            id,
            settings,
            settings_file,
            review_prompt_file,
        } => {
            let current = store.agent(id)?.settings;
            let settings = input(settings, settings_file.as_deref(), "settings", &current)?;
            let settings = with_review_prompt(&settings, review_prompt_file.as_deref())?;
            output(&store.update_agent_settings(id, &settings)?, json)
        }
    }
}

fn run_task_command(
    store: &mut Store,
    command: TaskCommand,
    json: bool,
) -> Result<i32, DomainError> {
    match command {
        observation @ (TaskCommand::Show { .. }
        | TaskCommand::List { .. }
        | TaskCommand::Status { .. }
        | TaskCommand::Wait { .. }) => run_task_observation(store, observation, json),
        mutation => run_task_mutation(store, mutation, json),
    }
}

fn run_task_observation(
    store: &Store,
    command: TaskCommand,
    json: bool,
) -> Result<i32, DomainError> {
    match command {
        TaskCommand::Show { id } => output(&store.task(id)?, json)?,
        TaskCommand::List { project } => output(&store.tasks(project)?, json)?,
        TaskCommand::Status { ids } => output(&store.observe(&ids, true)?, json)?,
        TaskCommand::Wait {
            ids,
            until,
            timeout,
        } => wait_for_tasks(
            store,
            WaitRequest {
                ids: &ids,
                until_all: until == "all",
                timeout,
                json,
            },
        )?,
        _ => unreachable!("mutation routed to task observation"),
    }
    Ok(0)
}

fn run_task_mutation(
    store: &mut Store,
    command: TaskCommand,
    json: bool,
) -> Result<i32, DomainError> {
    match command {
        lifecycle @ (TaskCommand::Add { .. }
        | TaskCommand::Complete { .. }
        | TaskCommand::Run { .. }
        | TaskCommand::PrepareRevision { .. }) => run_task_lifecycle(store, lifecycle, json),
        workflow => run_task_workflow(store, workflow, json),
    }
}

fn run_task_lifecycle(
    store: &mut Store,
    command: TaskCommand,
    json: bool,
) -> Result<i32, DomainError> {
    match command {
        TaskCommand::Add {
            project,
            agent,
            description,
            description_file,
            parent,
        } => add_task(
            store,
            AddTask {
                project,
                agent,
                description,
                description_file: description_file.as_deref(),
                parent,
                json,
            },
        )?,
        TaskCommand::Complete {
            id,
            result,
            evidence,
        } => {
            let task = store.complete_work(id, &result, evidence.as_deref())?;
            output(&task, json)?;
        }
        TaskCommand::Run {
            id,
            worktree,
            restart_session,
            prompt_file,
        } => {
            return run_task(
                store,
                RunTaskRequest {
                    id,
                    worktree,
                    restart_session,
                    prompt_file,
                    json,
                },
            );
        }
        TaskCommand::PrepareRevision {
            id,
            description_file,
            worktree,
        } => output(
            &store.prepare_revision(id, &std::fs::read_to_string(description_file)?, &worktree)?,
            json,
        )?,
        _ => unreachable!("workflow command routed to task lifecycle"),
    }
    Ok(0)
}

fn run_task_workflow(
    store: &mut Store,
    command: TaskCommand,
    json: bool,
) -> Result<i32, DomainError> {
    match command {
        TaskCommand::Depend {
            prerequisite,
            dependent,
        } => change_dependency(
            store,
            DependencyChange {
                prerequisite,
                dependent,
                add: true,
                json,
            },
        )?,
        TaskCommand::Undepend {
            prerequisite,
            dependent,
        } => change_dependency(
            store,
            DependencyChange {
                prerequisite,
                dependent,
                add: false,
                json,
            },
        )?,
        TaskCommand::Queue { id } => {
            let task = store.queue(id)?;
            output(&task, json)?;
        }
        TaskCommand::Recover { ids, project } => {
            let pending = store.recover_launches(&ids, project)?;
            output(&pending, json)?;
        }
        _ => unreachable!("lifecycle or observation command routed to task workflow"),
    }
    Ok(0)
}

struct AddTask<'a> {
    project: i64,
    agent: i64,
    description: Option<String>,
    description_file: Option<&'a std::path::Path>,
    parent: Option<i64>,
    json: bool,
}

fn add_task(store: &Store, request: AddTask<'_>) -> Result<(), DomainError> {
    let description = input(
        request.description,
        request.description_file,
        "description",
        "",
    )?;
    if description.is_empty() {
        return Err(DomainError::Invalid("task description is required".into()));
    }
    output(
        &store.add_work_task(WorkTaskRequest {
            project_id: request.project,
            agent_id: request.agent,
            description: &description,
            parent_task_id: request.parent,
        })?,
        request.json,
    )
}

struct RunTaskRequest {
    id: i64,
    worktree: Option<String>,
    restart_session: bool,
    prompt_file: Option<std::path::PathBuf>,
    json: bool,
}

fn run_task(store: &mut Store, request: RunTaskRequest) -> Result<i32, DomainError> {
    let prompt = request
        .prompt_file
        .as_deref()
        .map(std::fs::read_to_string)
        .transpose()?;
    let options = RunOptions {
        worktree: request.worktree.as_deref(),
        restart_session: request.restart_session,
        prompt_override: prompt.as_deref(),
    };
    match runner::run(store, request.id, &options)? {
        runner::Started::Task(started) => output(&store.task(started)?, request.json)?,
        runner::Started::Nothing(reason) => {
            eprintln!("{reason}");
            return Ok(runner::EXIT_DID_NOT_START);
        }
    }
    Ok(0)
}

struct DependencyChange {
    prerequisite: i64,
    dependent: i64,
    add: bool,
    json: bool,
}

fn change_dependency(store: &mut Store, change: DependencyChange) -> Result<(), DomainError> {
    if change.add {
        store.add_dependency(change.prerequisite, change.dependent)?;
    } else {
        store.remove_dependency(change.prerequisite, change.dependent)?;
    }
    output(&store.task(change.dependent)?, change.json)
}

struct WaitRequest<'a> {
    ids: &'a [i64],
    until_all: bool,
    timeout: Option<u64>,
    json: bool,
}

fn wait_for_tasks(store: &Store, request: WaitRequest<'_>) -> Result<(), DomainError> {
    let start = Instant::now();
    let limit = request.timeout.map(Duration::from_secs);
    warn_about_unqueued_work(store, request.ids)?;
    let mut failures = 0_u8;
    loop {
        let mut observation = match store.observe(request.ids, request.until_all) {
            Ok(observation) => {
                failures = 0;
                observation
            }
            Err(DomainError::NotFound(kind, id)) => return Err(DomainError::NotFound(kind, id)),
            Err(error @ DomainError::Invalid(_)) => return Err(error),
            Err(error) => {
                failures += 1;
                if failures >= 8 {
                    return Err(error);
                }
                eprintln!("warning: cannot observe tasks yet: {error}");
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
        };
        if observation.terminal || limit.is_some_and(|limit| start.elapsed() >= limit) {
            observation.timed_out = !observation.terminal;
            return output(&observation, request.json);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn run_checkpoint_command(
    store: &mut Store,
    command: CheckpointCommand,
    json: bool,
) -> Result<(), DomainError> {
    match command {
        CheckpointCommand::Create {
            id,
            criteria,
            criteria_file,
            adjudicator,
        } => {
            let criteria = input(criteria, criteria_file.as_deref(), "criteria", "")?;
            output(
                &store.create_checkpoint(CheckpointRequest {
                    subject_task_id: id,
                    criteria: &criteria,
                    adjudicator_agent_id: adjudicator,
                })?,
                json,
            )
        }
        CheckpointCommand::Decide {
            id,
            decision,
            evidence,
        } => output(&store.decide_checkpoint(id, decision, &evidence)?, json),
    }
}

/// A declared stage: `AGENT_ID:DESCRIPTION`, or `AGENT_ID:@FILE` to read the
/// description from a file.
fn parse_stage(value: &str) -> Result<(i64, String), DomainError> {
    let (agent, description) = value.split_once(':').ok_or_else(|| {
        DomainError::Invalid(format!(
            "stage {value:?} is not AGENT_ID:DESCRIPTION or AGENT_ID:@FILE"
        ))
    })?;
    let agent = agent.trim().parse::<i64>().map_err(|_| {
        DomainError::Invalid(format!("stage {value:?} does not start with an agent id"))
    })?;
    let description = match description.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)?,
        None => description.to_owned(),
    };
    Ok((agent, description))
}

fn run_loop_command(
    store: &mut Store,
    command: LoopCommand,
    json: bool,
) -> Result<(), DomainError> {
    match command {
        LoopCommand::Declare {
            project,
            stages,
            criteria,
            criteria_file,
            adjudicator,
            max_attempts,
            reuse_worktree,
        } => {
            let criteria = input(criteria, criteria_file.as_deref(), "criteria", "")?;
            let parsed = stages
                .iter()
                .map(|stage| parse_stage(stage))
                .collect::<Result<Vec<_>, _>>()?;
            let specs: Vec<StageSpec<'_>> = parsed
                .iter()
                .map(|(agent_id, description)| StageSpec {
                    agent_id: *agent_id,
                    description,
                })
                .collect();
            output(
                &store.declare_loop(LoopDeclaration {
                    project_id: project,
                    stages: &specs,
                    criteria: &criteria,
                    adjudicator_agent_id: adjudicator,
                    max_attempts,
                    reuse_worktree,
                })?,
                json,
            )
        }
        LoopCommand::Continuation { loop_id, task_id } => {
            output(&store.declare_loop_continuation(loop_id, task_id)?, json)
        }
        LoopCommand::Show { id } => output(&store.loop_report(id)?, json),
        LoopCommand::List { project } => output(&store.loops(project)?, json),
    }
}

fn run_tui(store: &mut Store, snapshot: bool, project: Option<i64>) -> Result<(), DomainError> {
    if snapshot {
        print!("{}", mush::tui::snapshot(store, project)?);
        Ok(())
    } else {
        mush::tui::run(store, project)
    }
}

fn run_runner_command(
    store: &mut Store,
    command: RunnerCommand,
    json: bool,
) -> Result<i32, DomainError> {
    match command {
        RunnerCommand::Start { id } => match runner::start(store, id)? {
            runner::Started::Task(started) => {
                output(&store.task(started)?, json)?;
                return Ok(runner::EXIT_EXECUTED);
            }
            // No execution began, which is the queue behaving normally: a
            // caller looping over `runner start` backs off rather than
            // treating this as a fault, and a supervising `serve` reads
            // the same code instead of guessing from the task's lock.
            runner::Started::Nothing(reason) => {
                eprintln!("{reason}");
                return Ok(runner::EXIT_DID_NOT_START);
            }
        },
        RunnerCommand::Tick => {
            let started = runner::tick(store)?;
            output(&started, json)?;
        }
        RunnerCommand::Serve => runner::serve(store)?,
        RunnerCommand::Status => output(&runner::status(store)?, json)?,
    }
    Ok(0)
}

/// Nothing advances an unqueued work task on its own, so waiting on one
/// blocks until the timeout — or forever without one. Waiting is still valid
/// (a foreground run or a manual completion can finish it), so this names the
/// tasks rather than refusing the wait.
fn warn_about_unqueued_work(store: &Store, ids: &[i64]) -> Result<(), DomainError> {
    let unqueued = ids
        .iter()
        .map(|id| store.task(*id))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|task| {
            task.kind == TaskKind::Work
                && task.status == TaskStatus::Pending
                && task.readiness_status == ReadinessStatus::Unqueued
        })
        .map(|task| format!("#{}", task.id))
        .collect::<Vec<_>>();
    if !unqueued.is_empty() {
        eprintln!(
            "warning: {} not queued, so Mush will not advance {} on its own; run task queue or expect this wait to reach its timeout",
            unqueued.join(", "),
            if unqueued.len() == 1 { "it" } else { "them" }
        );
    }
    Ok(())
}

fn output<T: serde::Serialize + std::fmt::Debug>(value: &T, json: bool) -> Result<(), DomainError> {
    if json {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{value:#?}");
    }
    Ok(())
}

fn input(
    inline: Option<String>,
    file: Option<&std::path::Path>,
    label: &str,
    default: &str,
) -> Result<String, DomainError> {
    match (inline, file) {
        (Some(value), None) => Ok(value),
        (None, Some(path)) => std::fs::read_to_string(path).map_err(Into::into),
        (None, None) => Ok(default.into()),
        (Some(_), Some(_)) => Err(DomainError::Invalid(format!(
            "pass only one {label} source"
        ))),
    }
}

fn with_review_prompt(
    settings: &str,
    path: Option<&std::path::Path>,
) -> Result<String, DomainError> {
    let Some(path) = path else {
        return Ok(settings.into());
    };
    let mut value: serde_json::Value = serde_json::from_str(settings)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| DomainError::Invalid("settings must be a JSON object".into()))?;
    object.insert(
        "review_prompt".into(),
        serde_json::Value::String(std::fs::read_to_string(path)?),
    );
    serde_json::to_string(&value).map_err(Into::into)
}
