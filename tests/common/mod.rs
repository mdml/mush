use mush::{Agent, DomainError, Store, Task};

#[allow(dead_code)]
pub trait StoreTestExt {
    fn register_agent_args(
        &self,
        project_id: i64,
        name: &str,
        harness: &str,
        model: &str,
        settings: &str,
        checkpoint: bool,
    ) -> Result<Agent, DomainError>;

    fn add_work_task_args(
        &self,
        project_id: i64,
        agent_id: i64,
        description: &str,
        parent_task_id: Option<i64>,
    ) -> Result<Task, DomainError>;

    fn claim_launch_args(
        &mut self,
        task_id: i64,
        runner_id: &str,
        runner_boot_id: &str,
        runner_pid: u32,
    ) -> Result<bool, DomainError>;

    fn begin_execution_args(
        &mut self,
        task_id: i64,
        session_id: Option<&str>,
        worktree_name: Option<&str>,
        artifact_dir: &std::path::Path,
        boot_id: &str,
    ) -> Result<Task, DomainError>;

    fn finish_work_execution_args(
        &mut self,
        task_id: i64,
        execution_attempt: i64,
        result: &str,
        evidence: &str,
    ) -> Result<Task, DomainError>;
}

impl StoreTestExt for Store {
    fn register_agent_args(
        &self,
        project_id: i64,
        name: &str,
        harness: &str,
        model: &str,
        settings: &str,
        checkpoint: bool,
    ) -> Result<Agent, DomainError> {
        self.register_agent(mush::store::AgentRegistration {
            project_id,
            name,
            harness,
            model,
            settings,
            checkpoint,
        })
    }

    fn add_work_task_args(
        &self,
        project_id: i64,
        agent_id: i64,
        description: &str,
        parent_task_id: Option<i64>,
    ) -> Result<Task, DomainError> {
        self.add_work_task(mush::store::WorkTaskRequest {
            project_id,
            agent_id,
            description,
            parent_task_id,
        })
    }

    fn claim_launch_args(
        &mut self,
        task_id: i64,
        runner_id: &str,
        runner_boot_id: &str,
        runner_pid: u32,
    ) -> Result<bool, DomainError> {
        self.claim_launch(mush::store::LaunchClaim {
            task_id,
            runner_id,
            runner_boot_id,
            runner_pid,
        })
    }

    fn begin_execution_args(
        &mut self,
        task_id: i64,
        session_id: Option<&str>,
        worktree_name: Option<&str>,
        artifact_dir: &std::path::Path,
        boot_id: &str,
    ) -> Result<Task, DomainError> {
        self.begin_execution(mush::store::ExecutionStart {
            task_id,
            session_id,
            worktree_name,
            artifact_dir,
            boot_id,
            claimed_runner_id: None,
        })
    }

    fn finish_work_execution_args(
        &mut self,
        task_id: i64,
        execution_attempt: i64,
        result: &str,
        evidence: &str,
    ) -> Result<Task, DomainError> {
        self.finish_work_execution(mush::store::WorkExecutionResult {
            owner: mush::store::ExecutionOwner {
                task_id,
                execution_attempt,
            },
            result,
            evidence,
        })
    }
}

#[cfg(unix)]
pub fn install_script(path: &std::path::Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, script).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
pub fn fake_claude(path: &std::path::Path, succeeds: bool) {
    let outcome = if succeeds {
        "printf '%s\\n' '{\"type\":\"result\",\"result\":\"fake executor result\"}'"
    } else {
        "echo 'simulated interruption' >&2; exit 7"
    };
    install_script(
        path,
        &format!(
            "#!/usr/bin/env bash\nif [[ ${{1:-}} == --version ]]; then echo '2.1.222 (Claude Code)'; exit 0; fi\nprompt=$(cat)\n{outcome}\n"
        ),
    );
}

#[cfg(unix)]
pub fn claude_settings(executable: &std::path::Path, review_prompt: Option<&str>) -> String {
    serde_json::json!({
        "executable": executable,
        "version": "2.1.222",
        "model": "claude-opus-5",
        "effort": "medium",
        "permission_mode": "acceptEdits",
        "tools": ["Bash", "Edit", "Read", "Write", "Glob", "Grep"],
        "allowed_tools": ["Bash(git *)", "Bash(just *)"],
        "review_prompt": review_prompt,
    })
    .to_string()
}

#[cfg(unix)]
pub fn git_project(path: &std::path::Path) {
    for arguments in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "init",
        ],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(&arguments)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
