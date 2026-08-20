use crate::{Agent, DomainError};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    process::Command,
};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClaudeSettings {
    executable: String,
    #[serde(default)]
    version: Option<String>,
    model: String,
    effort: String,
    permission_mode: String,
    tools: Vec<String>,
    allowed_tools: Vec<String>,
    #[serde(default)]
    native_subagents: bool,
    #[serde(default)]
    review_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CodexSettings {
    executable: String,
    #[serde(default)]
    version: Option<String>,
    model: String,
    effort: String,
    sandbox: String,
    approval_policy: String,
    #[serde(default)]
    native_subagents: bool,
    #[serde(default)]
    review_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CursorSettings {
    executable: String,
    #[serde(default)]
    version: Option<String>,
    model: String,
    approval_mode: String,
    #[serde(default)]
    review_prompt: Option<String>,
}

pub(super) struct Invocation<'a> {
    pub session_id: Option<&'a str>,
    pub worktree: Option<&'a str>,
    pub resuming: bool,
}

/// The harness-specific half of execution: invocation, version identity,
/// session semantics, worktree ownership, and result parsing.
pub(super) enum Harness {
    ClaudeCode(ClaudeSettings),
    Codex(CodexSettings),
    Cursor(CursorSettings),
}

/// The harness names [`Harness::from_agent`] accepts.
pub(super) fn is_executable(harness: &str) -> bool {
    matches!(harness, "claude-code" | "codex" | "cursor")
}

pub(super) fn validate_registration(
    harness: &str,
    settings: &str,
    checkpoint: bool,
) -> Result<(), DomainError> {
    let review_prompt = match harness {
        "claude-code" => parse_claude(settings)?.review_prompt,
        "codex" => parse_codex(settings)?.review_prompt,
        "cursor" => parse_cursor(settings)?.review_prompt,
        _ => return Ok(()),
    };
    if checkpoint && review_prompt.as_deref().is_none_or(|p| p.trim().is_empty()) {
        return Err(DomainError::Invalid(
            "an executable checkpoint agent requires a non-empty review_prompt".into(),
        ));
    }
    Ok(())
}

fn parse_claude(settings: &str) -> Result<ClaudeSettings, DomainError> {
    serde_json::from_str(settings)
        .map_err(|error| DomainError::Invalid(format!("invalid claude-code settings: {error}")))
}

fn parse_codex(settings: &str) -> Result<CodexSettings, DomainError> {
    serde_json::from_str(settings)
        .map_err(|error| DomainError::Invalid(format!("invalid codex settings: {error}")))
}

fn parse_cursor(settings: &str) -> Result<CursorSettings, DomainError> {
    let settings: CursorSettings = serde_json::from_str(settings)
        .map_err(|error| DomainError::Invalid(format!("invalid cursor settings: {error}")))?;
    validate_cursor_approval_mode(&settings.approval_mode)?;
    Ok(settings)
}

/// The cursor-agent invocation flag for one of Cursor's own run modes, or
/// `None` when the mode cannot run headless. `allowlist` has no flag because it
/// is the CLI's default rather than something to request.
fn cursor_approval_flag(mode: &str) -> Option<&'static str> {
    match mode {
        "unrestricted" => Some("--force"),
        "auto-review" => Some("--auto-review"),
        _ => None,
    }
}

/// Refuse a Cursor run mode that an executable agent cannot use. `allowlist`
/// is the dangerous one: run headless it denies every unlisted tool call and
/// still reports a successful, non-error result, so a task would complete
/// having executed nothing.
fn validate_cursor_approval_mode(mode: &str) -> Result<(), DomainError> {
    if cursor_approval_flag(mode).is_some() {
        return Ok(());
    }
    let reason = if mode == "allowlist" {
        "cursor-agent denies every unlisted tool call headless while still reporting success, so the task would complete having executed nothing"
    } else {
        "it is not one of cursor-agent's run modes"
    };
    Err(DomainError::Invalid(format!(
        "cursor approval_mode \"{mode}\" cannot run an executable agent: {reason}; register \"unrestricted\" or \"auto-review\""
    )))
}

impl Harness {
    pub(super) fn from_agent(agent: &Agent) -> Result<Self, DomainError> {
        match agent.harness.as_str() {
            "claude-code" => parse_claude(&agent.settings).map(Self::ClaudeCode),
            "codex" => parse_codex(&agent.settings).map(Self::Codex),
            "cursor" => parse_cursor(&agent.settings).map(Self::Cursor),
            other => Err(DomainError::Invalid(format!(
                "unsupported harness: {other}"
            ))),
        }
    }

    pub(super) fn display_name(&self) -> &'static str {
        match self {
            Self::ClaudeCode(_) => "Claude Code",
            Self::Codex(_) => "Codex",
            Self::Cursor(_) => "Cursor Agent",
        }
    }

    pub(super) fn executable(&self) -> &str {
        match self {
            Self::ClaudeCode(settings) => &settings.executable,
            Self::Codex(settings) => &settings.executable,
            Self::Cursor(settings) => &settings.executable,
        }
    }

    fn expected_version(&self) -> Option<&str> {
        match self {
            Self::ClaudeCode(settings) => settings.version.as_deref(),
            Self::Codex(settings) => settings.version.as_deref(),
            Self::Cursor(settings) => settings.version.as_deref(),
        }
    }

    pub(super) fn review_prompt(&self) -> Option<&str> {
        match self {
            Self::ClaudeCode(settings) => settings.review_prompt.as_deref(),
            Self::Codex(settings) => settings.review_prompt.as_deref(),
            Self::Cursor(settings) => settings.review_prompt.as_deref(),
        }
    }

    pub(super) fn mush_owns_worktree(&self) -> bool {
        matches!(self, Self::Codex(_) | Self::Cursor(_))
    }

    pub(super) fn worktree_root(&self) -> &'static str {
        match self {
            Self::ClaudeCode(_) => ".claude/worktrees",
            Self::Codex(_) | Self::Cursor(_) => ".mush/worktrees",
        }
    }

    pub(super) fn streamed_session_field(&self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode(_) => None,
            Self::Codex(_) => Some("thread_id"),
            Self::Cursor(_) => Some("session_id"),
        }
    }

    pub(super) fn captures_session_from_stream(&self) -> bool {
        self.streamed_session_field().is_some()
    }

    pub(super) fn assigned_session(&self, persisted: Option<String>) -> Option<String> {
        match self {
            Self::ClaudeCode(_) => Some(persisted.unwrap_or_else(|| Uuid::new_v4().to_string())),
            Self::Codex(_) | Self::Cursor(_) => persisted,
        }
    }

    pub(super) fn resolve_version(&self) -> Result<String, DomainError> {
        let name = self.display_name();
        let output = Command::new(self.executable())
            .arg("--version")
            .output()
            .map_err(|error| {
                DomainError::Invalid(format!("cannot inspect {name} version: {error}"))
            })?;
        let observed = String::from_utf8_lossy(&output.stdout);
        let observed = observed.trim();
        if !output.status.success() {
            return Err(DomainError::Invalid(format!(
                "cannot inspect {name} version: {} exited with {}",
                self.executable(),
                output.status
            )));
        }
        let Some(pinned) = self.expected_version() else {
            return Ok(observed.to_owned());
        };
        if !observed.split_whitespace().any(|token| token == pinned) {
            return Err(DomainError::Invalid(format!(
                "{name} version mismatch: expected {pinned}, observed {observed}"
            )));
        }
        Ok(observed.to_owned())
    }

    pub(super) fn arguments(&self, invocation: &Invocation<'_>) -> Vec<String> {
        match self {
            Self::ClaudeCode(settings) => claude_arguments(settings, invocation),
            Self::Codex(settings) => codex_arguments(settings, invocation),
            Self::Cursor(settings) => cursor_arguments(settings, invocation),
        }
    }

    pub(super) fn parse_result(&self, path: &Path) -> Result<String, DomainError> {
        match self {
            Self::Codex(_) => parse_codex_result(path, self.display_name()),
            Self::Cursor(_) => parse_result_event(path, self.display_name(), true),
            Self::ClaudeCode(_) => parse_result_event(path, self.display_name(), false),
        }
    }
}

fn claude_arguments(settings: &ClaudeSettings, invocation: &Invocation<'_>) -> Vec<String> {
    let session_id = invocation
        .session_id
        .expect("claude-code sessions are pre-assigned");
    let mut args = vec![
        "--print".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    if invocation.resuming {
        args.extend(["--resume".into(), session_id.into()]);
    } else {
        args.extend(["--session-id".into(), session_id.into()]);
        if let Some(name) = invocation.worktree {
            args.extend(["--worktree".into(), name.into()]);
        }
    }
    args.extend([
        "--model".into(),
        settings.model.clone(),
        "--effort".into(),
        settings.effort.clone(),
        "--permission-mode".into(),
        settings.permission_mode.clone(),
    ]);
    append_optional_list(&mut args, "--tools", &settings.tools);
    append_optional_list(&mut args, "--allowedTools", &settings.allowed_tools);
    if !settings.native_subagents {
        args.extend(["--disallowedTools".into(), "Task".into()]);
    }
    args
}

fn append_optional_list(args: &mut Vec<String>, flag: &str, values: &[String]) {
    if !values.is_empty() {
        args.extend([flag.into(), values.join(",")]);
    }
}

fn codex_arguments(settings: &CodexSettings, invocation: &Invocation<'_>) -> Vec<String> {
    let mut args = vec!["exec".into()];
    if invocation.resuming {
        args.push("resume".into());
    }
    args.extend([
        "--json".into(),
        "--model".into(),
        settings.model.clone(),
        "-c".into(),
        format!("model_reasoning_effort=\"{}\"", settings.effort),
        "-c".into(),
        format!("sandbox_mode=\"{}\"", settings.sandbox),
        "-c".into(),
        format!("approval_policy=\"{}\"", settings.approval_policy),
    ]);
    if !settings.native_subagents {
        args.extend(["--disable".into(), "multi_agent".into()]);
    }
    if invocation.resuming {
        args.push(
            invocation
                .session_id
                .expect("resume requires a captured thread id")
                .into(),
        );
    }
    args.push("-".into());
    args
}

fn cursor_arguments(settings: &CursorSettings, invocation: &Invocation<'_>) -> Vec<String> {
    let approval = cursor_approval_flag(&settings.approval_mode)
        .expect("cursor settings are validated before they reach an invocation");
    let mut args = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--model".into(),
        settings.model.clone(),
        approval.into(),
        "--trust".into(),
    ];
    if invocation.resuming {
        let session_id = invocation
            .session_id
            .expect("resume requires a captured chat id");
        args.extend(["--resume".into(), session_id.into()]);
    }
    args
}

fn parse_codex_result(path: &Path, name: &str) -> Result<String, DomainError> {
    let reader = BufReader::new(File::open(path)?);
    let mut message = None;
    for line in reader.lines() {
        let value: Value = serde_json::from_str(&line?)?;
        if let Some(failure) = codex_failure(&value) {
            return Err(DomainError::Invalid(format!(
                "{name} turn failed: {failure}; see {}",
                path.display()
            )));
        }
        if let Some(text) = codex_agent_message(&value) {
            message = Some(text.to_owned());
        }
    }
    message.ok_or_else(|| {
        DomainError::Invalid(format!(
            "{name} output has no agent message: {}",
            path.display()
        ))
    })
}

fn codex_failure(value: &Value) -> Option<&str> {
    (value.get("type").and_then(Value::as_str) == Some("turn.failed"))
        .then(|| value.pointer("/error/message").and_then(Value::as_str))
        .flatten()
        .or_else(|| {
            (value.get("type").and_then(Value::as_str) == Some("turn.failed"))
                .then_some("no detail reported")
        })
}

fn codex_agent_message(value: &Value) -> Option<&str> {
    (value.get("type").and_then(Value::as_str) == Some("item.completed"))
        .then(|| value.get("item"))
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("agent_message"))
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
}

fn parse_result_event(path: &Path, name: &str, checks_error: bool) -> Result<String, DomainError> {
    let reader = BufReader::new(File::open(path)?);
    let mut event = None;
    for line in reader.lines() {
        let value: Value = serde_json::from_str(&line?)?;
        if value.get("type").and_then(Value::as_str) == Some("result") {
            event = Some(value);
        }
    }
    let event = event.ok_or_else(|| {
        DomainError::Invalid(format!(
            "{name} output has no result event: {}",
            path.display()
        ))
    })?;
    if checks_error && event.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(DomainError::Invalid(format!(
            "{name} reported an error result; see {}",
            path.display()
        )));
    }
    event
        .get("result")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            DomainError::Invalid(format!(
                "{name} result event has no result text: {}",
                path.display()
            ))
        })
}
