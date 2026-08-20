use super::*;

/// A pending task that records an owner: an execution, a claimed delivery, or a
/// delivered one. Whether that owner still exists is a lock question, not a row
/// question.
struct OwnedRow {
    id: i64,
    readiness: String,
    generation: i64,
    attempts: i64,
}

fn owned_rows(connection: &Connection) -> Result<Vec<OwnedRow>, DomainError> {
    connection
        .prepare(
            "SELECT t.id,t.readiness_status,t.queue_generation,COALESCE(d.attempts,0)
             FROM tasks t LEFT JOIN launch_deliveries d ON d.task_id=t.id AND d.generation=t.queue_generation
             WHERE t.status='pending'
               AND (t.execution_status='running' OR d.state IN ('claimed','delivered')) ORDER BY t.id",
        )?
        .query_map([], |row| {
            Ok(OwnedRow {
                id: row.get(0)?,
                readiness: row.get(1)?,
                generation: row.get(2)?,
                attempts: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Return an abandoned row to the queue, re-deriving its readiness
/// from the graph rather than assuming it: reconciliation must never hand a
/// task a launch delivery its prerequisites do not yet justify.
fn requeue_abandoned_row(connection: &Connection, row: &OwnedRow) -> Result<(), DomainError> {
    let ready = prerequisites_satisfied(connection, row.id)?;
    connection.execute(
        "UPDATE tasks SET execution_status=CASE WHEN execution_status='running' THEN 'interrupted' ELSE execution_status END,execution_pid=NULL,readiness_status=?2,intervention=NULL WHERE id=?1 AND status='pending'",
        params![row.id, if ready { "ready" } else { "blocked" }],
    )?;
    if ready {
        connection.execute(
            "UPDATE launch_deliveries SET state='pending',claimed_at=NULL,runner_id=NULL,runner_boot_id=NULL,runner_pid=NULL WHERE task_id=?1 AND generation=?2",
            params![row.id, row.generation],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO launch_deliveries(task_id,generation,state) VALUES(?1,?2,'pending')",
            params![row.id, row.generation],
        )?;
    } else {
        connection.execute(
            "DELETE FROM launch_deliveries WHERE task_id=?1 AND generation=?2",
            params![row.id, row.generation],
        )?;
    }
    insert_transition(connection, row.id, "execution_requeued")?;
    ensure_launch_invariant(connection, row.id)
}

fn park_abandoned_row(connection: &Connection, row: &OwnedRow) -> Result<(), DomainError> {
    let diagnostic = format!(
        "execution ownership disappeared after {} launch attempts; correct the cause and run task recover",
        row.attempts
    );
    connection.execute(
        "UPDATE tasks SET execution_status=CASE WHEN execution_status='running' THEN 'interrupted' ELSE execution_status END,execution_pid=NULL,readiness_status='intervention_required',intervention=?2 WHERE id=?1 AND status='pending'",
        params![row.id, diagnostic],
    )?;
    connection.execute(
        "UPDATE launch_deliveries SET state='intervention_required',diagnostic=?3 WHERE task_id=?1 AND generation=?2",
        params![row.id, row.generation, diagnostic],
    )?;
    insert_transition(connection, row.id, "intervention_required")?;
    ensure_launch_invariant(connection, row.id)
}

fn recovery_candidates(connection: &Connection) -> Result<Vec<(i64, i64)>, DomainError> {
    connection.prepare("SELECT t.id,t.project_id FROM tasks t WHERE t.kind='work' AND t.status='pending' AND (t.readiness_status='intervention_required' OR t.execution_status IN ('running','interrupted')) ORDER BY t.id")?
        .query_map([], |row| Ok((row.get::<_,i64>(0)?, row.get::<_,i64>(1)?)))?
        .collect::<Result<Vec<_>,_>>()
        .map_err(Into::into)
}

fn recover_task(connection: &Connection, id: i64) -> Result<(), DomainError> {
    let task = query_task(connection, id)?.expect("recovery candidate");
    if task.readiness_status == ReadinessStatus::Unqueued {
        connection.execute(
            "UPDATE tasks SET execution_status='interrupted',execution_pid=NULL,intervention=NULL WHERE id=?1",
            [id],
        )?;
        return insert_transition(connection, id, "execution_recovered");
    }
    let ready = prerequisites_satisfied(connection, id)?;
    connection.execute(
        "UPDATE tasks SET execution_status=CASE WHEN execution_status='running' THEN 'interrupted' ELSE execution_status END,execution_pid=NULL,readiness_status=?2,intervention=NULL,queue_generation=queue_generation+1 WHERE id=?1",
        params![id, if ready { "ready" } else { "blocked" }],
    )?;
    if ready {
        let generation = queue_generation(connection, id)?;
        connection.execute(
            "INSERT INTO launch_deliveries(task_id,generation,state) VALUES(?1,?2,'pending')",
            params![id, generation],
        )?;
    }
    insert_transition(connection, id, "intervention_recovered")?;
    ensure_launch_invariant(connection, id)
}

enum AbandonedAction {
    Interrupt,
    Park,
    Requeue,
}

fn abandoned_action(row: &OwnedRow) -> AbandonedAction {
    if row.readiness == "unqueued" {
        AbandonedAction::Interrupt
    } else if row.attempts >= MAX_LAUNCH_ATTEMPTS {
        AbandonedAction::Park
    } else {
        AbandonedAction::Requeue
    }
}

fn reconcile_owned_row(connection: &Connection, row: &OwnedRow) -> Result<(), DomainError> {
    match abandoned_action(row) {
        AbandonedAction::Interrupt => {
            connection.execute(
                "UPDATE tasks SET execution_status='interrupted',execution_pid=NULL WHERE id=?1 AND execution_status='running'",
                [row.id],
            )?;
            insert_transition(connection, row.id, "execution_interrupted")
        }
        AbandonedAction::Park => park_abandoned_row(connection, row),
        AbandonedAction::Requeue => requeue_abandoned_row(connection, row),
    }
}

impl Store {
    pub fn recover_launches(
        &mut self,
        task_ids: &[i64],
        project_id: Option<i64>,
    ) -> Result<Vec<i64>, DomainError> {
        if task_ids.is_empty() && project_id.is_none() {
            return Err(DomainError::Invalid(
                "task recover requires one or more task ids or --project".into(),
            ));
        }
        // A task whose execution lock is held has a live execution and is never
        // taken over, whatever its rows say. Probe before opening the write
        // transaction so the filesystem check never holds the database.
        let mut recovering = Vec::new();
        for (id, task_project) in recovery_candidates(&self.connection)? {
            let selected = task_ids.contains(&id) || project_id == Some(task_project);
            if selected && !crate::lock::task_is_live(&self.path, id)? {
                recovering.push(id);
            }
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in &recovering {
            recover_task(&tx, *id)?;
        }
        tx.commit()?;
        Ok(recovering)
    }

    /// Return rows whose owner is gone to a state a runner can act on. Only
    /// `runner tick` and `runner serve` call this.
    ///
    /// Liveness is the task's execution lock and nothing else: a task whose lock
    /// can be taken has no live execution, and a task whose lock is held is
    /// never touched. There is no grace period, no missing-observation clock,
    /// and no boot identity involved, so a killed `serve` leaves its orphaned
    /// executions alone while a crashed one is requeued immediately.
    ///
    /// Deciding what to change happens without the database write lock, and the
    /// immediate transaction opens only once a row actually needs a write; a
    /// healthy running graph costs reads alone and never contends with the
    /// executions it is observing.
    pub fn reconcile(&mut self) -> Result<(), DomainError> {
        let abandoned = self.abandoned_rows()?;
        if abandoned.is_empty() {
            return Ok(());
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-read under the write lock: another process may have advanced any
        // of these rows between the read-only decision and this transaction.
        for row in owned_rows(&tx)? {
            if !abandoned.iter().any(|candidate| candidate.id == row.id) {
                continue;
            }
            reconcile_owned_row(&tx, &row)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn abandoned_rows(&self) -> Result<Vec<OwnedRow>, DomainError> {
        owned_rows(&self.connection)?
            .into_iter()
            .filter_map(|row| match crate::lock::task_is_live(&self.path, row.id) {
                Ok(true) => None,
                Ok(false) => Some(Ok(row)),
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub fn require_intervention(
        &mut self,
        task_id: i64,
        diagnostic: &str,
    ) -> Result<(), DomainError> {
        let bounded = bounded_text(diagnostic, MAX_DIAGNOSTIC_BYTES);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if task.status != TaskStatus::Pending
            || task.execution_status == Some(ExecutionStatus::Running)
        {
            let generation = queue_generation(&tx, task_id)?;
            tx.execute(
                "UPDATE launch_deliveries SET diagnostic=?3 WHERE task_id=?1 AND generation=?2",
                params![task_id, generation, bounded],
            )?;
            insert_transition(&tx, task_id, "runner_failure_ignored")?;
            tx.commit()?;
            return Ok(());
        }
        let generation = queue_generation(&tx, task_id)?;
        tx.execute("UPDATE launch_deliveries SET state='intervention_required',diagnostic=?3 WHERE task_id=?1 AND generation=?2", params![task_id,generation,bounded])?;
        tx.execute(
            "UPDATE tasks SET readiness_status='intervention_required',intervention=?2 WHERE id=?1",
            params![task_id, bounded],
        )?;
        insert_transition(&tx, task_id, "intervention_required")?;
        ensure_launch_invariant(&tx, task_id)?;
        tx.commit()?;
        Ok(())
    }
}
