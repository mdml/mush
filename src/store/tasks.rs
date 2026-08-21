use super::*;

fn revision_can_be_prepared(task: &Task) -> bool {
    task.kind == TaskKind::Work
        && task.status == TaskStatus::Pending
        && task.previous_task_id.is_some()
        && task.execution_attempt == 0
        && !matches!(
            task.readiness_status,
            ReadinessStatus::Claimed | ReadinessStatus::Running
        )
}

impl Store {
    pub fn register_project(&self, name: &str, path: &Path) -> Result<Project, DomainError> {
        let path = path.canonicalize().map_err(|error| {
            DomainError::Invalid(format!("project path {}: {error}", path.display()))
        })?;
        self.connection.execute(
            "INSERT INTO projects(name,path) VALUES(?1,?2)",
            params![name, path.to_string_lossy()],
        )?;
        self.project(self.connection.last_insert_rowid())
    }

    pub fn register_agent(
        &self,
        registration: AgentRegistration<'_>,
    ) -> Result<Agent, DomainError> {
        let AgentRegistration {
            project_id,
            name,
            harness,
            model,
            settings,
            checkpoint,
        } = registration;
        serde_json::from_str::<serde_json::Value>(settings)
            .map_err(|error| DomainError::Invalid(format!("settings must be JSON: {error}")))?;
        crate::executor::validate_agent_registration(harness, settings, checkpoint)?;
        self.project(project_id)?;
        self.connection.execute("INSERT INTO agents(project_id,name,harness,model,settings,checkpoint) VALUES(?1,?2,?3,?4,?5,?6)", params![project_id,name,harness,model,settings,checkpoint])?;
        self.agent(self.connection.last_insert_rowid())
    }

    /// Replace a registered agent's settings, revalidated as at registration.
    /// The first M3 gate attempt had no affordance to correct a misregistered
    /// executable path; this is that recovery path.
    pub fn update_agent_settings(
        &self,
        agent_id: i64,
        settings: &str,
    ) -> Result<Agent, DomainError> {
        let agent = self.agent(agent_id)?;
        serde_json::from_str::<serde_json::Value>(settings)
            .map_err(|error| DomainError::Invalid(format!("settings must be JSON: {error}")))?;
        crate::executor::validate_agent_registration(&agent.harness, settings, agent.checkpoint)?;
        self.connection.execute(
            "UPDATE agents SET settings=?2 WHERE id=?1",
            params![agent_id, settings],
        )?;
        self.agent(agent_id)
    }

    pub fn add_work_task(&self, request: WorkTaskRequest<'_>) -> Result<Task, DomainError> {
        let WorkTaskRequest {
            project_id,
            agent_id,
            description,
            parent_task_id,
        } = request;
        let agent = self.agent(agent_id)?;
        if agent.project_id != project_id {
            return Err(DomainError::Invalid(
                "agent is not registered to this project".into(),
            ));
        }
        if let Some(parent_id) = parent_task_id {
            let parent = self.task(parent_id)?;
            if parent.project_id != project_id {
                return Err(DomainError::Invalid(format!(
                    "parent task {parent_id} belongs to another project"
                )));
            }
            if parent.kind != TaskKind::Work {
                return Err(DomainError::Invalid(format!(
                    "task {parent_id} is a checkpoint; only work tasks can delegate or be delegated"
                )));
            }
            if parent.parent_task_id.is_some() {
                return Err(DomainError::Invalid(format!(
                    "task {parent_id} already has a parent; delegation is bounded to one level"
                )));
            }
            if parent.status == TaskStatus::Completed {
                return Err(DomainError::Invalid(format!(
                    "task {parent_id} is completed and cannot receive new children"
                )));
            }
        }
        self.connection.execute("INSERT INTO tasks(project_id,agent_id,kind,status,description,parent_task_id) VALUES(?1,?2,'work','pending',?3,?4)", params![project_id,agent_id,description,parent_task_id])?;
        self.task(self.connection.last_insert_rowid())
    }

    pub fn prepare_revision(
        &mut self,
        task_id: i64,
        description: &str,
        worktree_name: &str,
    ) -> Result<Task, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if !revision_can_be_prepared(&task) {
            return Err(DomainError::Invalid(
                "only an unstarted linked revision can receive checkpoint feedback".into(),
            ));
        }
        let changed = tx.execute(
            "UPDATE tasks SET description=?2,worktree_name=?3 WHERE id=?1 AND kind='work' AND status='pending' AND previous_task_id IS NOT NULL AND execution_attempt=0 AND readiness_status NOT IN ('claimed','running')",
            params![task_id, description, worktree_name],
        )?;
        if changed != 1 {
            return Err(DomainError::Invalid(
                "only an unstarted linked revision can receive checkpoint feedback".into(),
            ));
        }
        tx.commit()?;
        self.task(task_id)
    }

    pub fn complete_work(
        &mut self,
        task_id: i64,
        result: &str,
        evidence: Option<&str>,
    ) -> Result<Task, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if task.kind != TaskKind::Work {
            return Err(DomainError::Invalid(
                "only work tasks can be completed with a result".into(),
            ));
        }
        if task.status == TaskStatus::Completed {
            return Ok(task);
        }
        if task.execution_status == Some(ExecutionStatus::Running) {
            return Err(DomainError::Invalid(format!(
                "task {task_id} is already executing and cannot be completed manually"
            )));
        }
        ensure_children_finished(&tx, task_id)?;
        let evidence = evidence
            .map(str::to_owned)
            .unwrap_or_else(|| format!("## Work result\n\n{result}"));
        tx.execute(
            "UPDATE tasks SET status='completed',result=?2,evidence=?3,execution_status=CASE WHEN execution_status='running' THEN 'interrupted' ELSE execution_status END,execution_pid=NULL WHERE id=?1",
            params![task_id, result, evidence],
        )?;
        record_transition_and_advance(&tx, task_id, "completed")?;
        tx.commit()?;
        self.task(task_id)
    }

    /// Explicitly create the one checkpoint reviewing a work task. Creating it
    /// before the subject completes records declared review intent and leaves
    /// the checkpoint awaiting its subject; creating it again returns the
    /// existing checkpoint.
    pub fn create_checkpoint(&mut self, subject_task_id: i64) -> Result<Task, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let subject = query_task(&tx, subject_task_id)?
            .ok_or(DomainError::NotFound("task", subject_task_id))?;
        if subject.kind != TaskKind::Work {
            return Err(DomainError::Invalid(
                "only a work task can receive a checkpoint".into(),
            ));
        }
        if let Some(existing) = query_checkpoint(&tx, subject_task_id)? {
            return Ok(existing);
        }
        let checkpoint_agent: Option<i64> = tx
            .query_row(
                "SELECT id FROM agents WHERE project_id=?1 AND checkpoint=1",
                [subject.project_id],
                |row| row.get(0),
            )
            .optional()?;
        let checkpoint_agent = checkpoint_agent
            .ok_or_else(|| DomainError::Invalid("project has no checkpoint agent".into()))?;
        let evidence = if subject.status == TaskStatus::Completed {
            subject_evidence(&subject)
        } else {
            AWAITING_SUBJECT_EVIDENCE.to_owned()
        };
        tx.execute("INSERT INTO tasks(project_id,agent_id,kind,status,description,evidence,subject_task_id) VALUES(?1,?2,'checkpoint','pending',?3,?4,?5)", params![subject.project_id,checkpoint_agent,format!("Review work task {subject_task_id}: {}", subject.description),evidence,subject_task_id])?;
        let checkpoint_id = tx.last_insert_rowid();
        tx.commit()?;
        self.task(checkpoint_id)
    }

    /// Guard a checkpoint against its subject before execution: a checkpoint
    /// awaiting a subject that is not completed cannot run, and once the
    /// subject is complete a still-waiting placeholder is replaced with the
    /// subject's real evidence. Replacing only the placeholder keeps the sync
    /// idempotent and preserves review evidence from earlier attempts.
    pub fn sync_checkpoint_with_subject(&self, checkpoint_id: i64) -> Result<Task, DomainError> {
        let checkpoint = self.task(checkpoint_id)?;
        if checkpoint.kind != TaskKind::Checkpoint {
            return Err(DomainError::Invalid(format!(
                "task {checkpoint_id} is not a checkpoint"
            )));
        }
        let subject_id = checkpoint.subject_task_id.expect("checkpoint constraint");
        let subject = self.task(subject_id)?;
        if subject.status != TaskStatus::Completed {
            return Err(DomainError::Invalid(format!(
                "checkpoint {checkpoint_id} is awaiting its subject: task {subject_id} is not completed"
            )));
        }
        if checkpoint.evidence.as_deref() == Some(AWAITING_SUBJECT_EVIDENCE) {
            self.connection.execute(
                "UPDATE tasks SET evidence=?2 WHERE id=?1",
                params![checkpoint_id, subject_evidence(&subject)],
            )?;
        }
        self.task(checkpoint_id)
    }

    pub fn decide_checkpoint(
        &mut self,
        checkpoint_id: i64,
        decision: CheckpointDecision,
        evidence: &str,
    ) -> Result<Option<Task>, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let checkpoint =
            query_task(&tx, checkpoint_id)?.ok_or(DomainError::NotFound("task", checkpoint_id))?;
        if checkpoint.kind != TaskKind::Checkpoint {
            return Err(DomainError::Invalid(
                "decision target is not a checkpoint".into(),
            ));
        }
        if checkpoint.status == TaskStatus::Completed {
            return Err(DomainError::Invalid(
                "checkpoint already has a decision".into(),
            ));
        }
        let subject_id = checkpoint.subject_task_id.expect("checkpoint constraint");
        let subject_status: String = tx.query_row(
            "SELECT status FROM tasks WHERE id=?1",
            [subject_id],
            |row| row.get(0),
        )?;
        if TaskStatus::from_str(&subject_status)? != TaskStatus::Completed {
            return Err(DomainError::Invalid(format!(
                "checkpoint {checkpoint_id} is awaiting its subject: task {subject_id} is not completed"
            )));
        }
        tx.execute(
            "UPDATE tasks SET status='completed',decision=?2,evidence=?3 WHERE id=?1",
            params![checkpoint_id, decision.to_string(), evidence],
        )?;
        record_transition_and_advance(&tx, checkpoint_id, "checkpoint_decided")?;
        let follow_up_id = if decision == CheckpointDecision::NotMet {
            let subject = query_task(&tx, subject_id)?
                .ok_or(DomainError::NotFound("subject task", subject_id))?;
            // Carry the checkpoint's decision and evidence into the follow-up
            // description so the revision does not depend on transcript access.
            let description = format!(
                "{}\n\n## Checkpoint feedback (task {checkpoint_id}, {decision})\n\n{evidence}",
                subject.description
            );
            tx.execute("INSERT INTO tasks(project_id,agent_id,kind,status,description,parent_task_id,previous_task_id) VALUES(?1,?2,'work','pending',?3,?4,?5)", params![subject.project_id,subject.agent_id,description,subject.parent_task_id,subject.id])?;
            Some(tx.last_insert_rowid())
        } else {
            None
        };
        tx.commit()?;
        follow_up_id.map(|id| self.task(id)).transpose()
    }

    pub fn project(&self, id: i64) -> Result<Project, DomainError> {
        self.connection
            .query_row("SELECT id,name,path FROM projects WHERE id=?1", [id], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                })
            })
            .optional()?
            .ok_or(DomainError::NotFound("project", id))
    }

    pub fn agent(&self, id: i64) -> Result<Agent, DomainError> {
        self.connection.query_row("SELECT id,project_id,name,harness,model,settings,checkpoint FROM agents WHERE id=?1", [id], |r| Ok(Agent{id:r.get(0)?,project_id:r.get(1)?,name:r.get(2)?,harness:r.get(3)?,model:r.get(4)?,settings:r.get(5)?,checkpoint:r.get(6)?})).optional()?.ok_or(DomainError::NotFound("agent", id))
    }

    pub fn task(&self, id: i64) -> Result<Task, DomainError> {
        query_task(&self.connection, id)?.ok_or(DomainError::NotFound("task", id))
    }

    pub fn tasks(&self, project_id: Option<i64>) -> Result<Vec<Task>, DomainError> {
        let mut statement = self.connection.prepare(&format!(
            "{} WHERE (?1 IS NULL OR project_id=?1) ORDER BY id",
            TASK_SELECT
        ))?;
        statement
            .query_map([project_id], row_task)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}
