use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::{SqliteConnection, types::Json};

use crate::{AppError, ChangeResult, EditTask, Result, Store, db::get_task};

impl Store {
    /// 编辑普通字段；无实际变化时不更新时间，也不新增历史。
    pub async fn edit(&self, id: &str, input: EditTask) -> Result<ChangeResult> {
        validate(&input)?;
        // 先取得写事务再读当前记录，避免并发的局部编辑互相覆盖。
        let mut tx = self.write().await?;
        let before = get_task(&mut tx, id).await?;
        let mut task = before.clone();
        if let Some(title) = input.title {
            task.title = title;
        }
        if let Some(category) = input.category {
            task.category = category;
        }
        if let Some(description) = input.description {
            task.description = description;
        }
        if let Some(project) = input.project {
            task.project = project;
        }
        if let Some(sources) = input.sources {
            task.sources = Json(sources);
        }
        if task == before {
            tx.commit().await?;
            return Ok(ChangeResult {
                task,
                changed: false,
            });
        }
        if task.category == "work" && task.category != before.category {
            // 所有关联都受约束，包括已经触发或取消的提醒。
            let reminders = sqlx::query!(
                "SELECT id FROM reminders WHERE task_id = ? AND channel = 'wechat' ORDER BY id",
                id
            )
            .fetch_all(&mut *tx)
            .await?;
            if !reminders.is_empty() {
                return Err(AppError::ChannelForbidden {
                    task_id: id.to_owned(),
                    channel: "wechat",
                    reminder_ids: reminders.into_iter().map(|r| r.id).collect(),
                });
            }
        }
        task.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let sources = serde_json::to_string(&task.sources.0)?;
        // 只写本批允许编辑的字段，状态、父级和创建请求保持不变。
        sqlx::query!(
            "UPDATE tasks SET title = ?, category = ?, description = ?, project = ?, \
             sources = ?, updated_at = ? WHERE id = ?",
            task.title,
            task.category,
            task.description,
            task.project,
            sources,
            task.updated_at,
            id
        )
        .execute(&mut *tx)
        .await?;
        record(
            &mut tx,
            id,
            "edited",
            &task.updated_at,
            json!({"before": before, "after": task}),
        )
        .await?;
        tx.commit().await?;
        Ok(ChangeResult {
            task,
            changed: true,
        })
    }

    /// 追加进展备注，保留正文原文；重复正文也会追加一条独立历史。
    pub async fn note(&self, id: &str, body: &str) -> Result<ChangeResult> {
        nonempty("body", body)?;
        let mut tx = self.write().await?;
        let before = get_task(&mut tx, id).await?;
        let mut task = before.clone();
        task.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        sqlx::query!(
            "UPDATE tasks SET updated_at = ? WHERE id = ?",
            task.updated_at,
            id
        )
        .execute(&mut *tx)
        .await?;
        record(
            &mut tx,
            id,
            "note",
            &task.updated_at,
            json!({"body": body, "before": before, "after": task}),
        )
        .await?;
        tx.commit().await?;
        Ok(ChangeResult {
            task,
            changed: true,
        })
    }
}

fn validate(input: &EditTask) -> Result<()> {
    if let Some(category) = &input.category
        && !matches!(category.as_str(), "personal" | "work")
    {
        return Err(AppError::invalid(
            "category",
            "Category must be personal or work",
        ));
    }
    if let Some(title) = &input.title {
        nonempty("title", title)?;
    }
    if let Some(Some(project)) = &input.project {
        nonempty("project", project)?;
    }
    if let Some(sources) = &input.sources {
        for source in sources {
            nonempty("source", source)?;
        }
    }
    Ok(())
}

fn nonempty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(AppError::invalid(field, "Must not be empty"));
    }
    Ok(())
}

async fn record(
    conn: &mut SqliteConnection,
    id: &str,
    kind: &str,
    at: &str,
    changes: Value,
) -> Result<()> {
    let changes = serde_json::to_string(&changes)?;
    sqlx::query!(
        "INSERT INTO history (task_id, kind, at, changes) VALUES (?, ?, ?, ?)",
        id,
        kind,
        at,
        changes
    )
    .execute(conn)
    .await?;
    Ok(())
}
