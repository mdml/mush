use super::*;

fn validate_execution_start(
    connection: &Connection,
    task: &Task,
    claimed_runner_id: Option<&str>,
) -> Result<(), DomainError> {
    if task.status != TaskStatus::Pending {
        return Err(DomainError::Invalid(
            "only pending tasks can execute".into(),
        ));
    }
    if task.execution_status == Some(ExecutionStatus::Succeeded)
        && task.kind != TaskKind::Checkpoint
    {
        return Err(DomainError::Invalid(
            "task execution already succeeded".into(),
        ));
    }
    if task.readiness_status == ReadinessStatus::InterventionRequired {
        return Err(DomainError::Invalid(format!(
            "task {} requires intervention; use task recover before running it",
            task.id
        )));
    }
    if task.execution_status == Some(ExecutionStatus::Running) {
        return Err(DomainError::Invalid(format!(
            "task {} is already executing",
            task.id
        )));
    }
    validate_claim_owner(connection, task, claimed_runner_id)?;
    if !prerequisites_satisfied(connection, task.id)? {
        return Err(DomainError::Invalid(
            "task prerequisites are not completed".into(),
        ));
    }
    Ok(())
}

fn validate_claim_owner(
    connection: &Connection,
    task: &Task,
    claimed_runner_id: Option<&str>,
) -> Result<(), DomainError> {
    if task.readiness_status != ReadinessStatus::Claimed {
        return Ok(());
    }
    let generation = queue_generation(connection, task.id)?;
    let owns_claim: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM launch_deliveries WHERE task_id=?1 AND generation=?2 AND state='claimed' AND runner_id=?3)",
        params![task.id, generation, claimed_runner_id],
        |row| row.get(0),
    )?;
    if owns_claim {
        Ok(())
    } else {
        Err(DomainError::Invalid(format!(
            "task {} launch is claimed by another runner",
            task.id
        )))
    }
}

fn consume_launch_delivery(
    connection: &Connection,
    task_id: i64,
    claimed_runner_id: Option<&str>,
) -> Result<(), DomainError> {
    let generation = queue_generation(connection, task_id)?;
    if claimed_runner_id.is_some() {
        connection.execute("UPDATE launch_deliveries SET state='delivered' WHERE task_id=?1 AND generation=?2 AND state IN ('pending','claimed')", params![task_id,generation])?;
    } else {
        // A foreground execution consumes any pending reservation and clears
        // the detached runner identity that no longer owns it.
        connection.execute("UPDATE launch_deliveries SET state='delivered',runner_id=NULL,runner_boot_id=NULL,runner_pid=NULL,claimed_at=NULL WHERE task_id=?1 AND generation=?2 AND state IN ('pending','claimed')", params![task_id,generation])?;
    }
    Ok(())
}

impl Store {
    pub fn begin_execution(&mut self, request: ExecutionStart<'_>) -> Result<Task, DomainError> {
        self.begin_execution_request(ExecutionStart {
            claimed_runner_id: None,
            ..request
        })
    }

    pub fn begin_execution_owned(
        &mut self,
        request: ExecutionStart<'_>,
    ) -> Result<Task, DomainError> {
        self.begin_execution_request(request)
    }

    fn begin_execution_request(
        &mut self,
        request: ExecutionStart<'_>,
    ) -> Result<Task, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, request.task_id)?
            .ok_or(DomainError::NotFound("task", request.task_id))?;
        validate_execution_start(&tx, &task, request.claimed_runner_id)?;
        let persisted_session = task.session_id.as_deref().or(request.session_id);
        let persisted_worktree = task.worktree_name.as_deref().or(request.worktree_name);
        tx.execute(
            "UPDATE tasks SET execution_status='running',execution_attempt=execution_attempt+1,session_id=?2,worktree_name=?3,artifact_dir=?4,execution_boot_id=?5,execution_pid=?6,readiness_status=CASE WHEN readiness_status='unqueued' THEN readiness_status ELSE 'running' END,intervention=NULL WHERE id=?1",
            params![request.task_id,persisted_session,persisted_worktree,request.artifact_dir.to_string_lossy(),request.boot_id,i64::from(std::process::id())],
        )?;
        consume_launch_delivery(&tx, request.task_id, request.claimed_runner_id)?;
        insert_transition(&tx, request.task_id, "running")?;
        if task.readiness_status != ReadinessStatus::Unqueued {
            ensure_launch_invariant(&tx, request.task_id)?;
        }
        tx.commit()?;
        self.task(request.task_id)
    }

    pub fn record_execution_pid(&self, owner: ExecutionOwner, pid: u32) -> Result<(), DomainError> {
        let ExecutionOwner {
            task_id,
            execution_attempt,
        } = owner;
        let changed = self.connection.execute(
            "UPDATE tasks SET execution_pid=?3 WHERE id=?1 AND execution_status='running' AND execution_attempt=?2",
            params![task_id, execution_attempt, i64::from(pid)],
        )?;
        if changed != 1 {
            return Err(DomainError::Invalid(format!(
                "task {task_id} execution ownership changed before harness launch"
            )));
        }
        Ok(())
    }

    pub fn replace_execution_session(
        &mut self,
        owner: ExecutionOwner,
        session_id: Option<&str>,
    ) -> Result<(), DomainError> {
        let ExecutionOwner {
            task_id,
            execution_attempt,
        } = owner;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE tasks SET session_id=?3,execution_status='interrupted',execution_pid=NULL WHERE id=?1 AND status='pending' AND execution_status!='running' AND execution_attempt=?2",
            params![task_id, execution_attempt, session_id],
        )?;
        if changed == 1 {
            insert_transition(&tx, task_id, "execution_session_replaced")?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Persist a harness-assigned session id observed after launch, so an
    /// interrupted run can still resume the same session.
    pub fn record_execution_session(
        &self,
        owner: ExecutionOwner,
        session_id: &str,
    ) -> Result<(), DomainError> {
        let ExecutionOwner {
            task_id,
            execution_attempt,
        } = owner;
        self.connection.execute(
            "UPDATE tasks SET session_id=?3 WHERE id=?1 AND execution_status='running' AND execution_attempt=?2",
            params![task_id, execution_attempt, session_id],
        )?;
        Ok(())
    }

    pub fn interrupt_execution(&mut self, task_id: i64) -> Result<(), DomainError> {
        let execution_attempt = self.task(task_id)?.execution_attempt;
        self.interrupt_execution_owned(ExecutionOwner {
            task_id,
            execution_attempt,
        })
    }

    pub fn interrupt_execution_owned(&mut self, owner: ExecutionOwner) -> Result<(), DomainError> {
        self.interrupt_execution_owned_with_intervention(owner, None)
    }

    pub fn interrupt_execution_owned_with_intervention(
        &mut self,
        owner: ExecutionOwner,
        diagnostic: Option<&str>,
    ) -> Result<(), DomainError> {
        let ExecutionOwner {
            task_id,
            execution_attempt,
        } = owner;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let bounded = diagnostic.map(|value| bounded_text(value, MAX_DIAGNOSTIC_BYTES));
        let changed = tx.execute("UPDATE tasks SET execution_status='interrupted',execution_pid=NULL,readiness_status=CASE WHEN ?3 IS NOT NULL AND readiness_status!='unqueued' THEN 'intervention_required' ELSE readiness_status END,intervention=COALESCE(?3,intervention) WHERE id=?1 AND execution_status='running' AND execution_attempt=?2", params![task_id,execution_attempt,bounded])?;
        if changed == 1 {
            if let Some(diagnostic) = bounded {
                let generation = queue_generation(&tx, task_id)?;
                tx.execute("UPDATE launch_deliveries SET state='intervention_required',diagnostic=?3 WHERE task_id=?1 AND generation=?2 AND state!='intervention_required'", params![task_id,generation,diagnostic])?;
                insert_transition(&tx, task_id, "intervention_required")?;
                ensure_launch_invariant(&tx, task_id)?;
            } else {
                insert_transition(&tx, task_id, "execution_interrupted")?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn finish_checkpoint_execution(
        &mut self,
        owner: ExecutionOwner,
        evidence: &str,
    ) -> Result<Task, DomainError> {
        let ExecutionOwner {
            task_id,
            execution_attempt,
        } = owner;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if task.kind != TaskKind::Checkpoint {
            return Err(DomainError::Invalid(
                "only checkpoint tasks can record review evidence".into(),
            ));
        }
        // A checkpoint decided during its own execution keeps the decision's
        // evidence and readiness; the execution records only that it
        // succeeded. Otherwise the queue's job for this checkpoint is done:
        // its readiness completes so no pass restarts the review, while the
        // pending status says the decision is still owed.
        let changed = if task.status == TaskStatus::Completed {
            tx.execute("UPDATE tasks SET execution_status='succeeded',execution_pid=NULL WHERE id=?1 AND execution_status='running' AND execution_attempt=?2", params![task_id,execution_attempt])?
        } else {
            tx.execute("UPDATE tasks SET execution_status='succeeded',execution_pid=NULL,evidence=?3,readiness_status=CASE WHEN readiness_status='unqueued' THEN readiness_status ELSE 'completed' END WHERE id=?1 AND execution_status='running' AND execution_attempt=?2", params![task_id,execution_attempt,evidence])?
        };
        if changed != 1 {
            return Err(DomainError::Invalid(format!(
                "task {task_id} execution ownership changed before completion"
            )));
        }
        insert_transition(&tx, task_id, "checkpoint_execution_succeeded")?;
        ensure_launch_invariant(&tx, task_id)?;
        tx.commit()?;
        self.task(task_id)
    }

    pub fn finish_work_execution(
        &mut self,
        result: WorkExecutionResult<'_>,
    ) -> Result<Task, DomainError> {
        let WorkExecutionResult {
            owner:
                ExecutionOwner {
                    task_id,
                    execution_attempt,
                },
            result,
            evidence,
        } = result;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if task.kind != TaskKind::Work || task.status != TaskStatus::Pending {
            return Err(DomainError::Invalid(
                "only pending work tasks can finish execution".into(),
            ));
        }
        if task.execution_status != Some(ExecutionStatus::Running)
            || task.execution_attempt != execution_attempt
        {
            return Err(DomainError::Invalid(format!(
                "task {task_id} execution ownership changed before completion"
            )));
        }
        ensure_children_finished(&tx, task_id)?;
        tx.execute("UPDATE tasks SET status='completed',result=?2,evidence=?3,execution_status='succeeded',execution_pid=NULL WHERE id=?1", params![task_id,result,evidence])?;
        record_transition_and_advance(&tx, task_id, "completed")?;
        tx.commit()?;
        self.task(task_id)
    }
}
