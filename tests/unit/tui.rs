use super::*;
use crate::{ExecutionStatus, ReadinessStatus};
use ratatui::{Terminal, backend::TestBackend};

fn task(kind: TaskKind) -> Task {
    Task {
        id: 7,
        project_id: 2,
        agent_id: Some(3),
        kind,
        status: TaskStatus::Pending,
        description: "deterministic view".into(),
        result: None,
        evidence: None,
        decision: None,
        parent_task_id: None,
        previous_task_id: None,
        subject_task_id: None,
        execution_status: None,
        execution_attempt: 0,
        session_id: None,
        worktree_name: None,
        artifact_dir: None,
        execution_boot_id: None,
        execution_pid: None,
        readiness_status: ReadinessStatus::Unqueued,
        queue_generation: 0,
        intervention: None,
    }
}

#[test]
fn key_handling_clamps_navigation_and_returns_domain_actions() {
    let mut selected = 0;
    assert_eq!(
        handle_key(&mut selected, 0, KeyCode::Down),
        UiAction::Continue
    );
    assert_eq!(selected, 0);

    assert_eq!(
        handle_key(&mut selected, 2, KeyCode::Char('j')),
        UiAction::Continue
    );
    assert_eq!(selected, 1);
    handle_key(&mut selected, 2, KeyCode::Down);
    assert_eq!(selected, 1);
    handle_key(&mut selected, 2, KeyCode::Char('k'));
    assert_eq!(selected, 0);
    handle_key(&mut selected, 2, KeyCode::Up);
    assert_eq!(selected, 0);

    assert_eq!(
        handle_key(&mut selected, 2, KeyCode::Char('a')),
        UiAction::Decide(CheckpointDecision::Accepted)
    );
    assert_eq!(
        handle_key(&mut selected, 2, KeyCode::Char('b')),
        UiAction::Decide(CheckpointDecision::Blocked)
    );
    assert_eq!(
        handle_key(&mut selected, 2, KeyCode::Char('r')),
        UiAction::Decide(CheckpointDecision::RevisionRequested)
    );
    assert_eq!(
        handle_key(&mut selected, 2, KeyCode::Char('q')),
        UiAction::Quit
    );
    assert_eq!(
        handle_key(&mut selected, 2, KeyCode::Esc),
        UiAction::Continue
    );
}

#[test]
fn checkpoint_actions_are_offered_only_for_an_eligible_selection() {
    let mut work = task(TaskKind::Work);
    let mut checkpoint = task(TaskKind::Checkpoint);
    checkpoint.id = 8;
    checkpoint.subject_task_id = Some(work.id);
    let tasks = vec![work.clone(), checkpoint.clone()];
    assert!(eligible_checkpoint(&tasks, 1).is_none());

    work.status = TaskStatus::Completed;
    let tasks = vec![work, checkpoint];
    assert_eq!(eligible_checkpoint(&tasks, 1).map(|task| task.id), Some(8));
    assert!(eligible_checkpoint(&tasks, 0).is_none());
    assert!(eligible_checkpoint(&tasks, 99).is_none());
}

#[test]
fn deciding_the_selected_checkpoint_persists_its_evidence_and_decision() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&state.path().join("mush.sqlite")).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent(crate::store::AgentRegistration {
            project_id: project.id,
            name: "worker",
            harness: "manual",
            model: "model",
            settings: "{}",
            checkpoint: false,
        })
        .unwrap();
    store
        .register_agent(crate::store::AgentRegistration {
            project_id: project.id,
            name: "reviewer",
            harness: "manual",
            model: "review-model",
            settings: "{}",
            checkpoint: true,
        })
        .unwrap();
    let work = store
        .add_work_task(crate::store::WorkTaskRequest {
            project_id: project.id,
            agent_id: worker.id,
            description: "work",
            parent_task_id: None,
        })
        .unwrap();
    store
        .complete_work(work.id, "result", Some("subject evidence"))
        .unwrap();
    let checkpoint = store.create_checkpoint(work.id).unwrap();
    let tasks = store.tasks(Some(project.id)).unwrap();
    let selected = tasks
        .iter()
        .position(|task| task.id == checkpoint.id)
        .unwrap();

    decide_selected(&mut store, &tasks, selected, CheckpointDecision::Accepted).unwrap();

    let decided = store.task(checkpoint.id).unwrap();
    assert_eq!(decided.status, TaskStatus::Completed);
    assert_eq!(decided.decision, Some(CheckpointDecision::Accepted));
    assert_eq!(decided.evidence.as_deref(), Some("subject evidence"));
}

#[test]
fn deciding_an_ineligible_selection_is_a_no_op() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&state.path().join("mush.sqlite")).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent(crate::store::AgentRegistration {
            project_id: project.id,
            name: "worker",
            harness: "manual",
            model: "model",
            settings: "{}",
            checkpoint: false,
        })
        .unwrap();
    let work = store
        .add_work_task(crate::store::WorkTaskRequest {
            project_id: project.id,
            agent_id: worker.id,
            description: "work",
            parent_task_id: None,
        })
        .unwrap();
    let tasks = store.tasks(Some(project.id)).unwrap();

    decide_selected(&mut store, &tasks, 0, CheckpointDecision::Accepted).unwrap();

    assert_eq!(store.task(work.id).unwrap().status, TaskStatus::Pending);
}

#[test]
fn details_render_present_values_and_explicit_absences() {
    let empty = task(TaskKind::Work);
    let empty_details = task_details(&empty);
    assert!(empty_details.contains("Execution: —"));
    assert!(empty_details.contains("Markdown evidence\n—"));

    let mut populated = empty;
    populated.execution_status = Some(ExecutionStatus::Interrupted);
    populated.execution_attempt = 4;
    populated.parent_task_id = Some(1);
    populated.subject_task_id = Some(5);
    populated.previous_task_id = Some(6);
    populated.decision = Some(CheckpointDecision::RevisionRequested);
    populated.intervention = Some("repair configuration".into());
    populated.artifact_dir = Some("artifacts/task-7".into());
    populated.evidence = Some("asserted evidence".into());
    let details = task_details(&populated);
    for expected in [
        "Execution: interrupted",
        "Attempt: 4",
        "Parent: #1",
        "Subject: #5",
        "Previous: #6",
        "Decision: revision_requested",
        "Intervention: repair configuration",
        "Artifacts: artifacts/task-7",
        "Markdown evidence\nasserted evidence",
    ] {
        assert!(
            details.contains(expected),
            "missing {expected:?} from {details}"
        );
    }
    let execution = execution_summary(&populated);
    assert!(execution.contains("execution: interrupted (attempt 4)"));
    assert!(execution.contains("intervention: repair configuration"));
}

#[test]
fn interactive_tui_refuses_a_nonterminal_with_an_actionable_alternative() {
    let state = tempfile::tempdir().unwrap();
    let mut store = Store::open(&state.path().join("mush.sqlite")).unwrap();

    let error = run(&mut store, None).unwrap_err();

    assert_eq!(
        error.to_string(),
        "mush tui needs a terminal; use mush tui --snapshot for non-interactive inspection"
    );
}

#[test]
fn draw_handles_empty_and_selected_task_views() {
    let backend = TestBackend::new(180, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &[], 0)).unwrap();
    let empty = terminal.backend().buffer().content().iter().fold(
        String::new(),
        |mut output, cell| {
            output.push_str(cell.symbol());
            output
        },
    );
    assert!(empty.contains("No tasks"));

    let tasks = vec![task(TaskKind::Work)];
    terminal.draw(|frame| draw(frame, &tasks, 0)).unwrap();
    let rendered = terminal.backend().buffer().content().iter().fold(
        String::new(),
        |mut output, cell| {
            output.push_str(cell.symbol());
            output
        },
    );
    assert!(rendered.contains("> #7 [work / pending] deterministic view"));
    assert!(rendered.contains("Task #7"));

    let mut completed = task(TaskKind::Work);
    completed.status = TaskStatus::Completed;
    let mut waiting_checkpoint = task(TaskKind::Checkpoint);
    waiting_checkpoint.id = 8;
    waiting_checkpoint.subject_task_id = Some(9);
    let tasks = vec![completed, waiting_checkpoint];
    terminal.draw(|frame| draw(frame, &tasks, 1)).unwrap();
    let rendered = terminal.backend().buffer().content().iter().fold(
        String::new(),
        |mut output, cell| {
            output.push_str(cell.symbol());
            output
        },
    );
    assert!(rendered.contains("(no checkpoint yet)"));
    assert!(rendered.contains("(awaiting subject)"));
}
