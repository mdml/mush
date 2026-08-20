use super::*;

fn dependency_can_change(task: &Task) -> bool {
    task.execution_attempt == 0
        && task.status != TaskStatus::Completed
        && !matches!(
            task.readiness_status,
            ReadinessStatus::Claimed
                | ReadinessStatus::Running
                | ReadinessStatus::Completed
                | ReadinessStatus::InterventionRequired
        )
}

fn validate_dependency_tasks(prerequisite: &Task, dependent: &Task) -> Result<(), DomainError> {
    if prerequisite.id == dependent.id {
        return Err(DomainError::Invalid(
            "a task cannot depend on itself".into(),
        ));
    }
    if prerequisite.kind != TaskKind::Work || dependent.kind != TaskKind::Work {
        return Err(DomainError::Invalid(
            "this slice supports dependencies between work tasks only".into(),
        ));
    }
    if prerequisite.project_id != dependent.project_id {
        return Err(DomainError::Invalid(
            "dependency tasks must belong to the same project".into(),
        ));
    }
    if !dependency_can_change(dependent) {
        return Err(DomainError::Invalid(
            "dependencies cannot change after the dependent starts".into(),
        ));
    }
    Ok(())
}

fn validate_new_dependency(
    connection: &Connection,
    prerequisite: i64,
    dependent: i64,
) -> Result<(), DomainError> {
    let duplicate: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM task_dependencies WHERE prerequisite_task_id=?1 AND dependent_task_id=?2)", params![prerequisite,dependent], |row| row.get(0))?;
    if duplicate {
        return Err(DomainError::Invalid(format!(
            "dependency {prerequisite} -> {dependent} already exists"
        )));
    }
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM task_dependencies WHERE dependent_task_id=?1",
        [dependent],
        |row| row.get(0),
    )?;
    if count >= MAX_PREREQUISITES {
        return Err(DomainError::Invalid(format!(
            "task {dependent} cannot have more than {MAX_PREREQUISITES} prerequisites"
        )));
    }
    let cyclic: bool = connection.query_row("WITH RECURSIVE reachable(id) AS (SELECT dependent_task_id FROM task_dependencies WHERE prerequisite_task_id=?1 UNION SELECT d.dependent_task_id FROM task_dependencies d JOIN reachable r ON d.prerequisite_task_id=r.id) SELECT EXISTS(SELECT 1 FROM reachable WHERE id=?2)", params![dependent,prerequisite], |row| row.get(0))?;
    if cyclic {
        return Err(DomainError::Invalid(format!(
            "dependency {prerequisite} -> {dependent} would create a cycle"
        )));
    }
    Ok(())
}

impl Store {
    pub fn add_dependency(&mut self, prerequisite: i64, dependent: i64) -> Result<(), DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prerequisite_task =
            query_task(&tx, prerequisite)?.ok_or(DomainError::NotFound("task", prerequisite))?;
        let dependent_task =
            query_task(&tx, dependent)?.ok_or(DomainError::NotFound("task", dependent))?;
        validate_dependency_tasks(&prerequisite_task, &dependent_task)?;
        validate_new_dependency(&tx, prerequisite, dependent)?;
        tx.execute(
            "INSERT INTO task_dependencies(prerequisite_task_id,dependent_task_id) VALUES(?1,?2)",
            params![prerequisite, dependent],
        )
        .map_err(|error| DomainError::Invalid(format!("cannot add dependency: {error}")))?;
        recompute_queued_readiness(&tx, dependent, "dependency_added")?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_dependency(
        &mut self,
        prerequisite: i64,
        dependent: i64,
    ) -> Result<(), DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, dependent)?.ok_or(DomainError::NotFound("task", dependent))?;
        if !dependency_can_change(&task) {
            return Err(DomainError::Invalid(
                "dependencies cannot change after the dependent starts".into(),
            ));
        }
        let changed = tx.execute(
            "DELETE FROM task_dependencies WHERE prerequisite_task_id=?1 AND dependent_task_id=?2",
            params![prerequisite, dependent],
        )?;
        if changed == 0 {
            return Err(DomainError::Invalid("dependency does not exist".into()));
        }
        recompute_queued_readiness(&tx, dependent, "dependency_removed")?;
        tx.commit()?;
        Ok(())
    }

    pub fn all_dependencies(&self) -> Result<BTreeMap<i64, Vec<i64>>, DomainError> {
        let edges = self.connection.prepare("SELECT dependent_task_id,prerequisite_task_id FROM task_dependencies ORDER BY dependent_task_id,prerequisite_task_id")?
            .query_map([], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
        let mut grouped = BTreeMap::new();
        for (dependent, prerequisite) in edges {
            grouped
                .entry(dependent)
                .or_insert_with(Vec::new)
                .push(prerequisite);
        }
        Ok(grouped)
    }
}
