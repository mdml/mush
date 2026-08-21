//! The staged bounded checkpoint loop: declaration, budget-gated path
//! materialization with mechanical report passing, the loop-satisfying
//! success continuation, both stop paths, and the schema-level immutability
//! of the declared contract.

mod common;
use common::StoreTestExt;
use mush::{
    CheckpointDecision, LoopStatus, ReadinessStatus, Store,
    store::{LoopDeclaration, StageSpec},
};

fn fixture(database: &std::path::Path) -> (Store, tempfile::TempDir, i64, i64, i64) {
    let project_dir = tempfile::tempdir().unwrap();
    let store = Store::open(database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent_args(project.id, "worker", "manual", "human", "{}", false)
        .unwrap();
    let adjudicator = store
        .register_agent_args(
            project.id,
            "adjudicator",
            "manual",
            "human",
            "{\"adjudicator\":true}",
            true,
        )
        .unwrap();
    (store, project_dir, project.id, worker.id, adjudicator.id)
}

fn declare(
    store: &mut Store,
    project: i64,
    stages: &[(i64, &str)],
    max_attempts: i64,
) -> mush::LoopReport {
    let specs: Vec<StageSpec<'_>> = stages
        .iter()
        .map(|(agent_id, description)| StageSpec {
            agent_id: *agent_id,
            description,
        })
        .collect();
    store
        .declare_loop(LoopDeclaration {
            project_id: project,
            stages: &specs,
            criteria: "## Criteria\n\n- the declared behavior is covered",
            adjudicator_agent_id: None,
            max_attempts,
            reuse_worktree: false,
        })
        .unwrap()
}

#[test]
fn the_executable_checkpoint_example_runs_as_a_one_stage_loop() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, adjudicator) = fixture(&database);

    // Steps 1-2: declare the loop; the policy materializes attempt one and
    // its checkpoint with the declared criteria and adjudicator.
    let report = declare(&mut store, project, &[(worker, "Produce the plan")], 3);
    assert_eq!(report.status, LoopStatus::InProgress);
    assert_eq!(report.remaining_attempts, 2);
    let first_stage = report.attempts[0].stage_task_ids[0];
    let first_checkpoint = report.attempts[0].checkpoint_task_id;
    let checkpoint = store.task(first_checkpoint).unwrap();
    assert_eq!(checkpoint.agent_id, Some(adjudicator));
    assert_eq!(checkpoint.subject_task_id, Some(first_stage));
    assert_eq!(
        checkpoint.criteria.as_deref(),
        Some("## Criteria\n\n- the declared behavior is covered")
    );

    // Step 3: implementation is the declared success continuation, gated on
    // the loop itself rather than on any materialized checkpoint.
    let implementation = store
        .add_work_task_args(project, worker, "Implement the plan", None)
        .unwrap();
    store
        .declare_loop_continuation(report.declaration.id, implementation.id)
        .unwrap();
    store.queue(implementation.id).unwrap();
    assert_eq!(
        store.task(implementation.id).unwrap().readiness_status,
        ReadinessStatus::Blocked,
        "the continuation waits on the loop, not on a task edge"
    );

    // Steps 4-5: the first attempt runs and its checkpoint records not_met.
    store
        .complete_work(first_stage, "A plan missing verification", None)
        .unwrap();
    let outcome = store
        .decide_checkpoint(
            first_checkpoint,
            CheckpointDecision::NotMet,
            "criterion 'verification' is not stated",
        )
        .unwrap();

    // Step 6: the policy materializes the second attempt with the bounded
    // restart packet and an identical checkpoint contract.
    assert_eq!(outcome.materialized.len(), 2);
    let second_stage = &outcome.materialized[0];
    let second_checkpoint = &outcome.materialized[1];
    assert_eq!(second_stage.previous_task_id, Some(first_stage));
    assert!(second_stage.description.contains("Produce the plan"));
    assert!(
        second_stage
            .description
            .contains("A plan missing verification"),
        "the prior attempt's final report is spliced in: {}",
        second_stage.description
    );
    assert!(
        second_stage
            .description
            .contains("criterion 'verification' is not stated")
    );
    assert_eq!(
        second_checkpoint.criteria,
        store.task(first_checkpoint).unwrap().criteria,
        "the new checkpoint carries exactly the declared criteria"
    );
    assert_eq!(second_checkpoint.agent_id, Some(adjudicator));
    assert_eq!(second_checkpoint.subject_task_id, Some(second_stage.id));
    assert_eq!(second_checkpoint.previous_task_id, Some(first_checkpoint));
    let report = store.loop_report(report.declaration.id).unwrap();
    assert_eq!(report.remaining_attempts, 1);
    assert_eq!(
        store.task(implementation.id).unwrap().readiness_status,
        ReadinessStatus::Blocked
    );

    // Step 7: met from the second permitted attempt satisfies the
    // continuation.
    store
        .complete_work(second_stage.id, "A plan covering verification", None)
        .unwrap();
    let outcome = store
        .decide_checkpoint(
            second_checkpoint.id,
            CheckpointDecision::Met,
            "every declared criterion is satisfied",
        )
        .unwrap();
    assert!(outcome.materialized.is_empty());
    assert_eq!(
        store.task(implementation.id).unwrap().readiness_status,
        ReadinessStatus::Ready,
        "met on any permitted attempt makes the continuation eligible"
    );
    let report = store.loop_report(report.declaration.id).unwrap();
    assert_eq!(report.status, LoopStatus::Satisfied);
    assert!(report.next_action.contains(&format!(
        "continuation task {} is eligible",
        implementation.id
    )));
}

#[test]
fn a_two_stage_path_chains_stages_and_restarts_whole() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, _adjudicator) = fixture(&database);
    let reviewer = store
        .register_agent_args(project, "reviewer", "manual", "human", "{\"r\":1}", false)
        .unwrap();

    let report = declare(
        &mut store,
        project,
        &[
            (worker, "Implement the behavior"),
            (reviewer.id, "Review the implementation and report findings"),
        ],
        2,
    );
    let [implement, review] = report.attempts[0].stage_task_ids[..] else {
        panic!("two stages materialize");
    };
    let checkpoint = report.attempts[0].checkpoint_task_id;
    assert_eq!(
        store.task(checkpoint).unwrap().subject_task_id,
        Some(review),
        "the checkpoint adjudicates the final stage's report"
    );

    // The whole path restarts on not_met: both stages re-materialize, each
    // linked to its stage counterpart.
    store.complete_work(implement, "implemented", None).unwrap();
    store
        .complete_work(review, "## Review\n\ntwo blocking issues", None)
        .unwrap();
    let outcome = store
        .decide_checkpoint(
            checkpoint,
            CheckpointDecision::NotMet,
            "the review identifies blocking issues",
        )
        .unwrap();
    assert_eq!(outcome.materialized.len(), 3);
    let second_implement = &outcome.materialized[0];
    let second_review = &outcome.materialized[1];
    assert_eq!(second_implement.previous_task_id, Some(implement));
    assert_eq!(second_review.previous_task_id, Some(review));
    assert!(
        second_implement.description.contains("two blocking issues"),
        "the reviewer's report feeds the next attempt's first stage"
    );
    assert!(
        !second_review.description.contains("two blocking issues"),
        "later stages keep their declared descriptions"
    );

    // Loop members cannot be edge prerequisites; the continuation is the
    // loop's only outward face.
    let downstream = store
        .add_work_task_args(project, worker, "Downstream", None)
        .unwrap();
    let error = store
        .add_dependency(second_review.id, downstream.id)
        .unwrap_err()
        .to_string();
    assert!(error.contains("internal to loop"), "{error}");
}

#[test]
fn blocked_and_budget_exhaustion_stop_the_loop_legibly() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, _adjudicator) = fixture(&database);

    // blocked materializes nothing and stops for fresh judgment.
    let report = declare(&mut store, project, &[(worker, "Attempt the work")], 3);
    let stage = report.attempts[0].stage_task_ids[0];
    let checkpoint = report.attempts[0].checkpoint_task_id;
    store.complete_work(stage, "done", None).unwrap();
    let outcome = store
        .decide_checkpoint(
            checkpoint,
            CheckpointDecision::Blocked,
            "adjudication needs missing external state",
        )
        .unwrap();
    assert!(outcome.materialized.is_empty());
    let stopped = store.loop_report(report.declaration.id).unwrap();
    assert_eq!(stopped.status, LoopStatus::Blocked);
    assert!(stopped.next_action.contains("fresh judgment"));

    // not_met on the final permitted attempt reports budget exhaustion.
    let report = declare(&mut store, project, &[(worker, "One shot")], 1);
    let stage = report.attempts[0].stage_task_ids[0];
    let checkpoint = report.attempts[0].checkpoint_task_id;
    store.complete_work(stage, "done", None).unwrap();
    let outcome = store
        .decide_checkpoint(checkpoint, CheckpointDecision::NotMet, "gap remains")
        .unwrap();
    assert!(
        outcome.materialized.is_empty(),
        "an exhausted budget materializes nothing"
    );
    let stopped = store.loop_report(report.declaration.id).unwrap();
    assert_eq!(stopped.status, LoopStatus::Exhausted);
    assert_eq!(stopped.remaining_attempts, 0);
    assert!(stopped.next_action.contains("budget of 1 exhausted"));
}

#[cfg(unix)]
#[test]
fn infrastructure_recovery_does_not_consume_the_semantic_budget() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let executable = state.path().join("fake-claude");
    common::fake_claude(&executable, true);
    let mut store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent_args(
            project.id,
            "worker",
            "claude-code",
            "claude-opus-5",
            &common::claude_settings(&executable, None),
            false,
        )
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "adjudicator",
            "claude-code",
            "claude-opus-5",
            &common::claude_settings(&executable, Some("Adjudicate against the criteria")),
            true,
        )
        .unwrap();

    let report = declare(&mut store, project.id, &[(worker.id, "Do the work")], 3);
    let stage = report.attempts[0].stage_task_ids[0];
    assert_eq!(
        store.task(stage).unwrap().readiness_status,
        ReadinessStatus::Ready,
        "an executable first stage is queued at materialization"
    );
    assert_eq!(
        store
            .task(report.attempts[0].checkpoint_task_id)
            .unwrap()
            .readiness_status,
        ReadinessStatus::Blocked,
        "the queued checkpoint awaits its subject"
    );

    // An interrupted execution resumes the same task: recovery requeues it
    // without touching the loop's attempt count.
    store
        .begin_execution_args(
            stage,
            Some("session"),
            Some("worktree"),
            state.path(),
            "boot",
        )
        .unwrap();
    store.interrupt_execution(stage).unwrap();
    let recovered = store.recover_launches(&[stage], None).unwrap();
    assert_eq!(recovered, vec![stage]);
    let report = store.loop_report(report.declaration.id).unwrap();
    assert_eq!(report.attempts.len(), 1, "no semantic attempt was consumed");
    assert_eq!(report.remaining_attempts, 2);
    assert!(store.task(stage).unwrap().execution_attempt > 0);
}

#[cfg(unix)]
#[test]
fn launch_prompts_carry_stage_reports_and_the_checkpoint_packet() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let executable = state.path().join("fake-claude");
    common::fake_claude(&executable, true);
    let mut store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let implementer = store
        .register_agent_args(
            project.id,
            "implementer",
            "claude-code",
            "claude-opus-5",
            &common::claude_settings(&executable, None),
            false,
        )
        .unwrap();
    let adjudicator = store
        .register_agent_args(
            project.id,
            "adjudicator",
            "claude-code",
            "claude-opus-5",
            &common::claude_settings(&executable, Some("Adjudicate against the criteria")),
            true,
        )
        .unwrap();
    let report = declare(
        &mut store,
        project.id,
        &[
            (implementer.id, "Implement the behavior"),
            (
                implementer.id,
                "Review the implementation and report findings",
            ),
        ],
        2,
    );
    let _ = adjudicator;
    let [implement, review] = report.attempts[0].stage_task_ids[..] else {
        panic!("two stages materialize");
    };
    let checkpoint = report.attempts[0].checkpoint_task_id;
    let executor = mush::Executor::new(&database);
    let options = mush::executor::RunOptions::default();

    executor.run(&mut store, implement, &options).unwrap();
    executor.run(&mut store, review, &options).unwrap();
    let review_prompt = std::fs::read_to_string(
        state
            .path()
            .join(format!("artifacts/task-{review}/prompt.md")),
    )
    .unwrap();
    assert!(
        review_prompt.contains(&format!("## Input report from task {implement}")),
        "{review_prompt}"
    );
    assert!(
        review_prompt.contains("fake executor result"),
        "the prerequisite stage's report is passed at launch: {review_prompt}"
    );

    executor.run(&mut store, checkpoint, &options).unwrap();
    let checkpoint_prompt = std::fs::read_to_string(
        state
            .path()
            .join(format!("artifacts/task-{checkpoint}/prompt.md")),
    )
    .unwrap();
    for expected in [
        "Adjudicate against the criteria",
        "## Checkpoint packet",
        "the declared behavior is covered",
        &format!("mush checkpoint decide {checkpoint}"),
        &format!("### Subject result (task {review})"),
    ] {
        assert!(checkpoint_prompt.contains(expected), "{checkpoint_prompt}");
    }
}

#[test]
fn a_malformed_declaration_is_refused_before_anything_is_materialized() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, _adjudicator) = fixture(&database);
    let elsewhere_dir = tempfile::tempdir().unwrap();
    let elsewhere = store
        .register_project("elsewhere", elsewhere_dir.path())
        .unwrap();
    let foreign = store
        .register_agent_args(elsewhere.id, "foreign", "manual", "human", "{}", false)
        .unwrap();

    let declaration =
        |stages: &[StageSpec<'_>], criteria: &str, max_attempts: i64, project_id: i64| -> String {
            let mut store = Store::open(&database).unwrap();
            store
                .declare_loop(LoopDeclaration {
                    project_id,
                    stages,
                    criteria,
                    adjudicator_agent_id: None,
                    max_attempts,
                    reuse_worktree: false,
                })
                .unwrap_err()
                .to_string()
        };
    let stage = |agent_id: i64, description: &'static str| StageSpec {
        agent_id,
        description,
    };

    assert!(declaration(&[], "criteria", 2, project).contains("at least one work stage"));
    assert!(
        declaration(&[stage(worker, "work")], "  ", 2, project).contains("criteria are required")
    );
    assert!(declaration(&[stage(worker, "work")], "criteria", 0, project).contains("at least 1"));
    assert!(
        declaration(&[stage(worker, "work")], "criteria", 2, 9_999).contains("project not found")
    );
    assert!(
        declaration(&[stage(worker, " ")], "criteria", 2, project)
            .contains("every loop stage requires a description")
    );
    assert!(
        declaration(&[stage(9_999, "work")], "criteria", 2, project).contains("agent not found")
    );
    assert!(
        declaration(&[stage(foreign.id, "work")], "criteria", 2, project)
            .contains("not registered to this project")
    );
    assert!(
        store.loops(None).unwrap().is_empty(),
        "a refused declaration materializes nothing"
    );

    // The adjudicator assignment is validated at declaration too.
    let plain = tempfile::tempdir().unwrap();
    let plain_project = store.register_project("plain", plain.path()).unwrap();
    let plain_agent = store
        .register_agent_args(plain_project.id, "solo", "manual", "human", "{}", false)
        .unwrap();
    let error = store
        .declare_loop(LoopDeclaration {
            project_id: plain_project.id,
            stages: &[stage(plain_agent.id, "work")],
            criteria: "criteria",
            adjudicator_agent_id: None,
            max_attempts: 1,
            reuse_worktree: false,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("no default checkpoint agent"), "{error}");
    let error = store
        .declare_loop(LoopDeclaration {
            project_id: plain_project.id,
            stages: &[stage(plain_agent.id, "work")],
            criteria: "criteria",
            adjudicator_agent_id: Some(foreign.id),
            max_attempts: 1,
            reuse_worktree: false,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("not registered to this project"), "{error}");
}

#[test]
fn a_continuation_declaration_is_refused_unless_it_can_be_gated() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, _adjudicator) = fixture(&database);
    let report = declare(&mut store, project, &[(worker, "Work")], 1);
    let loop_id = report.declaration.id;

    // A checkpoint cannot be gated on a loop.
    let subject = store
        .add_work_task_args(project, worker, "Subject", None)
        .unwrap();
    let checkpoint = store
        .create_checkpoint_args(subject.id, "criteria", None)
        .unwrap();
    let error = store
        .declare_loop_continuation(loop_id, checkpoint.id)
        .unwrap_err()
        .to_string();
    assert!(error.contains("only a work task"), "{error}");

    // Nor may the continuation belong to another project.
    let elsewhere_dir = tempfile::tempdir().unwrap();
    let elsewhere = store
        .register_project("elsewhere", elsewhere_dir.path())
        .unwrap();
    let foreign_agent = store
        .register_agent_args(elsewhere.id, "foreign", "manual", "human", "{}", false)
        .unwrap();
    let foreign_task = store
        .add_work_task_args(elsewhere.id, foreign_agent.id, "Foreign", None)
        .unwrap();
    let error = store
        .declare_loop_continuation(loop_id, foreign_task.id)
        .unwrap_err()
        .to_string();
    assert!(error.contains("loop's project"), "{error}");

    // Nor a task that has already begun.
    let started = store
        .add_work_task_args(project, worker, "Started", None)
        .unwrap();
    store
        .complete_work(started.id, "already done", None)
        .unwrap();
    let error = store
        .declare_loop_continuation(loop_id, started.id)
        .unwrap_err()
        .to_string();
    assert!(error.contains("before the dependent begins"), "{error}");

    // Declaring the same continuation twice is idempotent, and an unknown
    // loop or task is reported as missing.
    let continuation = store
        .add_work_task_args(project, worker, "Continue", None)
        .unwrap();
    store
        .declare_loop_continuation(loop_id, continuation.id)
        .unwrap();
    let repeated = store
        .declare_loop_continuation(loop_id, continuation.id)
        .unwrap();
    assert_eq!(
        repeated.declaration.continuation_task_id,
        Some(continuation.id)
    );
    assert!(
        store
            .declare_loop_continuation(9_999, continuation.id)
            .unwrap_err()
            .to_string()
            .contains("loop not found")
    );
    let fresh = declare(&mut store, project, &[(worker, "Fresh")], 1);
    assert!(
        store
            .declare_loop_continuation(fresh.declaration.id, 9_999)
            .unwrap_err()
            .to_string()
            .contains("task not found")
    );
}

#[test]
fn a_satisfied_loop_without_a_continuation_reports_its_own_completion() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, _adjudicator) = fixture(&database);
    let report = declare(&mut store, project, &[(worker, "Work")], 2);
    let stage = report.attempts[0].stage_task_ids[0];
    store.complete_work(stage, "done", None).unwrap();
    store
        .decide_checkpoint(
            report.attempts[0].checkpoint_task_id,
            CheckpointDecision::Met,
            "satisfied",
        )
        .unwrap();
    let satisfied = store.loop_report(report.declaration.id).unwrap();
    assert_eq!(satisfied.status, LoopStatus::Satisfied);
    assert!(
        satisfied.next_action.contains("declares no continuation"),
        "{}",
        satisfied.next_action
    );
    assert!(store.loop_report(9_999).is_err());
}

#[test]
fn a_reused_worktree_carries_into_the_next_attempt() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, _adjudicator) = fixture(&database);
    let report = store
        .declare_loop(LoopDeclaration {
            project_id: project,
            stages: &[StageSpec {
                agent_id: worker,
                description: "Work in place",
            }],
            criteria: "the declared criteria hold",
            adjudicator_agent_id: None,
            max_attempts: 2,
            reuse_worktree: true,
        })
        .unwrap();
    // The first attempt records the worktree it ran in.
    let stage = report.attempts[0].stage_task_ids[0];
    store
        .begin_execution_args(stage, None, Some("shared-worktree"), state.path(), "boot")
        .unwrap();
    store.interrupt_execution(stage).unwrap();
    store.complete_work(stage, "first pass", None).unwrap();
    let outcome = store
        .decide_checkpoint(
            report.attempts[0].checkpoint_task_id,
            CheckpointDecision::NotMet,
            "another pass",
        )
        .unwrap();
    assert_eq!(
        outcome.materialized[0].worktree_name.as_deref(),
        Some("shared-worktree"),
        "the declared policy lets the next attempt reuse its counterpart's worktree"
    );
}

#[test]
fn the_tui_snapshot_restates_a_loop_without_transcript_access() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, _adjudicator) = fixture(&database);
    let report = declare(&mut store, project, &[(worker, "Work")], 2);
    let loop_id = report.declaration.id;
    let stage = report.attempts[0].stage_task_ids[0];

    let undecided = mush::tui::snapshot(&store, Some(project)).unwrap();
    assert!(undecided.contains(&format!("Loop #{loop_id} [in_progress] attempt 1/2")));
    assert!(undecided.contains("- the declared behavior is covered"));
    assert!(undecided.contains("decision: undecided"));
    assert!(undecided.contains("remaining attempts: 1"));
    assert!(undecided.contains(&format!("run stage 1 (task {stage})")));
    assert!(!undecided.contains("continuation: #"));
    // The loop's own evidence line is indented under its attempt; the
    // checkpoint task's inherited placeholder evidence is not adjudication
    // evidence and must not read as one.
    assert!(
        !undecided.contains("\n    evidence (Markdown):"),
        "an undecided attempt restates no adjudication evidence: {undecided}"
    );

    let continuation = store
        .add_work_task_args(project, worker, "Continue", None)
        .unwrap();
    store
        .declare_loop_continuation(loop_id, continuation.id)
        .unwrap();
    store.complete_work(stage, "done", None).unwrap();
    store
        .decide_checkpoint(
            report.attempts[0].checkpoint_task_id,
            CheckpointDecision::Met,
            "satisfied",
        )
        .unwrap();
    let decided = mush::tui::snapshot(&store, None).unwrap();
    assert!(decided.contains(&format!("Loop #{loop_id} [satisfied]")));
    assert!(decided.contains("decision: met"));
    assert!(
        decided.contains("\n    evidence (Markdown):\n      satisfied\n"),
        "the decided attempt restates its adjudication evidence: {decided}"
    );
    assert!(decided.contains(&format!("continuation: #{}", continuation.id)));
    assert!(decided.contains(&format!("(loop #{loop_id})")));
    assert!(decided.contains("criteria (Markdown):"));
}

#[test]
fn the_declared_contract_is_immutable_at_the_schema_layer() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, _adjudicator) = fixture(&database);
    let report = declare(&mut store, project, &[(worker, "Work")], 2);
    let stage = report.attempts[0].stage_task_ids[0];
    let checkpoint = report.attempts[0].checkpoint_task_id;
    let loop_id = report.declaration.id;
    let other = store
        .add_work_task_args(project, worker, "Other", None)
        .unwrap();

    let connection = rusqlite::Connection::open(&database).unwrap();
    for (statement, expected) in [
        (
            format!(
                "UPDATE tasks SET subject_task_id={} WHERE id={checkpoint}",
                other.id
            ),
            "never retargeted",
        ),
        (
            format!("UPDATE tasks SET criteria='moved criteria' WHERE id={checkpoint}"),
            "criteria are immutable",
        ),
        (
            format!("UPDATE tasks SET loop_id=NULL WHERE id={stage}"),
            "loop membership is immutable",
        ),
        (
            format!("UPDATE loops SET max_attempts=9 WHERE id={loop_id}"),
            "a declared loop is immutable",
        ),
        (
            format!("UPDATE loop_stages SET description='moved' WHERE loop_id={loop_id}"),
            "loop stages are immutable",
        ),
        (
            format!("DELETE FROM loop_stages WHERE loop_id={loop_id}"),
            "loop stages are immutable",
        ),
        (
            format!(
                "INSERT INTO tasks(project_id,agent_id,kind,status,description,subject_task_id) VALUES({project},{worker},'checkpoint','pending','no criteria',{})",
                other.id
            ),
            "requires declared criteria",
        ),
    ] {
        let error = connection.execute(&statement, []).unwrap_err().to_string();
        assert!(error.contains(expected), "{statement}: {error}");
    }

    // The continuation is declared once, even at the schema layer.
    store.declare_loop_continuation(loop_id, other.id).unwrap();
    let error = connection
        .execute(
            &format!("UPDATE loops SET continuation_task_id={stage} WHERE id={loop_id}"),
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("declared once"), "{error}");

    // And the domain refuses the invalid continuation declarations directly.
    let error = store
        .declare_loop_continuation(loop_id, stage)
        .unwrap_err()
        .to_string();
    assert!(error.contains("already declares"), "{error}");
    let second = declare(&mut store, project, &[(worker, "Second loop")], 1);
    let member = second.attempts[0].stage_task_ids[0];
    let error = store
        .declare_loop_continuation(second.declaration.id, member)
        .unwrap_err()
        .to_string();
    assert!(error.contains("loop member"), "{error}");
}

fn cli(database: &std::path::Path, arguments: &[&str]) -> serde_json::Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mush"))
        .arg("--database")
        .arg(database)
        .arg("--json")
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn the_public_cli_declares_steps_and_restates_a_loop() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let project = cli(
        &database,
        &[
            "project",
            "register",
            "--name",
            "project",
            "--path",
            project_dir.path().to_str().unwrap(),
        ],
    )["id"]
        .to_string();
    let worker = cli(
        &database,
        &[
            "agent",
            "register",
            "--project",
            &project,
            "--name",
            "worker",
            "--harness",
            "manual",
            "--model",
            "human",
        ],
    )["id"]
        .to_string();
    cli(
        &database,
        &[
            "agent",
            "register",
            "--project",
            &project,
            "--name",
            "adjudicator",
            "--harness",
            "manual",
            "--model",
            "human",
            "--settings",
            "{\"adjudicator\":true}",
            "--checkpoint",
        ],
    );

    let declared = cli(
        &database,
        &[
            "loop",
            "declare",
            "--project",
            &project,
            "--stage",
            &format!("{worker}:Produce the plan"),
            "--criteria",
            "the plan covers the requested behavior",
            "--max-attempts",
            "3",
        ],
    );
    let loop_id = declared["id"].to_string();
    assert_eq!(declared["status"], "in_progress");
    assert_eq!(declared["remaining_attempts"], 2);
    let stage = declared["attempts"][0]["stage_task_ids"][0].to_string();
    let checkpoint = declared["attempts"][0]["checkpoint_task_id"].to_string();

    let implementation = cli(
        &database,
        &[
            "task",
            "add",
            "--project",
            &project,
            "--agent",
            &worker,
            "--description",
            "Implement the plan",
        ],
    )["id"]
        .to_string();
    let with_continuation = cli(
        &database,
        &["loop", "continuation", &loop_id, &implementation],
    );
    assert_eq!(
        with_continuation["continuation_task_id"].to_string(),
        implementation
    );

    cli(
        &database,
        &["task", "complete", &stage, "--result", "the plan"],
    );
    let outcome = cli(
        &database,
        &[
            "checkpoint",
            "decide",
            &checkpoint,
            "--decision",
            "not_met",
            "--evidence",
            "verification is missing",
        ],
    );
    assert_eq!(outcome["materialized"].as_array().unwrap().len(), 2);

    let shown = cli(&database, &["loop", "show", &loop_id]);
    assert_eq!(shown["status"], "in_progress");
    assert_eq!(shown["remaining_attempts"], 1);
    assert_eq!(shown["attempts"][0]["decision"], "not_met");
    assert_eq!(
        shown["attempts"][0]["evidence"], "verification is missing",
        "the decided attempt restates its adjudication evidence"
    );
    assert!(
        shown["attempts"][1]["evidence"].is_null(),
        "the fresh attempt is not yet adjudicated: {shown}"
    );
    assert_eq!(shown["criteria"], "the plan covers the requested behavior");
    assert!(
        shown["next_action"]
            .as_str()
            .unwrap()
            .contains("run stage 1"),
        "{shown}"
    );
    let listed = cli(&database, &["loop", "list", "--project", &project]);
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

fn cli_failure(database: &std::path::Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mush"))
        .arg("--database")
        .arg(database)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "{arguments:?} unexpectedly passed"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn stage_declarations_accept_files_and_name_their_malformed_input() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let project = cli(
        &database,
        &[
            "project",
            "register",
            "--name",
            "project",
            "--path",
            project_dir.path().to_str().unwrap(),
        ],
    )["id"]
        .to_string();
    let worker = cli(
        &database,
        &[
            "agent",
            "register",
            "--project",
            &project,
            "--name",
            "worker",
            "--harness",
            "manual",
            "--model",
            "human",
        ],
    )["id"]
        .to_string();
    cli(
        &database,
        &[
            "agent",
            "register",
            "--project",
            &project,
            "--name",
            "adjudicator",
            "--harness",
            "manual",
            "--model",
            "human",
            "--settings",
            "{\"adjudicator\":true}",
            "--checkpoint",
        ],
    );
    let stage_file = state.path().join("stage.md");
    std::fs::write(&stage_file, "## Stage\n\nDo the declared work").unwrap();
    let criteria_file = state.path().join("criteria.md");
    std::fs::write(&criteria_file, "## Criteria\n\n- it holds").unwrap();

    let declared = cli(
        &database,
        &[
            "loop",
            "declare",
            "--project",
            &project,
            "--stage",
            &format!("{worker}:@{}", stage_file.display()),
            "--criteria-file",
            criteria_file.to_str().unwrap(),
            "--max-attempts",
            "2",
        ],
    );
    assert_eq!(declared["criteria"], "## Criteria\n\n- it holds");
    let stage = declared["attempts"][0]["stage_task_ids"][0].to_string();
    let shown = cli(&database, &["task", "show", &stage]);
    assert_eq!(shown["description"], "## Stage\n\nDo the declared work");

    for (stage_argument, expected) in [
        ("no-separator", "is not AGENT_ID:DESCRIPTION"),
        ("notanumber:work", "does not start with an agent id"),
    ] {
        let error = cli_failure(
            &database,
            &[
                "loop",
                "declare",
                "--project",
                &project,
                "--stage",
                stage_argument,
                "--criteria",
                "criteria",
                "--max-attempts",
                "1",
            ],
        );
        assert!(error.contains(expected), "{stage_argument}: {error}");
    }
}

#[test]
fn checkpoint_contracts_are_pinned_at_creation() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project, worker, adjudicator) = fixture(&database);
    let work = store
        .add_work_task_args(project, worker, "Work", None)
        .unwrap();
    let error = store
        .create_checkpoint_args(work.id, "  ", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("criteria are required"), "{error}");
    let checkpoint = store
        .create_checkpoint_args(work.id, "the declared criteria hold", Some(adjudicator))
        .unwrap();
    let repeated = store
        .create_checkpoint_args(work.id, "the declared criteria hold", None)
        .unwrap();
    assert_eq!(
        repeated.id, checkpoint.id,
        "an identical contract is idempotent"
    );
    let error = store
        .create_checkpoint_args(work.id, "different criteria", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("immutable"), "{error}");
}
