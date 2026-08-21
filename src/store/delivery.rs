use super::*;

/// A queued checkpoint needs an adjudicator the runner can actually launch. A
/// human adjudicator's checkpoint would otherwise become a ready delivery no
/// execution can ever claim, which parks as an intervention recovery cannot
/// fix; it is decided through the TUI or `checkpoint decide` without queueing.
fn ensure_adjudicator_is_executable(
    connection: &Connection,
    task: &Task,
) -> Result<(), DomainError> {
    let harness: Option<String> = connection
        .query_row(
            "SELECT harness FROM agents WHERE id=?1",
            [task.agent_id],
            |row| row.get(0),
        )
        .optional()?;
    if harness
        .as_deref()
        .is_some_and(crate::executor::harness_is_executable)
    {
        return Ok(());
    }
    Err(DomainError::Invalid(format!(
        "checkpoint {} is assigned to an adjudicator Mush cannot execute; decide it with checkpoint decide or the TUI instead of queueing it",
        task.id
    )))
}

fn launch_is_claimable(connection: &Connection, task: &Task) -> Result<bool, DomainError> {
    if task.status != TaskStatus::Pending {
        return Ok(false);
    }
    if task.readiness_status != ReadinessStatus::Ready {
        return Ok(false);
    }
    prerequisites_satisfied(connection, task.id)
}

impl Store {
    pub fn queue(&mut self, task_id: i64) -> Result<Task, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if task.status != TaskStatus::Pending {
            return Err(DomainError::Invalid(
                "only pending tasks can be queued".into(),
            ));
        }
        if task.kind == TaskKind::Checkpoint {
            ensure_adjudicator_is_executable(&tx, &task)?;
        }
        if task.readiness_status != ReadinessStatus::Unqueued {
            return Ok(task);
        }
        if task.execution_status == Some(ExecutionStatus::Running) {
            return Err(DomainError::Invalid(format!(
                "task {task_id} is already executing and cannot be queued"
            )));
        }
        queue_pending_task(&tx, task_id)?;
        tx.commit()?;
        self.task(task_id)
    }

    pub fn claim_launch(&mut self, claim: LaunchClaim<'_>) -> Result<bool, DomainError> {
        let LaunchClaim {
            task_id,
            runner_id,
            runner_boot_id,
            runner_pid,
        } = claim;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if !launch_is_claimable(&tx, &task)? {
            return Ok(false);
        }
        let generation = queue_generation(&tx, task_id)?;
        let changed = tx.execute("UPDATE launch_deliveries SET state='claimed',attempts=attempts+1,claimed_at=unixepoch(),runner_id=?3,runner_boot_id=?4,runner_pid=?5 WHERE task_id=?1 AND generation=?2 AND state='pending'", params![task_id,generation,runner_id,runner_boot_id,i64::from(runner_pid)])?;
        if changed == 1 {
            tx.execute("UPDATE tasks SET readiness_status='claimed' WHERE id=?1 AND readiness_status='ready'", [task_id])?;
            insert_transition(&tx, task_id, "launch_claimed")?;
            ensure_launch_invariant(&tx, task_id)?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }

    /// Queued tasks that are ready and carry an unclaimed delivery, in task
    /// order. This is the runner's queue.
    pub fn pending_launches(&self, limit: usize) -> Result<Vec<i64>, DomainError> {
        self.connection
            .prepare(
                "SELECT d.task_id FROM launch_deliveries d JOIN tasks t ON t.id=d.task_id AND t.queue_generation=d.generation
                 WHERE d.state='pending' AND t.status='pending' AND t.readiness_status='ready' ORDER BY d.task_id LIMIT ?1",
            )?
            .query_map([limit as i64], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Queued tasks whose execution lock is currently held, which is exactly
    /// the set of live executions. The runner's concurrency bound counts these.
    pub fn live_executions(&self) -> Result<Vec<i64>, DomainError> {
        let owned = self
            .connection
            .prepare(
                "SELECT id FROM tasks WHERE status='pending' AND readiness_status IN ('claimed','running') ORDER BY id",
            )?
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        owned
            .into_iter()
            .filter_map(|id| match crate::lock::task_is_live(&self.path, id) {
                Ok(true) => Some(Ok(id)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    /// What the runner surface reports: how much work is queued, owned, and
    /// parked. The serve-lock holder is a filesystem fact and is read separately.
    pub fn runner_counts(&self) -> Result<RunnerCounts, DomainError> {
        let count = |readiness: &str| -> Result<i64, DomainError> {
            self.connection
                .query_row(
                    "SELECT COUNT(*) FROM tasks WHERE status='pending' AND readiness_status=?1",
                    [readiness],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        };
        Ok(RunnerCounts {
            pending: count("ready")?,
            claimed: count("claimed")?,
            running: count("running")?,
            parked: count("intervention_required")?,
        })
    }
}
