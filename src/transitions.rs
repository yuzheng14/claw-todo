use chrono::{SecondsFormat, Utc};
use serde_json::json;
use sqlx::{SqliteConnection, types::Json};

use crate::{
    AppError, Result, Store, Task, Transition, TransitionResult,
    db::{get_task, record},
    parenting::require_open_ancestors,
};

impl Store {
    /// 执行状态操作；目标与全部受影响后代的修改、历史在同一事务内提交。
    pub async fn transition(&self, id: &str, action: Transition) -> Result<TransitionResult> {
        let target = match &action {
            Transition::Start => "in_progress",
            Transition::Block { .. } => "blocked",
            Transition::Done { .. } => "done",
            Transition::Cancel { .. } => "cancelled",
            Transition::Reopen => "pending",
        };
        if let Transition::Block { reason }
        | Transition::Cancel {
            reason: Some(reason),
            ..
        } = &action
            && reason.trim().is_empty()
        {
            return Err(AppError::invalid("reason", "Must not be empty"));
        }
        let mut tx = self.write().await?;
        let before = get_task(&mut tx, id).await?;
        // 允许同状态重试；跨状态时，关闭任务只能重开，处理中或阻塞的任务不能用重开重置进度。
        if (matches!(action, Transition::Reopen)
            && !is_closed(&before.status)
            && before.status != "pending")
            || (!matches!(action, Transition::Reopen)
                && is_closed(&before.status)
                && before.status != target)
        {
            return Err(AppError::InvalidState {
                task_id: id.to_owned(),
                status: before.status,
                requested_status: target,
            });
        }
        if matches!(action, Transition::Reopen)
            && is_closed(&before.status)
            && let Some(parent_id) = &before.parent_id
        {
            require_open_ancestors(&mut tx, parent_id).await?;
        }
        let mut task = before.clone();
        apply(&mut task, &action, target);
        if task == before {
            tx.commit().await?;
            return Ok(TransitionResult {
                task,
                changed: false,
                affected_task_ids: Vec::new(),
            });
        }

        // 原因修正不再次级联；只有真正进入关闭状态时才处理后代。
        let descendants = if is_closed(target) && before.status != target {
            open_descendants(&mut tx, id).await?
        } else {
            Vec::new()
        };
        let cascade = matches!(
            action,
            Transition::Done { cascade: true } | Transition::Cancel { cascade: true, .. }
        );
        if !cascade && !descendants.is_empty() {
            return Err(AppError::OpenDescendants {
                task_id: id.to_owned(),
                blocking_task_ids: descendants.into_iter().map(|t| t.id).collect(),
            });
        }
        let at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let task = save(&mut tx, &before, task, &at, None).await?;
        let mut affected_task_ids = vec![id.to_owned()];
        for before in descendants {
            let mut descendant = before.clone();
            apply(&mut descendant, &action, target);
            let descendant = save(&mut tx, &before, descendant, &at, Some(id)).await?;
            affected_task_ids.push(descendant.id);
        }
        tx.commit().await?;
        Ok(TransitionResult {
            task,
            changed: true,
            affected_task_ids,
        })
    }
}

fn is_closed(status: &str) -> bool {
    matches!(status, "done" | "cancelled")
}

fn apply(task: &mut Task, action: &Transition, target: &str) {
    task.status = target.to_owned();
    task.blocked_reason = match action {
        Transition::Block { reason } => Some(reason.clone()),
        _ => None,
    };
    task.cancel_reason = match action {
        Transition::Cancel { reason, .. } => reason.clone(),
        _ => None,
    };
}

async fn open_descendants(conn: &mut SqliteConnection, id: &str) -> Result<Vec<Task>> {
    // 先遍历完整子树，再筛开放状态，不能因中间节点已关闭而漏掉更深的后代。
    Ok(sqlx::query_as!(
        Task,
        r#"WITH RECURSIVE descendants(id) AS (
            SELECT id FROM tasks WHERE parent_id = ?
            UNION
            SELECT t.id FROM tasks t JOIN descendants d ON t.parent_id = d.id
        )
        SELECT id, title, description, status, category, project, parent_id,
            blocked_reason, cancel_reason, sources AS "sources: Json<Vec<String>>",
            created_at, updated_at, closed_at, creation_token
        FROM tasks WHERE id IN (SELECT id FROM descendants)
            AND status IN ('pending', 'in_progress', 'blocked') ORDER BY id"#,
        id
    )
    .fetch_all(conn)
    .await?)
}

async fn save(
    conn: &mut SqliteConnection,
    before: &Task,
    mut task: Task,
    at: &str,
    cascade_from: Option<&str>,
) -> Result<Task> {
    let status_changed = before.status != task.status;
    task.updated_at = at.to_owned();
    if status_changed {
        task.closed_at = is_closed(&task.status).then(|| at.to_owned());
    }
    sqlx::query!(
        "UPDATE tasks SET status = ?, blocked_reason = ?, cancel_reason = ?, \
         updated_at = ?, closed_at = ? WHERE id = ?",
        task.status,
        task.blocked_reason,
        task.cancel_reason,
        task.updated_at,
        task.closed_at,
        task.id
    )
    .execute(&mut *conn)
    .await?;
    let mut changes = json!({"before": before, "after": task});
    if let Some(root) = cascade_from {
        changes["cascade_from"] = json!(root);
    }
    let kind = if status_changed {
        "status_changed"
    } else {
        "edited"
    };
    record(conn, &task.id, kind, at, changes).await?;
    Ok(task)
}
