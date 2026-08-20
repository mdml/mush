use super::*;
pub(super) fn bounded_text(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    const NOTICE: &str = "\n[truncated; inspect task artifacts for full output]";
    if limit == 0 {
        return String::new();
    }
    if limit <= NOTICE.len() {
        return utf8_prefix(NOTICE, limit).to_owned();
    }
    let content_limit = limit - NOTICE.len();
    let mut bounded = utf8_prefix(value, content_limit).to_owned();
    bounded.push_str(NOTICE);
    bounded
}

fn utf8_prefix(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut boundary = limit;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

const ELIDED_FIELD_MARKER: &str = "[elided; inspect task artifacts]";

struct ObservationBudget {
    remaining: usize,
    elided: bool,
}

impl ObservationBudget {
    fn required(&mut self, value: &str, field_limit: usize) -> String {
        if value.len() <= field_limit && value.len() <= self.remaining {
            self.remaining -= value.len();
            return value.to_owned();
        }
        self.elided = true;
        self.remaining -= ELIDED_FIELD_MARKER.len();
        ELIDED_FIELD_MARKER.to_owned()
    }

    fn optional(&mut self, value: Option<&str>, field_limit: usize) -> Option<String> {
        let value = value?;
        if value.len() > field_limit || value.len() > self.remaining {
            self.elided = true;
            return None;
        }
        self.remaining -= value.len();
        Some(value.to_owned())
    }
}

fn bound_task(task: &mut Task, budget: &mut ObservationBudget, reserved: usize) {
    budget.remaining = budget.remaining.saturating_sub(reserved);
    task.description = budget.required(&task.description, MAX_FIELD_BYTES);
    task.result = budget.optional(task.result.as_deref(), MAX_FIELD_BYTES);
    task.evidence = budget.optional(task.evidence.as_deref(), MAX_FIELD_BYTES);
    task.intervention = budget.optional(task.intervention.as_deref(), MAX_DIAGNOSTIC_BYTES);
    task.session_id = budget.optional(task.session_id.as_deref(), MAX_IDENTIFIER_BYTES);
    task.worktree_name = budget.optional(task.worktree_name.as_deref(), MAX_IDENTIFIER_BYTES);
    task.artifact_dir = budget.optional(task.artifact_dir.as_deref(), MAX_PATH_BYTES);
    task.execution_boot_id =
        budget.optional(task.execution_boot_id.as_deref(), MAX_IDENTIFIER_BYTES);
    budget.remaining += reserved;
}

fn task_is_terminal(task: &Task) -> bool {
    matches!(
        task.readiness_status,
        ReadinessStatus::Completed | ReadinessStatus::InterventionRequired
    ) || task.status == TaskStatus::Completed
}

impl Store {
    pub fn observe(&self, ids: &[i64], until_all: bool) -> Result<TaskObservation, DomainError> {
        if ids.is_empty() || ids.len() > 32 {
            return Err(DomainError::Invalid(
                "status accepts 1 to 32 task ids".into(),
            ));
        }
        let cursor = self.connection.query_row(
            "SELECT COALESCE(MAX(id),0) FROM task_transitions",
            [],
            |r| r.get(0),
        )?;
        let mut tasks = ids
            .iter()
            .map(|id| self.task(*id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut budget = ObservationBudget {
            remaining: MAX_OBSERVATION_TEXT_BYTES,
            elided: false,
        };
        let task_count = tasks.len();
        for (index, task) in tasks.iter_mut().enumerate() {
            let reserved = (task_count - index - 1) * ELIDED_FIELD_MARKER.len();
            bound_task(task, &mut budget, reserved);
        }
        let terminal_count = tasks.iter().filter(|task| task_is_terminal(task)).count();
        let terminal = if until_all {
            terminal_count == tasks.len()
        } else {
            terminal_count > 0
        };
        Ok(TaskObservation {
            cursor,
            tasks,
            terminal,
            timed_out: false,
            elided: budget.elided,
        })
    }
}
