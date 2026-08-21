use mush::{
    ExecutionStatus, ReadinessStatus, Store, TaskStatus,
    store::{
        AgentRegistration, ExecutionOwner, ExecutionStart, WorkExecutionResult, WorkTaskRequest,
    },
};

struct Fixture {
    _state: tempfile::TempDir,
    project_dir: tempfile::TempDir,
    database: std::path::PathBuf,
    store: Store,
    project_id: i64,
    agent_id: i64,
}

impl Fixture {
    fn new() -> Self {
        let state = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let database = state.path().join("mush.sqlite");
        let store = Store::open(&database).unwrap();
        let project = store
            .register_project("quality", project_dir.path())
            .unwrap();
        let agent = store
            .register_agent(AgentRegistration {
                project_id: project.id,
                name: "worker",
                harness: "manual",
                model: "human",
                settings: "{}",
                checkpoint: false,
            })
            .unwrap();
        Self {
            _state: state,
            project_dir,
            database,
            store,
            project_id: project.id,
            agent_id: agent.id,
        }
    }

    fn task(&self, description: &str) -> i64 {
        self.store
            .add_work_task(WorkTaskRequest {
                project_id: self.project_id,
                agent_id: self.agent_id,
                description,
                parent_task_id: None,
            })
            .unwrap()
            .id
    }

    fn start(&mut self, task_id: i64) -> mush::Task {
        self.store
            .begin_execution(ExecutionStart {
                task_id,
                session_id: Some("initial-session"),
                worktree_name: Some("quality-worktree"),
                artifact_dir: &self.project_dir.path().join("artifacts"),
                boot_id: "test-boot",
                claimed_runner_id: None,
            })
            .unwrap()
    }
}

#[test]
fn a_claimed_launch_rejects_the_wrong_runner_without_consuming_the_claim() {
    let mut fixture = Fixture::new();
    let task_id = fixture.task("claimed work");
    fixture.store.queue(task_id).unwrap();
    assert!(
        fixture
            .store
            .claim_launch(mush::store::LaunchClaim {
                task_id,
                runner_id: "owner",
                runner_boot_id: "runner-boot",
                runner_pid: 42,
            })
            .unwrap()
    );

    let error = fixture
        .store
        .begin_execution_owned(ExecutionStart {
            task_id,
            session_id: None,
            worktree_name: None,
            artifact_dir: &fixture.project_dir.path().join("artifacts"),
            boot_id: "test-boot",
            claimed_runner_id: Some("intruder"),
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("claimed by another runner"), "{error}");

    let persisted = Store::open(&fixture.database)
        .unwrap()
        .task(task_id)
        .unwrap();
    assert_eq!(persisted.readiness_status, ReadinessStatus::Claimed);
    assert_eq!(persisted.execution_status, None);
    assert_eq!(persisted.execution_attempt, 0);
}

#[test]
fn an_unsatisfied_prerequisite_rejects_execution_without_mutating_the_task() {
    let mut fixture = Fixture::new();
    let prerequisite = fixture.task("prerequisite");
    let dependent = fixture.task("dependent");
    fixture
        .store
        .add_dependency(prerequisite, dependent)
        .unwrap();

    let error = fixture
        .store
        .begin_execution(ExecutionStart {
            task_id: dependent,
            session_id: Some("must-not-persist"),
            worktree_name: Some("must-not-persist"),
            artifact_dir: &fixture.project_dir.path().join("artifacts"),
            boot_id: "test-boot",
            claimed_runner_id: None,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("prerequisites are not completed"), "{error}");

    let persisted = Store::open(&fixture.database)
        .unwrap()
        .task(dependent)
        .unwrap();
    assert_eq!(persisted.status, TaskStatus::Pending);
    assert_eq!(persisted.execution_status, None);
    assert_eq!(persisted.execution_attempt, 0);
    assert_eq!(persisted.session_id, None);
    assert_eq!(persisted.worktree_name, None);
}

#[test]
fn stale_execution_owners_cannot_replace_pid_or_session_state() {
    let mut fixture = Fixture::new();
    let task_id = fixture.task("owned work");
    let running = fixture.start(task_id);
    let owner = ExecutionOwner {
        task_id,
        execution_attempt: running.execution_attempt,
    };
    fixture.store.record_execution_pid(owner, 314).unwrap();
    fixture
        .store
        .record_execution_session(owner, "current-session")
        .unwrap();

    let stale = ExecutionOwner {
        task_id,
        execution_attempt: owner.execution_attempt + 1,
    };
    let error = fixture
        .store
        .record_execution_pid(stale, 999)
        .unwrap_err()
        .to_string();
    assert!(error.contains("ownership changed"), "{error}");
    fixture
        .store
        .record_execution_session(stale, "stale-session")
        .unwrap();

    let persisted = fixture.store.task(task_id).unwrap();
    assert_eq!(persisted.execution_pid, Some(314));
    assert_eq!(persisted.session_id.as_deref(), Some("current-session"));

    fixture.store.interrupt_execution_owned(owner).unwrap();
    fixture
        .store
        .replace_execution_session(owner, Some("replacement-session"))
        .unwrap();
    let replaced = fixture.store.task(task_id).unwrap();
    assert_eq!(
        replaced.execution_status,
        Some(ExecutionStatus::Interrupted)
    );
    assert_eq!(replaced.execution_pid, None);
    assert_eq!(replaced.session_id.as_deref(), Some("replacement-session"));
}

#[test]
fn foreground_recovery_preserves_unqueued_policy_and_clears_intervention() {
    let mut fixture = Fixture::new();
    let task_id = fixture.task("foreground work");
    let running = fixture.start(task_id);
    fixture
        .store
        .interrupt_execution_owned_with_intervention(
            ExecutionOwner {
                task_id,
                execution_attempt: running.execution_attempt,
            },
            Some("foreground failure"),
        )
        .unwrap();

    let interrupted = fixture.store.task(task_id).unwrap();
    assert_eq!(interrupted.readiness_status, ReadinessStatus::Unqueued);
    assert_eq!(
        interrupted.intervention.as_deref(),
        Some("foreground failure")
    );

    assert_eq!(
        fixture.store.recover_launches(&[task_id], None).unwrap(),
        vec![task_id]
    );
    let recovered = Store::open(&fixture.database)
        .unwrap()
        .task(task_id)
        .unwrap();
    assert_eq!(recovered.readiness_status, ReadinessStatus::Unqueued);
    assert_eq!(
        recovered.execution_status,
        Some(ExecutionStatus::Interrupted)
    );
    assert_eq!(recovered.intervention, None);
}

#[test]
fn runner_failures_do_not_park_active_or_completed_work() {
    let mut fixture = Fixture::new();
    let running_id = fixture.task("running work");
    fixture.start(running_id);
    fixture
        .store
        .require_intervention(running_id, "late runner failure")
        .unwrap();
    let running = fixture.store.task(running_id).unwrap();
    assert_eq!(running.execution_status, Some(ExecutionStatus::Running));
    assert_eq!(running.readiness_status, ReadinessStatus::Unqueued);
    assert_eq!(running.intervention, None);

    let completed_id = fixture.task("completed work");
    fixture
        .store
        .complete_work(completed_id, "done", Some("evidence"))
        .unwrap();
    fixture
        .store
        .require_intervention(completed_id, "obsolete runner failure")
        .unwrap();
    let completed = fixture.store.task(completed_id).unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_ne!(
        completed.readiness_status,
        ReadinessStatus::InterventionRequired
    );
    assert_eq!(completed.intervention, None);
}

#[test]
fn observations_bound_unicode_without_splitting_code_points_and_define_any_vs_all() {
    let mut fixture = Fixture::new();
    let completed_id = fixture.task(&"🦀".repeat(2_000));
    fixture
        .store
        .complete_work(completed_id, &"é".repeat(4_000), None)
        .unwrap();
    let pending_id = fixture.task("pending");

    let any = fixture
        .store
        .observe(&[completed_id, pending_id], false)
        .unwrap();
    let all = fixture
        .store
        .observe(&[completed_id, pending_id], true)
        .unwrap();
    assert!(any.terminal);
    assert!(!all.terminal);
    assert!(any.elided);
    assert_eq!(any.tasks[0].description, "[elided; inspect task artifacts]");
    assert_eq!(any.tasks[0].result, None);
    serde_json::to_string(&any).expect("bounded Unicode remains valid UTF-8 JSON");

    let empty_error = fixture.store.observe(&[], false).unwrap_err().to_string();
    assert!(empty_error.contains("1 to 32 task ids"), "{empty_error}");
    let too_many_error = fixture
        .store
        .observe(&vec![pending_id; 33], true)
        .unwrap_err()
        .to_string();
    assert!(
        too_many_error.contains("1 to 32 task ids"),
        "{too_many_error}"
    );
}

#[test]
fn invalid_registration_and_cross_project_routing_leave_no_partial_rows() {
    let fixture = Fixture::new();
    let invalid = fixture
        .store
        .register_agent(AgentRegistration {
            project_id: fixture.project_id,
            name: "invalid",
            harness: "manual",
            model: "human",
            settings: "not json",
            checkpoint: false,
        })
        .unwrap_err()
        .to_string();
    assert!(invalid.contains("settings must be JSON"), "{invalid}");

    let other_dir = tempfile::tempdir().unwrap();
    let other = fixture
        .store
        .register_project("other", other_dir.path())
        .unwrap();
    let cross_project = fixture
        .store
        .add_work_task(WorkTaskRequest {
            project_id: other.id,
            agent_id: fixture.agent_id,
            description: "wrong project",
            parent_task_id: None,
        })
        .unwrap_err()
        .to_string();
    assert!(
        cross_project.contains("agent is not registered to this project"),
        "{cross_project}"
    );

    let reopened = Store::open(&fixture.database).unwrap();
    assert_eq!(reopened.tasks(None).unwrap().len(), 0);
    assert!(reopened.agent(fixture.agent_id + 1).is_err());
}

#[test]
fn execution_entry_and_completion_enforce_task_kind_and_lifecycle() {
    let mut fixture = Fixture::new();
    let completed_id = fixture.task("already complete");
    fixture
        .store
        .complete_work(completed_id, "done", None)
        .unwrap();
    let completed_error = fixture
        .store
        .begin_execution(ExecutionStart {
            task_id: completed_id,
            session_id: None,
            worktree_name: None,
            artifact_dir: &fixture.project_dir.path().join("completed-artifacts"),
            boot_id: "test-boot",
            claimed_runner_id: None,
        })
        .unwrap_err()
        .to_string();
    assert!(
        completed_error.contains("only pending tasks can execute"),
        "{completed_error}"
    );

    let parked_id = fixture.task("parked");
    fixture.store.queue(parked_id).unwrap();
    fixture
        .store
        .require_intervention(parked_id, "operator action required")
        .unwrap();
    let parked_error = fixture
        .store
        .begin_execution(ExecutionStart {
            task_id: parked_id,
            session_id: None,
            worktree_name: None,
            artifact_dir: &fixture.project_dir.path().join("parked-artifacts"),
            boot_id: "test-boot",
            claimed_runner_id: None,
        })
        .unwrap_err()
        .to_string();
    assert!(
        parked_error.contains("requires intervention"),
        "{parked_error}"
    );

    let running_work_id = fixture.task("running work");
    let running_work = fixture.start(running_work_id);
    let checkpoint_error = fixture
        .store
        .finish_checkpoint_execution(
            ExecutionOwner {
                task_id: running_work_id,
                execution_attempt: running_work.execution_attempt,
            },
            "not checkpoint evidence",
        )
        .unwrap_err()
        .to_string();
    assert!(
        checkpoint_error.contains("only checkpoint tasks"),
        "{checkpoint_error}"
    );
    assert_eq!(
        fixture
            .store
            .task(running_work_id)
            .unwrap()
            .execution_status,
        Some(ExecutionStatus::Running)
    );

    fixture
        .store
        .register_agent(AgentRegistration {
            project_id: fixture.project_id,
            name: "reviewer",
            harness: "manual",
            model: "reviewer",
            settings: "{}",
            checkpoint: true,
        })
        .unwrap();
    let subject_id = fixture.task("reviewed work");
    fixture
        .store
        .complete_work(subject_id, "done", Some("subject evidence"))
        .unwrap();
    let checkpoint = fixture
        .store
        .create_checkpoint(mush::store::CheckpointRequest {
            subject_task_id: subject_id,
            criteria: "the declared criteria hold",
            adjudicator_agent_id: None,
        })
        .unwrap();
    let running_checkpoint = fixture.start(checkpoint.id);
    let work_error = fixture
        .store
        .finish_work_execution(WorkExecutionResult {
            owner: ExecutionOwner {
                task_id: checkpoint.id,
                execution_attempt: running_checkpoint.execution_attempt,
            },
            result: "not a work result",
            evidence: "not work evidence",
        })
        .unwrap_err()
        .to_string();
    assert!(
        work_error.contains("only pending work tasks"),
        "{work_error}"
    );
    assert_eq!(
        fixture.store.task(checkpoint.id).unwrap().execution_status,
        Some(ExecutionStatus::Running)
    );
}

#[test]
fn diagnostics_are_utf8_bounded_and_recovery_requires_explicit_scope() {
    let mut fixture = Fixture::new();
    let task_id = fixture.task("diagnostic target");
    fixture.store.queue(task_id).unwrap();
    let diagnostic = "🦀".repeat(1_000);
    fixture
        .store
        .require_intervention(task_id, &diagnostic)
        .unwrap();

    let persisted = Store::open(&fixture.database)
        .unwrap()
        .task(task_id)
        .unwrap();
    let bounded = persisted.intervention.expect("intervention is durable");
    assert!(bounded.len() <= 1_024);
    assert!(bounded.ends_with("[truncated; inspect task artifacts for full output]"));
    serde_json::to_string(&bounded).expect("truncation preserves UTF-8 boundaries");

    let error = fixture
        .store
        .recover_launches(&[], None)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("requires one or more task ids or --project"),
        "{error}"
    );
    assert_eq!(
        fixture.store.task(task_id).unwrap().readiness_status,
        ReadinessStatus::InterventionRequired
    );
}

#[test]
fn task_validation_failures_preserve_registered_and_persisted_state() {
    let mut fixture = Fixture::new();
    let missing = fixture.project_dir.path().join("does-not-exist");
    let project_error = fixture
        .store
        .register_project("missing", &missing)
        .unwrap_err()
        .to_string();
    assert!(project_error.contains("project path"), "{project_error}");

    let settings_error = fixture
        .store
        .update_agent_settings(fixture.agent_id, "not json")
        .unwrap_err()
        .to_string();
    assert!(
        settings_error.contains("settings must be JSON"),
        "{settings_error}"
    );
    assert_eq!(
        fixture.store.agent(fixture.agent_id).unwrap().settings,
        "{}"
    );

    let original_id = fixture.task("original description");
    let revision_error = fixture
        .store
        .prepare_revision(original_id, "must not replace", "must-not-persist")
        .unwrap_err()
        .to_string();
    assert!(
        revision_error.contains("only an unstarted linked revision"),
        "{revision_error}"
    );
    let original = fixture.store.task(original_id).unwrap();
    assert_eq!(original.description, "original description");
    assert_eq!(original.worktree_name, None);

    let other_dir = tempfile::tempdir().unwrap();
    let other = fixture
        .store
        .register_project("parent-project", other_dir.path())
        .unwrap();
    let other_agent = fixture
        .store
        .register_agent(AgentRegistration {
            project_id: other.id,
            name: "other worker",
            harness: "manual",
            model: "human",
            settings: "{}",
            checkpoint: false,
        })
        .unwrap();
    let foreign_parent = fixture
        .store
        .add_work_task(WorkTaskRequest {
            project_id: other.id,
            agent_id: other_agent.id,
            description: "foreign parent",
            parent_task_id: None,
        })
        .unwrap();
    let count_before = fixture.store.tasks(None).unwrap().len();
    let parent_error = fixture
        .store
        .add_work_task(WorkTaskRequest {
            project_id: fixture.project_id,
            agent_id: fixture.agent_id,
            description: "invalid child",
            parent_task_id: Some(foreign_parent.id),
        })
        .unwrap_err()
        .to_string();
    assert!(
        parent_error.contains("belongs to another project"),
        "{parent_error}"
    );
    assert_eq!(fixture.store.tasks(None).unwrap().len(), count_before);
}

#[test]
fn direct_database_writes_cannot_bypass_checkpoint_subject_gating() {
    let fixture = Fixture::new();
    let subject = fixture.task("subject");
    let second_subject = fixture.task("second subject");
    fixture
        .store
        .register_agent(AgentRegistration {
            project_id: fixture.project_id,
            name: "reviewer",
            harness: "manual",
            model: "human-reviewer",
            settings: "{}",
            checkpoint: true,
        })
        .unwrap();
    let mut store = Store::open(&fixture.database).unwrap();
    let checkpoint = store
        .create_checkpoint(mush::store::CheckpointRequest {
            subject_task_id: subject,
            criteria: "the declared criteria hold",
            adjudicator_agent_id: None,
        })
        .unwrap()
        .id;
    let connection = rusqlite::Connection::open(&fixture.database).unwrap();

    for statement in [
        "UPDATE tasks SET readiness_status='ready' WHERE id=?1",
        "UPDATE tasks SET readiness_status='claimed' WHERE id=?1",
        "UPDATE tasks SET execution_status='running' WHERE id=?1",
        "UPDATE tasks SET decision='met' WHERE id=?1",
    ] {
        let error = connection
            .execute(statement, [checkpoint])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("checkpoint subject is not completed"),
            "{statement}: {error}"
        );
    }

    let error = connection
        .execute(
            "INSERT INTO tasks(project_id,agent_id,kind,status,description,subject_task_id,readiness_status)
             VALUES(?1,?2,'checkpoint','pending','bypass',?3,'ready')",
            rusqlite::params![fixture.project_id, fixture.agent_id, second_subject],
        )
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("checkpoint subject is not completed"),
        "{error}"
    );

    let error = connection
        .execute(
            "INSERT INTO task_dependencies(prerequisite_task_id,dependent_task_id) VALUES(?1,?2)",
            rusqlite::params![subject, checkpoint],
        )
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("only a work task can be a dependent"),
        "{error}"
    );

    let error = connection
        .execute(
            "INSERT INTO task_dependencies(prerequisite_task_id,dependent_task_id) VALUES(?1,?2)",
            rusqlite::params![checkpoint, subject],
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot gate its own subject"), "{error}");

    let untouched = store.task(checkpoint).unwrap();
    assert_eq!(untouched.readiness_status, ReadinessStatus::Unqueued);
    assert_eq!(untouched.execution_status, None);
    assert_eq!(untouched.decision, None);
}
