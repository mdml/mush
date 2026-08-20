use super::*;

pub(super) fn insert_transition(
    connection: &Connection,
    task_id: i64,
    kind: &str,
) -> Result<(), DomainError> {
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

pub(super) fn record_transition_and_advance(
    connection: &Connection,
    task_id: i64,
    kind: &str,
) -> Result<(), DomainError> {
    connection.execute("UPDATE tasks SET readiness_status=CASE WHEN readiness_status='unqueued' THEN readiness_status ELSE 'completed' END,intervention=NULL WHERE id=?1", [task_id])?;
    let generation = queue_generation(connection, task_id)?;
    connection.execute("UPDATE launch_deliveries SET state='delivered',runner_boot_id=NULL,runner_pid=NULL WHERE task_id=?1 AND generation=?2 AND state!='delivered'", params![task_id,generation])?;
    insert_transition(connection, task_id, kind)?;
    ensure_launch_invariant(connection, task_id)?;
    // A terminal transition can unblock edge dependents and, when the task is
    // a completed work subject, the checkpoint that reviews it.
    let dependents = connection.prepare("SELECT dependent_task_id FROM task_dependencies WHERE prerequisite_task_id=?1 UNION SELECT id FROM tasks WHERE subject_task_id=?1 AND kind='checkpoint' ORDER BY 1")?.query_map([task_id], |r| r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
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

pub(super) const MAX_LAUNCH_ATTEMPTS: i64 = 3;
const MAX_RETAINED_TRANSITIONS: i64 = 10_000;
/// Whether task `?1` may execute: every dependency-edge prerequisite is
/// completed — and, for a checkpoint prerequisite, decided `accepted` — and,
/// when `?1` is itself a checkpoint, its subject is completed. Only acceptance
/// satisfies a checkpoint prerequisite, so a requested revision leaves
/// dependents blocked while the checkpoint itself completes.
const PREREQUISITES_SATISFIED_SQL: &str = "NOT EXISTS(SELECT 1 FROM task_dependencies d JOIN tasks p ON p.id=d.prerequisite_task_id WHERE d.dependent_task_id=?1 AND (p.status!='completed' OR (p.kind='checkpoint' AND COALESCE(p.decision,'')!='accepted'))) AND NOT EXISTS(SELECT 1 FROM tasks c JOIN tasks s ON s.id=c.subject_task_id WHERE c.id=?1 AND s.status!='completed')";

pub(super) fn prerequisites_satisfied(
    connection: &Connection,
    task_id: i64,
) -> Result<bool, DomainError> {
    connection
        .query_row(
            &format!("SELECT {PREREQUISITES_SATISFIED_SQL}"),
            [task_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

pub(super) fn queue_generation(connection: &Connection, task_id: i64) -> Result<i64, DomainError> {
    connection
        .query_row(
            "SELECT queue_generation FROM tasks WHERE id=?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(DomainError::NotFound("task", task_id))
}

pub(super) fn recompute_queued_readiness(
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

pub(super) fn ensure_launch_invariant(
    connection: &Connection,
    task_id: i64,
) -> Result<(), DomainError> {
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
pub(super) fn ensure_children_finished(
    connection: &Connection,
    parent_id: i64,
) -> Result<(), DomainError> {
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
