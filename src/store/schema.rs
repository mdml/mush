use super::*;

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
        if version != 0 && !MIGRATABLE_SCHEMA_VERSIONS.contains(&version) {
            return Err(DomainError::Invalid(format!(
                "database schema version {version} was never shipped and cannot be migrated; restore {} from a backup written by a shipped build",
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
        if self.tasks_table_sql()?.is_some() {
            self.add_missing_task_columns()?;
            self.rebuild_tasks_for_decision_vocabulary()?;
        }
        install_current_schema(&self.connection)?;
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

    /// The stored `CREATE TABLE` statement for `tasks`, or `None` before the
    /// table exists.
    fn tasks_table_sql(&self) -> Result<Option<String>, DomainError> {
        self.connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='tasks'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// A version 3 `tasks` table predates every execution and readiness
    /// column, so add whichever the database is missing before the rebuild
    /// selects the full column list.
    fn add_missing_task_columns(&self) -> Result<(), DomainError> {
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
        Ok(())
    }

    /// Every shipped schema baked the M3-era decision vocabulary into the
    /// `tasks` table's CHECK constraint, which would reject the adjudication
    /// vocabulary this binary writes. SQLite cannot alter a CHECK, so the
    /// migration rebuilds the table from the current definition, mapping
    /// `accepted` to `met` and `revision_requested` to `not_met` in place.
    /// The rebuild is skipped once the stored definition already carries the
    /// current vocabulary.
    fn rebuild_tasks_for_decision_vocabulary(&self) -> Result<(), DomainError> {
        if self
            .tasks_table_sql()?
            .is_some_and(|sql| sql.contains("'not_met'"))
        {
            return Ok(());
        }
        // Foreign keys stay off for the copy-and-swap so dropping the old
        // table does not cascade into its children; the check afterwards
        // proves the swap left every reference intact.
        self.connection
            .execute_batch("PRAGMA foreign_keys = OFF;")?;
        let rebuild = format!(
            "{LOOP_TABLES_SQL}
             CREATE TABLE tasks_rebuilt ({});
             INSERT INTO tasks_rebuilt({REBUILT_TASK_COLUMNS})
                 SELECT {REBUILT_TASK_SOURCES} FROM tasks;
             DROP TABLE tasks;
             ALTER TABLE tasks_rebuilt RENAME TO tasks;",
            tasks_table_body()
        );
        self.connection.execute_batch(&rebuild)?;
        self.connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let broken: Option<String> = self
            .connection
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(2))
            .optional()?;
        if let Some(table) = broken {
            return Err(DomainError::Invalid(format!(
                "decision-vocabulary rebuild broke a foreign key into {table}; restore the pre-migration backup"
            )));
        }
        Ok(())
    }
}

/// The columns the rebuild copies; `criteria` and `loop_id` are new in this
/// schema and stay NULL for migrated rows.
const REBUILT_TASK_COLUMNS: &str = "id,project_id,agent_id,kind,status,description,result,evidence,decision,parent_task_id,previous_task_id,subject_task_id,execution_status,execution_attempt,session_id,worktree_name,artifact_dir,execution_boot_id,execution_pid,readiness_status,queue_generation,intervention,last_transition_at";

/// The same columns read from the old table, with the decision vocabulary
/// reinterpreted as the bounded checkpoint loop decision records.
const REBUILT_TASK_SOURCES: &str = "id,project_id,agent_id,kind,status,description,result,evidence,CASE decision WHEN 'accepted' THEN 'met' WHEN 'revision_requested' THEN 'not_met' ELSE decision END,parent_task_id,previous_task_id,subject_task_id,execution_status,execution_attempt,session_id,worktree_name,artifact_dir,execution_boot_id,execution_pid,readiness_status,queue_generation,intervention,last_transition_at";

/// The `tasks` table's column definitions and row constraints, shared by the
/// current schema and the migration rebuild so the two cannot diverge.
fn tasks_table_body() -> &'static str {
    "id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
                agent_id INTEGER REFERENCES agents(id), kind TEXT NOT NULL CHECK(kind IN ('work','checkpoint')),
                status TEXT NOT NULL CHECK(status IN ('pending','completed')), description TEXT NOT NULL,
                result TEXT, evidence TEXT, criteria TEXT, decision TEXT CHECK(decision IN ('met','not_met','blocked')),
                parent_task_id INTEGER REFERENCES tasks(id), previous_task_id INTEGER REFERENCES tasks(id),
                subject_task_id INTEGER REFERENCES tasks(id), loop_id INTEGER REFERENCES loops(id),
                execution_status TEXT CHECK(execution_status IN ('running','succeeded','interrupted')),
                execution_attempt INTEGER NOT NULL DEFAULT 0,
                session_id TEXT, worktree_name TEXT, artifact_dir TEXT,
                execution_boot_id TEXT, execution_pid INTEGER,
                readiness_status TEXT NOT NULL DEFAULT 'unqueued' CHECK(readiness_status IN ('unqueued','blocked','ready','claimed','running','completed','intervention_required')),
                queue_generation INTEGER NOT NULL DEFAULT 0, intervention TEXT,
                last_transition_at INTEGER NOT NULL DEFAULT 0,
                CHECK((kind = 'work' AND subject_task_id IS NULL AND decision IS NULL) OR
                      (kind = 'checkpoint' AND subject_task_id IS NOT NULL))"
}

/// The loop declaration and its ordered stage path. Loop state is derived
/// from member tasks, never stored, so the rows can stay immutable.
const LOOP_TABLES_SQL: &str = "CREATE TABLE IF NOT EXISTS loops (
                id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
                criteria TEXT NOT NULL CHECK(trim(criteria) != ''),
                adjudicator_agent_id INTEGER NOT NULL REFERENCES agents(id),
                max_attempts INTEGER NOT NULL CHECK(max_attempts >= 1),
                reuse_worktree INTEGER NOT NULL DEFAULT 0 CHECK(reuse_worktree IN (0,1)),
                continuation_task_id INTEGER REFERENCES tasks(id)
            );
            CREATE TABLE IF NOT EXISTS loop_stages (
                loop_id INTEGER NOT NULL REFERENCES loops(id), position INTEGER NOT NULL CHECK(position >= 1),
                agent_id INTEGER NOT NULL REFERENCES agents(id), description TEXT NOT NULL CHECK(trim(description) != ''),
                PRIMARY KEY(loop_id, position)
            );";

const CURRENT_SCHEMA_SQL: &str = r#"CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, path TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS agents (
                id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
                name TEXT NOT NULL, harness TEXT NOT NULL, model TEXT NOT NULL, settings TEXT NOT NULL,
                checkpoint INTEGER NOT NULL DEFAULT 0 CHECK(checkpoint IN (0,1)),
                UNIQUE(project_id, name), UNIQUE(project_id, harness, model, settings)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS one_checkpoint_agent ON agents(project_id) WHERE checkpoint = 1;
            CREATE TABLE IF NOT EXISTS tasks ({TASKS_TABLE_BODY});
            CREATE UNIQUE INDEX IF NOT EXISTS one_checkpoint_per_subject ON tasks(subject_task_id) WHERE kind = 'checkpoint';
            CREATE TRIGGER IF NOT EXISTS checkpoint_subject_immutable
            BEFORE UPDATE OF subject_task_id ON tasks
            WHEN NEW.subject_task_id IS NOT OLD.subject_task_id
            BEGIN
                SELECT RAISE(ABORT, 'a checkpoint is never retargeted to another subject');
            END;
            CREATE TRIGGER IF NOT EXISTS checkpoint_criteria_immutable
            BEFORE UPDATE OF criteria ON tasks
            WHEN NEW.criteria IS NOT OLD.criteria
            BEGIN
                SELECT RAISE(ABORT, 'checkpoint criteria are immutable');
            END;
            CREATE TRIGGER IF NOT EXISTS loop_membership_immutable
            BEFORE UPDATE OF loop_id ON tasks
            WHEN NEW.loop_id IS NOT OLD.loop_id
            BEGIN
                SELECT RAISE(ABORT, 'loop membership is immutable');
            END;
            CREATE TRIGGER IF NOT EXISTS loop_declaration_immutable
            BEFORE UPDATE ON loops
            WHEN NEW.project_id != OLD.project_id OR NEW.criteria != OLD.criteria
                OR NEW.adjudicator_agent_id != OLD.adjudicator_agent_id
                OR NEW.max_attempts != OLD.max_attempts OR NEW.reuse_worktree != OLD.reuse_worktree
            BEGIN
                SELECT RAISE(ABORT, 'a declared loop is immutable');
            END;
            CREATE TRIGGER IF NOT EXISTS loop_continuation_declared_once
            BEFORE UPDATE OF continuation_task_id ON loops
            WHEN OLD.continuation_task_id IS NOT NULL
                AND NEW.continuation_task_id IS NOT OLD.continuation_task_id
            BEGIN
                SELECT RAISE(ABORT, 'the loop success continuation is declared once');
            END;
            CREATE TRIGGER IF NOT EXISTS loop_stages_immutable
            BEFORE UPDATE ON loop_stages
            BEGIN
                SELECT RAISE(ABORT, 'declared loop stages are immutable');
            END;
            CREATE TRIGGER IF NOT EXISTS loop_stages_never_deleted
            BEFORE DELETE ON loop_stages
            BEGIN
                SELECT RAISE(ABORT, 'declared loop stages are immutable');
            END;
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
                SELECT RAISE(ABORT, 'only a work task can be a dependent') WHERE
                    (SELECT kind FROM tasks WHERE id=NEW.dependent_task_id) != 'work';
                SELECT RAISE(ABORT, 'a checkpoint prerequisite cannot gate its own subject') WHERE
                    (SELECT subject_task_id FROM tasks WHERE id=NEW.prerequisite_task_id) = NEW.dependent_task_id;
                SELECT RAISE(ABORT, 'dependency tasks must share a project') WHERE
                    (SELECT project_id FROM tasks WHERE id=NEW.prerequisite_task_id) != (SELECT project_id FROM tasks WHERE id=NEW.dependent_task_id);
                SELECT RAISE(ABORT, 'cannot change dependencies after dependent starts') WHERE
                    (SELECT status FROM tasks WHERE id=NEW.dependent_task_id) = 'completed' OR
                    (SELECT execution_attempt FROM tasks WHERE id=NEW.dependent_task_id) != 0 OR
                    (SELECT readiness_status FROM tasks WHERE id=NEW.dependent_task_id) IN ('claimed','running','completed','intervention_required');
                SELECT RAISE(ABORT, 'task prerequisite limit is {MAX_PREREQUISITES}') WHERE
                    (SELECT COUNT(*) FROM task_dependencies WHERE dependent_task_id=NEW.dependent_task_id) >= {MAX_PREREQUISITES};
                SELECT RAISE(ABORT, 'dependency would create a cycle') WHERE EXISTS(
                    WITH RECURSIVE edges(prereq, dep) AS (
                        SELECT prerequisite_task_id, dependent_task_id FROM task_dependencies
                        UNION ALL SELECT subject_task_id, id FROM tasks WHERE kind='checkpoint'
                    ), reachable(id) AS (
                        SELECT NEW.dependent_task_id
                        UNION SELECT e.dep FROM edges e JOIN reachable r ON e.prereq=r.id
                    ) SELECT 1 FROM reachable WHERE id=NEW.prerequisite_task_id
                );
            END;
            CREATE TRIGGER IF NOT EXISTS dependency_no_late_delete BEFORE DELETE ON task_dependencies BEGIN
                SELECT RAISE(ABORT, 'cannot change dependencies after dependent starts') WHERE
                    (SELECT status FROM tasks WHERE id=OLD.dependent_task_id) = 'completed' OR
                    (SELECT execution_attempt FROM tasks WHERE id=OLD.dependent_task_id) != 0 OR
                    (SELECT readiness_status FROM tasks WHERE id=OLD.dependent_task_id) IN ('claimed','running','completed','intervention_required');
            END;
            CREATE TRIGGER IF NOT EXISTS checkpoint_subject_gate_on_insert
            BEFORE INSERT ON tasks WHEN NEW.kind = 'checkpoint'
                AND (NEW.readiness_status IN ('ready','claimed','running')
                     OR NEW.execution_status = 'running' OR NEW.decision IS NOT NULL)
                AND (SELECT status FROM tasks WHERE id = NEW.subject_task_id) != 'completed'
            BEGIN
                SELECT RAISE(ABORT, 'checkpoint subject is not completed');
            END;
            CREATE TRIGGER IF NOT EXISTS checkpoint_readiness_requires_completed_subject
            BEFORE UPDATE OF readiness_status ON tasks
            WHEN NEW.kind = 'checkpoint' AND NEW.readiness_status IN ('ready','claimed','running')
                AND (SELECT status FROM tasks WHERE id = NEW.subject_task_id) != 'completed'
            BEGIN
                SELECT RAISE(ABORT, 'checkpoint subject is not completed');
            END;
            CREATE TRIGGER IF NOT EXISTS checkpoint_execution_requires_completed_subject
            BEFORE UPDATE OF execution_status ON tasks
            WHEN NEW.kind = 'checkpoint' AND NEW.execution_status = 'running'
                AND (SELECT status FROM tasks WHERE id = NEW.subject_task_id) != 'completed'
            BEGIN
                SELECT RAISE(ABORT, 'checkpoint subject is not completed');
            END;
            CREATE TRIGGER IF NOT EXISTS checkpoint_decision_requires_completed_subject
            BEFORE UPDATE OF decision ON tasks
            WHEN NEW.kind = 'checkpoint' AND NEW.decision IS NOT NULL AND OLD.decision IS NULL
                AND (SELECT status FROM tasks WHERE id = NEW.subject_task_id) != 'completed'
            BEGIN
                SELECT RAISE(ABORT, 'checkpoint subject is not completed');
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
            "#;

fn install_current_schema(connection: &Connection) -> Result<(), DomainError> {
    connection.execute_batch(LOOP_TABLES_SQL)?;
    let sql = CURRENT_SCHEMA_SQL
        .replace("{TASKS_TABLE_BODY}", tasks_table_body())
        .replace("{MAX_PREREQUISITES}", &MAX_PREREQUISITES.to_string());
    connection.execute_batch(&sql)?;
    Ok(())
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
