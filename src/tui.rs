use crate::{CheckpointDecision, DomainError, Store, Task, TaskKind, TaskStatus};
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
    for task in tasks {
        output.push_str(&format!(
            "#{} [{} / {}] {} (readiness: {})",
            task.id, task.kind, task.status, task.description, task.readiness_status
        ));
        if let Some(parent) = task.parent_task_id {
            output.push_str(&format!(" (parent #{parent})"));
        }
        if let Some(subject) = task.subject_task_id {
            output.push_str(&format!(" (subject #{subject})"));
        }
        if let Some(previous) = task.previous_task_id {
            output.push_str(&format!(" (previous #{previous})"));
        }
        output.push('\n');
        if let Some(prerequisites) = dependencies.get(&task.id) {
            output.push_str(&format!("  prerequisites: {:?}\n", prerequisites));
        }
        if awaits_checkpoint(&task, &reviewed) {
            output.push_str("  no checkpoint yet\n");
        }
        if let Some(subject) = awaited_subject(&task, &completed) {
            output.push_str(&format!("  awaiting subject: #{subject} not completed\n"));
        }
        if let Some(decision) = task.decision {
            output.push_str(&format!("  decision: {decision}\n"));
        }
        if let Some(execution) = task.execution_status {
            output.push_str(&format!(
                "  execution: {execution} (attempt {})\n",
                task.execution_attempt
            ));
        }
        if let Some(intervention) = &task.intervention {
            output.push_str(&format!("  intervention: {intervention}\n"));
        }
        if let Some(evidence) = task.evidence {
            output.push_str("  evidence (Markdown):\n");
            for line in evidence.lines() {
                output.push_str(&format!("    {line}\n"));
            }
        }
    }
    Ok(output)
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
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1).min(tasks.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                KeyCode::Char('a') => {
                    decide_selected(store, &tasks, selected, CheckpointDecision::Accepted)?
                }
                KeyCode::Char('b') => {
                    decide_selected(store, &tasks, selected, CheckpointDecision::Blocked)?
                }
                KeyCode::Char('r') => decide_selected(
                    store,
                    &tasks,
                    selected,
                    CheckpointDecision::RevisionRequested,
                )?,
                _ => {}
            }
        }
    }
}

fn decide_selected(
    store: &mut Store,
    tasks: &[Task],
    selected: usize,
    decision: CheckpointDecision,
) -> Result<(), DomainError> {
    let completed = completed_ids(tasks);
    if let Some(task) = tasks
        .get(selected)
        .filter(|task| task.kind == TaskKind::Checkpoint)
        // A checkpoint awaiting its subject cannot be decided before that
        // subject completes; the domain rejects it too, this merely keeps the
        // TUI from offering it.
        .filter(|task| awaited_subject(task, &completed).is_none())
    {
        let evidence = task.evidence.clone().unwrap_or_default();
        store.decide_checkpoint(task.id, decision, &evidence)?;
    }
    Ok(())
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
                    .title("Evidence (a accept, b block, r revise)")
                    .title_style(Style::default().add_modifier(Modifier::BOLD))
                    .borders(Borders::ALL),
            ),
        areas[1],
    );
}

fn task_details(task: &Task) -> String {
    format!(
        "Task #{}\nKind: {}\nStatus: {}\nReadiness: {}\nExecution: {}\nAttempt: {}\nParent: {}\nSubject: {}\nPrevious: {}\nDecision: {}\nIntervention: {}\nArtifacts: {}\n\nMarkdown evidence\n{}",
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
        task.decision.map_or_else(|| "—".into(), |d| d.to_string()),
        task.intervention.as_deref().unwrap_or("—"),
        task.artifact_dir.as_deref().unwrap_or("—"),
        task.evidence.as_deref().unwrap_or("—")
    )
}
