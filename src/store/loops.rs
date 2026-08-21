use super::tasks::resolve_adjudicator;
use super::*;

/// The bounded `not_met` package a restart carries into the next attempt's
/// first stage: the prior attempt's stage counterparts, its final report, and
/// the adjudication that cycled the loop.
struct RestartFeedback {
    checkpoint_id: i64,
    evidence: String,
    previous_stage_ids: Vec<i64>,
    final_report: String,
}

fn loop_row(connection: &Connection, loop_id: i64) -> Result<Loop, DomainError> {
    connection
        .query_row(
            "SELECT id,project_id,criteria,adjudicator_agent_id,max_attempts,reuse_worktree,continuation_task_id FROM loops WHERE id=?1",
            [loop_id],
            |row| {
                Ok(Loop {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    criteria: row.get(2)?,
                    adjudicator_agent_id: row.get(3)?,
                    max_attempts: row.get(4)?,
                    reuse_worktree: row.get(5)?,
                    continuation_task_id: row.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or(DomainError::NotFound("loop", loop_id))
}

fn stage_rows(connection: &Connection, loop_id: i64) -> Result<Vec<LoopStage>, DomainError> {
    connection
        .prepare("SELECT position,agent_id,description FROM loop_stages WHERE loop_id=?1 ORDER BY position")?
        .query_map([loop_id], |row| {
            Ok(LoopStage {
                position: row.get(0)?,
                agent_id: row.get(1)?,
                description: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn member_ids(connection: &Connection, loop_id: i64, kind: &str) -> Result<Vec<i64>, DomainError> {
    connection
        .prepare("SELECT id FROM tasks WHERE loop_id=?1 AND kind=?2 ORDER BY id")?
        .query_map(params![loop_id, kind], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn agent_is_executable(connection: &Connection, agent_id: i64) -> Result<bool, DomainError> {
    let harness: String = connection.query_row(
        "SELECT harness FROM agents WHERE id=?1",
        [agent_id],
        |row| row.get(0),
    )?;
    Ok(crate::executor::harness_is_executable(&harness))
}

/// Materialize one execution of the loop's path: every stage as an immutable
/// work task chained by ordinary dependency edges, then the attempt's
/// checkpoint with the final stage as its subject. Each created task whose
/// agent Mush can execute is queued, so already-declared iteration advances
/// without the declaring conversation; a non-executable assignment stays
/// unqueued for manual stepping. Returns the created task ids, checkpoint
/// last.
fn materialize_attempt(
    connection: &Connection,
    loop_id: i64,
    feedback: Option<&RestartFeedback>,
) -> Result<Vec<i64>, DomainError> {
    let declared = loop_row(connection, loop_id)?;
    let stages = stage_rows(connection, loop_id)?;
    let attempt = member_ids(connection, loop_id, "checkpoint")?.len() as i64 + 1;
    let mut created = Vec::new();
    for (index, stage) in stages.iter().enumerate() {
        let description = match (index, feedback) {
            (0, Some(feedback)) => format!(
                "{}\n\n## Prior attempt not_met (checkpoint {})\n\n### Final stage report (task {})\n\n{}\n\n### Adjudication evidence\n\n{}",
                stage.description,
                feedback.checkpoint_id,
                feedback
                    .previous_stage_ids
                    .last()
                    .expect("a prior attempt has stages"),
                feedback.final_report,
                feedback.evidence
            ),
            _ => stage.description.clone(),
        };
        let previous =
            feedback.and_then(|feedback| feedback.previous_stage_ids.get(index).copied());
        let worktree: Option<String> = match (declared.reuse_worktree, previous) {
            (true, Some(previous)) => connection.query_row(
                "SELECT worktree_name FROM tasks WHERE id=?1",
                [previous],
                |row| row.get(0),
            )?,
            _ => None,
        };
        connection.execute(
            "INSERT INTO tasks(project_id,agent_id,kind,status,description,previous_task_id,loop_id,worktree_name) VALUES(?1,?2,'work','pending',?3,?4,?5,?6)",
            params![declared.project_id, stage.agent_id, description, previous, loop_id, worktree],
        )?;
        let id = connection.last_insert_rowid();
        if let Some(prerequisite) = created.last() {
            connection.execute(
                "INSERT INTO task_dependencies(prerequisite_task_id,dependent_task_id) VALUES(?1,?2)",
                params![prerequisite, id],
            )?;
        }
        created.push(id);
    }
    let final_stage = *created.last().expect("a loop declares at least one stage");
    connection.execute(
        "INSERT INTO tasks(project_id,agent_id,kind,status,description,evidence,criteria,subject_task_id,previous_task_id,loop_id) VALUES(?1,?2,'checkpoint','pending',?3,?4,?5,?6,?7,?8)",
        params![
            declared.project_id,
            declared.adjudicator_agent_id,
            format!(
                "Adjudicate loop {loop_id} attempt {attempt}: does the result of task {final_stage} meet the declared criteria?"
            ),
            AWAITING_SUBJECT_EVIDENCE,
            declared.criteria,
            final_stage,
            feedback.map(|feedback| feedback.checkpoint_id),
            loop_id
        ],
    )?;
    created.push(connection.last_insert_rowid());
    for id in &created {
        let agent_id: i64 =
            connection.query_row("SELECT agent_id FROM tasks WHERE id=?1", [id], |row| {
                row.get(0)
            })?;
        if agent_is_executable(connection, agent_id)? {
            queue_pending_task(connection, *id)?;
        }
    }
    Ok(created)
}

/// The loop policy's `not_met` consequence: materialize and queue the next
/// path attempt while the declared budget permits one, and nothing otherwise.
/// The caller owns the transaction and has already recorded the decision.
pub(super) fn materialize_next_attempt_within_budget(
    connection: &Connection,
    loop_id: i64,
    checkpoint_id: i64,
    evidence: &str,
) -> Result<Vec<i64>, DomainError> {
    let declared = loop_row(connection, loop_id)?;
    let attempts_used = member_ids(connection, loop_id, "checkpoint")?.len() as i64;
    if attempts_used >= declared.max_attempts {
        return Ok(Vec::new());
    }
    let stage_count = stage_rows(connection, loop_id)?.len();
    let work_members = member_ids(connection, loop_id, "work")?;
    let previous_stage_ids = work_members[work_members.len() - stage_count..].to_vec();
    let final_stage = *previous_stage_ids
        .last()
        .expect("a loop declares at least one stage");
    let final_report: Option<String> = connection.query_row(
        "SELECT result FROM tasks WHERE id=?1",
        [final_stage],
        |row| row.get(0),
    )?;
    materialize_attempt(
        connection,
        loop_id,
        Some(&RestartFeedback {
            checkpoint_id,
            evidence: evidence.to_owned(),
            previous_stage_ids,
            final_report: final_report.unwrap_or_default(),
        }),
    )
}

fn report_for(connection: &Connection, loop_id: i64) -> Result<LoopReport, DomainError> {
    let declaration = loop_row(connection, loop_id)?;
    let stages = stage_rows(connection, loop_id)?;
    let work_members = member_ids(connection, loop_id, "work")?;
    let checkpoints = connection
        .prepare(
            "SELECT id,decision FROM tasks WHERE loop_id=?1 AND kind='checkpoint' ORDER BY id",
        )?
        .query_map([loop_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let attempts = checkpoints
        .iter()
        .enumerate()
        .map(|(index, (checkpoint_task_id, decision))| {
            let start = index * stages.len();
            Ok(LoopAttempt {
                number: index as i64 + 1,
                stage_task_ids: work_members[start..(start + stages.len()).min(work_members.len())]
                    .to_vec(),
                checkpoint_task_id: *checkpoint_task_id,
                decision: decision
                    .as_deref()
                    .map(CheckpointDecision::from_str)
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<LoopAttempt>, DomainError>>()?;
    let status = loop_status(&declaration, &attempts);
    let next_action = next_action(connection, &declaration, &attempts, status)?;
    Ok(LoopReport {
        remaining_attempts: declaration.max_attempts - attempts.len() as i64,
        declaration,
        stages,
        attempts,
        status,
        next_action,
    })
}

fn loop_status(declaration: &Loop, attempts: &[LoopAttempt]) -> LoopStatus {
    if attempts
        .iter()
        .any(|attempt| attempt.decision == Some(CheckpointDecision::Met))
    {
        return LoopStatus::Satisfied;
    }
    if attempts
        .iter()
        .any(|attempt| attempt.decision == Some(CheckpointDecision::Blocked))
    {
        return LoopStatus::Blocked;
    }
    let exhausted = attempts.len() as i64 >= declaration.max_attempts
        && attempts
            .last()
            .is_some_and(|attempt| attempt.decision == Some(CheckpointDecision::NotMet));
    if exhausted {
        LoopStatus::Exhausted
    } else {
        LoopStatus::InProgress
    }
}

/// The one thing a human needs next, restated from durable rows alone.
fn next_action(
    connection: &Connection,
    declaration: &Loop,
    attempts: &[LoopAttempt],
    status: LoopStatus,
) -> Result<String, DomainError> {
    let current = attempts.last().expect("a loop materializes its attempts");
    Ok(match status {
        LoopStatus::Satisfied => match declaration.continuation_task_id {
            Some(task) => format!("met; continuation task {task} is eligible"),
            None => "met; the loop is satisfied and declares no continuation".to_owned(),
        },
        LoopStatus::Blocked => format!(
            "checkpoint {} recorded blocked; the loop stops for fresh judgment",
            current.checkpoint_task_id
        ),
        LoopStatus::Exhausted => format!(
            "not_met on the final permitted attempt; budget of {} exhausted, the loop stops for fresh judgment",
            declaration.max_attempts
        ),
        LoopStatus::InProgress => {
            for (index, stage_task) in current.stage_task_ids.iter().enumerate() {
                let status: String = connection.query_row(
                    "SELECT status FROM tasks WHERE id=?1",
                    [stage_task],
                    |row| row.get(0),
                )?;
                if TaskStatus::from_str(&status)? != TaskStatus::Completed {
                    return Ok(format!(
                        "attempt {}: run stage {} (task {stage_task})",
                        current.number,
                        index + 1
                    ));
                }
            }
            format!(
                "attempt {}: adjudicate checkpoint {}",
                current.number, current.checkpoint_task_id
            )
        }
    })
}

impl Store {
    /// Declare a bounded loop and materialize its first attempt. The
    /// declaration is immutable; only the success continuation may be added
    /// later, once, through [`Store::declare_loop_continuation`].
    pub fn declare_loop(
        &mut self,
        declaration: LoopDeclaration<'_>,
    ) -> Result<LoopReport, DomainError> {
        let LoopDeclaration {
            project_id,
            stages,
            criteria,
            adjudicator_agent_id,
            max_attempts,
            reuse_worktree,
        } = declaration;
        if stages.is_empty() {
            return Err(DomainError::Invalid(
                "a loop declares at least one work stage".into(),
            ));
        }
        if criteria.trim().is_empty() {
            return Err(DomainError::Invalid(
                "checkpoint criteria are required".into(),
            ));
        }
        if max_attempts < 1 {
            return Err(DomainError::Invalid(
                "max_attempts counts the initial attempt and must be at least 1".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [project_id],
            |row| row.get(0),
        )?;
        if !project_exists {
            return Err(DomainError::NotFound("project", project_id));
        }
        for stage in stages {
            if stage.description.trim().is_empty() {
                return Err(DomainError::Invalid(
                    "every loop stage requires a description".into(),
                ));
            }
            let agent_project: Option<i64> = tx
                .query_row(
                    "SELECT project_id FROM agents WHERE id=?1",
                    [stage.agent_id],
                    |row| row.get(0),
                )
                .optional()?;
            match agent_project {
                None => return Err(DomainError::NotFound("agent", stage.agent_id)),
                Some(agent_project) if agent_project != project_id => {
                    return Err(DomainError::Invalid(format!(
                        "stage agent {} is not registered to this project",
                        stage.agent_id
                    )));
                }
                Some(_) => {}
            }
        }
        let adjudicator = resolve_adjudicator(&tx, project_id, adjudicator_agent_id)?;
        tx.execute(
            "INSERT INTO loops(project_id,criteria,adjudicator_agent_id,max_attempts,reuse_worktree) VALUES(?1,?2,?3,?4,?5)",
            params![project_id, criteria, adjudicator, max_attempts, reuse_worktree],
        )?;
        let loop_id = tx.last_insert_rowid();
        for (index, stage) in stages.iter().enumerate() {
            tx.execute(
                "INSERT INTO loop_stages(loop_id,position,agent_id,description) VALUES(?1,?2,?3,?4)",
                params![loop_id, index as i64 + 1, stage.agent_id, stage.description],
            )?;
        }
        materialize_attempt(&tx, loop_id, None)?;
        tx.commit()?;
        self.loop_report(loop_id)
    }

    /// Declare which work task the loop's `met` makes eligible. The
    /// continuation is declared once, before the named task begins, and is
    /// gated on the loop without an explicit dependency edge.
    pub fn declare_loop_continuation(
        &mut self,
        loop_id: i64,
        task_id: i64,
    ) -> Result<LoopReport, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let declared = loop_row(&tx, loop_id)?;
        if declared.continuation_task_id == Some(task_id) {
            drop(tx);
            return self.loop_report(loop_id);
        }
        if let Some(existing) = declared.continuation_task_id {
            return Err(DomainError::Invalid(format!(
                "loop {loop_id} already declares task {existing} as its success continuation"
            )));
        }
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if task.kind != TaskKind::Work {
            return Err(DomainError::Invalid(
                "only a work task can be a loop's success continuation".into(),
            ));
        }
        if task.project_id != declared.project_id {
            return Err(DomainError::Invalid(
                "the continuation must belong to the loop's project".into(),
            ));
        }
        if task.loop_id.is_some() {
            return Err(DomainError::Invalid(
                "a loop member cannot be a success continuation".into(),
            ));
        }
        let started = task.status == TaskStatus::Completed
            || task.execution_attempt != 0
            || matches!(
                task.readiness_status,
                ReadinessStatus::Claimed
                    | ReadinessStatus::Running
                    | ReadinessStatus::Completed
                    | ReadinessStatus::InterventionRequired
            );
        if started {
            return Err(DomainError::Invalid(
                "the success continuation is declared before the dependent begins".into(),
            ));
        }
        tx.execute(
            "UPDATE loops SET continuation_task_id=?2 WHERE id=?1",
            params![loop_id, task_id],
        )?;
        recompute_queued_readiness(&tx, task_id, "loop_continuation_declared")?;
        tx.commit()?;
        self.loop_report(loop_id)
    }

    /// Restate one loop from durable rows: declaration, attempts, decisions,
    /// remaining budget, and the next eligible action.
    pub fn loop_report(&self, loop_id: i64) -> Result<LoopReport, DomainError> {
        report_for(&self.connection, loop_id)
    }

    /// Every declared loop, optionally scoped to one project.
    pub fn loops(&self, project_id: Option<i64>) -> Result<Vec<LoopReport>, DomainError> {
        let ids = self
            .connection
            .prepare("SELECT id FROM loops WHERE (?1 IS NULL OR project_id=?1) ORDER BY id")?
            .query_map([project_id], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| report_for(&self.connection, id))
            .collect()
    }

    /// The reports a loop stage receives at launch: the bounded result of
    /// each same-loop prerequisite stage, in task order.
    pub fn stage_input_reports(&self, task_id: i64) -> Result<Vec<(i64, String)>, DomainError> {
        self.connection
            .prepare(
                "SELECT p.id, COALESCE(p.result,'') FROM task_dependencies d
                 JOIN tasks t ON t.id=d.dependent_task_id
                 JOIN tasks p ON p.id=d.prerequisite_task_id
                 WHERE d.dependent_task_id=?1 AND t.loop_id IS NOT NULL AND p.loop_id=t.loop_id
                 ORDER BY p.id",
            )?
            .query_map([task_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}
