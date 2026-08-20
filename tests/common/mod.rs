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
