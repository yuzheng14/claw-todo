use std::io::Write;

use serde_json::{Value, json};

use crate::{
    AppError, ChangeResult, CreateResult, HistoryEntry, ListResult, Reminder, ReminderResult,
    Result, TaskDetail, TransitionResult,
};

/// 命令的业务结果；输出层负责机器协议和后续人类可读展示。
pub(crate) enum Response {
    Created(CreateResult),
    Changed(ChangeResult),
    Transitioned(TransitionResult),
    Listed(ListResult),
    Detail(TaskDetail),
    History(Vec<HistoryEntry>),
    ReminderChanged(ReminderResult),
    Reminders(Vec<Reminder>),
    ReminderRemoved(Reminder),
    Help(String),
    Version(String),
}

impl Response {
    fn data(&self) -> serde_json::Result<Value> {
        match self {
            Self::Created(value) => serde_json::to_value(value),
            Self::Changed(value) => serde_json::to_value(value),
            Self::Transitioned(value) => serde_json::to_value(value),
            Self::Listed(value) => serde_json::to_value(value),
            Self::Detail(value) => serde_json::to_value(value),
            Self::History(value) => serde_json::to_value(value),
            Self::ReminderChanged(value) => serde_json::to_value(value),
            Self::Reminders(value) => serde_json::to_value(value),
            Self::ReminderRemoved(value) => serde_json::to_value(value),
            Self::Help(value) => Ok(json!({"help": value})),
            Self::Version(value) => Ok(json!({"version": value.trim_end()})),
        }
    }
}

pub(crate) fn write_success(response: &Response, as_json: bool) -> Result<()> {
    let text = match (as_json, response) {
        (false, Response::Help(text) | Response::Version(text)) => text.clone(),
        _ => serde_json::to_string(&json!({"ok": true, "data": response.data()?}))?,
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(text.as_bytes())?;
    if !text.ends_with('\n') {
        stdout.write_all(b"\n")?;
    }
    Ok(())
}

fn error_details(error: &AppError) -> (&'static str, Value, u8) {
    match error {
        AppError::InvalidInput { field, .. } => ("INVALID_INPUT", json!({"field": field}), 2),
        AppError::NotFound { entity, id } => ("NOT_FOUND", json!({"entity": entity, "id": id}), 3),
        AppError::CreationTokenConflict {
            creation_token,
            task_id,
        } => (
            "CREATION_TOKEN_CONFLICT",
            json!({"creation_token": creation_token, "task_id": task_id}),
            4,
        ),
        AppError::ClosedAncestor {
            parent_id,
            ancestor_ids,
        } => (
            "CLOSED_ANCESTOR",
            json!({"parent_id": parent_id, "ancestor_ids": ancestor_ids}),
            4,
        ),
        AppError::CycleDetected { task_id, parent_id } => (
            "CYCLE_DETECTED",
            json!({"task_id": task_id, "parent_id": parent_id}),
            4,
        ),
        AppError::InvalidState {
            task_id,
            status,
            requested_status,
        } => (
            "INVALID_STATE",
            json!({"task_id": task_id, "status": status, "requested_status": requested_status}),
            4,
        ),
        AppError::OpenDescendants {
            task_id,
            blocking_task_ids,
        } => (
            "OPEN_DESCENDANTS",
            json!({"task_id": task_id, "blocking_task_ids": blocking_task_ids}),
            4,
        ),
        AppError::ChannelForbidden {
            task_id,
            channel,
            reminder_ids,
        } => (
            "CHANNEL_FORBIDDEN",
            json!({"task_id": task_id, "channel": channel, "reminder_ids": reminder_ids}),
            4,
        ),
        AppError::Database(error) if crate::error::is_busy(error) => {
            ("DATABASE_BUSY", json!({}), 5)
        }
        AppError::Migration(
            sqlx::migrate::MigrateError::Execute(error)
            | sqlx::migrate::MigrateError::ExecuteMigration(error, _),
        ) if crate::error::is_busy(error) => ("DATABASE_BUSY", json!({}), 5),
        AppError::Database(_) => ("DATABASE_ERROR", json!({}), 1),
        AppError::Migration(_) => ("MIGRATION_ERROR", json!({}), 1),
        AppError::Io(_) => ("IO_ERROR", json!({}), 1),
        AppError::Json(_) => ("JSON_ERROR", json!({}), 1),
    }
}

pub(crate) fn exit_code(error: &AppError) -> u8 {
    error_details(error).2
}

/// 管道下游提前关闭（例如 `head`）不是业务失败，也不应 panic。
pub(crate) fn write_failure_exit_code(error: &AppError) -> u8 {
    match error {
        AppError::Io(error) if error.kind() == std::io::ErrorKind::BrokenPipe => 0,
        _ => 1,
    }
}

pub(crate) fn write_error(error: &AppError, as_json: bool) -> Result<()> {
    let (code, context, _) = error_details(error);
    write_failure(code, &error.to_string(), context, as_json)
}

pub(crate) fn write_argument_error(message: &str, as_json: bool) -> Result<()> {
    write_failure("INVALID_ARGUMENT", message, json!({}), as_json)
}

fn write_failure(code: &str, message: &str, context: Value, as_json: bool) -> Result<()> {
    if as_json {
        let envelope =
            json!({"ok": false, "error": {"code": code, "message": message, "context": context}});
        writeln!(std::io::stdout().lock(), "{envelope}")?;
    } else {
        let mut stderr = std::io::stderr().lock();
        writeln!(stderr, "Error [{code}]: {message}")?;
        if context != json!({}) {
            writeln!(stderr, "Context: {context}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infrastructure_errors_keep_stable_codes_and_busy_is_retryable() {
        let busy = AppError::Database(sqlx::Error::PoolTimedOut);
        assert_eq!(error_details(&busy).0, "DATABASE_BUSY");
        assert_eq!(exit_code(&busy), 5);
        let io = AppError::Io(std::io::Error::other("failed"));
        assert_eq!(error_details(&io).0, "IO_ERROR");
        assert_eq!(exit_code(&io), 1);
        assert_eq!(
            write_failure_exit_code(&AppError::Io(std::io::ErrorKind::BrokenPipe.into())),
            0
        );
    }
}
