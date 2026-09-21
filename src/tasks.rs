use std::collections::HashSet;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::{SqliteConnection, types::Json};
use uuid::Uuid;

use crate::{AppError, CreateResult, CreateTask, HistoryEntry, Result, Store, Task, db::get_task};

impl Store {
    pub async fn create(&self, input: CreateTask) -> Result<CreateResult> {
        // Keep the initial request, including optional values and source order.
        let creation_request = serde_json::to_string(&input)?;
        let mut tx = self.write().await?;
        if let Some(creation_token) = &input.creation_token {
            let existing = sqlx::query!(
                "SELECT id, creation_request FROM tasks WHERE creation_token = ?",
                creation_token
            )
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(existing) = existing {
                if existing.creation_request != creation_request {
                    return Err(AppError::CreationTokenConflict {
                        creation_token: creation_token.clone(),
                        task_id: existing.id,
                    });
                }
                // A retry must succeed even if the task or its parent is now closed.
                let task = get_task(&mut tx, &existing.id).await?;
                tx.commit().await?;
                return Ok(CreateResult {
                    task,
                    deduplicated: true,
                });
            }
        }
        validate(&input)?;
        if let Some(parent_id) = &input.parent_id {
            require_open_ancestors(&mut tx, parent_id).await?;
        }
        let at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let task = Task {
            id: Uuid::new_v4().to_string(),
            title: input.title,
            description: input.description,
            status: "pending".into(),
            category: input.category,
            project: input.project,
            parent_id: input.parent_id,
            blocked_reason: None,
            cancel_reason: None,
            sources: Json(input.sources),
            created_at: at.clone(),
            updated_at: at.clone(),
            closed_at: None,
            creation_token: input.creation_token,
        };
        let sources = serde_json::to_string(&task.sources.0)?;
        sqlx::query!(
            "INSERT INTO tasks (id, title, description, status, category, project, parent_id, \
             sources, created_at, updated_at, creation_token, creation_request) \
             VALUES (?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?)",
            task.id,
            task.title,
            task.description,
            task.category,
            task.project,
            task.parent_id,
            sources,
            task.created_at,
            task.updated_at,
            task.creation_token,
            creation_request
        )
        .execute(&mut *tx)
        .await?;
        let changes = serde_json::to_string(&json!({"before": null, "after": task}))?;
        sqlx::query!(
            "INSERT INTO history (task_id, kind, at, changes) VALUES (?, 'created', ?, ?)",
            task.id,
            at,
            changes
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(CreateResult {
            task,
            deduplicated: false,
        })
    }

    pub async fn get(&self, id: &str) -> Result<Task> {
        let mut conn = self.pool.acquire().await?;
        get_task(&mut conn, id).await
    }

    pub async fn history(&self, id: &str) -> Result<Vec<HistoryEntry>> {
        let mut tx = self.pool.begin().await?;
        get_task(&mut tx, id).await?;
        let entries = sqlx::query_as!(HistoryEntry,
            r#"SELECT id, task_id, kind, at, changes AS "changes: Json<Value>" FROM history WHERE task_id = ? ORDER BY id"#,
            id
        ).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(entries)
    }
}

fn validate(input: &CreateTask) -> Result<()> {
    if !matches!(input.category.as_str(), "personal" | "work") {
        return Err(AppError::invalid(
            "category",
            "Category must be personal or work",
        ));
    }
    for (field, value) in [
        ("title", Some(input.title.as_str())),
        ("project", input.project.as_deref()),
        ("parent_id", input.parent_id.as_deref()),
        ("creation_token", input.creation_token.as_deref()),
    ]
    .into_iter()
    .chain(input.sources.iter().map(|s| ("source", Some(s.as_str()))))
    {
        if value.is_some_and(|v| v.trim().is_empty()) {
            return Err(AppError::invalid(field, "Must not be empty"));
        }
    }
    Ok(())
}

pub(crate) async fn require_open_ancestors(
    conn: &mut SqliteConnection,
    parent_id: &str,
) -> Result<()> {
    let mut current = Some(parent_id.to_owned());
    let mut seen = HashSet::new();
    let mut closed = Vec::new();
    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            return Err(AppError::invalid(
                "parent_id",
                "The existing parent chain contains a cycle",
            ));
        }
        let parent = get_task(conn, &id).await?;
        if matches!(parent.status.as_str(), "done" | "cancelled") {
            closed.push(parent.id);
        }
        current = parent.parent_id;
    }
    if !closed.is_empty() {
        return Err(AppError::ClosedAncestor {
            parent_id: parent_id.to_owned(),
            ancestor_ids: closed,
        });
    }
    Ok(())
}
