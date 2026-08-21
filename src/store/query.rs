use super::*;

/// Placeholder evidence for a checkpoint created before its subject completed.
pub(super) const AWAITING_SUBJECT_EVIDENCE: &str = "Awaiting subject completion; evidence pending.";

/// The evidence a checkpoint inherits from its completed subject.
pub(super) fn subject_evidence(subject: &Task) -> String {
    subject.evidence.clone().unwrap_or_else(|| {
        format!(
            "## Work result\n\n{}",
            subject.result.as_deref().unwrap_or("")
        )
    })
}

pub(super) const TASK_SELECT: &str = "SELECT id,project_id,agent_id,kind,status,description,result,evidence,criteria,decision,parent_task_id,previous_task_id,subject_task_id,loop_id,execution_status,execution_attempt,session_id,worktree_name,artifact_dir,execution_boot_id,execution_pid,readiness_status,queue_generation,COALESCE(intervention,(SELECT diagnostic FROM launch_deliveries WHERE task_id=tasks.id AND generation=tasks.queue_generation AND state='intervention_required')) FROM tasks";

pub(super) fn query_checkpoint(
    connection: &Connection,
    subject_id: i64,
) -> Result<Option<Task>, DomainError> {
    connection
        .query_row(
            &format!("{TASK_SELECT} WHERE subject_task_id=?1"),
            [subject_id],
            row_task,
        )
        .optional()
        .map_err(Into::into)
}

pub(super) fn query_task(connection: &Connection, id: i64) -> Result<Option<Task>, DomainError> {
    connection
        .query_row(&format!("{TASK_SELECT} WHERE id=?1"), [id], row_task)
        .optional()
        .map_err(Into::into)
}

pub(super) fn row_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let kind: String = row.get(3)?;
    let status: String = row.get(4)?;
    let decision: Option<String> = row.get(9)?;
    let execution_status: Option<String> = row.get(14)?;
    Ok(Task {
        id: row.get(0)?,
        project_id: row.get(1)?,
        agent_id: row.get(2)?,
        kind: parse_database_enum(3, &kind)?,
        status: parse_database_enum(4, &status)?,
        description: row.get(5)?,
        result: row.get(6)?,
        evidence: row.get(7)?,
        criteria: row.get(8)?,
        decision: decision
            .map(|value| parse_database_enum(9, &value))
            .transpose()?,
        parent_task_id: row.get(10)?,
        previous_task_id: row.get(11)?,
        subject_task_id: row.get(12)?,
        loop_id: row.get(13)?,
        execution_status: execution_status
            .map(|value| parse_database_enum(14, &value))
            .transpose()?,
        execution_attempt: row.get(15)?,
        session_id: row.get(16)?,
        worktree_name: row.get(17)?,
        artifact_dir: row.get(18)?,
        execution_boot_id: row.get(19)?,
        execution_pid: row.get(20)?,
        readiness_status: parse_database_enum(21, &row.get::<_, String>(21)?)?,
        queue_generation: row.get(22)?,
        intervention: row.get(23)?,
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
