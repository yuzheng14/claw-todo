pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Invalid {field}: {message}")]
    InvalidInput { field: String, message: String },
    #[error("Database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("File operation failed: {0}")]
    Io(#[from] std::io::Error),
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
