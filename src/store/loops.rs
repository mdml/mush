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

/// A loop declaration loaded with its ordered stage path.
struct LoopContext {
    declared: Loop,
    stages: Vec<LoopStage>,
}

fn load_loop(connection: &Connection, loop_id: i64) -> Result<LoopContext, DomainError> {
    let declared = connection
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
        .ok_or(DomainError::NotFound("loop", loop_id))?;
    let stages = connection
        .prepare(
            "SELECT position,agent_id,description FROM loop_stages WHERE loop_id=?1 ORDER BY position",
        )?
        .query_map([loop_id], |row| {
            Ok(LoopStage {
                position: row.get(0)?,
                agent_id: row.get(1)?,
                description: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LoopContext { declared, stages })
}

fn member_ids(
    context: &LoopContext,
    connection: &Connection,
    kind: &str,
) -> Result<Vec<i64>, DomainError> {
    connection
        .prepare("SELECT id FROM tasks WHERE loop_id=?1 AND kind=?2 ORDER BY id")?
        .query_map(params![context.declared.id, kind], |row| row.get(0))?
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

/// The declared stage description, extended on a restart's first stage with
/// the bounded feedback packet. No agent authors another agent's instructions.
fn stage_description(
    stage: &LoopStage,
    index: usize,
    feedback: Option<&RestartFeedback>,
) -> String {
    match (index, feedback) {
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
    }
}

/// Materialize the path's work tasks, chaining consecutive stages with
/// ordinary dependency edges and linking each to its stage counterpart.
fn materialize_stages(
    connection: &Connection,
    context: &LoopContext,
    feedback: Option<&RestartFeedback>,
) -> Result<Vec<i64>, DomainError> {
    let mut created = Vec::new();
    for (index, stage) in context.stages.iter().enumerate() {
        let previous =
            feedback.and_then(|feedback| feedback.previous_stage_ids.get(index).copied());
        let worktree = counterpart_worktree(connection, context, previous)?;
        connection.execute(
            "INSERT INTO tasks(project_id,agent_id,kind,status,description,previous_task_id,loop_id,worktree_name) VALUES(?1,?2,'work','pending',?3,?4,?5,?6)",
            params![
                context.declared.project_id,
                stage.agent_id,
                stage_description(stage, index, feedback),
                previous,
                context.declared.id,
                worktree
            ],
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
    Ok(created)
}

/// The worktree a restarted stage may reuse when the declared policy permits
/// that execution choice.
fn counterpart_worktree(
    connection: &Connection,
    context: &LoopContext,
    previous: Option<i64>,
) -> Result<Option<String>, DomainError> {
    match (context.declared.reuse_worktree, previous) {
        (true, Some(previous)) => connection
            .query_row(
                "SELECT worktree_name FROM tasks WHERE id=?1",
                [previous],
                |row| row.get(0),
            )
            .map_err(Into::into),
        _ => Ok(None),
    }
}

fn materialize_checkpoint(
    connection: &Connection,
    context: &LoopContext,
    final_stage: i64,
    feedback: Option<&RestartFeedback>,
) -> Result<i64, DomainError> {
    let attempt = member_ids(context, connection, "checkpoint")?.len() as i64 + 1;
    connection.execute(
        "INSERT INTO tasks(project_id,agent_id,kind,status,description,evidence,criteria,subject_task_id,previous_task_id,loop_id) VALUES(?1,?2,'checkpoint','pending',?3,?4,?5,?6,?7,?8)",
        params![
            context.declared.project_id,
            context.declared.adjudicator_agent_id,
            format!(
                "Adjudicate loop {} attempt {attempt}: does the result of task {final_stage} meet the declared criteria?",
                context.declared.id
            ),
            AWAITING_SUBJECT_EVIDENCE,
            context.declared.criteria,
            final_stage,
            feedback.map(|feedback| feedback.checkpoint_id),
            context.declared.id
        ],
    )?;
    Ok(connection.last_insert_rowid())
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
    context: &LoopContext,
    feedback: Option<&RestartFeedback>,
) -> Result<Vec<i64>, DomainError> {
    let mut created = materialize_stages(connection, context, feedback)?;
    let final_stage = *created.last().expect("a loop declares at least one stage");
    created.push(materialize_checkpoint(
        connection,
        context,
        final_stage,
        feedback,
    )?);
    queue_executable_members(connection, &created)?;
    Ok(created)
}

fn queue_executable_members(connection: &Connection, created: &[i64]) -> Result<(), DomainError> {
    for id in created {
        let agent_id: i64 =
            connection.query_row("SELECT agent_id FROM tasks WHERE id=?1", [id], |row| {
                row.get(0)
            })?;
        if agent_is_executable(connection, agent_id)? {
            queue_pending_task(connection, *id)?;
        }
    }
    Ok(())
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
    let context = load_loop(connection, loop_id)?;
    let attempts_used = member_ids(&context, connection, "checkpoint")?.len() as i64;
    if attempts_used >= context.declared.max_attempts {
        return Ok(Vec::new());
    }
    let work_members = member_ids(&context, connection, "work")?;
    let previous_stage_ids = work_members[work_members.len() - context.stages.len()..].to_vec();
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
        &context,
        Some(&RestartFeedback {
            checkpoint_id,
            evidence: evidence.to_owned(),
            previous_stage_ids,
            final_report: final_report.unwrap_or_default(),
        }),
    )
}

fn report_for(connection: &Connection, loop_id: i64) -> Result<LoopReport, DomainError> {
    let context = load_loop(connection, loop_id)?;
    let attempts = materialized_attempts(connection, &context)?;
    let status = loop_status(&context.declared, &attempts);
    let next_action = next_action(connection, &context.declared, &attempts, status)?;
    let LoopContext { declared, stages } = context;
    Ok(LoopReport {
        remaining_attempts: declared.max_attempts - attempts.len() as i64,
        declaration: declared,
        stages,
        attempts,
        status,
        next_action,
    })
}

fn materialized_attempts(
    connection: &Connection,
    context: &LoopContext,
) -> Result<Vec<LoopAttempt>, DomainError> {
    let work_members = member_ids(context, connection, "work")?;
    let checkpoints = connection
        .prepare(
            "SELECT id,decision,evidence FROM tasks WHERE loop_id=?1 AND kind='checkpoint' ORDER BY id",
        )?
        .query_map([context.declared.id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    checkpoints
        .iter()
        .enumerate()
        .map(|(index, (checkpoint_task_id, decision, evidence))| {
            let start = index * context.stages.len();
            let end = (start + context.stages.len()).min(work_members.len());
            let decision = decision
                .as_deref()
                .map(CheckpointDecision::from_str)
                .transpose()?;
            Ok(LoopAttempt {
                number: index as i64 + 1,
                stage_task_ids: work_members[start..end].to_vec(),
                checkpoint_task_id: *checkpoint_task_id,
                decision,
                // Only a decided checkpoint's evidence is adjudication
                // evidence. An undecided one still carries its subject's
                // inherited evidence or the awaiting-subject placeholder,
                // which would misread as a verdict.
                evidence: decision.and(evidence.clone()),
            })
        })
        .collect()
}

fn loop_status(declaration: &Loop, attempts: &[LoopAttempt]) -> LoopStatus {
    let decided = |decision| {
        attempts
            .iter()
            .any(|attempt| attempt.decision == Some(decision))
    };
    if decided(CheckpointDecision::Met) {
        return LoopStatus::Satisfied;
    }
    if decided(CheckpointDecision::Blocked) {
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
        LoopStatus::InProgress => in_progress_action(connection, current)?,
    })
}

/// Within the current attempt: the first unfinished stage, or the owed
/// adjudication once every stage has completed.
fn in_progress_action(
    connection: &Connection,
    current: &LoopAttempt,
) -> Result<String, DomainError> {
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
    Ok(format!(
        "attempt {}: adjudicate checkpoint {}",
        current.number, current.checkpoint_task_id
    ))
}

/// Reject a declaration whose shape cannot form a bounded loop.
fn validate_declaration(
    connection: &Connection,
    declaration: &LoopDeclaration<'_>,
) -> Result<(), DomainError> {
    if declaration.stages.is_empty() {
        return Err(DomainError::Invalid(
            "a loop declares at least one work stage".into(),
        ));
    }
    if declaration.criteria.trim().is_empty() {
        return Err(DomainError::Invalid(
            "checkpoint criteria are required".into(),
        ));
    }
    if declaration.max_attempts < 1 {
        return Err(DomainError::Invalid(
            "max_attempts counts the initial attempt and must be at least 1".into(),
        ));
    }
    let project_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
        [declaration.project_id],
        |row| row.get(0),
    )?;
    if !project_exists {
        return Err(DomainError::NotFound("project", declaration.project_id));
    }
    for stage in declaration.stages {
        validate_stage(connection, declaration.project_id, stage)?;
    }
    Ok(())
}

fn validate_stage(
    connection: &Connection,
    project_id: i64,
    stage: &StageSpec<'_>,
) -> Result<(), DomainError> {
    if stage.description.trim().is_empty() {
        return Err(DomainError::Invalid(
            "every loop stage requires a description".into(),
        ));
    }
    let agent_project: Option<i64> = connection
        .query_row(
            "SELECT project_id FROM agents WHERE id=?1",
            [stage.agent_id],
            |row| row.get(0),
        )
        .optional()?;
    match agent_project {
        None => Err(DomainError::NotFound("agent", stage.agent_id)),
        Some(agent_project) if agent_project != project_id => Err(DomainError::Invalid(format!(
            "stage agent {} is not registered to this project",
            stage.agent_id
        ))),
        Some(_) => Ok(()),
    }
}

/// Reject a continuation declaration the loop's single-task reading cannot
/// honor: only an unstarted, non-member work task in the loop's project may
/// be gated on the loop.
fn validate_continuation_task(declared: &Loop, task: &Task) -> Result<(), DomainError> {
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
    Ok(())
}

impl Store {
    /// Declare a bounded loop and materialize its first attempt. The
    /// declaration is immutable; only the success continuation may be added
    /// later, once, through [`Store::declare_loop_continuation`].
    pub fn declare_loop(
        &mut self,
        declaration: LoopDeclaration<'_>,
    ) -> Result<LoopReport, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_declaration(&tx, &declaration)?;
        let adjudicator = resolve_adjudicator(
            &tx,
            declaration.project_id,
            declaration.adjudicator_agent_id,
        )?;
        tx.execute(
            "INSERT INTO loops(project_id,criteria,adjudicator_agent_id,max_attempts,reuse_worktree) VALUES(?1,?2,?3,?4,?5)",
            params![
                declaration.project_id,
                declaration.criteria,
                adjudicator,
                declaration.max_attempts,
                declaration.reuse_worktree
            ],
        )?;
        let loop_id = tx.last_insert_rowid();
        for (index, stage) in declaration.stages.iter().enumerate() {
            tx.execute(
                "INSERT INTO loop_stages(loop_id,position,agent_id,description) VALUES(?1,?2,?3,?4)",
                params![loop_id, index as i64 + 1, stage.agent_id, stage.description],
            )?;
        }
        let context = load_loop(&tx, loop_id)?;
        materialize_attempt(&tx, &context, None)?;
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
        let declared = load_loop(&tx, loop_id)?.declared;
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
        validate_continuation_task(&declared, &task)?;
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
