use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{AppError, Result, Task};
use sqlx::{
    Sqlite, SqliteConnection, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    types::Json,
};

#[derive(Clone)]
pub struct Store {
    pub(crate) pool: SqlitePool,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err(AppError::invalid("db", "Database path must not be empty"));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(15));
        // PRAGMA journal_mode may return SQLITE_BUSY without invoking SQLite's
        // busy handler when several processes initialize a new file together.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let mut delay = Duration::from_millis(10);
        let pool = loop {
            match SqlitePoolOptions::new()
                .max_connections(2)
                .acquire_timeout(Duration::from_secs(30))
                .connect_with(options.clone())
                .await
            {
                Ok(pool) => break pool,
                Err(error)
                    if crate::error::is_busy(&error) && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_millis(250));
                }
                Err(error) => return Err(error.into()),
            }
        };
        // SQLite's SQLx migration lock is a no-op. An outer write transaction
        // serializes concurrent first starts, including the migration ledger.
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::migrate!().run_direct(None, &mut *tx, false).await?;
        tx.commit().await?;
        Ok(Self { pool })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub(crate) async fn write(&self) -> Result<Transaction<'static, Sqlite>> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }
}

pub(crate) async fn get_task(conn: &mut SqliteConnection, id: &str) -> Result<Task> {
    sqlx::query_as!(
        Task,
        r#"SELECT id, title, description, status, category, project, parent_id,
        blocked_reason, cancel_reason, sources AS "sources: Json<Vec<String>>",
        created_at, updated_at, closed_at, creation_token FROM tasks WHERE id = ?"#,
        id
    )
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| AppError::NotFound {
        entity: "task",
        id: id.to_owned(),
    })
}

pub fn default_db_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| AppError::invalid("db", "Cannot find user data directory; pass --db"))?;
    Ok(base.data_local_dir().join("claw-todo").join("todos.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_first_open_migrates_once_and_enables_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/todos.db");
        let mut workers = Vec::new();
        for _ in 0..6 {
            let path = path.clone();
            workers.push(tokio::spawn(async move { Store::open(path).await }));
        }
        for worker in workers {
            let store = worker.await.unwrap().unwrap();
            let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&store.pool)
                .await
                .unwrap();
            let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&store.pool)
                .await
                .unwrap();
            let migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
                .fetch_one(&store.pool)
                .await
                .unwrap();
            assert_eq!(mode, "wal");
            assert_eq!(foreign_keys, 1);
            assert_eq!(migrations, 1);
            store.close().await;
        }
    }
}
