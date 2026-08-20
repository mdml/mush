use crate::domain::{
    Agent, CheckpointDecision, DomainError, ExecutionStatus, Project, ReadinessStatus,
    RunnerCounts, Task, TaskKind, TaskObservation, TaskStatus,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
};

pub struct Store {
    connection: Connection,
    path: PathBuf,
}

/// The schema this binary writes and reads.
const SCHEMA_VERSION: i64 = 11;

/// The oldest binary permitted to open a version 11 database. A binary older
/// than the version a database records refuses rather than opening, so the
/// pre-M4 hazard where an old binary silently re-stamps `user_version` cannot
/// recur.
const MIN_BINARY_VERSION: &str = "0.1.0";

/// The last shipped schema, and therefore the only older input the single
/// migration to version 11 accepts. Versions 4 through 10 were never released.
const LAST_SHIPPED_SCHEMA_VERSION: i64 = 3;

impl Store {
    pub fn open(path: &Path) -> Result<Self, DomainError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        // The busy timeout lets nested invocations (a delegating agent running
        // `mush` inside an execution) share one WAL database without spurious
        // "database is locked" failures.
        connection.execute_batch(
            "PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;",
        )?;
        let store = Self {
            connection,
            path: path.to_path_buf(),
        };
        store.prepare_schema()?;
        Ok(store)
    }

    /// The database this store is open on. Lock files live beside it.
    pub fn database_path(&self) -> &Path {
        &self.path
    }

    /// Bring the database to [`SCHEMA_VERSION`], or refuse to open it.
    ///
    /// Migration happens only while holding the database's serve lock, so a
    /// starting runner and a concurrent CLI command cannot migrate one database
    /// at once, and only after a successful backup, because the step is one-way.
    fn prepare_schema(&self) -> Result<(), DomainError> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(DomainError::Invalid(format!(
                "database schema version {version} is newer than supported version {SCHEMA_VERSION}"
            )));
        }
        if version == SCHEMA_VERSION {
            return self.refuse_an_outdated_binary();
        }
        if version > LAST_SHIPPED_SCHEMA_VERSION {
            return Err(DomainError::Invalid(format!(
                "database schema version {version} was never shipped and cannot be migrated; restore the {}.pre-v{SCHEMA_VERSION}.bak backup",
                self.path.display()
            )));
        }
        let Some(mut lock) =
            crate::lock::FileLock::try_acquire(&crate::lock::serve_lock_path(&self.path))?
        else {
            let holder = crate::lock::serve_lock_holder(&self.path)?
                .unwrap_or_else(|| "another process".to_owned());
            return Err(DomainError::Invalid(format!(
                "database schema version {version} must be migrated to {SCHEMA_VERSION}, but {holder} holds the serve lock; stop it and retry"
            )));
        };
        lock.describe(&format!(
            "schema migration to version {SCHEMA_VERSION} (pid {})",
            std::process::id()
        ))?;
        self.back_up_before_migration()?;
        self.migrate_to_current_version()?;
        Ok(())
    }

    /// Copy the database beside itself before a one-way step, and refuse to
    /// migrate if the copy fails. A database with no tables has nothing to lose
    /// and needs no backup.
    fn back_up_before_migration(&self) -> Result<(), DomainError> {
        let populated: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='tasks')",
            [],
            |row| row.get(0),
        )?;
        if !populated {
            return Ok(());
        }
        // Fold the write-ahead log into the database file so the copy is a
        // complete database rather than a torn prefix of one.
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let mut backup = self.path.clone().into_os_string();
        backup.push(format!(".pre-v{SCHEMA_VERSION}.bak"));
        let backup = PathBuf::from(backup);
        std::fs::copy(&self.path, &backup).map_err(|error| {
            DomainError::Invalid(format!(
                "refusing to migrate: cannot back up {} to {}: {error}",
                self.path.display(),
                backup.display()
            ))
        })?;
        Ok(())
    }

    /// Refuse to open a database that records a newer minimum binary version
    /// than this binary.
    fn refuse_an_outdated_binary(&self) -> Result<(), DomainError> {
        let required: Option<String> = self
            .connection
            .query_row(
                "SELECT min_binary_version FROM schema_meta WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(required) = required else {
            return Ok(());
        };
        let current = env!("CARGO_PKG_VERSION");
        if release_order(&required) > release_order(current) {
            return Err(DomainError::Invalid(format!(
                "database requires mush {required} or newer, but this binary is {current}; upgrade mush rather than opening it"
            )));
        }
        Ok(())
    }

    fn migrate_to_current_version(&self) -> Result<(), DomainError> {
        // The schema batch is idempotent and represents the complete current
        // schema, so run it once and stamp only after every statement and
        // backfill succeeds.
        self.connection.execute_batch(
            "DROP TRIGGER IF EXISTS dependency_invariants_on_insert;
             DROP TRIGGER IF EXISTS dependency_no_late_delete;",
        )?;
        self.connection.execute_batch(
            &format!("CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, path TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS agents (
                id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
                name TEXT NOT NULL, harness TEXT NOT NULL, model TEXT NOT NULL, settings TEXT NOT NULL,
                checkpoint INTEGER NOT NULL DEFAULT 0 CHECK(checkpoint IN (0,1)),
                UNIQUE(project_id, name), UNIQUE(project_id, harness, model, settings)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS one_checkpoint_agent ON agents(project_id) WHERE checkpoint = 1;
            CREATE TABLE IF NOT EXISTS tasks (
                id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
                agent_id INTEGER REFERENCES agents(id), kind TEXT NOT NULL CHECK(kind IN ('work','checkpoint')),
                status TEXT NOT NULL CHECK(status IN ('pending','completed')), description TEXT NOT NULL,
                result TEXT, evidence TEXT, decision TEXT CHECK(decision IN ('accepted','blocked','revision_requested')),
                parent_task_id INTEGER REFERENCES tasks(id), previous_task_id INTEGER REFERENCES tasks(id),
                subject_task_id INTEGER REFERENCES tasks(id),
                execution_status TEXT CHECK(execution_status IN ('running','succeeded','interrupted')),
                execution_attempt INTEGER NOT NULL DEFAULT 0,
                session_id TEXT, worktree_name TEXT, artifact_dir TEXT,
                execution_boot_id TEXT, execution_pid INTEGER,
                readiness_status TEXT NOT NULL DEFAULT 'unqueued' CHECK(readiness_status IN ('unqueued','blocked','ready','claimed','running','completed','intervention_required')),
                queue_generation INTEGER NOT NULL DEFAULT 0, intervention TEXT,
                last_transition_at INTEGER NOT NULL DEFAULT 0,
                CHECK((kind = 'work' AND subject_task_id IS NULL AND decision IS NULL) OR
                      (kind = 'checkpoint' AND subject_task_id IS NOT NULL))
            );
            CREATE UNIQUE INDEX IF NOT EXISTS one_checkpoint_per_subject ON tasks(subject_task_id) WHERE kind = 'checkpoint';
            CREATE TABLE IF NOT EXISTS task_dependencies (
                prerequisite_task_id INTEGER NOT NULL REFERENCES tasks(id),
                dependent_task_id INTEGER NOT NULL REFERENCES tasks(id),
                PRIMARY KEY(prerequisite_task_id, dependent_task_id),
                CHECK(prerequisite_task_id != dependent_task_id)
            );
            CREATE TABLE IF NOT EXISTS task_transitions (
                id INTEGER PRIMARY KEY AUTOINCREMENT, task_id INTEGER NOT NULL REFERENCES tasks(id),
                kind TEXT NOT NULL, status TEXT NOT NULL, execution_status TEXT,
                readiness_status TEXT NOT NULL, committed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS schema_meta (
                id INTEGER PRIMARY KEY CHECK(id = 1), min_binary_version TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS launch_deliveries (
                task_id INTEGER NOT NULL REFERENCES tasks(id), generation INTEGER NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('pending','claimed','delivered','intervention_required')),
                attempts INTEGER NOT NULL DEFAULT 0, claimed_at INTEGER, runner_id TEXT,
                runner_boot_id TEXT, runner_pid INTEGER, diagnostic TEXT,
                PRIMARY KEY(task_id,generation)
            );
            CREATE TRIGGER IF NOT EXISTS dependency_invariants_on_insert BEFORE INSERT ON task_dependencies BEGIN
                SELECT RAISE(ABORT, 'dependencies are work-task only') WHERE
                    (SELECT kind FROM tasks WHERE id=NEW.prerequisite_task_id) != 'work' OR
                    (SELECT kind FROM tasks WHERE id=NEW.dependent_task_id) != 'work';
                SELECT RAISE(ABORT, 'dependency tasks must share a project') WHERE
                    (SELECT project_id FROM tasks WHERE id=NEW.prerequisite_task_id) != (SELECT project_id FROM tasks WHERE id=NEW.dependent_task_id);
                SELECT RAISE(ABORT, 'cannot change dependencies after dependent starts') WHERE
                    (SELECT status FROM tasks WHERE id=NEW.dependent_task_id) = 'completed' OR
                    (SELECT execution_attempt FROM tasks WHERE id=NEW.dependent_task_id) != 0 OR
                    (SELECT readiness_status FROM tasks WHERE id=NEW.dependent_task_id) IN ('claimed','running','completed','intervention_required');
                SELECT RAISE(ABORT, 'task prerequisite limit is {MAX_PREREQUISITES}') WHERE
                    (SELECT COUNT(*) FROM task_dependencies WHERE dependent_task_id=NEW.dependent_task_id) >= {MAX_PREREQUISITES};
                SELECT RAISE(ABORT, 'dependency would create a cycle') WHERE EXISTS(
                    WITH RECURSIVE reachable(id) AS (
                        SELECT dependent_task_id FROM task_dependencies WHERE prerequisite_task_id=NEW.dependent_task_id
                        UNION SELECT d.dependent_task_id FROM task_dependencies d JOIN reachable r ON d.prerequisite_task_id=r.id
                    ) SELECT 1 FROM reachable WHERE id=NEW.prerequisite_task_id
                );
            END;
            CREATE TRIGGER IF NOT EXISTS dependency_no_late_delete BEFORE DELETE ON task_dependencies BEGIN
                SELECT RAISE(ABORT, 'cannot change dependencies after dependent starts') WHERE
                    (SELECT status FROM tasks WHERE id=OLD.dependent_task_id) = 'completed' OR
                    (SELECT execution_attempt FROM tasks WHERE id=OLD.dependent_task_id) != 0 OR
                    (SELECT readiness_status FROM tasks WHERE id=OLD.dependent_task_id) IN ('claimed','running','completed','intervention_required');
            END;
            CREATE TRIGGER IF NOT EXISTS readiness_status_values_on_insert
            BEFORE INSERT ON tasks
            WHEN NEW.readiness_status NOT IN ('unqueued','blocked','ready','claimed','running','completed','intervention_required')
            BEGIN
                SELECT RAISE(ABORT, 'invalid readiness status');
            END;
            CREATE TRIGGER IF NOT EXISTS readiness_status_values_on_update
            BEFORE UPDATE OF readiness_status ON tasks
            WHEN NEW.readiness_status NOT IN ('unqueued','blocked','ready','claimed','running','completed','intervention_required')
            BEGIN
                SELECT RAISE(ABORT, 'invalid readiness status');
            END;
            CREATE TRIGGER IF NOT EXISTS bounded_delegation_on_insert
            BEFORE INSERT ON tasks WHEN NEW.parent_task_id IS NOT NULL
            BEGIN
                SELECT CASE
                    WHEN NEW.kind != 'work' THEN RAISE(ABORT, 'only work tasks may have a parent')
                    WHEN (SELECT kind FROM tasks WHERE id = NEW.parent_task_id) != 'work' THEN RAISE(ABORT, 'a checkpoint cannot be a parent')
                    WHEN (SELECT parent_task_id FROM tasks WHERE id = NEW.parent_task_id) IS NOT NULL THEN RAISE(ABORT, 'delegation is bounded to one level')
                END;
            END;
            CREATE TRIGGER IF NOT EXISTS bounded_delegation_on_update
            BEFORE UPDATE OF parent_task_id ON tasks WHEN NEW.parent_task_id IS NOT NULL
            BEGIN
                SELECT CASE
                    WHEN NEW.kind != 'work' THEN RAISE(ABORT, 'only work tasks may have a parent')
                    WHEN (SELECT kind FROM tasks WHERE id = NEW.parent_task_id) != 'work' THEN RAISE(ABORT, 'a checkpoint cannot be a parent')
                    WHEN (SELECT parent_task_id FROM tasks WHERE id = NEW.parent_task_id) IS NOT NULL THEN RAISE(ABORT, 'delegation is bounded to one level')
                END;
            END;
            CREATE TRIGGER IF NOT EXISTS parent_completion_requires_finished_children
            BEFORE UPDATE OF status ON tasks WHEN NEW.status = 'completed' AND OLD.status != 'completed'
            BEGIN
                SELECT RAISE(ABORT, 'a parent task cannot complete while direct children are unfinished or running')
                WHERE EXISTS(SELECT 1 FROM tasks WHERE parent_task_id = NEW.id
                             AND (status != 'completed' OR execution_status = 'running'));
            END;
            CREATE TRIGGER IF NOT EXISTS no_new_child_under_completed_parent_on_insert
            BEFORE INSERT ON tasks WHEN NEW.parent_task_id IS NOT NULL
            BEGIN
                SELECT RAISE(ABORT, 'cannot create a child under a completed parent')
                WHERE (SELECT status FROM tasks WHERE id = NEW.parent_task_id) = 'completed';
            END;
            CREATE TRIGGER IF NOT EXISTS no_new_child_under_completed_parent_on_update
            BEFORE UPDATE OF parent_task_id ON tasks WHEN NEW.parent_task_id IS NOT NULL
            BEGIN
                SELECT RAISE(ABORT, 'cannot create a child under a completed parent')
                WHERE (SELECT status FROM tasks WHERE id = NEW.parent_task_id) = 'completed';
            END;
            ")
        )?;
        // A version 3 `tasks` table predates every execution and readiness
        // column, so add whichever the database is missing before stamping.
        let columns = self
            .connection
            .prepare("PRAGMA table_info(tasks)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        for (name, definition) in [
            (
                "execution_status",
                "TEXT CHECK(execution_status IN ('running','succeeded','interrupted'))",
            ),
            ("execution_attempt", "INTEGER NOT NULL DEFAULT 0"),
            ("session_id", "TEXT"),
            ("worktree_name", "TEXT"),
            ("artifact_dir", "TEXT"),
            ("execution_boot_id", "TEXT"),
            ("execution_pid", "INTEGER"),
            ("readiness_status", "TEXT NOT NULL DEFAULT 'unqueued'"),
            ("queue_generation", "INTEGER NOT NULL DEFAULT 0"),
            ("intervention", "TEXT"),
            ("last_transition_at", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !columns.iter().any(|column| column == name) {
                self.connection.execute(
                    &format!("ALTER TABLE tasks ADD COLUMN {name} {definition}"),
                    [],
                )?;
            }
        }
        self.connection.execute(
            "UPDATE tasks SET last_transition_at=unixepoch() WHERE last_transition_at=0",
            [],
        )?;
        self.connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS task_transitions_task_id ON task_transitions(task_id);
             CREATE INDEX IF NOT EXISTS pending_launch_deliveries ON launch_deliveries(state) WHERE state!='delivered';",
        )?;
        self.connection.execute(
            "INSERT INTO schema_meta(id,min_binary_version) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET min_binary_version=excluded.min_binary_version",
            [MIN_BINARY_VERSION],
        )?;
        self.connection
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

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
        project_id: i64,
        name: &str,
        harness: &str,
        model: &str,
        settings: &str,
        checkpoint: bool,
    ) -> Result<Agent, DomainError> {
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

    pub fn add_work_task(
        &self,
        project_id: i64,
        agent_id: i64,
        description: &str,
        parent_task_id: Option<i64>,
    ) -> Result<Task, DomainError> {
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
        if task.kind != TaskKind::Work
            || task.status != TaskStatus::Pending
            || task.previous_task_id.is_none()
            || task.execution_attempt != 0
            || matches!(
                task.readiness_status,
                ReadinessStatus::Claimed | ReadinessStatus::Running
            )
        {
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
        insert_transition(&tx, checkpoint_id, "checkpoint_decided")?;
        let follow_up_id = if decision == CheckpointDecision::RevisionRequested {
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

    pub fn begin_execution(
        &mut self,
        task_id: i64,
        session_id: Option<&str>,
        worktree_name: Option<&str>,
        artifact_dir: &Path,
        boot_id: &str,
    ) -> Result<Task, DomainError> {
        self.begin_execution_owned(
            task_id,
            session_id,
            worktree_name,
            artifact_dir,
            boot_id,
            None,
        )
    }

    pub fn begin_execution_owned(
        &mut self,
        task_id: i64,
        session_id: Option<&str>,
        worktree_name: Option<&str>,
        artifact_dir: &Path,
        boot_id: &str,
        claimed_runner_id: Option<&str>,
    ) -> Result<Task, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
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
                "task {task_id} requires intervention; use task recover before running it"
            )));
        }
        if task.execution_status == Some(ExecutionStatus::Running) {
            return Err(DomainError::Invalid(format!(
                "task {task_id} is already executing"
            )));
        }
        if task.readiness_status == ReadinessStatus::Claimed {
            let generation = queue_generation(&tx, task_id)?;
            let owns_claim: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM launch_deliveries WHERE task_id=?1 AND generation=?2 AND state='claimed' AND runner_id=?3)",
                params![task_id, generation, claimed_runner_id],
                |row| row.get(0),
            )?;
            if !owns_claim {
                return Err(DomainError::Invalid(format!(
                    "task {task_id} launch is claimed by another runner"
                )));
            }
        }
        if !prerequisites_satisfied(&tx, task_id)? {
            return Err(DomainError::Invalid(
                "task prerequisites are not completed".into(),
            ));
        }
        let persisted_session = task.session_id.as_deref().or(session_id);
        let persisted_worktree = task.worktree_name.as_deref().or(worktree_name);
        tx.execute(
            "UPDATE tasks SET execution_status='running',execution_attempt=execution_attempt+1,session_id=?2,worktree_name=?3,artifact_dir=?4,execution_boot_id=?5,execution_pid=?6,readiness_status=CASE WHEN readiness_status='unqueued' THEN readiness_status ELSE 'running' END,intervention=NULL WHERE id=?1",
            params![task_id,persisted_session,persisted_worktree,artifact_dir.to_string_lossy(),boot_id,i64::from(std::process::id())],
        )?;
        let generation = queue_generation(&tx, task_id)?;
        if claimed_runner_id.is_some() {
            tx.execute("UPDATE launch_deliveries SET state='delivered' WHERE task_id=?1 AND generation=?2 AND state IN ('pending','claimed')", params![task_id,generation])?;
        } else {
            // A foreground execution consumes any pending reservation. The
            // reserved pid belongs to a runner that no longer owns this
            // delivery and must not make a later foreground crash look like a
            // detached-runner failure that is safe to relaunch automatically.
            tx.execute("UPDATE launch_deliveries SET state='delivered',runner_id=NULL,runner_boot_id=NULL,runner_pid=NULL,claimed_at=NULL WHERE task_id=?1 AND generation=?2 AND state IN ('pending','claimed')", params![task_id,generation])?;
        }
        insert_transition(&tx, task_id, "running")?;
        if task.readiness_status != ReadinessStatus::Unqueued {
            ensure_launch_invariant(&tx, task_id)?;
        }
        tx.commit()?;
        self.task(task_id)
    }

    pub fn record_execution_pid(
        &self,
        task_id: i64,
        execution_attempt: i64,
        pid: u32,
    ) -> Result<(), DomainError> {
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
        task_id: i64,
        execution_attempt: i64,
        session_id: Option<&str>,
    ) -> Result<(), DomainError> {
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
        task_id: i64,
        execution_attempt: i64,
        session_id: &str,
    ) -> Result<(), DomainError> {
        self.connection.execute(
            "UPDATE tasks SET session_id=?3 WHERE id=?1 AND execution_status='running' AND execution_attempt=?2",
            params![task_id, execution_attempt, session_id],
        )?;
        Ok(())
    }

    pub fn interrupt_execution(&mut self, task_id: i64) -> Result<(), DomainError> {
        let execution_attempt = self.task(task_id)?.execution_attempt;
        self.interrupt_execution_owned(task_id, execution_attempt)
    }

    pub fn interrupt_execution_owned(
        &mut self,
        task_id: i64,
        execution_attempt: i64,
    ) -> Result<(), DomainError> {
        self.interrupt_execution_owned_with_intervention(task_id, execution_attempt, None)
    }

    pub fn interrupt_execution_owned_with_intervention(
        &mut self,
        task_id: i64,
        execution_attempt: i64,
        diagnostic: Option<&str>,
    ) -> Result<(), DomainError> {
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
        task_id: i64,
        execution_attempt: i64,
        evidence: &str,
    ) -> Result<Task, DomainError> {
        let task = self.task(task_id)?;
        if task.kind != TaskKind::Checkpoint || task.status != TaskStatus::Pending {
            return Err(DomainError::Invalid(
                "only pending checkpoint tasks can record review evidence".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE tasks SET execution_status='succeeded',execution_pid=NULL,evidence=?3 WHERE id=?1 AND execution_status='running' AND execution_attempt=?2", params![task_id,execution_attempt,evidence])?;
        if changed != 1 {
            return Err(DomainError::Invalid(format!(
                "task {task_id} execution ownership changed before completion"
            )));
        }
        insert_transition(&tx, task_id, "checkpoint_execution_succeeded")?;
        tx.commit()?;
        self.task(task_id)
    }

    pub fn finish_work_execution(
        &mut self,
        task_id: i64,
        execution_attempt: i64,
        result: &str,
        evidence: &str,
    ) -> Result<Task, DomainError> {
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

    pub fn add_dependency(&mut self, prerequisite: i64, dependent: i64) -> Result<(), DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prerequisite_task =
            query_task(&tx, prerequisite)?.ok_or(DomainError::NotFound("task", prerequisite))?;
        let dependent_task =
            query_task(&tx, dependent)?.ok_or(DomainError::NotFound("task", dependent))?;
        if prerequisite == dependent {
            return Err(DomainError::Invalid(
                "a task cannot depend on itself".into(),
            ));
        }
        if prerequisite_task.kind != TaskKind::Work || dependent_task.kind != TaskKind::Work {
            return Err(DomainError::Invalid(
                "this slice supports dependencies between work tasks only".into(),
            ));
        }
        if prerequisite_task.project_id != dependent_task.project_id {
            return Err(DomainError::Invalid(
                "dependency tasks must belong to the same project".into(),
            ));
        }
        if dependent_task.execution_attempt != 0
            || dependent_task.status == TaskStatus::Completed
            || matches!(
                dependent_task.readiness_status,
                ReadinessStatus::Claimed
                    | ReadinessStatus::Running
                    | ReadinessStatus::Completed
                    | ReadinessStatus::InterventionRequired
            )
        {
            return Err(DomainError::Invalid(
                "dependencies cannot change after the dependent starts".into(),
            ));
        }
        let duplicate: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM task_dependencies WHERE prerequisite_task_id=?1 AND dependent_task_id=?2)", params![prerequisite,dependent], |row| row.get(0))?;
        if duplicate {
            return Err(DomainError::Invalid(format!(
                "dependency {prerequisite} -> {dependent} already exists"
            )));
        }
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM task_dependencies WHERE dependent_task_id=?1",
            [dependent],
            |row| row.get(0),
        )?;
        if count >= MAX_PREREQUISITES {
            return Err(DomainError::Invalid(format!(
                "task {dependent} cannot have more than {MAX_PREREQUISITES} prerequisites"
            )));
        }
        let cyclic: bool = tx.query_row("WITH RECURSIVE reachable(id) AS (SELECT dependent_task_id FROM task_dependencies WHERE prerequisite_task_id=?1 UNION SELECT d.dependent_task_id FROM task_dependencies d JOIN reachable r ON d.prerequisite_task_id=r.id) SELECT EXISTS(SELECT 1 FROM reachable WHERE id=?2)", params![dependent,prerequisite], |row| row.get(0))?;
        if cyclic {
            return Err(DomainError::Invalid(format!(
                "dependency {prerequisite} -> {dependent} would create a cycle"
            )));
        }
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
        if task.execution_attempt != 0
            || task.status == TaskStatus::Completed
            || matches!(
                task.readiness_status,
                ReadinessStatus::Claimed
                    | ReadinessStatus::Running
                    | ReadinessStatus::Completed
                    | ReadinessStatus::InterventionRequired
            )
        {
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

    pub fn queue(&mut self, task_id: i64) -> Result<Task, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if task.kind != TaskKind::Work || task.status != TaskStatus::Pending {
            return Err(DomainError::Invalid(
                "only pending work tasks can be queued".into(),
            ));
        }
        if task.readiness_status != ReadinessStatus::Unqueued {
            return Ok(task);
        }
        if task.execution_status == Some(ExecutionStatus::Running) {
            return Err(DomainError::Invalid(format!(
                "task {task_id} is already executing and cannot be queued"
            )));
        }
        let ready = prerequisites_satisfied(&tx, task_id)?;
        let state = if ready { "ready" } else { "blocked" };
        tx.execute("UPDATE tasks SET queue_generation=queue_generation+1,readiness_status=?2,intervention=NULL WHERE id=?1", params![task_id,state])?;
        let generation = queue_generation(&tx, task_id)?;
        if ready {
            tx.execute("INSERT OR IGNORE INTO launch_deliveries(task_id,generation,state) VALUES(?1,?2,'pending')", params![task_id,generation])?;
        }
        insert_transition(&tx, task_id, "queued")?;
        ensure_launch_invariant(&tx, task_id)?;
        tx.commit()?;
        self.task(task_id)
    }

    pub fn claim_launch(
        &mut self,
        task_id: i64,
        runner_id: &str,
        runner_boot_id: &str,
        runner_pid: u32,
    ) -> Result<bool, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = query_task(&tx, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
        if task.status != TaskStatus::Pending
            || task.readiness_status != ReadinessStatus::Ready
            || !prerequisites_satisfied(&tx, task_id)?
        {
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
        let candidates = self.connection.prepare("SELECT t.id,t.project_id FROM tasks t WHERE t.kind='work' AND t.status='pending' AND (t.readiness_status='intervention_required' OR t.execution_status IN ('running','interrupted')) ORDER BY t.id")?
            .query_map([], |row| Ok((row.get::<_,i64>(0)?, row.get::<_,i64>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
        let mut recovering = Vec::new();
        for (id, task_project) in candidates {
            if (task_ids.contains(&id) || project_id == Some(task_project))
                && !crate::lock::task_is_live(&self.path, id)?
            {
                recovering.push(id);
            }
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in &recovering {
            let task = query_task(&tx, *id)?.expect("recovery candidate");
            if task.readiness_status == ReadinessStatus::Unqueued {
                tx.execute(
                    "UPDATE tasks SET execution_status='interrupted',execution_pid=NULL,intervention=NULL WHERE id=?1",
                    [id],
                )?;
                insert_transition(&tx, *id, "execution_recovered")?;
                continue;
            }
            // Readiness is re-derived from the graph rather than assumed:
            // recovery must never hand a task a launch delivery its
            // prerequisites do not yet justify.
            let ready = prerequisites_satisfied(&tx, *id)?;
            tx.execute(
                "UPDATE tasks SET execution_status=CASE WHEN execution_status='running' THEN 'interrupted' ELSE execution_status END,execution_pid=NULL,readiness_status=?2,intervention=NULL,queue_generation=queue_generation+1 WHERE id=?1",
                params![id, if ready { "ready" } else { "blocked" }],
            )?;
            if ready {
                let generation = queue_generation(&tx, *id)?;
                tx.execute(
                    "INSERT INTO launch_deliveries(task_id,generation,state) VALUES(?1,?2,'pending')",
                    params![id, generation],
                )?;
            }
            insert_transition(&tx, *id, "intervention_recovered")?;
            ensure_launch_invariant(&tx, *id)?;
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
            if row.readiness == "unqueued" {
                // A foreground `task run` that died. It owns no delivery, so
                // recording the interruption is the whole reconciliation.
                tx.execute(
                    "UPDATE tasks SET execution_status='interrupted',execution_pid=NULL WHERE id=?1 AND execution_status='running'",
                    [row.id],
                )?;
                insert_transition(&tx, row.id, "execution_interrupted")?;
            } else if row.attempts >= MAX_LAUNCH_ATTEMPTS {
                park_abandoned_row(&tx, &row)?;
            } else {
                requeue_abandoned_row(&tx, &row)?;
            }
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
        let mut remaining = MAX_OBSERVATION_TEXT_BYTES;
        let mut elided = false;
        let task_count = tasks.len();
        for (index, task) in tasks.iter_mut().enumerate() {
            let reserved = (task_count - index - 1) * ELIDED_FIELD_MARKER.len();
            let mut available = remaining.saturating_sub(reserved);
            task.description = bounded_required(
                &task.description,
                MAX_FIELD_BYTES,
                &mut available,
                &mut elided,
            );
            task.result = bounded_optional(
                task.result.as_deref(),
                MAX_FIELD_BYTES,
                &mut available,
                &mut elided,
            );
            task.evidence = bounded_optional(
                task.evidence.as_deref(),
                MAX_FIELD_BYTES,
                &mut available,
                &mut elided,
            );
            task.intervention = bounded_optional(
                task.intervention.as_deref(),
                MAX_DIAGNOSTIC_BYTES,
                &mut available,
                &mut elided,
            );
            task.session_id = bounded_optional(
                task.session_id.as_deref(),
                MAX_IDENTIFIER_BYTES,
                &mut available,
                &mut elided,
            );
            task.worktree_name = bounded_optional(
                task.worktree_name.as_deref(),
                MAX_IDENTIFIER_BYTES,
                &mut available,
                &mut elided,
            );
            task.artifact_dir = bounded_optional(
                task.artifact_dir.as_deref(),
                MAX_PATH_BYTES,
                &mut available,
                &mut elided,
            );
            task.execution_boot_id = bounded_optional(
                task.execution_boot_id.as_deref(),
                MAX_IDENTIFIER_BYTES,
                &mut available,
                &mut elided,
            );
            remaining = available + reserved;
        }
        let terminal_count = tasks
            .iter()
            .filter(|t| {
                matches!(
                    t.readiness_status,
                    ReadinessStatus::Completed | ReadinessStatus::InterventionRequired
                ) || t.status == TaskStatus::Completed
            })
            .count();
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
            elided,
        })
    }
}

/// Order two `major.minor.patch` versions. Unparseable components sort as zero,
/// so a malformed recorded version never locks a caller out of its database.
fn release_order(version: &str) -> (u64, u64, u64) {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u64>().unwrap_or_default());
    (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    )
}

fn bounded_text(value: &str, limit: usize) -> String {
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

fn bounded_required(
    value: &str,
    field_limit: usize,
    remaining: &mut usize,
    elided: &mut bool,
) -> String {
    if value.len() <= field_limit && value.len() <= *remaining {
        *remaining -= value.len();
        return value.to_owned();
    }
    *elided = true;
    *remaining -= ELIDED_FIELD_MARKER.len();
    ELIDED_FIELD_MARKER.to_owned()
}

fn bounded_optional(
    value: Option<&str>,
    field_limit: usize,
    remaining: &mut usize,
    elided: &mut bool,
) -> Option<String> {
    let value = value?;
    if value.len() > field_limit || value.len() > *remaining {
        *elided = true;
        return None;
    }
    *remaining -= value.len();
    Some(value.to_owned())
}

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

fn insert_transition(connection: &Connection, task_id: i64, kind: &str) -> Result<(), DomainError> {
    connection.execute(
        "UPDATE tasks SET last_transition_at=unixepoch() WHERE id=?1",
        [task_id],
    )?;
    connection.execute("INSERT INTO task_transitions(task_id,kind,status,execution_status,readiness_status) SELECT id,?2,status,execution_status,readiness_status FROM tasks WHERE id=?1", params![task_id,kind])?;
    connection.execute(
        "DELETE FROM task_transitions WHERE id <= (SELECT MAX(id)-?1 FROM task_transitions)",
        [MAX_RETAINED_TRANSITIONS],
    )?;
    Ok(())
}

fn record_transition_and_advance(
    connection: &Connection,
    task_id: i64,
    kind: &str,
) -> Result<(), DomainError> {
    connection.execute("UPDATE tasks SET readiness_status=CASE WHEN readiness_status='unqueued' THEN readiness_status ELSE 'completed' END,intervention=NULL WHERE id=?1", [task_id])?;
    let generation = queue_generation(connection, task_id)?;
    connection.execute("UPDATE launch_deliveries SET state='delivered',runner_boot_id=NULL,runner_pid=NULL WHERE task_id=?1 AND generation=?2 AND state!='delivered'", params![task_id,generation])?;
    insert_transition(connection, task_id, kind)?;
    ensure_launch_invariant(connection, task_id)?;
    let dependents = connection.prepare("SELECT dependent_task_id FROM task_dependencies WHERE prerequisite_task_id=?1 ORDER BY dependent_task_id")?.query_map([task_id], |r| r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
    for id in dependents {
        let advanced = connection.execute(&format!("UPDATE tasks SET readiness_status='ready' WHERE id=?1 AND readiness_status='blocked' AND {PREREQUISITES_SATISFIED_SQL}"), [id])?;
        if advanced == 1 {
            let generation = queue_generation(connection, id)?;
            connection.execute("INSERT OR IGNORE INTO launch_deliveries(task_id,generation,state) VALUES(?1,?2,'pending')", params![id,generation])?;
            insert_transition(connection, id, "ready")?;
            ensure_launch_invariant(connection, id)?;
        }
    }
    Ok(())
}

const MAX_PREREQUISITES: i64 = 8;
const MAX_LAUNCH_ATTEMPTS: i64 = 3;
const MAX_RETAINED_TRANSITIONS: i64 = 10_000;
const MAX_DIAGNOSTIC_BYTES: usize = 1024;
const MAX_FIELD_BYTES: usize = 4096;
const MAX_OBSERVATION_TEXT_BYTES: usize = 32 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 1024;
const PREREQUISITES_SATISFIED_SQL: &str = "NOT EXISTS(SELECT 1 FROM task_dependencies d JOIN tasks p ON p.id=d.prerequisite_task_id WHERE d.dependent_task_id=?1 AND p.status!='completed')";

fn prerequisites_satisfied(connection: &Connection, task_id: i64) -> Result<bool, DomainError> {
    connection
        .query_row(
            &format!("SELECT {PREREQUISITES_SATISFIED_SQL}"),
            [task_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn queue_generation(connection: &Connection, task_id: i64) -> Result<i64, DomainError> {
    connection
        .query_row(
            "SELECT queue_generation FROM tasks WHERE id=?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(DomainError::NotFound("task", task_id))
}

fn recompute_queued_readiness(
    connection: &Connection,
    task_id: i64,
    kind: &str,
) -> Result<(), DomainError> {
    let task = query_task(connection, task_id)?.ok_or(DomainError::NotFound("task", task_id))?;
    if task.readiness_status == ReadinessStatus::Unqueued {
        return Ok(());
    }
    let ready = prerequisites_satisfied(connection, task_id)?;
    let generation = queue_generation(connection, task_id)?;
    if ready {
        connection.execute(
            "UPDATE tasks SET readiness_status='ready' WHERE id=?1",
            [task_id],
        )?;
        connection.execute("INSERT INTO launch_deliveries(task_id,generation,state) VALUES(?1,?2,'pending') ON CONFLICT(task_id,generation) DO NOTHING", params![task_id,generation])?;
    } else {
        connection.execute(
            "UPDATE tasks SET readiness_status='blocked' WHERE id=?1",
            [task_id],
        )?;
        connection.execute(
            "DELETE FROM launch_deliveries WHERE task_id=?1 AND generation=?2 AND state='pending'",
            params![task_id, generation],
        )?;
    }
    insert_transition(connection, task_id, kind)?;
    ensure_launch_invariant(connection, task_id)
}

fn ensure_launch_invariant(connection: &Connection, task_id: i64) -> Result<(), DomainError> {
    let (readiness, delivery): (String, Option<String>) = connection.query_row(
        "SELECT t.readiness_status,(SELECT state FROM launch_deliveries d WHERE d.task_id=t.id AND d.generation=t.queue_generation) FROM tasks t WHERE t.id=?1",
        [task_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let valid = matches!(
        (readiness.as_str(), delivery.as_deref()),
        ("unqueued", None)
            | ("blocked", None)
            | ("ready", Some("pending"))
            | ("claimed", Some("claimed"))
            | ("running", Some("delivered"))
            | ("completed", Some("delivered"))
            | ("completed", None)
            | ("intervention_required", Some("intervention_required"))
    );
    if valid {
        Ok(())
    } else {
        Err(DomainError::Invalid(format!(
            "launch invariant violated for task {task_id}: readiness={readiness}, delivery={}",
            delivery.as_deref().unwrap_or("none")
        )))
    }
}

/// A parent must integrate its delegated children before completing: refuse
/// completion while any direct child is not completed or is still recorded as
/// running, naming the blocking children so the caller can recover them.
fn ensure_children_finished(connection: &Connection, parent_id: i64) -> Result<(), DomainError> {
    let blocking = connection
        .prepare(
            "SELECT id FROM tasks WHERE parent_task_id=?1
             AND (status != 'completed' OR execution_status = 'running') ORDER BY id",
        )?
        .query_map([parent_id], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if blocking.is_empty() {
        return Ok(());
    }
    let blocking = blocking
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Err(DomainError::Invalid(format!(
        "cannot complete task {parent_id}: direct child tasks [{blocking}] are unfinished or running; complete, recover, or revise them first"
    )))
}

/// Placeholder evidence for a checkpoint created before its subject completed.
const AWAITING_SUBJECT_EVIDENCE: &str = "Awaiting subject completion; evidence pending.";

/// The evidence a checkpoint inherits from its completed subject.
fn subject_evidence(subject: &Task) -> String {
    subject.evidence.clone().unwrap_or_else(|| {
        format!(
            "## Work result\n\n{}",
            subject.result.as_deref().unwrap_or("")
        )
    })
}

const TASK_SELECT: &str = "SELECT id,project_id,agent_id,kind,status,description,result,evidence,decision,parent_task_id,previous_task_id,subject_task_id,execution_status,execution_attempt,session_id,worktree_name,artifact_dir,execution_boot_id,execution_pid,readiness_status,queue_generation,COALESCE(intervention,(SELECT diagnostic FROM launch_deliveries WHERE task_id=tasks.id AND generation=tasks.queue_generation AND state='intervention_required')) FROM tasks";

fn query_checkpoint(connection: &Connection, subject_id: i64) -> Result<Option<Task>, DomainError> {
    connection
        .query_row(
            &format!("{TASK_SELECT} WHERE subject_task_id=?1"),
            [subject_id],
            row_task,
        )
        .optional()
        .map_err(Into::into)
}

fn query_task(connection: &Connection, id: i64) -> Result<Option<Task>, DomainError> {
    connection
        .query_row(&format!("{TASK_SELECT} WHERE id=?1"), [id], row_task)
        .optional()
        .map_err(Into::into)
}

fn row_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let kind: String = row.get(3)?;
    let status: String = row.get(4)?;
    let decision: Option<String> = row.get(8)?;
    let execution_status: Option<String> = row.get(12)?;
    Ok(Task {
        id: row.get(0)?,
        project_id: row.get(1)?,
        agent_id: row.get(2)?,
        kind: parse_database_enum(3, &kind)?,
        status: parse_database_enum(4, &status)?,
        description: row.get(5)?,
        result: row.get(6)?,
        evidence: row.get(7)?,
        decision: decision
            .map(|value| parse_database_enum(8, &value))
            .transpose()?,
        parent_task_id: row.get(9)?,
        previous_task_id: row.get(10)?,
        subject_task_id: row.get(11)?,
        execution_status: execution_status
            .map(|value| parse_database_enum(12, &value))
            .transpose()?,
        execution_attempt: row.get(13)?,
        session_id: row.get(14)?,
        worktree_name: row.get(15)?,
        artifact_dir: row.get(16)?,
        execution_boot_id: row.get(17)?,
        execution_pid: row.get(18)?,
        readiness_status: parse_database_enum(19, &row.get::<_, String>(19)?)?,
        queue_generation: row.get(20)?,
        intervention: row.get(21)?,
    })
}

fn parse_database_enum<T: FromStr>(column: usize, value: &str) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    T::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })
}
