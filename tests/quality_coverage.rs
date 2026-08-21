use mush::{
    CheckpointDecision, DomainError, ExecutionStatus, Executor, ReadinessStatus, Store, TaskKind,
    TaskStatus,
    executor::{RunOptions, acquire_execution_lock, validate_agent_registration},
    lock,
    store::{AgentRegistration, ExecutionStart, WorkTaskRequest},
};
use std::{path::PathBuf, process::Command, str::FromStr};

fn mush_command(database: &std::path::Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mush"))
        .arg("--database")
        .arg(database)
        .args(arguments)
        .output()
        .unwrap()
}

fn successful_json(database: &std::path::Path, command_arguments: &[&str]) -> serde_json::Value {
    let mut arguments = vec!["--json"];
    arguments.extend_from_slice(command_arguments);
    let output = mush_command(database, &arguments);
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn domain_names_round_trip_and_reject_unknown_values() {
    for (text, value) in [
        ("work", TaskKind::Work),
        ("checkpoint", TaskKind::Checkpoint),
    ] {
        assert_eq!(TaskKind::from_str(text).unwrap(), value);
        assert_eq!(value.to_string(), text);
    }
    for (text, value) in [
        ("unqueued", ReadinessStatus::Unqueued),
        ("blocked", ReadinessStatus::Blocked),
        ("ready", ReadinessStatus::Ready),
        ("claimed", ReadinessStatus::Claimed),
        ("running", ReadinessStatus::Running),
        ("completed", ReadinessStatus::Completed),
        (
            "intervention_required",
            ReadinessStatus::InterventionRequired,
        ),
    ] {
        assert_eq!(ReadinessStatus::from_str(text).unwrap(), value);
        assert_eq!(value.to_string(), text);
    }
    for (text, value) in [
        ("running", ExecutionStatus::Running),
        ("succeeded", ExecutionStatus::Succeeded),
        ("interrupted", ExecutionStatus::Interrupted),
    ] {
        assert_eq!(ExecutionStatus::from_str(text).unwrap(), value);
        assert_eq!(value.to_string(), text);
    }
    for (text, value) in [
        ("pending", TaskStatus::Pending),
        ("completed", TaskStatus::Completed),
    ] {
        assert_eq!(TaskStatus::from_str(text).unwrap(), value);
        assert_eq!(value.to_string(), text);
    }
    for alias in ["not_met", "not-met"] {
        assert_eq!(
            CheckpointDecision::from_str(alias).unwrap(),
            CheckpointDecision::NotMet
        );
    }
    assert_eq!(
        CheckpointDecision::from_str("met").unwrap().to_string(),
        "met"
    );
    assert_eq!(
        CheckpointDecision::from_str("blocked").unwrap().to_string(),
        "blocked"
    );

    for error in [
        TaskKind::from_str("build").unwrap_err(),
        ReadinessStatus::from_str("waiting").unwrap_err(),
        ExecutionStatus::from_str("failed").unwrap_err(),
        TaskStatus::from_str("running").unwrap_err(),
        CheckpointDecision::from_str("maybe").unwrap_err(),
    ] {
        assert!(matches!(error, DomainError::Invalid(_)));
        assert!(error.to_string().contains("unknown"));
    }
}

#[test]
fn state_path_environment_fallbacks_and_missing_home_are_cli_contracts() {
    let state = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mush"))
        .args(["--json", "runner", "status"])
        .env_remove("MUSH_DATABASE")
        .env("MUSH_STATE_DIR", state.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(state.path().join("mush.sqlite").is_file());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report.is_object());

    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mush"))
        .args(["runner", "status"])
        .env_remove("MUSH_DATABASE")
        .env_remove("MUSH_STATE_DIR")
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(home.path().join(".mush/mush.sqlite").is_file());
    assert!(String::from_utf8_lossy(&output.stdout).contains("RunnerReport"));

    let output = Command::new(env!("CARGO_BIN_EXE_mush"))
        .args(["runner", "status"])
        .env_remove("MUSH_DATABASE")
        .env_remove("MUSH_STATE_DIR")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("HOME is unset"));
}

#[test]
fn cli_mutations_accept_file_inputs_and_report_json_outcomes() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let input_dir = tempfile::tempdir().unwrap();
    let settings = input_dir.path().join("settings.json");
    let review = input_dir.path().join("review.md");
    let description = input_dir.path().join("description.md");
    std::fs::write(&settings, "{}").unwrap();
    std::fs::write(&review, "Review carefully.").unwrap();
    std::fs::write(&description, "Implement the behavior").unwrap();

    let project = successful_json(
        &database,
        &[
            "project",
            "register",
            "--name",
            "quality",
            "--path",
            project_dir.path().to_str().unwrap(),
        ],
    );
    let project_id = project["id"].as_i64().unwrap().to_string();
    let agent = successful_json(
        &database,
        &[
            "agent",
            "register",
            "--project",
            &project_id,
            "--name",
            "worker",
            "--harness",
            "manual",
            "--model",
            "human",
            "--settings-file",
            settings.to_str().unwrap(),
            "--review-prompt-file",
            review.to_str().unwrap(),
        ],
    );
    assert_eq!(agent["name"], "worker");
    assert!(
        agent["settings"]
            .as_str()
            .unwrap()
            .contains("review_prompt")
    );
    let agent_id = agent["id"].as_i64().unwrap().to_string();
    successful_json(
        &database,
        &[
            "agent",
            "register",
            "--project",
            &project_id,
            "--name",
            "reviewer",
            "--harness",
            "manual",
            "--model",
            "human",
            "--checkpoint",
        ],
    );

    let updated = successful_json(
        &database,
        &[
            "agent",
            "update",
            &agent_id,
            "--settings",
            "{\"mode\":\"safe\"}",
        ],
    );
    assert!(updated["settings"].as_str().unwrap().contains("safe"));

    let first = successful_json(
        &database,
        &[
            "task",
            "add",
            "--project",
            &project_id,
            "--agent",
            &agent_id,
            "--description-file",
            description.to_str().unwrap(),
        ],
    );
    let first_id = first["id"].as_i64().unwrap().to_string();
    let second = successful_json(
        &database,
        &[
            "task",
            "add",
            "--project",
            &project_id,
            "--agent",
            &agent_id,
            "--description",
            "Dependent work",
        ],
    );
    let second_id = second["id"].as_i64().unwrap().to_string();

    let dependent = successful_json(&database, &["task", "depend", &first_id, &second_id]);
    assert_eq!(dependent["readiness_status"], "unqueued");
    let dependent = successful_json(&database, &["task", "undepend", &first_id, &second_id]);
    assert_eq!(dependent["id"], second["id"]);
    let queued = successful_json(&database, &["task", "queue", &second_id]);
    assert_eq!(queued["readiness_status"], "ready");

    let shown = successful_json(&database, &["task", "show", &first_id]);
    assert_eq!(shown["description"], "Implement the behavior");
    let listed = successful_json(&database, &["task", "list", "--project", &project_id]);
    assert_eq!(listed.as_array().unwrap().len(), 2);
    let completed = successful_json(
        &database,
        &[
            "task",
            "complete",
            &first_id,
            "--result",
            "done",
            "--evidence",
            "tested",
        ],
    );
    assert_eq!(completed["status"], "completed");
    let checkpoint = successful_json(&database, &["checkpoint", "create", &first_id]);
    let checkpoint_id = checkpoint["id"].as_i64().unwrap().to_string();
    let decided = successful_json(
        &database,
        &[
            "checkpoint",
            "decide",
            &checkpoint_id,
            "--decision",
            "met",
            "--evidence",
            "reviewed",
        ],
    );
    assert!(decided.is_null());
    let checkpoint = successful_json(&database, &["task", "show", &checkpoint_id]);
    assert_eq!(checkpoint["decision"], "met");
    assert!(successful_json(&database, &["runner", "status"]).is_object());
    let snapshot = mush_command(&database, &["tui", "--snapshot", "--project", &project_id]);
    assert!(snapshot.status.success());
    assert!(String::from_utf8_lossy(&snapshot.stdout).contains("Implement the behavior"));
}

#[test]
fn cli_validation_failures_have_specific_exit_codes_and_messages() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let project = successful_json(
        &database,
        &[
            "project",
            "register",
            "--name",
            "quality",
            "--path",
            project_dir.path().to_str().unwrap(),
        ],
    );
    let project_id = project["id"].as_i64().unwrap().to_string();
    let agent = successful_json(
        &database,
        &[
            "agent",
            "register",
            "--project",
            &project_id,
            "--name",
            "worker",
            "--harness",
            "manual",
            "--model",
            "human",
        ],
    );
    let agent_id = agent["id"].as_i64().unwrap().to_string();

    let empty = mush_command(
        &database,
        &[
            "task",
            "add",
            "--project",
            &project_id,
            "--agent",
            &agent_id,
        ],
    );
    assert_eq!(empty.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&empty.stderr).contains("task description is required"));

    let missing = mush_command(&database, &["task", "show", "999"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("task not found: 999"));

    let bad_file = mush_command(
        &database,
        &[
            "task",
            "add",
            "--project",
            &project_id,
            "--agent",
            &agent_id,
            "--description-file",
            "/definitely/missing/mush-description",
        ],
    );
    assert_eq!(bad_file.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad_file.stderr).contains("I/O error"));

    let tui = mush_command(&database, &["tui"]);
    assert_eq!(tui.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&tui.stderr).contains("needs a terminal"));

    let start = mush_command(&database, &["runner", "start"]);
    assert_eq!(start.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&start.stderr).contains("no queued task"));
}

#[test]
fn locks_report_ownership_contention_and_release() {
    assert_eq!(
        lock::task_lock_path(std::path::Path::new("mush.sqlite"), 7),
        PathBuf::from("locks/task-7.lock")
    );
    assert_eq!(
        lock::serve_lock_path(std::path::Path::new("state/mush.sqlite")),
        PathBuf::from("state/locks/serve.lock")
    );

    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let task_path = lock::task_lock_path(&database, 4);
    let mut first = lock::FileLock::try_acquire(&task_path).unwrap().unwrap();
    assert_eq!(first.path(), task_path);
    first.describe("test execution").unwrap();
    assert!(lock::is_held(&task_path).unwrap());
    assert!(lock::task_is_live(&database, 4).unwrap());
    assert!(lock::FileLock::try_acquire(&task_path).unwrap().is_none());
    drop(first);
    assert!(!lock::task_is_live(&database, 4).unwrap());

    let mut serve = lock::acquire_serve_lock(&database, "quality test")
        .unwrap()
        .unwrap();
    assert!(
        lock::serve_lock_holder(&database)
            .unwrap()
            .unwrap()
            .contains("quality test")
    );
    assert!(
        lock::acquire_serve_lock(&database, "second")
            .unwrap()
            .is_none()
    );
    serve.describe("").unwrap();
    assert_eq!(
        lock::serve_lock_holder(&database).unwrap().as_deref(),
        Some("an unidentified process")
    );
    drop(serve);
    assert_eq!(lock::serve_lock_holder(&database).unwrap(), None);

    let parent_file = state.path().join("not-a-directory");
    std::fs::write(&parent_file, "file").unwrap();
    assert!(matches!(
        lock::FileLock::try_acquire(&parent_file.join("lock")),
        Err(DomainError::Io(_))
    ));
}

#[test]
fn executor_registration_and_ownership_failures_are_explicit() {
    validate_agent_registration("manual", "not json", true).unwrap();
    let invalid = validate_agent_registration("claude-code", "not json", false).unwrap_err();
    assert!(invalid.to_string().contains("invalid claude-code settings"));
    let missing_review = validate_agent_registration(
        "claude-code",
        r#"{"executable":"claude","version":"1","model":"model","effort":"medium","permission_mode":"acceptEdits","tools":[],"allowed_tools":[]}"#,
        true,
    )
    .unwrap_err();
    assert!(missing_review.to_string().contains("review_prompt"));

    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&database).unwrap();
    let project = store
        .register_project("quality", project_dir.path())
        .unwrap();
    let agent = store
        .register_agent(AgentRegistration {
            project_id: project.id,
            name: "manual",
            harness: "manual",
            model: "human",
            settings: "{}",
            checkpoint: false,
        })
        .unwrap();
    let task = store
        .add_work_task(WorkTaskRequest {
            project_id: project.id,
            agent_id: agent.id,
            description: "work",
            parent_task_id: None,
        })
        .unwrap();

    let first = acquire_execution_lock(&database, task.id).unwrap();
    let collision = acquire_execution_lock(&database, task.id).unwrap_err();
    assert!(collision.to_string().contains("already executing"));
    assert!(collision.to_string().contains("task-1.lock"));
    drop(first);

    let unknown = Executor::new(&database)
        .run(&mut store, 999, &RunOptions::default())
        .unwrap_err();
    assert!(matches!(unknown, DomainError::NotFound("task", 999)));

    store
        .begin_execution(ExecutionStart {
            task_id: task.id,
            session_id: None,
            worktree_name: Some("worktree"),
            artifact_dir: &state.path().join("artifacts"),
            boot_id: "boot",
            claimed_runner_id: None,
        })
        .unwrap();
    let running = Executor::new(&database)
        .run(&mut store, task.id, &RunOptions::default())
        .unwrap_err();
    assert!(running.to_string().contains("already executing"));
}

fn cursor_settings_json(approval_mode: &str) -> String {
    serde_json::json!({
        "executable": "cursor-agent",
        "version": "2026.08.11-e8db854",
        "model": "composer-2.5",
        "approval_mode": approval_mode,
    })
    .to_string()
}

#[test]
fn cursor_settings_require_an_executable_approval_mode() {
    validate_agent_registration("cursor", &cursor_settings_json("unrestricted"), false).unwrap();
    validate_agent_registration("cursor", &cursor_settings_json("auto-review"), false).unwrap();

    let allowlist =
        validate_agent_registration("cursor", &cursor_settings_json("allowlist"), false)
            .unwrap_err()
            .to_string();
    assert!(
        allowlist.contains(r#"cursor approval_mode "allowlist""#),
        "{allowlist}"
    );
    assert!(allowlist.contains("still reporting success"), "{allowlist}");
    assert!(
        allowlist.contains(r#"register "unrestricted" or "auto-review""#),
        "{allowlist}"
    );

    let unknown_mode = validate_agent_registration("cursor", &cursor_settings_json("plan"), false)
        .unwrap_err()
        .to_string();
    assert!(
        unknown_mode.contains("not one of cursor-agent's run modes"),
        "{unknown_mode}"
    );

    let missing = validate_agent_registration(
        "cursor",
        r#"{"executable":"cursor-agent","model":"composer-2.5"}"#,
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(missing.contains("invalid cursor settings"), "{missing}");
    assert!(missing.contains("approval_mode"), "{missing}");

    let foreign = validate_agent_registration(
        "cursor",
        r#"{"executable":"cursor-agent","model":"composer-2.5","approval_mode":"auto-review","permission_mode":"acceptEdits"}"#,
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(
        foreign.contains("unknown field `permission_mode`"),
        "{foreign}"
    );

    let checkpoint =
        validate_agent_registration("cursor", &cursor_settings_json("auto-review"), true)
            .unwrap_err()
            .to_string();
    assert!(checkpoint.contains("review_prompt"), "{checkpoint}");
}

#[test]
fn cursor_registration_and_repair_report_approval_mode_failures_through_the_cli() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let project = successful_json(
        &database,
        &[
            "project",
            "register",
            "--name",
            "cursor",
            "--path",
            project_dir.path().to_str().unwrap(),
        ],
    );
    let project_id = project["id"].as_i64().unwrap().to_string();
    let register = |settings: &str| -> std::process::Output {
        mush_command(
            &database,
            &[
                "agent",
                "register",
                "--project",
                &project_id,
                "--name",
                "worker",
                "--harness",
                "cursor",
                "--model",
                "composer-2.5",
                "--settings",
                settings,
            ],
        )
    };

    let refused = register(&cursor_settings_json("allowlist"));
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains(r#"cursor approval_mode "allowlist""#)
    );

    // Settings written before the approval mode became required no longer
    // register; the operator repairs them explicitly rather than inheriting a
    // silent default.
    let stale = register(r#"{"executable":"cursor-agent","model":"composer-2.5"}"#);
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("approval_mode"));

    let agent = successful_json(
        &database,
        &[
            "agent",
            "register",
            "--project",
            &project_id,
            "--name",
            "worker",
            "--harness",
            "cursor",
            "--model",
            "composer-2.5",
            "--settings",
            &cursor_settings_json("unrestricted"),
        ],
    );
    let agent_id = agent["id"].as_i64().unwrap().to_string();

    let updated = successful_json(
        &database,
        &[
            "agent",
            "update",
            &agent_id,
            "--settings",
            &cursor_settings_json("auto-review"),
        ],
    );
    let stored: serde_json::Value =
        serde_json::from_str(updated["settings"].as_str().unwrap()).unwrap();
    assert_eq!(stored["approval_mode"], "auto-review");

    let refused_update = mush_command(
        &database,
        &[
            "agent",
            "update",
            &agent_id,
            "--settings",
            &cursor_settings_json("allowlist"),
        ],
    );
    assert_eq!(refused_update.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused_update.stderr).contains("allowlist"));
    let unchanged = successful_json(&database, &["agent", "update", &agent_id]);
    let unchanged: serde_json::Value =
        serde_json::from_str(unchanged["settings"].as_str().unwrap()).unwrap();
    assert_eq!(unchanged["approval_mode"], "auto-review");
}
