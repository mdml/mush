use mush::{CheckpointDecision, ExecutionStatus, Executor, Store, TaskKind, TaskStatus, tui};
use serde_json::Value;
use std::process::Command;
mod common;
use common::StoreTestExt;
#[cfg(unix)]
use common::{claude_settings, fake_claude, git_project, install_script};

#[cfg(unix)]
#[test]
fn claude_executor_persists_artifacts_and_runs_an_independent_checkpoint() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("claude");
    fake_claude(&executable, true);
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
            &claude_settings(&executable, None),
            false,
        )
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "reviewer",
            "claude-code",
            "claude-opus-5",
            &claude_settings(&executable, Some("Verify independently")),
            true,
        )
        .unwrap();
    let work = store
        .add_work_task_args(project.id, worker.id, "Implement real work", None)
        .unwrap();
    let completed = Executor::new(&database)
        .run(
            &mut store,
            work.id,
            &mush::executor::RunOptions {
                worktree: Some("mush-task-test"),
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(completed.kind, TaskKind::Work);
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(completed.execution_status, Some(ExecutionStatus::Succeeded));
    let checkpoint = store
        .create_checkpoint_args(work.id, "the declared criteria hold", None)
        .unwrap();
    assert_eq!(checkpoint.kind, TaskKind::Checkpoint);
    assert_eq!(checkpoint.subject_task_id, Some(work.id));
    let reviewed = Executor::new(&database)
        .run(
            &mut store,
            checkpoint.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(reviewed.status, TaskStatus::Pending);
    assert_eq!(reviewed.execution_status, Some(ExecutionStatus::Succeeded));
    assert!(
        reviewed
            .evidence
            .as_deref()
            .unwrap()
            .contains("fake executor result")
    );
    let continued = Executor::new(&database)
        .run(
            &mut store,
            checkpoint.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: Some("Here is the requested PR body"),
            },
        )
        .unwrap();
    assert_eq!(continued.execution_attempt, 2);
    assert_eq!(continued.session_id, reviewed.session_id);
    assert!(state.path().join("artifacts/task-1/prompt.md").is_file());
    assert!(
        state
            .path()
            .join("artifacts/task-2/stdout-1.jsonl")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn interrupted_execution_resumes_same_session_after_restart() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("claude");
    fake_claude(&executable, false);
    let (work_id, session_id);
    {
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
                &claude_settings(&executable, None),
                false,
            )
            .unwrap();
        store
            .register_agent_args(
                project.id,
                "reviewer",
                "claude-code",
                "claude-opus-5",
                &claude_settings(&executable, Some("Review")),
                true,
            )
            .unwrap();
        let work = store
            .add_work_task_args(project.id, worker.id, "Work", None)
            .unwrap();
        work_id = work.id;
        assert!(
            Executor::new(&database)
                .run(
                    &mut store,
                    work.id,
                    &mush::executor::RunOptions {
                        worktree: Some("resume-test"),
                        restart_session: false,
                        prompt_override: None
                    }
                )
                .is_err()
        );
        let interrupted = store.task(work.id).unwrap();
        assert_eq!(
            interrupted.execution_status,
            Some(ExecutionStatus::Interrupted)
        );
        session_id = interrupted.session_id.unwrap();
    }
    std::fs::create_dir_all(project_dir.path().join(".claude/worktrees/resume-test")).unwrap();
    fake_claude(&executable, true);
    let mut restarted = Store::open(&database).unwrap();
    let completed = Executor::new(&database)
        .run(
            &mut restarted,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(completed.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(completed.execution_attempt, 2);
    let checkpoint = restarted
        .create_checkpoint_args(work_id, "the declared criteria hold", None)
        .unwrap();
    assert_eq!(checkpoint.subject_task_id, Some(work_id));
    assert_eq!(restarted.tasks(None).unwrap().len(), 2);
}

#[cfg(unix)]
fn fake_cursor(path: &std::path::Path, version: &str, outcome: &str) {
    install_script(
        path,
        &format!(
            "#!/usr/bin/env bash\nif [[ ${{1:-}} == --version ]]; then echo '{version}'; exit 0; fi\nprompt=$(cat)\n{outcome}\n"
        ),
    );
}

#[cfg(unix)]
const CURSOR_SUCCESS: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"chat-1234","model":"Composer 2.5"}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"fake cursor result","session_id":"chat-1234","duration_ms":5,"usage":{"inputTokens":10,"outputTokens":2,"cacheReadTokens":0}}'"#;

#[cfg(unix)]
const CURSOR_ERROR_RESULT: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"chat-1234","model":"Composer 2.5"}'
printf '%s\n' '{"type":"result","subtype":"error","is_error":true,"result":"model refused","session_id":"chat-1234","duration_ms":5,"usage":{"inputTokens":10,"outputTokens":2,"cacheReadTokens":0}}'"#;

#[cfg(unix)]
const CURSOR_NO_RESULT: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"chat-1234","model":"Composer 2.5"}'"#;

#[cfg(unix)]
fn cursor_settings(executable: &std::path::Path, review_prompt: Option<&str>) -> String {
    cursor_settings_with_mode(executable, review_prompt, "unrestricted")
}

#[cfg(unix)]
fn cursor_settings_with_mode(
    executable: &std::path::Path,
    review_prompt: Option<&str>,
    approval_mode: &str,
) -> String {
    serde_json::json!({
        "executable": executable,
        "version": "2026.08.04-aaa8809",
        "model": "composer-2.5",
        "approval_mode": approval_mode,
        "review_prompt": review_prompt,
    })
    .to_string()
}

#[cfg(unix)]
fn cursor_fixture(
    state: &std::path::Path,
    project_dir: &std::path::Path,
) -> (Store, std::path::PathBuf, i64) {
    let database = state.join("mush.sqlite");
    let executable = state.join("cursor-agent");
    fake_cursor(&executable, "2026.08.04-aaa8809", CURSOR_SUCCESS);
    git_project(project_dir);
    let store = Store::open(&database).unwrap();
    let project = store.register_project("project", project_dir).unwrap();
    let worker = store
        .register_agent_args(
            project.id,
            "worker",
            "cursor",
            "composer-2.5",
            &cursor_settings(&executable, None),
            false,
        )
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "reviewer",
            "cursor",
            "composer-2.5",
            &cursor_settings(&executable, Some("Verify independently")),
            true,
        )
        .unwrap();
    let work = store
        .add_work_task_args(project.id, worker.id, "Implement cursor work", None)
        .unwrap();
    (store, executable, work.id)
}

#[cfg(unix)]
fn launch_arguments(state: &std::path::Path, task_id: i64, attempt: i64) -> Vec<String> {
    let launch: Value = serde_json::from_str(
        &std::fs::read_to_string(
            state.join(format!("artifacts/task-{task_id}/launch-{attempt}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    launch["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

#[cfg(unix)]
#[test]
fn cursor_executor_captures_session_owns_worktree_and_reviews_independently() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _executable, work_id) = cursor_fixture(state.path(), project_dir.path());
    let completed = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(completed.kind, TaskKind::Work);
    assert_eq!(completed.status, TaskStatus::Completed);
    let checkpoint = store
        .create_checkpoint_args(work_id, "the declared criteria hold", None)
        .unwrap();
    assert_eq!(checkpoint.kind, TaskKind::Checkpoint);
    assert_eq!(completed.execution_status, Some(ExecutionStatus::Succeeded));
    assert_eq!(completed.session_id.as_deref(), Some("chat-1234"));
    assert_eq!(completed.result.as_deref(), Some("fake cursor result"));
    assert!(
        project_dir
            .path()
            .join(format!(".mush/worktrees/mush-task-{work_id}"))
            .is_dir()
    );
    let first = launch_arguments(state.path(), work_id, 1);
    assert!(!first.contains(&"--resume".to_owned()));
    assert!(!first.contains(&"--worktree".to_owned()));
    let reviewed = Executor::new(&database)
        .run(
            &mut store,
            checkpoint.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(reviewed.status, TaskStatus::Pending);
    assert_eq!(reviewed.session_id.as_deref(), Some("chat-1234"));
    let evidence = reviewed.evidence.unwrap();
    assert!(evidence.contains("## Cursor Agent execution"));
    assert!(evidence.contains("fake cursor result"));
    let continued = Executor::new(&database)
        .run(
            &mut store,
            checkpoint.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: Some("Follow up"),
            },
        )
        .unwrap();
    assert_eq!(continued.execution_attempt, 2);
    let resumed = launch_arguments(state.path(), checkpoint.id, 2);
    assert!(resumed.contains(&"--resume".to_owned()));
    assert!(resumed.contains(&"chat-1234".to_owned()));
}

#[cfg(unix)]
fn cursor_first_launch_arguments(approval_mode: &str) -> Vec<String> {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("cursor-agent");
    fake_cursor(&executable, "2026.08.04-aaa8809", CURSOR_SUCCESS);
    git_project(project_dir.path());
    let mut store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent_args(
            project.id,
            "worker",
            "cursor",
            "composer-2.5",
            &cursor_settings_with_mode(&executable, None, approval_mode),
            false,
        )
        .unwrap();
    let work = store
        .add_work_task_args(project.id, worker.id, "Implement cursor work", None)
        .unwrap();
    Executor::new(&database)
        .run(&mut store, work.id, &mush::executor::RunOptions::default())
        .unwrap();
    launch_arguments(state.path(), work.id, 1)
}

#[cfg(unix)]
#[test]
fn cursor_approval_mode_chooses_the_vendor_flag_and_trust_stays_mush_owned() {
    let unrestricted = cursor_first_launch_arguments("unrestricted");
    assert_eq!(
        unrestricted,
        [
            "-p",
            "--output-format",
            "stream-json",
            "--model",
            "composer-2.5",
            "--force",
            "--trust",
        ]
    );

    let auto_review = cursor_first_launch_arguments("auto-review");
    assert_eq!(
        auto_review,
        [
            "-p",
            "--output-format",
            "stream-json",
            "--model",
            "composer-2.5",
            "--auto-review",
            "--trust",
        ]
    );
    assert!(
        !auto_review.contains(&"--force".to_owned()),
        "an auto-review agent must not carry the old unconditional autonomy flag"
    );
}

#[cfg(unix)]
#[test]
fn cursor_auto_review_resume_keeps_the_session_and_the_registered_mode() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("cursor-agent");
    fake_cursor(&executable, "2026.08.04-aaa8809", CURSOR_ERROR_RESULT);
    git_project(project_dir.path());
    let mut store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent_args(
            project.id,
            "worker",
            "cursor",
            "composer-2.5",
            &cursor_settings_with_mode(&executable, None, "auto-review"),
            false,
        )
        .unwrap();
    let work = store
        .add_work_task_args(project.id, worker.id, "Implement cursor work", None)
        .unwrap();
    Executor::new(&database)
        .run(&mut store, work.id, &mush::executor::RunOptions::default())
        .unwrap_err();
    assert_eq!(
        store.task(work.id).unwrap().session_id.as_deref(),
        Some("chat-1234")
    );

    fake_cursor(&executable, "2026.08.04-aaa8809", CURSOR_SUCCESS);
    let completed = Executor::new(&database)
        .run(&mut store, work.id, &mush::executor::RunOptions::default())
        .unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(completed.session_id.as_deref(), Some("chat-1234"));
    let resumed = launch_arguments(state.path(), work.id, 2);
    assert_eq!(
        resumed,
        [
            "-p",
            "--output-format",
            "stream-json",
            "--model",
            "composer-2.5",
            "--auto-review",
            "--trust",
            "--resume",
            "chat-1234",
        ]
    );
}

#[cfg(unix)]
#[test]
fn cursor_large_prompt_does_not_deadlock_against_early_stream_output() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("cursor-agent");
    // Flood stdout with far more than the OS pipe buffer before reading stdin,
    // reproducing a harness that streams events while the prompt is still being
    // written.
    install_script(
        &executable,
        concat!(
            "#!/usr/bin/env bash\n",
            "if [[ ${1:-} == --version ]]; then echo '2026.08.04-aaa8809'; exit 0; fi\n",
            "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"chat-1234\"}'\n",
            "for _ in $(seq 1 2000); do printf '%s\\n' '{\"type\":\"thinking\",\"subtype\":\"delta\",\"text\":\"pad pad pad pad pad pad pad pad pad pad\",\"session_id\":\"chat-1234\"}'; done\n",
            "prompt=$(cat)\n",
            "printf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"fake cursor result\",\"session_id\":\"chat-1234\",\"duration_ms\":5,\"usage\":{\"inputTokens\":1,\"outputTokens\":1,\"cacheReadTokens\":0}}'\n",
        ),
    );
    git_project(project_dir.path());
    let store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent_args(
            project.id,
            "worker",
            "cursor",
            "composer-2.5",
            &cursor_settings(&executable, None),
            false,
        )
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "reviewer",
            "cursor",
            "composer-2.5",
            &cursor_settings(&executable, Some("Review")),
            true,
        )
        .unwrap();
    let work = store
        .add_work_task_args(project.id, worker.id, &"x".repeat(200_000), None)
        .unwrap();
    let work_id = work.id;
    drop(store);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut store = Store::open(&database).unwrap();
        let outcome = Executor::new(&database).run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        );
        sender.send(outcome.map(|task| task.kind)).unwrap();
    });
    let outcome = receiver
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("cursor execution deadlocked on a large prompt");
    assert_eq!(outcome.unwrap(), TaskKind::Work);
}

#[cfg(unix)]
#[test]
fn cursor_error_result_interrupts_then_resumes_the_captured_chat() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, executable, work_id) = cursor_fixture(state.path(), project_dir.path());
    fake_cursor(&executable, "2026.08.04-aaa8809", CURSOR_ERROR_RESULT);
    let error = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("error result"));
    let interrupted = store.task(work_id).unwrap();
    assert_eq!(
        interrupted.execution_status,
        Some(ExecutionStatus::Interrupted)
    );
    assert_eq!(interrupted.session_id.as_deref(), Some("chat-1234"));
    fake_cursor(&executable, "2026.08.04-aaa8809", CURSOR_SUCCESS);
    let completed = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(completed.execution_attempt, 2);
    assert_eq!(completed.session_id.as_deref(), Some("chat-1234"));
    let resumed = launch_arguments(state.path(), work_id, 2);
    assert!(resumed.contains(&"--resume".to_owned()));
    assert!(resumed.contains(&"chat-1234".to_owned()));
}

#[cfg(unix)]
#[test]
fn cursor_missing_result_event_fails_and_interrupts() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, executable, work_id) = cursor_fixture(state.path(), project_dir.path());
    fake_cursor(&executable, "2026.08.04-aaa8809", CURSOR_NO_RESULT);
    let error = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("no result event"));
    assert_eq!(
        store.task(work_id).unwrap().execution_status,
        Some(ExecutionStatus::Interrupted)
    );
}

#[cfg(unix)]
#[test]
fn cursor_version_mismatch_refuses_to_launch() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, executable, work_id) = cursor_fixture(state.path(), project_dir.path());
    fake_cursor(&executable, "2026.09.01-0000000", CURSOR_SUCCESS);
    let error = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("Cursor Agent version mismatch"));
    assert_eq!(store.task(work_id).unwrap().execution_status, None);
    assert!(
        !project_dir
            .path()
            .join(format!(".mush/worktrees/mush-task-{work_id}"))
            .exists()
    );
}

/// `codex --version` prints `codex-cli <version>`, so the version is not the
/// first token of the line as it is for Claude Code.
#[cfg(unix)]
fn fake_codex(path: &std::path::Path, version: &str, outcome: &str) {
    install_script(
        path,
        &format!(
            "#!/usr/bin/env bash\nif [[ ${{1:-}} == --version ]]; then echo 'codex-cli {version}'; exit 0; fi\nprompt=$(cat)\n{outcome}\n"
        ),
    );
}

/// The event vocabulary observed from codex 0.146.1 under `codex exec --json`:
/// the thread id arrives on `thread.started`, intermediate narration and the
/// final answer both arrive as completed `agent_message` items, and the run ends
/// with `turn.completed`.
#[cfg(unix)]
const CODEX_SUCCESS: &str = r#"printf '%s\n' '{"type":"thread.started","thread_id":"019ffcbc-77e3-7611-9ac6-04b1b54e43b9"}'
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"I will start now."}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_1","type":"file_change","changes":[{"path":"b.txt","kind":"add"}]}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"fake codex result"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":2}}'"#;

#[cfg(unix)]
const CODEX_TURN_FAILED: &str = r#"printf '%s\n' '{"type":"thread.started","thread_id":"019ffcbc-77e3-7611-9ac6-04b1b54e43b9"}'
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"turn.failed","error":{"message":"model refused"}}'"#;

#[cfg(unix)]
fn codex_settings(executable: &std::path::Path, review_prompt: Option<&str>) -> String {
    serde_json::json!({
        "executable": executable,
        "version": "0.146.1",
        "model": "gpt-5.1-codex-max",
        "effort": "medium",
        "sandbox": "workspace-write",
        "approval_policy": "never",
        "review_prompt": review_prompt,
    })
    .to_string()
}

#[cfg(unix)]
fn codex_fixture(
    state: &std::path::Path,
    project_dir: &std::path::Path,
) -> (Store, std::path::PathBuf, i64) {
    let database = state.join("mush.sqlite");
    let executable = state.join("codex");
    fake_codex(&executable, "0.146.1", CODEX_SUCCESS);
    git_project(project_dir);
    let store = Store::open(&database).unwrap();
    let project = store.register_project("project", project_dir).unwrap();
    let worker = store
        .register_agent_args(
            project.id,
            "worker",
            "codex",
            "gpt-5.1-codex-max",
            &codex_settings(&executable, None),
            false,
        )
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "reviewer",
            "codex",
            "gpt-5.1-codex-max",
            &codex_settings(&executable, Some("Verify independently")),
            true,
        )
        .unwrap();
    let work = store
        .add_work_task_args(project.id, worker.id, "Implement codex work", None)
        .unwrap();
    (store, executable, work.id)
}

#[cfg(unix)]
#[test]
fn codex_executor_captures_thread_owns_worktree_and_reviews_independently() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _executable, work_id) = codex_fixture(state.path(), project_dir.path());
    let completed = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(completed.execution_status, Some(ExecutionStatus::Succeeded));
    assert_eq!(
        completed.session_id.as_deref(),
        Some("019ffcbc-77e3-7611-9ac6-04b1b54e43b9")
    );
    // The last agent message is the result, not the first narration line.
    assert_eq!(completed.result.as_deref(), Some("fake codex result"));
    assert!(
        project_dir
            .path()
            .join(format!(".mush/worktrees/mush-task-{work_id}"))
            .is_dir()
    );
    let first = launch_arguments(state.path(), work_id, 1);
    assert_eq!(first.first().map(String::as_str), Some("exec"));
    assert!(!first.contains(&"resume".to_owned()));
    assert!(first.contains(&"--json".to_owned()));
    assert!(first.contains(&"sandbox_mode=\"workspace-write\"".to_owned()));
    assert!(first.contains(&"approval_policy=\"never\"".to_owned()));
    assert!(first.contains(&"model_reasoning_effort=\"medium\"".to_owned()));
    assert_eq!(first.last().map(String::as_str), Some("-"));
    let checkpoint = store
        .create_checkpoint_args(work_id, "the declared criteria hold", None)
        .unwrap();
    let reviewed = Executor::new(&database)
        .run(
            &mut store,
            checkpoint.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(reviewed.status, TaskStatus::Pending);
    let evidence = reviewed.evidence.unwrap();
    assert!(evidence.contains("## Codex execution"));
    assert!(evidence.contains("fake codex result"));
    let continued = Executor::new(&database)
        .run(
            &mut store,
            checkpoint.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: Some("Follow up"),
            },
        )
        .unwrap();
    assert_eq!(continued.execution_attempt, 2);
    let resumed = launch_arguments(state.path(), checkpoint.id, 2);
    assert_eq!(resumed.first().map(String::as_str), Some("exec"));
    assert_eq!(resumed.get(1).map(String::as_str), Some("resume"));
    assert!(resumed.contains(&"019ffcbc-77e3-7611-9ac6-04b1b54e43b9".to_owned()));
    assert_eq!(resumed.last().map(String::as_str), Some("-"));
}

#[cfg(unix)]
#[test]
fn codex_launch_disables_native_subagents_unless_opted_in() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, executable, work_id) = codex_fixture(state.path(), project_dir.path());
    Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    let default = launch_arguments(state.path(), work_id, 1);
    assert!(default.contains(&"--disable".to_owned()));
    assert!(default.contains(&"multi_agent".to_owned()));
    let mut opted: serde_json::Value =
        serde_json::from_str(&codex_settings(&executable, None)).unwrap();
    opted["native_subagents"] = serde_json::Value::Bool(true);
    let project_id = store.task(work_id).unwrap().project_id;
    let opted_agent = store
        .register_agent_args(
            project_id,
            "worker-with-subagents",
            "codex",
            "gpt-5.1-codex-max",
            &opted.to_string(),
            false,
        )
        .unwrap();
    let opted_work = store
        .add_work_task_args(project_id, opted_agent.id, "Delegating work", None)
        .unwrap();
    Executor::new(&database)
        .run(
            &mut store,
            opted_work.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert!(!launch_arguments(state.path(), opted_work.id, 1).contains(&"multi_agent".to_owned()));
}

#[cfg(unix)]
#[test]
fn codex_turn_failure_interrupts_the_task() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, executable, work_id) = codex_fixture(state.path(), project_dir.path());
    fake_codex(&executable, "0.146.1", CODEX_TURN_FAILED);
    let error = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Codex turn failed: model refused")
    );
    let interrupted = store.task(work_id).unwrap();
    assert_eq!(
        interrupted.execution_status,
        Some(ExecutionStatus::Interrupted)
    );
    // The thread id is still captured, so the interrupted run can be resumed.
    assert_eq!(
        interrupted.session_id.as_deref(),
        Some("019ffcbc-77e3-7611-9ac6-04b1b54e43b9")
    );
}

/// An agent that pins no version keeps working across a harness upgrade, and
/// each launch records the version that actually ran so the execution record
/// stays exact even though the gate no longer is.
#[cfg(unix)]
#[test]
fn unpinned_agent_survives_a_harness_upgrade_and_records_the_observed_version() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, executable, _work_id) = codex_fixture(state.path(), project_dir.path());
    let mut unpinned: serde_json::Value =
        serde_json::from_str(&codex_settings(&executable, None)).unwrap();
    unpinned.as_object_mut().unwrap().remove("version");
    let project_id = store.task(1).unwrap().project_id;
    let agent = store
        .register_agent_args(
            project_id,
            "unpinned-worker",
            "codex",
            "gpt-5.1-codex-max",
            &unpinned.to_string(),
            false,
        )
        .unwrap();
    let before = store
        .add_work_task_args(project_id, agent.id, "Work before the upgrade", None)
        .unwrap();
    let completed = Executor::new(&database)
        .run(
            &mut store,
            before.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(completed.execution_status, Some(ExecutionStatus::Succeeded));
    assert!(
        completed
            .evidence
            .unwrap()
            .contains("- version: `codex-cli 0.146.1`")
    );
    fake_codex(&executable, "0.147.0", CODEX_SUCCESS);
    let after = store
        .add_work_task_args(project_id, agent.id, "Work after the upgrade", None)
        .unwrap();
    let upgraded = Executor::new(&database)
        .run(
            &mut store,
            after.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(upgraded.execution_status, Some(ExecutionStatus::Succeeded));
    assert!(
        upgraded
            .evidence
            .unwrap()
            .contains("- version: `codex-cli 0.147.0`")
    );
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(
            state
                .path()
                .join(format!("artifacts/task-{}/launch-1.json", after.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["version"], "codex-cli 0.147.0");
}

/// A harness that cannot report a version at all is a configuration failure
/// whether or not the agent pins one.
#[cfg(unix)]
#[test]
fn unpinned_agent_still_refuses_an_executable_that_cannot_report_a_version() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, executable, work_id) = codex_fixture(state.path(), project_dir.path());
    let mut unpinned: serde_json::Value =
        serde_json::from_str(&codex_settings(&executable, None)).unwrap();
    unpinned.as_object_mut().unwrap().remove("version");
    store
        .update_agent_settings(1, &unpinned.to_string())
        .unwrap();
    install_script(&executable, "#!/usr/bin/env bash\nexit 3\n");
    let error = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("cannot inspect Codex version"));
    assert_eq!(store.task(work_id).unwrap().execution_status, None);
}

#[cfg(unix)]
#[test]
fn codex_version_mismatch_refuses_to_launch() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, executable, work_id) = codex_fixture(state.path(), project_dir.path());
    fake_codex(&executable, "0.147.0", CODEX_SUCCESS);
    let error = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("Codex version mismatch"));
    assert_eq!(store.task(work_id).unwrap().execution_status, None);
}

#[cfg(unix)]
#[test]
fn executable_checkpoint_agent_requires_a_review_prompt_on_every_harness() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let store = Store::open(&state.path().join("mush.sqlite")).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let error = store
        .register_agent_args(
            project.id,
            "reviewer",
            "codex",
            "gpt-5.1-codex-max",
            &codex_settings(&state.path().join("codex"), None),
            true,
        )
        .unwrap_err();
    assert!(error.to_string().contains("requires a non-empty"));
}

#[test]
fn unknown_harness_is_rejected_by_the_executor() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let mut store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let agent = store
        .register_agent_args(project.id, "manual", "manual", "human", "{}", false)
        .unwrap();
    let work = store
        .add_work_task_args(project.id, agent.id, "Work", None)
        .unwrap();
    let error = Executor::new(&database)
        .run(
            &mut store,
            work.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("unsupported harness: manual"));
}

fn cli(database: &std::path::Path, arguments: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_mush"))
        .arg("--database")
        .arg(database)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn headless_cli_and_tui_snapshot_execute_the_acceptance_scenario_across_processes() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let project_path = project_dir.path().to_str().unwrap();

    let project: Value = serde_json::from_str(&cli(
        &database,
        &[
            "--json",
            "project",
            "register",
            "--name",
            "demo",
            "--path",
            project_path,
        ],
    ))
    .unwrap();
    let project_id = project["id"].as_i64().unwrap().to_string();
    let agent: Value = serde_json::from_str(&cli(
        &database,
        &[
            "--json",
            "agent",
            "register",
            "--project",
            &project_id,
            "--name",
            "exact-manual",
            "--harness",
            "manual",
            "--model",
            "human",
            "--settings",
            "{\"temperature\":0}",
            "--checkpoint",
        ],
    ))
    .unwrap();
    let agent_id = agent["id"].as_i64().unwrap().to_string();
    let work: Value = serde_json::from_str(&cli(
        &database,
        &[
            "--json",
            "task",
            "add",
            "--project",
            &project_id,
            "--agent",
            &agent_id,
            "--description",
            "Ship one manual attempt",
        ],
    ))
    .unwrap();
    let work_id = work["id"].as_i64().unwrap();
    let work_id_text = work_id.to_string();
    assert!(cli(&database, &["tui", "--snapshot"]).contains("[work / pending]"));

    let subtask: Value = serde_json::from_str(&cli(
        &database,
        &[
            "--json",
            "task",
            "add",
            "--project",
            &project_id,
            "--agent",
            &agent_id,
            "--description",
            "Delegated slice",
            "--parent",
            &work_id_text,
        ],
    ))
    .unwrap();
    assert_eq!(subtask["parent_task_id"], work_id);
    let subtask_id = subtask["id"].as_i64().unwrap().to_string();
    assert!(cli(&database, &["tui", "--snapshot"]).contains(&format!("(parent #{work_id})")));

    // The parent cannot complete before its delegated slice is integrated.
    cli(
        &database,
        &[
            "task",
            "complete",
            &subtask_id,
            "--result",
            "Slice complete",
        ],
    );
    let completed: Value = serde_json::from_str(&cli(
        &database,
        &[
            "--json",
            "task",
            "complete",
            &work_id_text,
            "--result",
            "Candidate complete",
            "--evidence",
            "## Checks\n\n- manual result supplied",
        ],
    ))
    .unwrap();
    assert_eq!(completed["id"], work_id);
    assert_eq!(completed["status"], "completed");
    let unreviewed = cli(&database, &["tui", "--snapshot"]);
    assert!(unreviewed.contains("no checkpoint yet"));
    assert!(!unreviewed.contains("[checkpoint / pending]"));

    let checkpoint: Value = serde_json::from_str(&cli(
        &database,
        &[
            "--json",
            "checkpoint",
            "create",
            &work_id_text,
            "--criteria",
            "## Criteria\n\n- the manual checks are recorded",
        ],
    ))
    .unwrap();
    let checkpoint_id = checkpoint["id"].as_i64().unwrap().to_string();
    let review = cli(&database, &["tui", "--snapshot"]);
    assert!(review.contains("[checkpoint / pending]"));
    assert!(review.contains("## Checks"));
    assert!(review.contains("the manual checks are recorded"));

    let outcome: Value = serde_json::from_str(&cli(
        &database,
        &[
            "--json",
            "checkpoint",
            "decide",
            &checkpoint_id,
            "--decision",
            "not_met",
            "--evidence",
            "## Review\n\nRevise the candidate.",
        ],
    ))
    .unwrap();
    assert_eq!(outcome["checkpoint"]["decision"], "not_met");
    assert!(
        outcome["materialized"].as_array().unwrap().is_empty(),
        "outside a declared loop, not_met adjudicates without creating work"
    );
    assert!(cli(&database, &["tui", "--snapshot"]).contains("decision: not_met"));
}

#[test]
fn manual_control_loop_persists_and_revision_creates_one_linked_attempt() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (work_id, checkpoint_id);

    {
        let mut store = Store::open(&database).unwrap();
        let project = store.register_project("mush", project_dir.path()).unwrap();
        let agent = store
            .register_agent_args(
                project.id,
                "manual",
                "manual",
                "human",
                "{\"mode\":\"exact\"}",
                true,
            )
            .unwrap();
        let work = store
            .add_work_task_args(project.id, agent.id, "Implement the first slice", None)
            .unwrap();
        work_id = work.id;

        let before = tui::snapshot(&store, Some(project.id)).unwrap();
        assert!(before.contains("[work / pending] Implement the first slice"));

        let completed = store
            .complete_work(
                work.id,
                "The candidate is ready",
                Some("## Verification\n\n- `cargo test` passed"),
            )
            .unwrap();
        assert_eq!(completed.status, TaskStatus::Completed);
        assert_eq!(store.tasks(Some(project.id)).unwrap().len(), 1);

        let checkpoint = store
            .create_checkpoint_args(work.id, "the declared criteria hold", None)
            .unwrap();
        checkpoint_id = checkpoint.id;
        assert_eq!(checkpoint.kind, TaskKind::Checkpoint);
        assert_eq!(checkpoint.subject_task_id, Some(work.id));

        let retry = store.complete_work(work.id, "ignored retry", None).unwrap();
        assert_eq!(retry.id, work.id);
        let repeated = store
            .create_checkpoint_args(work.id, "the declared criteria hold", None)
            .unwrap();
        assert_eq!(repeated.id, checkpoint.id);
        assert_eq!(store.tasks(Some(project.id)).unwrap().len(), 2);

        let review = tui::snapshot(&store, Some(project.id)).unwrap();
        assert!(review.contains("evidence (Markdown):"));
        assert!(review.contains("- `cargo test` passed"));
    }

    {
        let mut restarted = Store::open(&database).unwrap();
        let outcome = restarted
            .decide_checkpoint(
                checkpoint_id,
                CheckpointDecision::NotMet,
                "## Decision\n\nPlease address the review.",
            )
            .unwrap();
        assert_eq!(
            outcome.checkpoint.decision,
            Some(CheckpointDecision::NotMet)
        );
        assert!(
            outcome.materialized.is_empty(),
            "outside a declared loop, not_met adjudicates only; fresh judgment owns any revision"
        );
        assert_eq!(restarted.tasks(None).unwrap().len(), 2);
    }

    let restarted_again = Store::open(&database).unwrap();
    let tasks = restarted_again.tasks(None).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[1].decision, Some(CheckpointDecision::NotMet));
    let _ = work_id;
}

#[test]
fn checkpoint_accept_and_block_do_not_create_follow_up_work() {
    for decision in [CheckpointDecision::Met, CheckpointDecision::Blocked] {
        let state = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let database = state.path().join("mush.sqlite");
        let mut store = Store::open(&database).unwrap();
        let project = store
            .register_project("project", project_dir.path())
            .unwrap();
        let agent = store
            .register_agent_args(project.id, "reviewer", "manual", "human", "{}", true)
            .unwrap();
        let work = store
            .add_work_task_args(project.id, agent.id, "Work", None)
            .unwrap();
        store.complete_work(work.id, "Done", None).unwrap();
        let checkpoint = store
            .create_checkpoint_args(work.id, "the declared criteria hold", None)
            .unwrap();
        assert!(
            store
                .decide_checkpoint(checkpoint.id, decision, "Reviewed")
                .unwrap()
                .materialized
                .is_empty()
        );
        assert_eq!(store.tasks(None).unwrap().len(), 2);
    }
}

#[test]
fn unstarted_revision_can_receive_checkpoint_delta_and_reuse_worktree() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let mut store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let agent = store
        .register_agent_args(project.id, "reviewer", "manual", "human", "{}", true)
        .unwrap();
    let report = store
        .declare_loop(mush::store::LoopDeclaration {
            project_id: project.id,
            stages: &[mush::store::StageSpec {
                agent_id: agent.id,
                description: "Original",
            }],
            criteria: "the declared criteria hold",
            adjudicator_agent_id: None,
            max_attempts: 2,
            reuse_worktree: false,
        })
        .unwrap();
    let work = report.attempts[0].stage_task_ids[0];
    let checkpoint = report.attempts[0].checkpoint_task_id;
    store.complete_work(work, "Done", None).unwrap();
    let outcome = store
        .decide_checkpoint(checkpoint, CheckpointDecision::NotMet, "Delta")
        .unwrap();
    let revision = outcome.materialized[0].clone();
    assert!(revision.description.contains("Original"));
    assert!(revision.description.contains("not_met"));
    assert!(revision.description.contains("Delta"));
    let prepared = store
        .prepare_revision(revision.id, "Fix only the delta", "existing-worktree")
        .unwrap();
    assert_eq!(prepared.description, "Fix only the delta");
    assert_eq!(prepared.worktree_name.as_deref(), Some("existing-worktree"));
    assert_eq!(prepared.previous_task_id, Some(work));
}

fn delegation_fixture(database: &std::path::Path) -> (Store, tempfile::TempDir, i64, i64) {
    let project_dir = tempfile::tempdir().unwrap();
    let store = Store::open(database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent_args(project.id, "worker", "manual", "human", "{}", false)
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "reviewer",
            "manual",
            "human",
            "{\"review\":true}",
            true,
        )
        .unwrap();
    (store, project_dir, project.id, worker.id)
}

#[test]
fn delegation_is_fenced_to_one_level_of_work_tasks_in_the_domain() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
    let coordinator = store
        .add_work_task_args(project_id, worker_id, "Coordinate", None)
        .unwrap();
    let subtask = store
        .add_work_task_args(project_id, worker_id, "Delegated", Some(coordinator.id))
        .unwrap();
    assert_eq!(subtask.parent_task_id, Some(coordinator.id));

    let too_deep = store
        .add_work_task_args(project_id, worker_id, "Too deep", Some(subtask.id))
        .unwrap_err();
    assert!(too_deep.to_string().contains(&format!(
        "task {} already has a parent; delegation is bounded to one level",
        subtask.id
    )));

    // A subtask naming its own parent creates a sibling; depth still holds.
    let sibling = store
        .add_work_task_args(project_id, worker_id, "Sibling", Some(coordinator.id))
        .unwrap();
    assert_eq!(sibling.parent_task_id, Some(coordinator.id));

    store.complete_work(subtask.id, "Done", None).unwrap();
    store.complete_work(sibling.id, "Done", None).unwrap();
    store.complete_work(coordinator.id, "Done", None).unwrap();
    let checkpoint = store
        .create_checkpoint_args(coordinator.id, "the declared criteria hold", None)
        .unwrap();
    let checkpoint_parent = store
        .add_work_task_args(project_id, worker_id, "Reviewer work", Some(checkpoint.id))
        .unwrap_err();
    assert!(checkpoint_parent.to_string().contains("is a checkpoint"));
}

#[test]
fn delegation_fence_is_guaranteed_in_the_schema() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
    let coordinator = store
        .add_work_task_args(project_id, worker_id, "Coordinate", None)
        .unwrap();
    let subtask = store
        .add_work_task_args(project_id, worker_id, "Delegated", Some(coordinator.id))
        .unwrap();

    // Bypass the domain layer entirely; the schema trigger must still hold.
    // Each violation is checked while only one trigger applies, because the
    // firing order of multiple triggers on one statement is unspecified.
    let raw = rusqlite::Connection::open(&database).unwrap();
    let insert = "INSERT INTO tasks(project_id,agent_id,kind,status,description,parent_task_id) VALUES(?1,?2,?3,'pending','bypass',?4)";
    let too_deep = raw
        .execute(
            insert,
            rusqlite::params![project_id, worker_id, "work", subtask.id],
        )
        .unwrap_err();
    assert!(
        too_deep
            .to_string()
            .contains("delegation is bounded to one level")
    );
    let update = raw
        .execute(
            "UPDATE tasks SET parent_task_id=?2 WHERE id=?1",
            rusqlite::params![coordinator.id, subtask.id],
        )
        .unwrap_err();
    assert!(
        update
            .to_string()
            .contains("delegation is bounded to one level")
    );

    store.complete_work(subtask.id, "Done", None).unwrap();
    store.complete_work(coordinator.id, "Done", None).unwrap();
    let checkpoint = store
        .create_checkpoint_args(coordinator.id, "the declared criteria hold", None)
        .unwrap();
    drop(store);
    let checkpoint_parent = raw
        .execute(
            insert,
            rusqlite::params![project_id, worker_id, "work", checkpoint.id],
        )
        .unwrap_err();
    assert!(
        checkpoint_parent
            .to_string()
            .contains("a checkpoint cannot be a parent")
    );
}

#[test]
fn a_subtask_checkpoint_adjudicates_without_creating_follow_up_work() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
    let coordinator = store
        .add_work_task_args(project_id, worker_id, "Coordinate", None)
        .unwrap();
    let subtask = store
        .add_work_task_args(project_id, worker_id, "Delegated", Some(coordinator.id))
        .unwrap();
    store.complete_work(subtask.id, "Done", None).unwrap();
    let checkpoint = store
        .create_checkpoint_args(subtask.id, "the declared criteria hold", None)
        .unwrap();
    let outcome = store
        .decide_checkpoint(
            checkpoint.id,
            CheckpointDecision::NotMet,
            "## Review\n\nMissing tests.",
        )
        .unwrap();
    // The checkpoint owns adjudication only: outside a declared loop, not_met
    // materializes nothing, and the checkpoint keeps its named subject.
    assert!(outcome.materialized.is_empty());
    assert_eq!(
        store.task(checkpoint.id).unwrap().subject_task_id,
        Some(subtask.id)
    );
    assert_eq!(
        store.task(checkpoint.id).unwrap().evidence.as_deref(),
        Some("## Review\n\nMissing tests.")
    );
    let _ = coordinator;
}

#[test]
fn completing_work_no_longer_creates_a_checkpoint_until_asked() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
    let work = store
        .add_work_task_args(project_id, worker_id, "Work", None)
        .unwrap();
    store.complete_work(work.id, "Done", None).unwrap();
    assert_eq!(store.tasks(None).unwrap().len(), 1);
    let unreviewed = tui::snapshot(&store, None).unwrap();
    assert!(unreviewed.contains("no checkpoint yet"));

    let checkpoint = store
        .create_checkpoint_args(work.id, "the declared criteria hold", None)
        .unwrap();
    assert_eq!(checkpoint.subject_task_id, Some(work.id));
    let reviewed = tui::snapshot(&store, None).unwrap();
    assert!(!reviewed.contains("no checkpoint yet"));
}

#[cfg(unix)]
#[test]
fn delegated_task_runs_with_its_own_identity_in_the_environment() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("claude");
    // The fake harness echoes MUSH_TASK_ID back as its result.
    install_script(
        &executable,
        concat!(
            "#!/usr/bin/env bash\n",
            "if [[ ${1:-} == --version ]]; then echo '2.1.222 (Claude Code)'; exit 0; fi\n",
            "prompt=$(cat)\n",
            "printf '%s\\n' '{\"type\":\"result\",\"result\":\"ran task '\"$MUSH_TASK_ID\"'\"}'\n",
        ),
    );
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
            &claude_settings(&executable, None),
            false,
        )
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "reviewer",
            "claude-code",
            "claude-opus-5",
            &claude_settings(&executable, Some("Review")),
            true,
        )
        .unwrap();
    let coordinator = store
        .add_work_task_args(project.id, worker.id, "Coordinate", None)
        .unwrap();
    let subtask = store
        .add_work_task_args(project.id, worker.id, "Delegated", Some(coordinator.id))
        .unwrap();
    let completed = Executor::new(&database)
        .run(
            &mut store,
            subtask.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(completed.parent_task_id, Some(coordinator.id));
    assert_eq!(
        completed.result.as_deref(),
        Some(format!("ran task {}", subtask.id).as_str())
    );
}

#[test]
fn checkpoint_awaiting_its_subject_waits_then_syncs_evidence() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
    let work = store
        .add_work_task_args(project_id, worker_id, "Work", None)
        .unwrap();

    let awaiting = store
        .create_checkpoint_args(work.id, "the declared criteria hold", None)
        .unwrap();
    assert_eq!(awaiting.subject_task_id, Some(work.id));
    assert_eq!(
        awaiting.evidence.as_deref(),
        Some("Awaiting subject completion; evidence pending.")
    );
    assert_eq!(
        store
            .create_checkpoint_args(work.id, "the declared criteria hold", None)
            .unwrap()
            .id,
        awaiting.id
    );

    let visible = tui::snapshot(&store, None).unwrap();
    assert!(visible.contains(&format!("awaiting subject: #{} not completed", work.id)));

    let premature_decision = store
        .decide_checkpoint(awaiting.id, CheckpointDecision::Met, "Looks done")
        .unwrap_err();
    assert!(premature_decision.to_string().contains(&format!(
        "checkpoint {} is awaiting its subject: task {} is not completed",
        awaiting.id, work.id
    )));
    let premature_run = Executor::new(&database)
        .run(
            &mut store,
            awaiting.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(
        premature_run
            .to_string()
            .contains("is awaiting its subject")
    );
    assert_eq!(store.task(awaiting.id).unwrap().execution_status, None);

    store
        .complete_work(work.id, "Done", Some("## Verification\n\nreal evidence"))
        .unwrap();
    let synced = store.sync_checkpoint_with_subject(awaiting.id).unwrap();
    assert_eq!(
        synced.evidence.as_deref(),
        Some("## Verification\n\nreal evidence")
    );
    assert_eq!(
        store
            .sync_checkpoint_with_subject(awaiting.id)
            .unwrap()
            .evidence,
        synced.evidence
    );
    assert!(
        !tui::snapshot(&store, None)
            .unwrap()
            .contains("awaiting subject:")
    );
    assert!(
        store
            .decide_checkpoint(awaiting.id, CheckpointDecision::Met, "Reviewed")
            .unwrap()
            .materialized
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn checkpoint_awaiting_its_subject_runs_after_completion_with_synced_evidence() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("claude");
    fake_claude(&executable, true);
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
            &claude_settings(&executable, None),
            false,
        )
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "reviewer",
            "claude-code",
            "claude-opus-5",
            &claude_settings(&executable, Some("Review")),
            true,
        )
        .unwrap();
    let work = store
        .add_work_task_args(project.id, worker.id, "Work", None)
        .unwrap();
    let awaiting = store
        .create_checkpoint_args(work.id, "the declared criteria hold", None)
        .unwrap();
    assert!(
        Executor::new(&database)
            .run(
                &mut store,
                awaiting.id,
                &mush::executor::RunOptions {
                    worktree: None,
                    restart_session: false,
                    prompt_override: None
                }
            )
            .unwrap_err()
            .to_string()
            .contains("is awaiting its subject")
    );
    Executor::new(&database)
        .run(
            &mut store,
            work.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    let reviewed = Executor::new(&database)
        .run(
            &mut store,
            awaiting.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(reviewed.execution_status, Some(ExecutionStatus::Succeeded));
    assert!(
        reviewed
            .evidence
            .as_deref()
            .unwrap()
            .contains("fake executor result")
    );
}

#[cfg(unix)]
#[test]
fn claude_launch_disallows_native_subagents_unless_opted_in() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("claude");
    fake_claude(&executable, true);
    let mut store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let fenced = store
        .register_agent_args(
            project.id,
            "fenced",
            "claude-code",
            "claude-opus-5",
            &claude_settings(&executable, None),
            false,
        )
        .unwrap();
    let mut opted_settings: Value =
        serde_json::from_str(&claude_settings(&executable, None)).unwrap();
    opted_settings["native_subagents"] = Value::Bool(true);
    let opted = store
        .register_agent_args(
            project.id,
            "opted",
            "claude-code",
            "claude-opus-5",
            &opted_settings.to_string(),
            false,
        )
        .unwrap();
    store
        .register_agent_args(
            project.id,
            "reviewer",
            "claude-code",
            "claude-opus-5",
            &claude_settings(&executable, Some("Review")),
            true,
        )
        .unwrap();

    let fenced_work = store
        .add_work_task_args(project.id, fenced.id, "Fenced", None)
        .unwrap();
    Executor::new(&database)
        .run(
            &mut store,
            fenced_work.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    let fenced_arguments = launch_arguments(state.path(), fenced_work.id, 1);
    let disallow = fenced_arguments
        .iter()
        .position(|argument| argument == "--disallowedTools")
        .expect("default launch disallows native subagents");
    assert_eq!(fenced_arguments[disallow + 1], "Task");

    let opted_work = store
        .add_work_task_args(project.id, opted.id, "Opted", None)
        .unwrap();
    Executor::new(&database)
        .run(
            &mut store,
            opted_work.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    let opted_arguments = launch_arguments(state.path(), opted_work.id, 1);
    assert!(!opted_arguments.contains(&"--disallowedTools".to_owned()));
}

#[test]
fn nested_store_open_waits_out_a_busy_database() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    drop(Store::open(&database).unwrap());
    let lock = rusqlite::Connection::open(&database).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let thread_database = database.clone();
    let writer = std::thread::spawn(move || {
        let project_dir = tempfile::tempdir().unwrap();
        let store = Store::open(&thread_database).unwrap();
        store
            .register_project("nested", project_dir.path())
            .map(|_| ())
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    lock.execute_batch("COMMIT;").unwrap();
    writer
        .join()
        .unwrap()
        .expect("nested writer should wait for the lock instead of failing busy");
}

#[test]
fn parent_completion_waits_for_direct_children_in_the_domain() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
    let parent = store
        .add_work_task_args(project_id, worker_id, "Coordinate", None)
        .unwrap();
    let child = store
        .add_work_task_args(project_id, worker_id, "Delegated", Some(parent.id))
        .unwrap();

    let pending = store.complete_work(parent.id, "Done", None).unwrap_err();
    assert!(
        pending
            .to_string()
            .contains(&format!("direct child tasks [{}]", child.id))
    );

    store
        .begin_execution_args(child.id, None, None, state.path(), "boot")
        .unwrap();
    let running = store.complete_work(parent.id, "Done", None).unwrap_err();
    assert!(running.to_string().contains(&child.id.to_string()));

    store.interrupt_execution(child.id).unwrap();
    assert!(store.complete_work(parent.id, "Done", None).is_err());

    store.complete_work(child.id, "Child done", None).unwrap();
    let completed = store.complete_work(parent.id, "Done", None).unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
}

#[test]
fn a_completed_parent_cannot_gain_children_in_the_domain() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
    let parent = store
        .add_work_task_args(project_id, worker_id, "Coordinate", None)
        .unwrap();
    store.complete_work(parent.id, "Done", None).unwrap();
    let error = store
        .add_work_task_args(project_id, worker_id, "Late child", Some(parent.id))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("completed and cannot receive new children")
    );
}

#[test]
fn parent_completion_invariant_is_guaranteed_in_the_schema() {
    let state = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
    let parent = store
        .add_work_task_args(project_id, worker_id, "Coordinate", None)
        .unwrap();
    let child = store
        .add_work_task_args(project_id, worker_id, "Delegated", Some(parent.id))
        .unwrap();
    drop(store);

    // Bypass the domain layer entirely; the schema triggers must still hold.
    let raw = rusqlite::Connection::open(&database).unwrap();
    let complete_parent = "UPDATE tasks SET status='completed' WHERE id=?1";
    let unfinished = raw.execute(complete_parent, [parent.id]).unwrap_err();
    assert!(unfinished.to_string().contains("unfinished or running"));

    // A completed child still recorded as running keeps the parent blocked.
    raw.execute(
        "UPDATE tasks SET status='completed', execution_status='running' WHERE id=?1",
        [child.id],
    )
    .unwrap();
    let still_running = raw.execute(complete_parent, [parent.id]).unwrap_err();
    assert!(still_running.to_string().contains("unfinished or running"));

    raw.execute(
        "UPDATE tasks SET execution_status='succeeded' WHERE id=?1",
        [child.id],
    )
    .unwrap();
    assert_eq!(raw.execute(complete_parent, [parent.id]).unwrap(), 1);

    let insert = raw
        .execute(
            "INSERT INTO tasks(project_id,agent_id,kind,status,description,parent_task_id) VALUES(?1,?2,'work','pending','bypass',?3)",
            rusqlite::params![project_id, worker_id, parent.id],
        )
        .unwrap_err();
    assert!(
        insert
            .to_string()
            .contains("cannot create a child under a completed parent")
    );

    raw.execute(
        "INSERT INTO tasks(project_id,agent_id,kind,status,description) VALUES(?1,?2,'work','pending','loose')",
        rusqlite::params![project_id, worker_id],
    )
    .unwrap();
    let loose_id = raw.last_insert_rowid();
    let reparent = raw
        .execute(
            "UPDATE tasks SET parent_task_id=?2 WHERE id=?1",
            rusqlite::params![loose_id, parent.id],
        )
        .unwrap_err();
    assert!(
        reparent
            .to_string()
            .contains("cannot create a child under a completed parent")
    );
}

#[test]
fn concurrent_child_creation_and_parent_completion_cannot_both_commit() {
    for _ in 0..10 {
        let state = tempfile::tempdir().unwrap();
        let database = state.path().join("mush.sqlite");
        let (store, _project_dir, project_id, worker_id) = delegation_fixture(&database);
        let parent = store
            .add_work_task_args(project_id, worker_id, "Coordinate", None)
            .unwrap();
        drop(store);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let complete = {
            let (database, barrier) = (database.clone(), barrier.clone());
            std::thread::spawn(move || {
                let mut store = Store::open(&database).unwrap();
                barrier.wait();
                store.complete_work(parent.id, "Done", None).is_ok()
            })
        };
        let delegate = {
            let (database, barrier) = (database.clone(), barrier.clone());
            std::thread::spawn(move || {
                let store = Store::open(&database).unwrap();
                barrier.wait();
                store
                    .add_work_task_args(project_id, worker_id, "Delegated", Some(parent.id))
                    .is_ok()
            })
        };
        let completed = complete.join().unwrap();
        let delegated = delegate.join().unwrap();
        // Whichever write wins the race, the final state is never a completed
        // parent holding an unfinished child.
        let store = Store::open(&database).unwrap();
        let tasks = store.tasks(None).unwrap();
        let parent_completed = tasks
            .iter()
            .any(|task| task.id == parent.id && task.status == TaskStatus::Completed);
        let unfinished_child = tasks.iter().any(|task| {
            task.parent_task_id == Some(parent.id) && task.status != TaskStatus::Completed
        });
        assert_eq!(parent_completed, completed);
        assert!(
            !(parent_completed && unfinished_child),
            "invalid final state"
        );
        assert!(completed || delegated, "one racing write must commit");
    }
}

#[cfg(unix)]
#[test]
fn successful_parent_harness_result_is_refused_while_a_child_runs() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("claude");
    fake_claude(&executable, true);
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
            &claude_settings(&executable, None),
            false,
        )
        .unwrap();
    let parent = store
        .add_work_task_args(project.id, worker.id, "Coordinate", None)
        .unwrap();
    let child = store
        .add_work_task_args(project.id, worker.id, "Delegated", Some(parent.id))
        .unwrap();
    store
        .begin_execution_args(child.id, None, None, state.path(), "boot")
        .unwrap();

    // The harness reports success, but the running child refuses completion.
    let error = Executor::new(&database)
        .run(
            &mut store,
            parent.id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("direct child tasks [{}]", child.id))
    );
    let refused = store.task(parent.id).unwrap();
    assert_eq!(refused.status, TaskStatus::Pending);
    assert_eq!(refused.execution_status, Some(ExecutionStatus::Interrupted));
    assert_eq!(
        store.task(child.id).unwrap().execution_status,
        Some(ExecutionStatus::Running)
    );
}

fn cli_failure(database: &std::path::Path, arguments: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_mush"))
        .arg("--database")
        .arg(database)
        .args(arguments)
        .output()
        .unwrap();
    assert!(!output.status.success(), "command unexpectedly succeeded");
    String::from_utf8(output.stderr).unwrap()
}

#[cfg(unix)]
#[test]
fn cli_completion_surfaces_name_the_blocking_children() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("claude");
    fake_claude(&executable, true);
    let store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    let worker = store
        .register_agent_args(
            project.id,
            "worker",
            "claude-code",
            "claude-opus-5",
            &claude_settings(&executable, None),
            false,
        )
        .unwrap();
    let parent = store
        .add_work_task_args(project.id, worker.id, "Coordinate", None)
        .unwrap();
    let child = store
        .add_work_task_args(project.id, worker.id, "Delegated", Some(parent.id))
        .unwrap();
    drop(store);

    let expected = format!("direct child tasks [{}]", child.id);
    let manual = cli_failure(
        &database,
        &[
            "task",
            "complete",
            &parent.id.to_string(),
            "--result",
            "Done",
        ],
    );
    assert!(manual.contains(&expected), "stderr: {manual}");
    let executed = cli_failure(&database, &["task", "run", &parent.id.to_string()]);
    assert!(executed.contains(&expected), "stderr: {executed}");
}

#[cfg(unix)]
#[test]
fn executable_checkpoint_registration_requires_a_review_prompt() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let executable = state.path().join("claude");
    let store = Store::open(&database).unwrap();
    let project = store
        .register_project("project", project_dir.path())
        .unwrap();
    for (name, prompt) in [
        ("missing", None),
        ("empty", Some("")),
        ("blank", Some("  \n")),
    ] {
        for (harness, settings) in [
            ("claude-code", claude_settings(&executable, prompt)),
            ("cursor", cursor_settings(&executable, prompt)),
        ] {
            let error = store
                .register_agent_args(project.id, name, harness, "model", &settings, true)
                .unwrap_err();
            assert!(error.to_string().contains("non-empty review_prompt"));
        }
    }
    let reviewer = store
        .register_agent_args(
            project.id,
            "reviewer",
            "claude-code",
            "claude-opus-5",
            &claude_settings(&executable, Some("Review independently")),
            true,
        )
        .unwrap();
    // A manual reviewer is not executable and needs no review prompt; it lives
    // in its own project because a project has one checkpoint agent.
    let manual_project_dir = tempfile::tempdir().unwrap();
    let manual_project = store
        .register_project("manual-project", manual_project_dir.path())
        .unwrap();
    store
        .register_agent_args(manual_project.id, "human", "manual", "human", "{}", true)
        .unwrap();
    // Updating revalidates: an executable reviewer cannot drop its prompt.
    let dropped = store
        .update_agent_settings(reviewer.id, &claude_settings(&executable, None))
        .unwrap_err();
    assert!(dropped.to_string().contains("non-empty review_prompt"));
}

#[cfg(unix)]
#[test]
fn agent_update_repins_the_executable_and_recovers_the_same_session() {
    let state = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let database = state.path().join("mush.sqlite");
    let (mut store, symlink_path, work_id) = cursor_fixture(state.path(), project_dir.path());

    // The first attempt runs on the registered version but ends in an error
    // result, leaving an interrupted task with a captured chat id.
    fake_cursor(&symlink_path, "2026.08.04-aaa8809", CURSOR_ERROR_RESULT);
    Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert_eq!(
        store.task(work_id).unwrap().session_id.as_deref(),
        Some("chat-1234")
    );

    // The mutable vendor path then auto-updates, so resume is refused.
    fake_cursor(&symlink_path, "2026.09.01-0000000", CURSOR_SUCCESS);
    let mismatch = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap_err();
    assert!(mismatch.to_string().contains("version mismatch"));

    // Repinning the agent to a versioned executable path recovers the same
    // session without touching the vendor symlink.
    let versioned = state
        .path()
        .join("versions-2026.08.04-aaa8809-cursor-agent");
    fake_cursor(&versioned, "2026.08.04-aaa8809", CURSOR_SUCCESS);
    let updated: Value = serde_json::from_str(&cli(
        &database,
        &[
            "--json",
            "agent",
            "update",
            "1",
            "--settings",
            &cursor_settings(&versioned, None),
        ],
    ))
    .unwrap();
    assert!(
        updated["settings"]
            .as_str()
            .unwrap()
            .contains("versions-2026.08.04-aaa8809-cursor-agent")
    );
    let completed = Executor::new(&database)
        .run(
            &mut store,
            work_id,
            &mush::executor::RunOptions {
                worktree: None,
                restart_session: false,
                prompt_override: None,
            },
        )
        .unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(completed.session_id.as_deref(), Some("chat-1234"));
    let resumed = launch_arguments(state.path(), work_id, 2);
    assert!(resumed.contains(&"--resume".to_owned()));
    assert!(resumed.contains(&"chat-1234".to_owned()));
}
