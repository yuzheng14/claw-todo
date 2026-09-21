pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Invalid {field}: {message}")]
    InvalidInput { field: String, message: String },
    #[error("{entity} {id} was not found")]
    NotFound { entity: &'static str, id: String },
    #[error(
        "Creation token {creation_token} is already bound to a different request for task {task_id}"
    )]
    CreationTokenConflict {
        creation_token: String,
        task_id: String,
    },
    #[error("Parent {parent_id} has closed ancestors: {ancestor_ids:?}")]
    ClosedAncestor {
        parent_id: String,
        ancestor_ids: Vec<String>,
    },
    #[error("Task {task_id} cannot transition from {status} to {requested_status}")]
    InvalidState {
        task_id: String,
        status: String,
        requested_status: &'static str,
    },
    #[error("Task {task_id} has open descendants: {blocking_task_ids:?}")]
    OpenDescendants {
        task_id: String,
        blocking_task_ids: Vec<String>,
    },
    #[error("Task {task_id} cannot become a work task with {channel} reminders: {reminder_ids:?}")]
    ChannelForbidden {
        task_id: String,
        channel: &'static str,
        reminder_ids: Vec<String>,
    },
    #[error("Database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("File operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl AppError {
    pub fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::InvalidInput {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub(crate) fn is_busy(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(error) => error
            .code()
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| matches!(code & 0xff, 5 | 6)),
        sqlx::Error::PoolTimedOut => true,
        _ => false,
    }
}
