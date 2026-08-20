use super::*;

#[test]
fn automatic_capacity_unions_supervised_children_with_live_locks() {
    let supervised = BTreeSet::from([1, 2]);

    assert_eq!(available_capacity(&supervised, &[2, 3]), 1);
    assert_eq!(available_capacity(&supervised, &[1, 2, 3, 4, 5]), 0);
    assert_eq!(available_capacity(&BTreeSet::new(), &[]), CONCURRENCY_BOUND);
}

#[test]
fn an_empty_serve_pass_reaps_reconciles_and_starts_nothing() {
    let state = tempfile::tempdir().unwrap();
    let mut store = Store::open(&state.path().join("mush.sqlite")).unwrap();
    let database = store.database_path().to_path_buf();
    let mut children = BTreeMap::new();

    serve_pass(&mut store, &database, &mut children).unwrap();

    assert!(children.is_empty());
    assert_eq!(store.pending_launches(CONCURRENCY_BOUND).unwrap(), Vec::<i64>::new());
}

#[cfg(unix)]
#[test]
fn reaping_children_distinguishes_success_from_a_failure_that_needs_intervention() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&state.path().join("mush.sqlite")).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let agent = store
        .register_agent(crate::store::AgentRegistration {
            project_id: project.id,
            name: "worker",
            harness: "manual",
            model: "model",
            settings: "{}",
            checkpoint: false,
        })
        .unwrap();
    let create = |store: &mut Store, description| {
        let task = store
            .add_work_task(crate::store::WorkTaskRequest {
                project_id: project.id,
                agent_id: agent.id,
                description,
                parent_task_id: None,
            })
            .unwrap();
        store.queue(task.id).unwrap()
    };
    let succeeded = create(&mut store, "successful child");
    let failed = create(&mut store, "failed child");
    let mut success = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
    let mut failure = Command::new("sh").args(["-c", "exit 1"]).spawn().unwrap();
    success.wait().unwrap();
    failure.wait().unwrap();
    let mut children = BTreeMap::from([(succeeded.id, success), (failed.id, failure)]);

    reap(&mut store, &mut children).unwrap();

    assert!(children.is_empty());
    assert_eq!(store.task(succeeded.id).unwrap().readiness_status, ReadinessStatus::Ready);
    let failed = store.task(failed.id).unwrap();
    assert_eq!(failed.readiness_status, ReadinessStatus::InterventionRequired);
    assert!(failed.intervention.unwrap().contains("exited with exit status: 1"));
}

#[cfg(unix)]
#[test]
fn child_status_classification_matches_the_runner_exit_contract() {
    use std::os::unix::process::ExitStatusExt;

    assert_eq!(
        classify_child_status(ExitStatus::from_raw(0)),
        ChildOutcome::Executed
    );
    assert_eq!(
        classify_child_status(ExitStatus::from_raw(EXIT_DID_NOT_START << 8)),
        ChildOutcome::DidNotStart
    );
    assert_eq!(
        classify_child_status(ExitStatus::from_raw(1 << 8)),
        ChildOutcome::Failed
    );
    assert_eq!(
        classify_child_status(ExitStatus::from_raw(15)),
        ChildOutcome::Failed
    );
}
