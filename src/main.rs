use clap::{Args, Parser, Subcommand};
use mush::{
    CheckpointDecision, DomainError, ReadinessStatus, Store, TaskKind, TaskStatus, database_path,
    executor::RunOptions, runner,
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
    Create {
        id: i64,
    },
    Decide {
        id: i64,
        #[arg(long)]
        decision: CheckpointDecision,
        #[arg(long)]
        evidence: String,
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
    match app.command {
        Command::Project {
            command: ProjectCommand::Register { name, path },
        } => output(&store.register_project(&name, &path)?, app.json)?,
        Command::Agent {
            command: AgentCommand::Register(args),
        } => {
            let settings = input(
                args.settings,
                args.settings_file.as_deref(),
                "settings",
                "{}",
            )?;
            let settings = with_review_prompt(&settings, args.review_prompt_file.as_deref())?;
            output(
                &store.register_agent(
                    args.project,
                    &args.name,
                    &args.harness,
                    &args.model,
                    &settings,
                    args.checkpoint,
                )?,
                app.json,
            )?
        }
        Command::Agent {
            command:
                AgentCommand::Update {
                    id,
                    settings,
                    settings_file,
                    review_prompt_file,
                },
        } => {
            // Updating starts from the agent's current settings, so a review
            // prompt can be added or replaced without restating the rest.
            let current = store.agent(id)?.settings;
            let settings = input(settings, settings_file.as_deref(), "settings", &current)?;
            let settings = with_review_prompt(&settings, review_prompt_file.as_deref())?;
            output(&store.update_agent_settings(id, &settings)?, app.json)?
        }
        Command::Task { command } => match command {
            TaskCommand::Add {
                project,
                agent,
                description,
                description_file,
                parent,
            } => {
                let description =
                    input(description, description_file.as_deref(), "description", "")?;
                if description.is_empty() {
                    return Err(DomainError::Invalid("task description is required".into()));
                }
                output(
                    &store.add_work_task(project, agent, &description, parent)?,
                    app.json,
                )?
            }
            TaskCommand::Show { id } => output(&store.task(id)?, app.json)?,
            TaskCommand::List { project } => output(&store.tasks(project)?, app.json)?,
            TaskCommand::Complete {
                id,
                result,
                evidence,
            } => {
                let task = store.complete_work(id, &result, evidence.as_deref())?;
                output(&task, app.json)?;
            }
            TaskCommand::Run {
                id,
                worktree,
                restart_session,
                prompt_file,
            } => {
                // `task run` is the human front door over the same engine
                // `runner start` uses. It refuses no readiness state: whether
                // the task is queued decides the bookkeeping, not whether the
                // command is allowed.
                let prompt = prompt_file
                    .as_deref()
                    .map(std::fs::read_to_string)
                    .transpose()?;
                let options = RunOptions {
                    worktree: worktree.as_deref(),
                    restart_session,
                    prompt_override: prompt.as_deref(),
                };
                match runner::run(&mut store, id, &options)? {
                    runner::Started::Task(started) => output(&store.task(started)?, app.json)?,
                    runner::Started::Nothing(reason) => {
                        eprintln!("{reason}");
                        return Ok(runner::EXIT_DID_NOT_START);
                    }
                }
            }
            TaskCommand::PrepareRevision {
                id,
                description_file,
                worktree,
            } => output(
                &store.prepare_revision(
                    id,
                    &std::fs::read_to_string(description_file)?,
                    &worktree,
                )?,
                app.json,
            )?,
            TaskCommand::Depend {
                prerequisite,
                dependent,
            } => {
                store.add_dependency(prerequisite, dependent)?;
                output(&store.task(dependent)?, app.json)?;
            }
            TaskCommand::Undepend {
                prerequisite,
                dependent,
            } => {
                store.remove_dependency(prerequisite, dependent)?;
                output(&store.task(dependent)?, app.json)?;
            }
            TaskCommand::Queue { id } => {
                let task = store.queue(id)?;
                output(&task, app.json)?;
            }
            TaskCommand::Status { ids } => output(&store.observe(&ids, true)?, app.json)?,
            TaskCommand::Wait {
                ids,
                until,
                timeout,
            } => {
                // Waiting is pure observation: it polls current rows and
                // returns, and it neither reconciles nor launches.
                let start = Instant::now();
                let limit = timeout.map(Duration::from_secs);
                warn_about_unqueued_work(&store, &ids)?;
                let mut consecutive_observation_failures = 0_u8;
                loop {
                    let mut observation = match store.observe(&ids, until == "all") {
                        Ok(observation) => {
                            consecutive_observation_failures = 0;
                            observation
                        }
                        Err(DomainError::NotFound(kind, id)) => {
                            return Err(DomainError::NotFound(kind, id));
                        }
                        Err(error @ DomainError::Invalid(_)) => return Err(error),
                        Err(error) => {
                            consecutive_observation_failures += 1;
                            if consecutive_observation_failures >= 8 {
                                return Err(error);
                            }
                            eprintln!("warning: cannot observe tasks yet: {error}");
                            std::thread::sleep(Duration::from_millis(250));
                            continue;
                        }
                    };
                    if observation.terminal {
                        output(&observation, app.json)?;
                        break;
                    }
                    if limit.is_some_and(|limit| start.elapsed() >= limit) {
                        observation.timed_out = true;
                        output(&observation, app.json)?;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            TaskCommand::Recover { ids, project } => {
                let pending = store.recover_launches(&ids, project)?;
                output(&pending, app.json)?;
            }
        },
        Command::Checkpoint { command } => match command {
            CheckpointCommand::Create { id } => output(&store.create_checkpoint(id)?, app.json)?,
            CheckpointCommand::Decide {
                id,
                decision,
                evidence,
            } => output(&store.decide_checkpoint(id, decision, &evidence)?, app.json)?,
        },
        Command::Tui {
            snapshot: true,
            project,
        } => print!("{}", mush::tui::snapshot(&store, project)?),
        Command::Tui {
            snapshot: false,
            project,
        } => mush::tui::run(&mut store, project)?,
        Command::Runner { command } => match command {
            RunnerCommand::Start { id } => match runner::start(&mut store, id)? {
                runner::Started::Task(started) => {
                    output(&store.task(started)?, app.json)?;
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
                let started = runner::tick(&mut store)?;
                output(&started, app.json)?;
            }
            RunnerCommand::Serve => runner::serve(&mut store)?,
            RunnerCommand::Status => output(&runner::status(&store)?, app.json)?,
        },
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
