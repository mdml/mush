use crate::{
    CheckpointDecision, DomainError, LoopReport, ReadinessStatus, Store, Task, TaskKind, TaskStatus,
};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use std::io::{self, IsTerminal};

pub fn snapshot(store: &Store, project_id: Option<i64>) -> Result<String, DomainError> {
    let tasks = store.tasks(project_id)?;
    let reviewed = reviewed_subjects(&tasks);
    let completed = completed_ids(&tasks);
    let dependencies = store.all_dependencies()?;
    let mut output = String::from("Mush tasks\n");
    for task in &tasks {
        output.push_str(&snapshot_task(
            task,
            dependencies.get(&task.id).map(Vec::as_slice),
            &reviewed,
            &completed,
        ));
    }
    for report in store.loops(project_id)? {
        output.push_str(&snapshot_loop(&report));
    }
    Ok(output)
}

/// Restate a loop without transcript access: criteria, attempt number,
/// remaining budget, decisions, and the next eligible action.
fn snapshot_loop(report: &LoopReport) -> String {
    let mut output = format!(
        "Loop #{} [{}] attempt {}/{} (adjudicator agent {})\n",
        report.declaration.id,
        report.status,
        report.attempts.len(),
        report.declaration.max_attempts,
        report.declaration.adjudicator_agent_id
    );
    output.push_str("  criteria (Markdown):\n");
    for line in report.declaration.criteria.lines() {
        output.push_str(&format!("    {line}\n"));
    }
    for attempt in &report.attempts {
        output.push_str(&format!(
            "  attempt {}: stages {:?}, checkpoint #{}, decision: {}\n",
            attempt.number,
            attempt.stage_task_ids,
            attempt.checkpoint_task_id,
            attempt
                .decision
                .map_or_else(|| "undecided".to_owned(), |decision| decision.to_string())
        ));
    }
    if let Some(continuation) = report.declaration.continuation_task_id {
        output.push_str(&format!("  continuation: #{continuation}\n"));
    }
    output.push_str(&format!(
        "  remaining attempts: {}\n  next: {}\n",
        report.remaining_attempts, report.next_action
    ));
    output
}

fn snapshot_task(
    task: &Task,
    prerequisites: Option<&[i64]>,
    reviewed: &std::collections::HashSet<i64>,
    completed: &std::collections::HashSet<i64>,
) -> String {
    let mut output = format!(
        "#{} [{} / {}] {} (readiness: {})",
        task.id, task.kind, task.status, task.description, task.readiness_status
    );
    output.push_str(&relationship_summary(task));
    output.push('\n');
    if let Some(prerequisites) = prerequisites {
        output.push_str(&format!("  prerequisites: {prerequisites:?}\n"));
    }
    output.push_str(&review_summary(task, reviewed, completed));
    output.push_str(&execution_summary(task));
    output.push_str(&criteria_summary(task));
    output.push_str(&evidence_summary(task));
    output
}

fn criteria_summary(task: &Task) -> String {
    let Some(criteria) = &task.criteria else {
        return String::new();
    };
    let mut output = String::from("  criteria (Markdown):\n");
    for line in criteria.lines() {
        output.push_str(&format!("    {line}\n"));
    }
    output
}

fn relationship_summary(task: &Task) -> String {
    let mut output = String::new();
    if let Some(parent) = task.parent_task_id {
        output.push_str(&format!(" (parent #{parent})"));
    }
    if let Some(subject) = task.subject_task_id {
        output.push_str(&format!(" (subject #{subject})"));
    }
    if let Some(previous) = task.previous_task_id {
        output.push_str(&format!(" (previous #{previous})"));
    }
    if let Some(loop_id) = task.loop_id {
        output.push_str(&format!(" (loop #{loop_id})"));
    }
    output
}

fn review_summary(
    task: &Task,
    reviewed: &std::collections::HashSet<i64>,
    completed: &std::collections::HashSet<i64>,
) -> String {
    let mut output = String::new();
    if awaits_checkpoint(task, reviewed) {
        output.push_str("  no checkpoint yet\n");
    }
    if let Some(subject) = awaited_subject(task, completed) {
        output.push_str(&format!("  awaiting subject: #{subject} not completed\n"));
    }
    if awaits_decision(task, completed) {
        output.push_str("  awaiting decision\n");
    }
    if let Some(decision) = task.decision {
        output.push_str(&format!("  decision: {decision}\n"));
    }
    output
}

fn execution_summary(task: &Task) -> String {
    let mut output = String::new();
    if let Some(execution) = task.execution_status {
        output.push_str(&format!(
            "  execution: {execution} (attempt {})\n",
            task.execution_attempt
        ));
    }
    if let Some(intervention) = &task.intervention {
        output.push_str(&format!("  intervention: {intervention}\n"));
    }
    output
}

fn evidence_summary(task: &Task) -> String {
    let Some(evidence) = &task.evidence else {
        return String::new();
    };
    let mut output = String::from("  evidence (Markdown):\n");
    for line in evidence.lines() {
        output.push_str(&format!("    {line}\n"));
    }
    output
}

/// Ids of work tasks that already have a checkpoint reviewing them.
fn reviewed_subjects(tasks: &[Task]) -> std::collections::HashSet<i64> {
    tasks
        .iter()
        .filter_map(|task| task.subject_task_id)
        .collect()
}

/// Completed work whose explicit checkpoint has not been created yet.
fn awaits_checkpoint(task: &Task, reviewed: &std::collections::HashSet<i64>) -> bool {
    task.kind == TaskKind::Work
        && task.status == TaskStatus::Completed
        && !reviewed.contains(&task.id)
}

/// Ids of completed tasks.
fn completed_ids(tasks: &[Task]) -> std::collections::HashSet<i64> {
    tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Completed)
        .map(|task| task.id)
        .collect()
}

/// An undecided checkpoint whose subject is completed and which has no queue
/// activity in flight: the graph is waiting on adjudication, not on execution.
fn awaits_decision(task: &Task, completed: &std::collections::HashSet<i64>) -> bool {
    task.kind == TaskKind::Checkpoint
        && task.status == TaskStatus::Pending
        && task
            .subject_task_id
            .is_some_and(|subject| completed.contains(&subject))
        && matches!(
            task.readiness_status,
            ReadinessStatus::Unqueued | ReadinessStatus::Completed
        )
}

/// The uncompleted subject a checkpoint created before it finished is waiting
/// for.
fn awaited_subject(task: &Task, completed: &std::collections::HashSet<i64>) -> Option<i64> {
    (task.kind == TaskKind::Checkpoint && task.status == TaskStatus::Pending)
        .then_some(task.subject_task_id)
        .flatten()
        .filter(|subject| !completed.contains(subject))
}

pub fn run(store: &mut Store, project_id: Option<i64>) -> Result<(), DomainError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(DomainError::Invalid(
            "mush tui needs a terminal; use mush tui --snapshot for non-interactive inspection"
                .into(),
        ));
    }
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = event_loop(&mut terminal, store, project_id);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &mut Store,
    project_id: Option<i64>,
) -> Result<(), DomainError> {
    let mut selected = 0usize;
    loop {
        let tasks = store.tasks(project_id)?;
        selected = selected.min(tasks.len().saturating_sub(1));
        terminal.draw(|frame| draw(frame, &tasks, selected))?;
        if let Event::Key(key) = event::read()? {
            match handle_key(&mut selected, tasks.len(), key.code) {
                UiAction::Quit => return Ok(()),
                UiAction::Decide(decision) => decide_selected(store, &tasks, selected, decision)?,
                UiAction::Continue => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiAction {
    Continue,
    Quit,
    Decide(CheckpointDecision),
}

/// Apply one key to navigation state and return any domain action the event
/// loop should perform. Keeping this decision independent of terminal I/O
/// makes navigation and shortcuts deterministic.
fn handle_key(selected: &mut usize, task_count: usize, key: KeyCode) -> UiAction {
    match key {
        KeyCode::Char('q') => UiAction::Quit,
        KeyCode::Down | KeyCode::Char('j') => {
            *selected = (*selected + 1).min(task_count.saturating_sub(1));
            UiAction::Continue
        }
        KeyCode::Up | KeyCode::Char('k') => {
            *selected = selected.saturating_sub(1);
            UiAction::Continue
        }
        KeyCode::Char('m') => UiAction::Decide(CheckpointDecision::Met),
        KeyCode::Char('b') => UiAction::Decide(CheckpointDecision::Blocked),
        KeyCode::Char('n') => UiAction::Decide(CheckpointDecision::NotMet),
        _ => UiAction::Continue,
    }
}

fn decide_selected(
    store: &mut Store,
    tasks: &[Task],
    selected: usize,
    decision: CheckpointDecision,
) -> Result<(), DomainError> {
    if let Some(task) = eligible_checkpoint(tasks, selected) {
        let evidence = task.evidence.clone().unwrap_or_default();
        store.decide_checkpoint(task.id, decision, &evidence)?;
    }
    Ok(())
}

/// Return the selected checkpoint only when the TUI may offer a decision.
/// The domain repeats this guard when the decision is persisted.
fn eligible_checkpoint(tasks: &[Task], selected: usize) -> Option<&Task> {
    let completed = completed_ids(tasks);
    tasks
        .get(selected)
        .filter(|task| task.kind == TaskKind::Checkpoint)
        .filter(|task| awaited_subject(task, &completed).is_none())
}

fn draw(frame: &mut ratatui::Frame<'_>, tasks: &[Task], selected: usize) {
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(frame.area());
    let reviewed = reviewed_subjects(tasks);
    let completed = completed_ids(tasks);
    let items: Vec<ListItem> = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| {
            let marker = if index == selected { ">" } else { " " };
            let parent = task
                .parent_task_id
                .map(|id| format!(" (parent #{id})"))
                .unwrap_or_default();
            let unreviewed = if awaits_checkpoint(task, &reviewed) {
                " (no checkpoint yet)"
            } else if awaited_subject(task, &completed).is_some() {
                " (awaiting subject)"
            } else if awaits_decision(task, &completed) {
                " (awaiting decision)"
            } else {
                ""
            };
            ListItem::new(format!(
                "{marker} #{} [{} / {}] {}{parent}{unreviewed}",
                task.id, task.kind, task.status, task.description
            ))
        })
        .collect();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title("Tasks (j/k, q)")
                .borders(Borders::ALL),
        ),
        areas[0],
    );
    let details = tasks
        .get(selected)
        .map(task_details)
        .unwrap_or_else(|| "No tasks".into());
    frame.render_widget(
        Paragraph::new(details)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White))
            .block(
                Block::default()
                    .title("Evidence (m met, n not met, b blocked)")
                    .title_style(Style::default().add_modifier(Modifier::BOLD))
                    .borders(Borders::ALL),
            ),
        areas[1],
    );
}

fn task_details(task: &Task) -> String {
    format!(
        "Task #{}\nKind: {}\nStatus: {}\nReadiness: {}\nExecution: {}\nAttempt: {}\nParent: {}\nSubject: {}\nPrevious: {}\nLoop: {}\nDecision: {}\nIntervention: {}\nArtifacts: {}\n\nCriteria\n{}\n\nMarkdown evidence\n{}",
        task.id,
        task.kind,
        task.status,
        task.readiness_status,
        task.execution_status
            .map_or_else(|| "—".into(), |value| value.to_string()),
        task.execution_attempt,
        task.parent_task_id
            .map_or_else(|| "—".into(), |id| format!("#{id}")),
        task.subject_task_id
            .map_or_else(|| "—".into(), |id| format!("#{id}")),
        task.previous_task_id
            .map_or_else(|| "—".into(), |id| format!("#{id}")),
        task.loop_id
            .map_or_else(|| "—".into(), |id| format!("#{id}")),
        task.decision.map_or_else(|| "—".into(), |d| d.to_string()),
        task.intervention.as_deref().unwrap_or("—"),
        task.artifact_dir.as_deref().unwrap_or("—"),
        task.criteria.as_deref().unwrap_or("—"),
        task.evidence.as_deref().unwrap_or("—")
    )
}

#[cfg(test)]
mod tests {
    include!("../tests/unit/tui.rs");
}
